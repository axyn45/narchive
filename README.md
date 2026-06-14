# narchive

A resilient, concurrent command-line downloader for Netease Cloud Music written in Rust.

## Features

- **Concurrent Downloads**: Download multiple tracks simultaneously with custom concurrency limits.
- **Dynamic UI**: Uses a multi-progress bar terminal interface showing overall and individual track progress.
- **Metadata Tagging**: Automatically embeds lyrics, cover art, and metadata (including Netease song IDs) into downloaded tracks.
- **Session Resumption**: Resumes interrupted download tasks seamlessly using local session directories and `.narchive-dl` configs.
- **Error Handling**: Implements 3x retries with backoff for transient issues and fails fast on restricted or VIP-only tracks.

## Setup

This tool requires an active instance of a Netease Cloud Music API. It is designed to work with [NeteaseCloudMusicApiEnhanced](https://github.com/neteasecloudmusicapienhanced/api-enhanced).

1. Configure the API endpoint URL in a `.env` file in the project root:
   ```env
   NETEASE_API=http://localhost:3000
   # Optional: Add cookie for VIP/restricted tracks
   # COOKIE=your_cookie_here
   ```
2. Build the project:
   ```bash
   cargo build --release
   ```

## CLI Arguments

| Argument | Environment Variable | Description |
| --- | --- | --- |
| `--api` | `NETEASE_API` | Endpoint URL for the Netease API |
| `--cookie` | `USER_COOKIE` | Cookie for a logged in user |
| `--download-path` | `DOWNLOAD_PATH` | Download destination folder path |
| `--user-agent` | `USER_AGENT` | Custom User Agent |
| `--query-params` | `QUERY_PARAMS` | Custom query parameters (e.g. `key=val&another=123`) |
| `--br` | `TARGET_BR` | Target bitrate in bps (e.g. `320000`) |
| `--track` | — | Track ID(s) to download (can be specified multiple times) |
| `--album` | — | Album ID(s) to download (can be specified multiple times) |
| `--playlist` | — | Playlist ID(s) to download (can be specified multiple times) |
| `--resume` | — | Path to the download folder to resume |
| `--concurrent` | `CONCURRENT_DOWNLOADS` | Maximum concurrent downloads (default: `3`) |

## Usage

### Download Tracks, Albums, or Playlists
Pass the respective IDs to `--track`, `--album`, or `--playlist`. You can specify a custom download directory and concurrency limit:

```bash
./target/release/narchive --track 123456 --album 789012 --playlist 345678 --download-path ./downloads --concurrent 4
```

*Note: If no download path is provided, a directory named `narchive-<random_id>` will be created in the current path.*

### Resume Download Session
Resume an unfinished session by specifying the directory containing the `.narchive-dl` config file:

```bash
./target/release/narchive --resume ./downloads
```
