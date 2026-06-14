mod args;
mod config;
mod api;
mod metadata;
mod utils;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use clap::Parser;
use chrono::Utc;
use lofty::picture::MimeType;

use args::Args;
use config::{DownloadConfig, get_resume_config_path};
use api::{
    fetch_album_song_ids, fetch_playlist_song_ids, fetch_song_details,
    fetch_song_download_url, fetch_lyric,
};
use metadata::{get_netease_id_from_file, apply_metadata};
use utils::{generate_download_id, sanitize_filename};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load variables from .env if present
    let _ = dotenvy::dotenv();

    // 2. Parse command line arguments
    let args = Args::parse();

    if args.concurrent == 0 {
        eprintln!("Error: Concurrency limit (--concurrent / CONCURRENT_DOWNLOADS) must be at least 1.");
        std::process::exit(1);
    }

    // 3. Resolve configurations from command-line overrides or environment variables
    let cli_api = args.api.clone();
    let cli_cookie = args.cookie.clone();
    let cli_user_agent = args.user_agent.clone();
    let cli_query_params = args.query_params.clone();
    let cli_br = args.br;
    let cli_tracks = args.tracks;
    let cli_albums = args.albums;
    let cli_playlists = args.playlists;
    let download_path = args.download_path.clone();

    // The API endpoint URL is always required for execution (specified on CLI or loaded from .env)
    let resolved_api = match cli_api {
        Some(api) => api,
        None => {
            eprintln!("Error: Netease API endpoint URL is required.");
            eprintln!("Please specify via '--api' argument or 'NETEASE_API' in .env");
            std::process::exit(1);
        }
    };
    let resolved_cookie = cli_cookie;

    let mut config: DownloadConfig;
    let session_dir: PathBuf;

    // 4. Handle Session Resume or Task Creation
    if let Some(resume_path_str) = &args.resume {
        let resume_path = Path::new(resume_path_str);
        let config_file_path = match get_resume_config_path(resume_path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };

        // Read and parse current configuration (which does not contain api or cookie)
        let config_str = match fs::read_to_string(&config_file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: Failed to read configuration file '{:?}': {}", config_file_path, e);
                std::process::exit(1);
            }
        };

        let mut val: serde_json::Value = match serde_json::from_str(&config_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error: The configuration file '{:?}' is corrupted or contains invalid JSON.", config_file_path);
                eprintln!("Details: {}", e);
                std::process::exit(1);
            }
        };

        // Migrate 'download_id' (remove it if exists, since we no longer keep it)
        if let Some(obj) = val.as_object_mut() {
            obj.remove("download_id");
        }

        // Migrate 'time_created' -> 'time'
        if let Some(time_created_val) = val.get("time_created") {
            if let Some(time_str) = time_created_val.as_str() {
                let ms = if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(time_str, "%Y%m%d%H%M%S") {
                    dt.and_utc().timestamp_millis() as u64
                } else {
                    chrono::Utc::now().timestamp_millis() as u64
                };
                if let Some(obj) = val.as_object_mut() {
                    obj.insert("time".to_string(), serde_json::json!(ms));
                    obj.remove("time_created");
                }
            }
        }

        // Migrate 'download_path' -> 'path'
        if let Some(download_path_val) = val.get("download_path") {
            if let Some(path_str) = download_path_val.as_str() {
                let abs_path = if let Ok(canon) = fs::canonicalize(path_str) {
                    canon.to_string_lossy().into_owned()
                } else {
                    path_str.to_string()
                };
                if let Some(obj) = val.as_object_mut() {
                    obj.insert("path".to_string(), serde_json::json!(abs_path));
                    obj.remove("download_path");
                }
            }
        }

        config = match serde_json::from_value(val) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("Error: Failed to parse configuration in '{:?}': {}", resume_path, e);
                std::process::exit(1);
            }
        };

        // Ignore the loaded timestamp and update with the latest millisecond timestamp
        config.time = Utc::now().timestamp_millis() as u64;

        // Determine target download directory (session_dir)
        // If download_path argument is set, overwrite config.path with it
        if let Some(ref dl_path) = download_path {
            let target_path = Path::new(dl_path);
            if let Err(e) = create_dir_one_level(target_path) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            let abs_target_path = fs::canonicalize(target_path)?
                .to_string_lossy()
                .into_owned();
            
            if config.path != abs_target_path {
                config.path = abs_target_path;
            }
            session_dir = fs::canonicalize(target_path)?;
        } else {
            // Otherwise, default to the resume folder itself
            session_dir = fs::canonicalize(resume_path)?;
            let abs_session_path = session_dir.to_string_lossy().into_owned();
            if config.path != abs_session_path {
                config.path = abs_session_path;
            }
        }

        // If command-line/env values differ from configuration, overwrite and save
        // We always update the timestamp, so we always write back the configuration
        let mut modified = true;

        if let Some(ref ua) = cli_user_agent {
            if Some(ua) != config.user_agent.as_ref() {
                config.user_agent = Some(ua.clone());
                modified = true;
            }
        }
        if let Some(ref qp) = cli_query_params {
            if Some(qp) != config.query_params.as_ref() {
                config.query_params = Some(qp.clone());
                modified = true;
            }
        }
        if let Some(br) = cli_br {
            if Some(br) != config.br {
                config.br = Some(br);
                modified = true;
            }
        }
        if !cli_tracks.is_empty() {
            if cli_tracks != config.tracks {
                config.tracks = cli_tracks.clone();
                modified = true;
            }
        }
        if !cli_albums.is_empty() {
            if cli_albums != config.albums {
                config.albums = cli_albums.clone();
                modified = true;
            }
        }
        if !cli_playlists.is_empty() {
            if cli_playlists != config.playlists {
                config.playlists = cli_playlists.clone();
                modified = true;
            }
        }

        let target_config_path = session_dir.join(".narchive-dl");
        if modified {
            println!("⚙️ Configuration overridden. Updating .narchive-dl...");
            let updated_config_str = serde_json::to_string_pretty(&config)?;
            fs::write(&target_config_path, updated_config_str)?;
            if config_file_path != target_config_path && config_file_path.parent() == target_config_path.parent() {
                let _ = fs::remove_file(&config_file_path);
            }
        } else {
            println!("🔄 Resuming session. Using existing configuration.");
            if config_file_path != target_config_path {
                let updated_config_str = serde_json::to_string_pretty(&config)?;
                fs::write(&target_config_path, updated_config_str)?;
                if config_file_path.parent() == target_config_path.parent() {
                    let _ = fs::remove_file(&config_file_path);
                }
            }
        }
    } else {
        // Create new download session. Ensure at least one track, album, or playlist target is supplied.
        if cli_tracks.is_empty() && cli_albums.is_empty() && cli_playlists.is_empty() {
            eprintln!("Error: No tracks, albums, or playlists specified for download.");
            eprintln!("Please specify at least one target using --track, --album, or --playlist");
            std::process::exit(1);
        }

        let download_id = generate_download_id();

        // Determine download directory
        let target_dir = match &download_path {
            Some(path_str) => {
                let path = Path::new(path_str);
                if let Err(e) = create_dir_one_level(path) {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                fs::canonicalize(path)?
            }
            None => {
                let folder_name = format!("narchive-{}", download_id);
                let path = Path::new(&folder_name);
                fs::create_dir(path)?;
                fs::canonicalize(path)?
            }
        };

        session_dir = target_dir;

        config = DownloadConfig {
            time: Utc::now().timestamp_millis() as u64,
            path: session_dir.to_string_lossy().into_owned(),
            user_agent: cli_user_agent,
            query_params: cli_query_params,
            br: cli_br,
            tracks: cli_tracks,
            albums: cli_albums,
            playlists: cli_playlists,
        };

        let config_file_path = session_dir.join(".narchive-dl");
        let config_str = serde_json::to_string_pretty(&config)?;
        fs::write(config_file_path, config_str)?;

        println!("Created new download session folder: {:?}", session_dir);
    }

    // 5. Initialize Http Client with custom user agent if specified
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(ref ua) = config.user_agent {
        if let Ok(header_val) = reqwest::header::HeaderValue::from_str(ua) {
            headers.insert(reqwest::header::USER_AGENT, header_val);
        }
    }
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    // 6. Gather and resolve all target song IDs
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    spinner.set_message("Resolving song lists from API...");

    let all_target_song_ids = resolve_song_ids(&client, &resolved_api, resolved_cookie.as_deref(), &config, &spinner).await;

    if all_target_song_ids.is_empty() {
        spinner.finish_and_clear();
        eprintln!("Error: Resolved 0 target songs to download.");
        std::process::exit(1);
    }

    // 7. Collect local files in the session directory to see what is already downloaded
    spinner.set_message("Scanning session directory for existing downloads...");
    let downloaded_songs = scan_local_downloads(&session_dir);

    // 8. Identify missing song IDs
    let mut missing_ids = vec![];
    for &id in &all_target_song_ids {
        if !downloaded_songs.contains_key(&id) {
            missing_ids.push(id);
        }
    }

    if missing_ids.is_empty() {
        spinner.finish_and_clear();
        println!("✨ All songs are already downloaded! Task complete.");
        return Ok(());
    }

    // 9. Fetch details (metadata) for all target song IDs to use during tag writing
    spinner.set_message(format!(
        "Fetching metadata for {} target songs ({} missing)...",
        all_target_song_ids.len(),
        missing_ids.len()
    ));
    let target_ids_vec: Vec<u64> = all_target_song_ids.iter().copied().collect();
    let song_details = match fetch_song_details(&client, &resolved_api, resolved_cookie.as_deref(), &config, &target_ids_vec).await {
        Ok(details) => details,
        Err(e) => {
            spinner.finish_and_clear();
            eprintln!("Error: Failed to fetch song details: {}", e);
            std::process::exit(1);
        }
    };

    spinner.finish_and_clear();

    println!(
        "🔍 Resolved {} unique target songs. Found {} missing songs to download.",
        all_target_song_ids.len(),
        missing_ids.len()
    );

    // 10. Process downloading of missing songs concurrently with dynamic TUI progress bars
    let total_missing = missing_ids.len();
    let song_details = Arc::new(song_details);
    let mp = Arc::new(indicatif::MultiProgress::new());
    
    let overall_pb = mp.add(indicatif::ProgressBar::new(total_missing as u64));
    overall_pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.green/blue}] {pos}/{len} ({percent}%) {msg}")
            .unwrap()
            .progress_chars("█░")
    );
    overall_pb.set_message("Downloading songs...");
    overall_pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let sem = Arc::new(tokio::sync::Semaphore::new(args.concurrent));
    let mut join_set = tokio::task::JoinSet::new();

    for &song_id in &missing_ids {
        let sem = Arc::clone(&sem);
        let mp = Arc::clone(&mp);
        let overall_pb = overall_pb.clone();
        let client = client.clone();
        let resolved_api = resolved_api.clone();
        let resolved_cookie = resolved_cookie.clone();
        let config = config.clone();
        let session_dir = session_dir.clone();
        let song_details = Arc::clone(&song_details);

        let detail = match song_details.get(&song_id) {
            Some(d) => d.clone(),
            None => {
                overall_pb.inc(1);
                continue;
            }
        };

        join_set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            download_single_song(
                client,
                resolved_api,
                resolved_cookie,
                config,
                session_dir,
                song_id,
                detail,
                mp,
                overall_pb,
            ).await;
        });
    }

    while let Some(res) = join_set.join_next().await {
        if let Err(e) = res {
            eprintln!("Task join error: {}", e);
        }
    }

    overall_pb.finish_with_message("Done!");
    println!("\n✨ Download session completed successfully!");
    Ok(())
}

fn create_dir_one_level(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    
    if let Some(parent) = path.parent() {
        let parent_exists = if parent.as_os_str().is_empty() {
            true
        } else {
            parent.exists()
        };
        
        if parent_exists {
            fs::create_dir(path)
                .map_err(|e| format!("Failed to create directory '{:?}': {}", path, e))?;
            Ok(())
        } else {
            Err(format!(
                "Parent directory '{:?}' does not exist. Only 1-level directory creation is supported.",
                parent
            ))
        }
    } else {
        Err(format!("Invalid path '{:?}'", path))
    }
}

async fn resolve_song_ids(
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
                spinner.println(format!("  \x1b[33m⚠️\x1b[0m Warning: Failed to fetch song IDs for album {}: {}", album_id, e));
            }
        }
    }

    // Playlist tracks
    for &playlist_id in &config.playlists {
        spinner.set_message(format!("Resolving playlist {}...", playlist_id));
        match fetch_playlist_song_ids(client, resolved_api, resolved_cookie, config, playlist_id).await {
            Ok(ids) => {
                for id in ids {
                    all_target_song_ids.insert(id);
                }
            }
            Err(e) => {
                spinner.println(format!("  \x1b[33m⚠️\x1b[0m Warning: Failed to fetch song IDs for playlist {}: {}", playlist_id, e));
            }
        }
    }

    all_target_song_ids
}

fn scan_local_downloads(session_dir: &Path) -> HashMap<u64, PathBuf> {
    let mut downloaded_songs = HashMap::new();
    if let Ok(entries) = fs::read_dir(session_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if ext_lower == "mp3" || ext_lower == "ogg" || ext_lower == "flac" || ext_lower == "wav" {
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

async fn download_single_song(
    client: reqwest::Client,
    resolved_api: String,
    resolved_cookie: Option<String>,
    config: DownloadConfig,
    session_dir: PathBuf,
    song_id: u64,
    detail: crate::api::SongDetail,
    mp: Arc<indicatif::MultiProgress>,
    overall_pb: indicatif::ProgressBar,
) {
    let artist_names = detail.ar.as_ref()
        .map(|artists| artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "))
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
            .unwrap()
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
        let url_data = match fetch_song_download_url(&client, &resolved_api, resolved_cookie.as_deref(), &config, song_id).await {
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

        let ext = url_data.file_type.unwrap_or_else(|| "mp3".to_string()).to_lowercase();
        
        let temp_filename = format!("{}.{}.tmp", song_id, ext);
        let temp_filepath = session_dir.join(&temp_filename);
        
        let sanitized_base = sanitize_filename(&display_name);
        let final_filename = format!("{}.{}", sanitized_base, ext);
        let final_filepath = session_dir.join(&final_filename);

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
                    .unwrap()
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
            pb.inc(chunk.len() as u64);
        }

        if let Some(err_msg) = download_failed {
            error_details = format!("Write error: {}", err_msg);
            let _ = fs::remove_file(&temp_filepath);
            continue;
        }
        
        drop(temp_file);

        // Fetch lyrics
        pb.set_style(
            indicatif::ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} {msg}")
                .unwrap()
        );
        pb.set_message(format!("Fetching lyrics: {}", display_name));
        let lyric = fetch_lyric(&client, &resolved_api, resolved_cookie.as_deref(), &config, song_id).await;

        // Download cover artwork
        let mut cover_bytes = None;
        let mut cover_mime = None;
        if let Some(album) = &detail.al {
            if let Some(pic_url) = &album.pic_url {
                pb.set_message(format!("Downloading cover: {}", display_name));
                if let Ok(cover_resp) = client.get(pic_url).send().await {
                    if cover_resp.status().is_success() {
                        let mime_str = cover_resp.headers()
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

        // Embed tags
        pb.set_message(format!("Embedding tags: {}", display_name));
        if let Err(e) = apply_metadata(&temp_filepath, &detail, lyric, cover_bytes, cover_mime) {
            let _ = mp.println(format!("  \x1b[33m⚠️\x1b[0m Warning: Failed to embed tags for {}: {}", display_name, e));
        }

        // Finalize
        if let Err(e) = fs::rename(&temp_filepath, &final_filepath) {
            error_details = format!("Failed to save final file: {}", e);
            let _ = fs::remove_file(&temp_filepath);
            continue;
        }

        let _ = mp.println(format!("  \x1b[32m✔\x1b[0m Saved: \x1b[1m{}\x1b[0m", final_filename));
        success = true;
        break;
    }

    if !success {
        if error_details == "Restricted/unavailable" {
            let _ = mp.println(format!("  \x1b[33m🔒\x1b[0m Restricted: {}", display_name));
        } else {
            let _ = mp.println(format!("  \x1b[31m✘\x1b[0m Failed: {} ({})", display_name, error_details));
        }
    }

    pb.finish_and_clear();
    overall_pb.inc(1);
}
