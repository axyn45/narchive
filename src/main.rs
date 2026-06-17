mod api;
mod args;
mod config;
mod download;
mod metadata;
mod utils;

use chrono::Utc;
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use api::fetch_song_details;
use args::Args;
use config::{DownloadConfig, get_resume_config_path};
use download::{download_single_song, resolve_song_ids, scan_local_downloads};
use utils::{create_dir_one_level, format_bytes, generate_download_id};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load variables from .env if present
    let _ = dotenvy::dotenv();

    // 2. Parse command line arguments
    let args = Args::parse();

    if args.concurrent == 0 {
        eprintln!(
            "Error: Concurrency limit (--concurrent / CONCURRENT_DOWNLOADS) must be at least 1."
        );
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
                eprintln!(
                    "Error: Failed to read configuration file '{:?}': {}",
                    config_file_path, e
                );
                std::process::exit(1);
            }
        };

        let mut val: serde_json::Value = match serde_json::from_str(&config_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "Error: The configuration file '{:?}' is corrupted or contains invalid JSON.",
                    config_file_path
                );
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
                let ms = if let Ok(dt) =
                    chrono::NaiveDateTime::parse_from_str(time_str, "%Y%m%d%H%M%S")
                {
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
                eprintln!(
                    "Error: Failed to parse configuration in '{:?}': {}",
                    resume_path, e
                );
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
        if args.no_metadata && !config.no_metadata {
            config.no_metadata = true;
            modified = true;
        }
        if args.no_cover && !config.no_cover {
            config.no_cover = true;
            modified = true;
        }
        if args.no_id_suffix && !config.no_id_suffix {
            config.no_id_suffix = true;
            modified = true;
        }

        let target_config_path = session_dir.join(".narchive-dl");
        if modified {
            println!("⚙️ Configuration overridden. Updating .narchive-dl...");
            let updated_config_str = serde_json::to_string_pretty(&config)?;
            fs::write(&target_config_path, updated_config_str)?;
            if config_file_path != target_config_path
                && config_file_path.parent() == target_config_path.parent()
            {
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
            no_metadata: args.no_metadata,
            no_cover: args.no_cover,
            no_id_suffix: args.no_id_suffix,
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
            .unwrap(),
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    spinner.set_message("Resolving song lists from API...");

    let all_target_song_ids = resolve_song_ids(
        &client,
        &resolved_api,
        resolved_cookie.as_deref(),
        &config,
        &spinner,
    )
    .await;

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
    let song_details = match fetch_song_details(
        &client,
        &resolved_api,
        resolved_cookie.as_deref(),
        &config,
        &target_ids_vec,
    )
    .await
    {
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

    let total_active_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_session_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Spawn a background task to compute and display the total download speed
    let overall_pb_clone = overall_pb.clone();
    let total_active_bytes_clone = Arc::clone(&total_active_bytes);
    tokio::spawn(async move {
        let mut last_bytes = 0;
        let mut last_time = tokio::time::Instant::now();
        while !overall_pb_clone.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            let current_bytes = total_active_bytes_clone.load(std::sync::atomic::Ordering::Relaxed);
            let current_time = tokio::time::Instant::now();
            let elapsed = current_time.duration_since(last_time).as_secs_f64();
            if elapsed > 0.0 {
                let diff = current_bytes.saturating_sub(last_bytes);
                let speed = diff as f64 / elapsed;
                overall_pb_clone.set_message(format!(
                    "Downloading songs... ({}/s)",
                    format_bytes(speed as u64)
                ));
                last_bytes = current_bytes;
                last_time = current_time;
            }
        }
    });

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
        let total_active_bytes = Arc::clone(&total_active_bytes);
        let total_session_bytes = Arc::clone(&total_session_bytes);

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
                total_active_bytes,
                total_session_bytes,
            )
            .await;
        });
    }

    while let Some(res) = join_set.join_next().await {
        if let Err(e) = res {
            eprintln!("Task join error: {}", e);
        }
    }

    overall_pb.finish_with_message("Done!");
    let total_downloaded_size = total_session_bytes.load(std::sync::atomic::Ordering::SeqCst);
    println!(
        "\n✨ Download session completed successfully! Total downloaded: {}",
        format_bytes(total_downloaded_size)
    );
    Ok(())
}
