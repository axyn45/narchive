use clap::Parser;

/// CLI arguments for 'narchive'
#[derive(Parser, Debug)]
#[command(
    name = "narchive",
    version = "0.1.0",
    about = "Netease Cloud Music playlist/album/songs downloader"
)]
pub struct Args {
    /// Endpoint URL for the Netease API
    #[arg(long, env = "NETEASE_API")]
    pub api: Option<String>,

    /// Cookie for a logged in user
    #[arg(long, env = "USER_COOKIE")]
    pub cookie: Option<String>,

    /// Download destination folder path
    #[arg(long, env = "DOWNLOAD_PATH", default_value = "./narchive-dl")]
    pub download_path: String,

    /// Custom User Agent
    #[arg(long, env = "USER_AGENT")]
    pub user_agent: Option<String>,

    /// Custom query parameters separated by ampersand (e.g. 'key=val&another=123')
    #[arg(long, env = "QUERY_PARAMS")]
    pub query_params: Option<String>,

    /// Target bitrate in bps (corresponds to the 'br' parameter, e.g. 320000)
    #[arg(long, env = "TARGET_BR")]
    pub br: Option<u32>,

    /// Track IDs to download
    #[arg(long = "track")]
    pub tracks: Vec<u64>,

    /// Album IDs to download
    #[arg(long = "album")]
    pub albums: Vec<u64>,

    /// Playlist IDs to download
    #[arg(long = "playlist")]
    pub playlists: Vec<u64>,

    /// Resume a download session by its 8-character ID
    #[arg(long)]
    pub resume: Option<String>,

    /// Maximum concurrent downloads
    #[arg(long = "concurrent", env = "CONCURRENT_DOWNLOADS", default_value_t = 3)]
    pub concurrent: usize,
}
