use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Persistent configuration stored as JSON in the session folder.
/// Note: api and cookie are excluded to be specified at runtime or loaded from .env.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DownloadConfig {
    pub download_id: String,
    pub time: u64,
    pub path: String,
    pub user_agent: Option<String>,
    pub query_params: Option<String>,
    pub br: Option<u32>,
    pub tracks: Vec<u64>,
    pub albums: Vec<u64>,
    pub playlists: Vec<u64>,
}

/// Find the unique session folder matching 'narchive-*-<resume_id>' in the download path
pub fn find_resume_dir(download_path: &str, resume_id: &str) -> Result<PathBuf, String> {
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
