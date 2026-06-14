use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use clap::Parser;
use serde::{Deserialize, Serialize};
use chrono::{Datelike, TimeZone, Utc};
use rand::Rng;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::picture::{Picture, PictureType, MimeType};
use lofty::tag::{ItemKey, ItemValue, TagItem, Tag, Accessor};
use lofty::config::WriteOptions;

/// CLI arguments for 'narchive'
#[derive(Parser, Debug)]
#[command(
    name = "narchive",
    version = "0.1.0",
    about = "Netease Cloud Music playlist/album/songs downloader"
)]
struct Args {
    /// Endpoint URL for the Netease API
    #[arg(long, env = "NETEASE_API")]
    api: Option<String>,

    /// Cookie for a logged in user
    #[arg(long, env = "USER_COOKIE")]
    cookie: Option<String>,

    /// Download destination folder path
    #[arg(long, env = "DOWNLOAD_PATH", default_value = "./narchive-dl")]
    download_path: String,

    /// Custom User Agent
    #[arg(long, env = "USER_AGENT")]
    user_agent: Option<String>,

    /// Custom query parameters separated by ampersand (e.g. 'key=val&another=123')
    #[arg(long, env = "QUERY_PARAMS")]
    query_params: Option<String>,

    /// Target bitrate in bps (corresponds to the 'br' parameter, e.g. 320000)
    #[arg(long, env = "TARGET_BR")]
    br: Option<u32>,

    /// Track IDs to download
    #[arg(long = "track")]
    tracks: Vec<u64>,

    /// Album IDs to download
    #[arg(long = "album")]
    albums: Vec<u64>,

    /// Playlist IDs to download
    #[arg(long = "playlist")]
    playlists: Vec<u64>,

    /// Resume a download session by its 8-character ID
    #[arg(long)]
    resume: Option<String>,
}

/// Persistent configuration stored as JSON in the session folder.
/// Note: api and cookie are excluded to be specified at runtime or loaded from .env.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct DownloadConfig {
    download_id: String,
    time_created: String,
    download_path: String,
    user_agent: Option<String>,
    query_params: Option<String>,
    br: Option<u32>,
    tracks: Vec<u64>,
    albums: Vec<u64>,
    playlists: Vec<u64>,
}

// --- API RESPONSE TYPES ---

#[derive(Deserialize, Debug, Clone)]
struct SongArtist {
    name: String,
}

#[derive(Deserialize, Debug, Clone)]
struct SongAlbum {
    name: Option<String>,
    #[serde(rename = "picUrl")]
    pic_url: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct SongDetail {
    id: u64,
    name: String,
    ar: Option<Vec<SongArtist>>,
    al: Option<SongAlbum>,
    no: Option<u32>, // track number
    #[serde(rename = "publishTime")]
    publish_time: Option<i64>,
}

#[derive(Deserialize, Debug, Clone)]
struct SongDetailResponse {
    songs: Option<Vec<SongDetail>>,
}

#[derive(Deserialize, Debug, Clone)]
struct AlbumResponseSong {
    id: u64,
}

#[derive(Deserialize, Debug, Clone)]
struct AlbumResponse {
    songs: Option<Vec<AlbumResponseSong>>,
}

#[derive(Deserialize, Debug, Clone)]
struct PlaylistTrackResponse {
    songs: Option<Vec<AlbumResponseSong>>,
}

#[derive(Deserialize, Debug, Clone)]
struct SongUrlData {
    url: Option<String>,
    #[serde(rename = "type")]
    file_type: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct SongUrlResponse {
    data: Option<Vec<SongUrlData>>,
}

#[derive(Deserialize, Debug, Clone)]
struct LyricInfo {
    lyric: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct LyricResponse {
    lrc: Option<LyricInfo>,
}

// --- HELPER FUNCTIONS ---

/// Generate a random 8-character lowercase alphanumeric download ID
fn generate_download_id() -> String {
    let mut rng = rand::thread_rng();
    let chars: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..8)
        .map(|_| {
            let idx = rng.gen_range(0..chars.len());
            chars[idx] as char
        })
        .collect()
}

/// Sanitize text to form a safe filename by replacing invalid characters with underscores
fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Find the unique session folder matching 'narchive-*-<resume_id>' in the download path
fn find_resume_dir(download_path: &str, resume_id: &str) -> Result<PathBuf, String> {
    let read_dir = fs::read_dir(download_path)
        .map_err(|e| format!("Failed to read download path '{}': {}", download_path, e))?;
    
    let mut matching_dirs = vec![];
    for entry in read_dir {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    if dir_name.starts_with("narchive-") && dir_name.ends_with(&format!("-{}", resume_id)) {
                        matching_dirs.push(path);
                    }
                }
            }
        }
    }
    
    if matching_dirs.is_empty() {
        return Err(format!("No download session folder found ending with '-{}' in '{}'", resume_id, download_path));
    }
    if matching_dirs.len() > 1 {
        return Err(format!("Multiple download session folders found ending with '-{}' in '{}': {:?}", resume_id, download_path, matching_dirs));
    }
    
    Ok(matching_dirs.remove(0))
}

/// Retrieve the Netease ID from an audio file's metadata
fn get_netease_id_from_file(path: &Path) -> Option<u64> {
    let tagged_file = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged_file.primary_tag()?;
    
    // Check all tag items for the Comment field containing our ID
    for item in tag.items() {
        if item.key() == &ItemKey::Comment {
            if let ItemValue::Text(val) = item.value() {
                if val.starts_with("NETEASE_ID:") {
                    if let Ok(id) = val["NETEASE_ID:".len()..].parse::<u64>() {
                        return Some(id);
                    }
                }
            }
        }
    }
    
    // Fallback: Check if it was saved as a custom text frame
    if let Some(id_str) = tag.get_string(&ItemKey::Unknown("NETEASE_ID".to_string())) {
        if let Ok(id) = id_str.parse::<u64>() {
            return Some(id);
        }
    }
    
    None
}

/// Build URL with API endpoint, path, query parameters, cookie, and custom queries
fn build_url(
    api_base: &str,
    path: &str,
    params: &[(&str, &str)],
    cookie: Option<&str>,
    config: &DownloadConfig,
) -> Result<reqwest::Url, Box<dyn std::error::Error>> {
    let mut url = reqwest::Url::parse(&format!("{}/{}", api_base.trim_end_matches('/'), path.trim_start_matches('/')))?;
    {
        let mut query_pairs = url.query_pairs_mut();
        for &(k, v) in params {
            query_pairs.append_pair(k, v);
        }
        if let Some(cookie_val) = cookie {
            query_pairs.append_pair("cookie", cookie_val);
        }
        // Use randomCNIP=true by default to bypass regional limitations
        query_pairs.append_pair("randomCNIP", "true");
    }
    
    // Append raw custom query string if present
    if let Some(custom_query) = &config.query_params {
        let current_query = url.query().unwrap_or("");
        if !current_query.is_empty() {
            url.set_query(Some(&format!("{}&{}", current_query, custom_query)));
        } else {
            url.set_query(Some(custom_query));
        }
    }
    
    Ok(url)
}

/// Batch fetch details for a list of song IDs
async fn fetch_song_details(
    client: &reqwest::Client,
    api_base: &str,
    cookie: Option<&str>,
    config: &DownloadConfig,
    song_ids: &[u64],
) -> Result<HashMap<u64, SongDetail>, Box<dyn std::error::Error>> {
    let mut details = HashMap::new();
    
    // Batch in chunks of 100 to avoid exceeding URL limits
    for chunk in song_ids.chunks(100) {
        let ids_str = chunk.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        let url = build_url(api_base, "song/detail", &[("ids", &ids_str)], cookie, config)?;
        
        let resp = client.get(url).send().await?;
        if !resp.status().is_success() {
            println!("Warning: failed to fetch song details for batch: HTTP {}", resp.status());
            continue;
        }
        
        let body: SongDetailResponse = resp.json().await?;
        if let Some(songs) = body.songs {
            for song in songs {
                details.insert(song.id, song);
            }
        }
    }
    
    Ok(details)
}

/// Fetch list of song IDs in an album
async fn fetch_album_song_ids(
    client: &reqwest::Client,
    api_base: &str,
    cookie: Option<&str>,
    config: &DownloadConfig,
    album_id: u64,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let url = build_url(api_base, "album", &[("id", &album_id.to_string())], cookie, config)?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(format!("Failed to fetch album {}: HTTP {}", album_id, resp.status()).into());
    }
    
    let body: AlbumResponse = resp.json().await?;
    let mut song_ids = vec![];
    if let Some(songs) = body.songs {
        for song in songs {
            song_ids.push(song.id);
        }
    }
    Ok(song_ids)
}

/// Fetch list of song IDs in a playlist
async fn fetch_playlist_song_ids(
    client: &reqwest::Client,
    api_base: &str,
    cookie: Option<&str>,
    config: &DownloadConfig,
    playlist_id: u64,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let url = build_url(api_base, "playlist/track/all", &[("id", &playlist_id.to_string())], cookie, config)?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(format!("Failed to fetch playlist {}: HTTP {}", playlist_id, resp.status()).into());
    }
    
    let body: PlaylistTrackResponse = resp.json().await?;
    let mut song_ids = vec![];
    if let Some(songs) = body.songs {
        for song in songs {
            song_ids.push(song.id);
        }
    }
    Ok(song_ids)
}

/// Fetch download URL for a single song ID
async fn fetch_song_download_url(
    client: &reqwest::Client,
    api_base: &str,
    cookie: Option<&str>,
    config: &DownloadConfig,
    song_id: u64,
) -> Result<Option<SongUrlData>, Box<dyn std::error::Error>> {
    let song_id_str = song_id.to_string();
    let mut params = vec![("id", song_id_str.as_str())];
    
    let br_str;
    if let Some(br) = config.br {
        br_str = br.to_string();
        params.push(("br", br_str.as_str()));
    }

    let url = build_url(api_base, "song/url", &params, cookie, config)?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    
    let body: SongUrlResponse = resp.json().await?;
    if let Some(mut data_list) = body.data {
        if !data_list.is_empty() {
            return Ok(Some(data_list.remove(0)));
        }
    }
    Ok(None)
}

/// Fetch lyric text for a song ID
async fn fetch_lyric(
    client: &reqwest::Client,
    api_base: &str,
    cookie: Option<&str>,
    config: &DownloadConfig,
    song_id: u64,
) -> Option<String> {
    let url = build_url(api_base, "lyric", &[("id", &song_id.to_string())], cookie, config).ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    
    let body: LyricResponse = resp.json().await.ok()?;
    body.lrc.and_then(|l| l.lyric)
}

/// Apply metadata, lyrics, and cover art to the audio file
fn apply_metadata(
    filepath: &Path,
    song_detail: &SongDetail,
    lyric: Option<String>,
    cover_bytes: Option<Vec<u8>>,
    cover_mime: Option<MimeType>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tagged_file = Probe::open(filepath)?.read()?;
    let primary_type = tagged_file.primary_tag_type();
    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            tagged_file.insert_tag(Tag::new(primary_type));
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    // Set standard Accessor fields
    tag.set_title(song_detail.name.clone());

    if let Some(artists) = &song_detail.ar {
        let artist_str = artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
        tag.set_artist(artist_str);
    }

    if let Some(album) = &song_detail.al {
        if let Some(album_name) = &album.name {
            tag.set_album(album_name.clone());
        }
    }

    // Set track number
    if let Some(track_no) = song_detail.no {
        tag.set_track(track_no);
    }

    if let Some(publish_time) = song_detail.publish_time {
        if let Some(dt) = Utc.timestamp_millis_opt(publish_time).single() {
            tag.set_year(dt.year() as u32);
            tag.insert(TagItem::new(
                ItemKey::ReleaseDate,
                ItemValue::Text(dt.format("%Y-%m-%d").to_string()),
            ));
        }
    }

    // Set custom Netease ID frame/comment
    let _ = tag.insert(TagItem::new(
        ItemKey::Unknown("NETEASE_ID".to_string()),
        ItemValue::Text(song_detail.id.to_string()),
    ));
    // Also save it inside the standard Comment tag as a robust format-agnostic fallback
    let _ = tag.insert(TagItem::new(
        ItemKey::Comment,
        ItemValue::Text(format!("NETEASE_ID:{}", song_detail.id)),
    ));

    // Set lyrics tag
    if let Some(lyrics_text) = lyric {
        tag.insert(TagItem::new(
            ItemKey::Lyrics,
            ItemValue::Text(lyrics_text),
        ));
    }

    // Embed album cover art
    if let Some(bytes) = cover_bytes {
        // Clear any existing pictures first to avoid duplicates
        while !tag.pictures().is_empty() {
            tag.remove_picture(0);
        }
        
        let picture = Picture::new_unchecked(
            PictureType::CoverFront,
            cover_mime,
            None,
            bytes,
        );
        tag.push_picture(picture);
    }

    tagged_file.save_to_path(filepath, WriteOptions::default())?;
    Ok(())
}

// --- MAIN RUNTIME ---

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load variables from .env if present
    let _ = dotenvy::dotenv();

    // 2. Parse command line arguments
    let args = Args::parse();

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

    // Ensure the download base path directory exists
    fs::create_dir_all(&download_path)?;

    let mut config: DownloadConfig;
    let session_dir: PathBuf;

    // 4. Handle Session Resume or Task Creation
    if let Some(resume_id) = &args.resume {
        // Search and locate the session directory
        session_dir = match find_resume_dir(&download_path, resume_id) {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        };

        let config_file_path = session_dir.join("config.json");
        if !config_file_path.exists() {
            eprintln!("Error: config.json not found in session directory '{:?}'", session_dir);
            std::process::exit(1);
        }

        // Read and parse current config.json (which does not contain api or cookie)
        let config_str = fs::read_to_string(&config_file_path)?;
        config = match serde_json::from_str(&config_str) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("Error: Failed to parse config.json in '{:?}': {}", session_dir, e);
                std::process::exit(1);
            }
        };

        // If command-line/env values differ from config.json, overwrite and save
        let mut modified = false;

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

        if modified {
            println!("Configuration overridden. Updating config.json...");
            let updated_config_str = serde_json::to_string_pretty(&config)?;
            fs::write(&config_file_path, updated_config_str)?;
        } else {
            println!("Resuming session '{}'. Using existing configuration.", resume_id);
        }
    } else {
        // Create new download session. Ensure at least one track, album, or playlist target is supplied.
        if cli_tracks.is_empty() && cli_albums.is_empty() && cli_playlists.is_empty() {
            eprintln!("Error: No tracks, albums, or playlists specified for download.");
            eprintln!("Please specify at least one target using --track, --album, or --playlist");
            std::process::exit(1);
        }

        let download_id = generate_download_id();
        let time_created = Utc::now().format("%Y%m%d%H%M%S").to_string();

        let folder_name = format!("narchive-{}-{}", time_created, download_id);
        session_dir = Path::new(&download_path).join(&folder_name);
        fs::create_dir_all(&session_dir)?;

        config = DownloadConfig {
            download_id,
            time_created,
            download_path: download_path.clone(),
            user_agent: cli_user_agent,
            query_params: cli_query_params,
            br: cli_br,
            tracks: cli_tracks,
            albums: cli_albums,
            playlists: cli_playlists,
        };

        let config_file_path = session_dir.join("config.json");
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
    println!("Resolving song lists from API...");
    let mut all_target_song_ids = HashSet::new();

    // Direct tracks
    for &track_id in &config.tracks {
        all_target_song_ids.insert(track_id);
    }

    // Album tracks
    for &album_id in &config.albums {
        println!("Fetching song IDs for album {}...", album_id);
        match fetch_album_song_ids(&client, &resolved_api, resolved_cookie.as_deref(), &config, album_id).await {
            Ok(ids) => {
                for id in ids {
                    all_target_song_ids.insert(id);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to fetch song IDs for album {}: {}", album_id, e);
            }
        }
    }

    // Playlist tracks
    for &playlist_id in &config.playlists {
        println!("Fetching song IDs for playlist {}...", playlist_id);
        match fetch_playlist_song_ids(&client, &resolved_api, resolved_cookie.as_deref(), &config, playlist_id).await {
            Ok(ids) => {
                for id in ids {
                    all_target_song_ids.insert(id);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to fetch song IDs for playlist {}: {}", playlist_id, e);
            }
        }
    }

    if all_target_song_ids.is_empty() {
        eprintln!("Error: Resolved 0 target songs to download.");
        std::process::exit(1);
    }

    println!("Resolved {} unique target songs.", all_target_song_ids.len());

    // 7. Collect local files in the session directory to see what is already downloaded
    let mut downloaded_songs = HashMap::new();
    if let Ok(entries) = fs::read_dir(&session_dir) {
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

    // 8. Identify missing song IDs
    let mut missing_ids = vec![];
    for &id in &all_target_song_ids {
        if !downloaded_songs.contains_key(&id) {
            missing_ids.push(id);
        }
    }

    if missing_ids.is_empty() {
        println!("All songs are already downloaded! Task complete.");
        return Ok(());
    }

    println!("Found {} missing songs to download.", missing_ids.len());

    // 9. Fetch details (metadata) for all target song IDs to use during tag writing
    println!("Fetching metadata for target songs...");
    let target_ids_vec: Vec<u64> = all_target_song_ids.iter().copied().collect();
    let song_details = fetch_song_details(&client, &resolved_api, resolved_cookie.as_deref(), &config, &target_ids_vec).await?;

    // 10. Process downloading of missing songs
    let total_missing = missing_ids.len();
    for (idx, &song_id) in missing_ids.iter().enumerate() {
        let progress_prefix = format!("[{}/{}]", idx + 1, total_missing);
        
        let detail = match song_details.get(&song_id) {
            Some(d) => d.clone(),
            None => {
                eprintln!("{} Warning: Song metadata not found for ID {}, skipping...", progress_prefix, song_id);
                continue;
            }
        };

        let artist_names = detail.ar.as_ref()
            .map(|artists| artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        
        let display_name = if artist_names.is_empty() {
            detail.name.clone()
        } else {
            format!("{} - {}", artist_names, detail.name)
        };

        println!("{} Downloading song: \"{}\" (ID: {})", progress_prefix, display_name, song_id);

        // Fetch download link from song url API
        let url_data = match fetch_song_download_url(&client, &resolved_api, resolved_cookie.as_deref(), &config, song_id).await {
            Ok(Some(data)) => data,
            _ => {
                eprintln!("{} Error: Failed to fetch download URL for ID {}, skipping...", progress_prefix, song_id);
                continue;
            }
        };

        let download_url = match url_data.url {
            Some(url) if !url.is_empty() => url,
            _ => {
                eprintln!("{} Warning: Song ID {} is copyright-restricted or not available, skipping...", progress_prefix, song_id);
                continue;
            }
        };

        // Determine extension (default to mp3 if type is unavailable)
        let ext = url_data.file_type.unwrap_or_else(|| "mp3".to_string()).to_lowercase();
        
        // Define paths for temporary download and final output
        let temp_filename = format!("{}.tmp.{}", song_id, ext);
        let temp_filepath = session_dir.join(&temp_filename);
        
        let sanitized_base = sanitize_filename(&display_name);
        let final_filename = format!("{}.{}", sanitized_base, ext);
        let final_filepath = session_dir.join(&final_filename);

        // Download audio file stream to temporary location
        println!("  -> Streaming audio data...");
        let mut download_resp = match client.get(&download_url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => {
                eprintln!("  -> Error: Failed to download audio from url, skipping...");
                continue;
            }
        };

        let mut temp_file = match File::create(&temp_filepath) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  -> Error: Failed to create temp file: {}", e);
                continue;
            }
        };

        let mut download_failed = false;
        while let Some(chunk) = download_resp.chunk().await.unwrap_or(None) {
            if let Err(e) = temp_file.write_all(&chunk) {
                eprintln!("  -> Error: Failed to write chunk to disk: {}", e);
                download_failed = true;
                break;
            }
        }

        if download_failed {
            let _ = fs::remove_file(&temp_filepath);
            continue;
        }
        
        // Flush and close the file handle
        drop(temp_file);

        // Fetch lyrics
        println!("  -> Fetching lyrics...");
        let lyric = fetch_lyric(&client, &resolved_api, resolved_cookie.as_deref(), &config, song_id).await;

        // Download album cover artwork if available
        let mut cover_bytes = None;
        let mut cover_mime = None;
        if let Some(album) = &detail.al {
            if let Some(pic_url) = &album.pic_url {
                println!("  -> Downloading album cover...");
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

        // Apply metadata tags to temp file
        println!("  -> Embedding tags and metadata...");
        if let Err(e) = apply_metadata(&temp_filepath, &detail, lyric, cover_bytes, cover_mime) {
            eprintln!("  -> Warning: Failed to write metadata tags: {}", e);
            // We still keep the file even if metadata writing fails
        }

        // Rename from temp file to final filename atomically
        if let Err(e) = fs::rename(&temp_filepath, &final_filepath) {
            eprintln!("  -> Error: Failed to rename temp file to final path: {}", e);
            let _ = fs::remove_file(&temp_filepath);
        } else {
            println!("  -> Successfully saved: {:?}", final_filename);
        }
    }

    println!("\nDownload session completed successfully!");
    Ok(())
}
