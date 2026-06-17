use rand::Rng;
use std::fs;
use std::path::Path;

/// Generate a random 8-character lowercase alphanumeric download ID
pub fn generate_download_id() -> String {
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
pub fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
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

/// Enforce 1-level directory creation rule
pub fn create_dir_one_level(path: &Path) -> Result<(), String> {
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

/// Format bytes into a human-readable string (KB, MB, GB)
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
