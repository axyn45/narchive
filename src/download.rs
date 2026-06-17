use lofty::picture::MimeType;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::{
    SongDetail, fetch_album_song_ids, fetch_lyric, fetch_playlist_song_ids, fetch_song_download_url,
};
use crate::config::DownloadConfig;
use crate::metadata::{apply_metadata, get_netease_id_from_file};
use crate::utils::sanitize_filename;

/// Gathers and resolves all target song IDs from config (tracks, albums, playlists)
pub async fn resolve_song_ids(
    client: &reqwest::Client,
    resolved_api: &str,
    resolved_cookie: Option<&str>,
    config: &DownloadConfig,
    spinner: &indicatif::ProgressBar,
) -> HashSet<u64> {
    let mut all_target_song_ids = HashSet::new();

    // Direct tracks
    for &track_id in &config.tracks {
        all_target_song_ids.insert(track_id);
    }

    // Album tracks
    for &album_id in &config.albums {
        spinner.set_message(format!("Resolving album {}...", album_id));
        match fetch_album_song_ids(client, resolved_api, resolved_cookie, config, album_id).await {
            Ok(ids) => {
                for id in ids {
                    all_target_song_ids.insert(id);
                }
            }
            Err(e) => {
                spinner.println(format!(
                    "  \x1b[33m⚠️\x1b[0m Warning: Failed to fetch song IDs for album {}: {}",
                    album_id, e
                ));
            }
        }
    }

    // Playlist tracks
    for &playlist_id in &config.playlists {
        spinner.set_message(format!("Resolving playlist {}...", playlist_id));
        match fetch_playlist_song_ids(client, resolved_api, resolved_cookie, config, playlist_id)
            .await
        {
            Ok(ids) => {
                for id in ids {
                    all_target_song_ids.insert(id);
                }
            }
            Err(e) => {
                spinner.println(format!(
                    "  \x1b[33m⚠️\x1b[0m Warning: Failed to fetch song IDs for playlist {}: {}",
                    playlist_id, e
                ));
            }
        }
    }

    all_target_song_ids
}

/// Scans the download session directory to find already downloaded songs by reading Netease ID metadata
pub fn scan_local_downloads(session_dir: &Path) -> HashMap<u64, PathBuf> {
    let mut downloaded_songs = HashMap::new();
    if let Ok(entries) = fs::read_dir(session_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if ext_lower == "mp3"
                            || ext_lower == "ogg"
                            || ext_lower == "flac"
                            || ext_lower == "wav"
                        {
                            if let Some(song_id) = get_netease_id_from_file(&path) {
                                downloaded_songs.insert(song_id, path.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    downloaded_songs
}

/// Downloads a single song, fetches its lyric/cover, embeds metadata, and updates progress bars
pub async fn download_single_song(
    client: reqwest::Client,
    resolved_api: String,
    resolved_cookie: Option<String>,
    config: DownloadConfig,
    session_dir: PathBuf,
    song_id: u64,
    detail: SongDetail,
    mp: Arc<indicatif::MultiProgress>,
    overall_pb: indicatif::ProgressBar,
    total_active_bytes: Arc<std::sync::atomic::AtomicU64>,
    total_session_bytes: Arc<std::sync::atomic::AtomicU64>,
) {
    let artist_names = detail
        .ar
        .as_ref()
        .map(|artists| {
            artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let display_name = if artist_names.is_empty() {
        detail.name.clone()
    } else {
        format!("{} - {}", artist_names, detail.name)
    };

    let pb = mp.insert_before(&overall_pb, indicatif::ProgressBar::new_spinner());
    pb.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("  {spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb.set_message(format!("Queued: {}", display_name));

    let mut success = false;
    let mut error_details = String::new();

    for attempt in 1..=3 {
        if attempt > 1 {
            pb.set_message(format!("Retrying (attempt {}): {}", attempt, display_name));
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        }

        // Fetch download link
        pb.set_message(format!("Fetching URL: {}", display_name));
        let url_data = match fetch_song_download_url(
            &client,
            &resolved_api,
            resolved_cookie.as_deref(),
            &config,
            song_id,
        )
        .await
        {
            Ok(Some(data)) => data,
            _ => {
                error_details = "Failed to fetch download URL".to_string();
                continue;
            }
        };

        let download_url = match url_data.url {
            Some(url) if !url.is_empty() => url,
            _ => {
                error_details = "Restricted/unavailable".to_string();
                break; // Permanent error, do not retry
            }
        };

        let ext = url_data
            .file_type
            .unwrap_or_else(|| "mp3".to_string())
            .to_lowercase();

        let temp_filename = format!("{}.{}.tmp", song_id, ext);
        let temp_filepath = session_dir.join(&temp_filename);

        let sanitized_base = sanitize_filename(&display_name);
        let mut final_filename = format!("{}.{}", sanitized_base, ext);
        let mut final_filepath = session_dir.join(&final_filename);

        // Handle name collisions if the file already exists but belongs to a different song ID
        if final_filepath.exists() {
            if let Some(existing_id) = get_netease_id_from_file(&final_filepath) {
                if existing_id != song_id {
                    let unique_filename = format!("{} [{}].{}", sanitized_base, song_id, ext);
                    final_filename = unique_filename;
                    final_filepath = session_dir.join(&final_filename);
                }
            }
        }

        // Start downloading stream
        pb.set_message(format!("Connecting: {}", display_name));
        let mut download_resp = match client.get(&download_url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => {
                error_details = "Failed to connect to download URL".to_string();
                continue;
            }
        };

        let content_length = download_resp.content_length();
        if let Some(total_bytes) = content_length {
            pb.set_length(total_bytes);
            pb.set_style(
                indicatif::ProgressStyle::default_bar()
                    .template("  {spinner:.cyan} {msg:.bold} [{bar:25.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                    .unwrap()
                    .progress_chars("█░")
            );
        } else {
            pb.set_style(
                indicatif::ProgressStyle::default_spinner()
                    .template("  {spinner:.cyan} {msg:.bold} {bytes} ({bytes_per_sec})")
                    .unwrap(),
            );
        }
        pb.set_position(0);
        pb.set_message(display_name.clone());

        let mut temp_file = match File::create(&temp_filepath) {
            Ok(f) => f,
            Err(e) => {
                error_details = format!("Failed to create temp file: {}", e);
                continue;
            }
        };

        let mut download_failed = None;
        while let Some(chunk) = download_resp.chunk().await.unwrap_or(None) {
            if let Err(e) = temp_file.write_all(&chunk) {
                download_failed = Some(e.to_string());
                break;
            }
            let chunk_len = chunk.len() as u64;
            pb.inc(chunk_len);
            total_active_bytes.fetch_add(chunk_len, std::sync::atomic::Ordering::Relaxed);
        }

        if let Some(err_msg) = download_failed {
            error_details = format!("Write error: {}", err_msg);
            let _ = fs::remove_file(&temp_filepath);
            continue;
        }

        drop(temp_file);

        // Fetch lyrics
        let lyric = if config.no_metadata {
            None
        } else {
            pb.set_style(
                indicatif::ProgressStyle::default_spinner()
                    .template("  {spinner:.cyan} {msg}")
                    .unwrap(),
            );
            pb.set_message(format!("Fetching lyrics: {}", display_name));
            fetch_lyric(
                &client,
                &resolved_api,
                resolved_cookie.as_deref(),
                &config,
                song_id,
            )
            .await
        };

        // Download cover artwork
        let mut cover_bytes = None;
        let mut cover_mime = None;
        if !config.no_cover {
            if let Some(album) = &detail.al {
                if let Some(pic_url) = &album.pic_url {
                    pb.set_message(format!("Downloading cover: {}", display_name));
                    if let Ok(cover_resp) = client.get(pic_url).send().await {
                        if cover_resp.status().is_success() {
                            let mime_str = cover_resp
                                .headers()
                                .get(reqwest::header::CONTENT_TYPE)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("image/jpeg")
                                .to_string();

                            if let Ok(bytes) = cover_resp.bytes().await {
                                cover_bytes = Some(bytes.to_vec());
                                cover_mime = match mime_str.as_str() {
                                    "image/png" => Some(MimeType::Png),
                                    _ => Some(MimeType::Jpeg),
                                };
                            }
                        }
                    }
                }
            }
        }

        // Embed tags
        pb.set_message(format!("Embedding tags: {}", display_name));
        if let Err(e) = apply_metadata(
            &temp_filepath,
            &detail,
            lyric,
            cover_bytes,
            cover_mime,
            config.no_metadata,
        ) {
            let _ = mp.println(format!(
                "  \x1b[33m⚠️\x1b[0m Warning: Failed to embed tags for {}: {}",
                display_name, e
            ));
        }

        // Finalize
        if let Err(e) = fs::rename(&temp_filepath, &final_filepath) {
            error_details = format!("Failed to save final file: {}", e);
            let _ = fs::remove_file(&temp_filepath);
            continue;
        }

        if let Ok(metadata) = fs::metadata(&final_filepath) {
            total_session_bytes.fetch_add(metadata.len(), std::sync::atomic::Ordering::SeqCst);
        }

        let _ = mp.println(format!(
            "  \x1b[32m✔\x1b[0m Saved: \x1b[1m{}\x1b[0m",
            final_filename
        ));
        success = true;
        break;
    }

    if !success {
        if error_details == "Restricted/unavailable" {
            let _ = mp.println(format!("  \x1b[33m🔒\x1b[0m Restricted: {}", display_name));
        } else {
            let _ = mp.println(format!(
                "  \x1b[31m✘\x1b[0m Failed: {} ({})",
                display_name, error_details
            ));
        }
    }

    pb.finish_and_clear();
    overall_pb.inc(1);
}
