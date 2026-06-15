use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Persistent configuration stored as JSON in the session folder.
/// Note: api and cookie are excluded to be specified at runtime or loaded from .env.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DownloadConfig {
    pub time: u64,
    pub path: String,
    pub user_agent: Option<String>,
    pub query_params: Option<String>,
    pub br: Option<u32>,
    pub tracks: Vec<u64>,
    pub albums: Vec<u64>,
    pub playlists: Vec<u64>,
    #[serde(default)]
    pub no_metadata: bool,
    #[serde(default)]
    pub no_cover: bool,
}

/// Find the configuration file in the specified resume directory
pub fn get_resume_config_path(resume_dir: &Path) -> Result<PathBuf, String> {
    if !resume_dir.exists() {
        return Err(format!("Resume directory '{:?}' does not exist.", resume_dir));
    }
    if !resume_dir.is_dir() {
        return Err(format!("Resume path '{:?}' is not a directory.", resume_dir));
    }

    let files = [".narchive-dl", ".config", "config.json"];
    for file in &files {
        let path = resume_dir.join(file);
        if path.exists() && path.is_file() {
            return Ok(path);
        }
    }

    Err(format!(
        "No download configuration file (.narchive-dl, .config, or config.json) found in '{:?}'",
        resume_dir
    ))
}
