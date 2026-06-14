use std::collections::HashMap;
use serde::Deserialize;
use crate::config::DownloadConfig;

#[derive(Deserialize, Debug, Clone)]
pub struct SongArtist {
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SongAlbum {
    pub name: Option<String>,
    #[serde(rename = "picUrl")]
    pub pic_url: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SongDetail {
    pub id: u64,
    pub name: String,
    pub ar: Option<Vec<SongArtist>>,
    pub al: Option<SongAlbum>,
    pub no: Option<u32>, // track number
    #[serde(rename = "publishTime")]
    pub publish_time: Option<i64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SongDetailResponse {
    pub songs: Option<Vec<SongDetail>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AlbumResponseSong {
    pub id: u64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AlbumResponse {
    pub songs: Option<Vec<AlbumResponseSong>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PlaylistTrackResponse {
    pub songs: Option<Vec<AlbumResponseSong>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SongUrlData {
    pub url: Option<String>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SongUrlResponse {
    pub data: Option<Vec<SongUrlData>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LyricInfo {
    pub lyric: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LyricResponse {
    pub lrc: Option<LyricInfo>,
}

/// Build URL with API endpoint, path, query parameters, cookie, and custom queries
pub fn build_url(
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
pub async fn fetch_song_details(
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
pub async fn fetch_album_song_ids(
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
pub async fn fetch_playlist_song_ids(
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
pub async fn fetch_song_download_url(
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
pub async fn fetch_lyric(
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
