use std::path::Path;
use chrono::{TimeZone, Utc, Datelike};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::picture::{Picture, PictureType, MimeType};
use lofty::tag::{ItemKey, ItemValue, TagItem, Tag, Accessor};
use lofty::config::WriteOptions;
use crate::api::SongDetail;

/// Retrieve the Netease ID from an audio file's metadata
pub fn get_netease_id_from_file(path: &Path) -> Option<u64> {
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

/// Apply metadata, lyrics, and cover art to the audio file
pub fn apply_metadata(
    filepath: &Path,
    song_detail: &SongDetail,
    lyric: Option<String>,
    cover_bytes: Option<Vec<u8>>,
    cover_mime: Option<MimeType>,
    no_metadata: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tagged_file = Probe::open(filepath)?.guess_file_type()?.read()?;
    let primary_type = tagged_file.primary_tag_type();
    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            tagged_file.insert_tag(Tag::new(primary_type));
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    if !no_metadata {
        // Set standard Accessor fields
        tag.set_title(song_detail.name.clone());

        if let Some(artists) = &song_detail.ar {
            tag.remove_key(&ItemKey::TrackArtist);
            for artist in artists {
                tag.push(TagItem::new(
                    ItemKey::TrackArtist,
                    ItemValue::Text(artist.name.clone()),
                ));
            }
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

        // Set lyrics tag
        if let Some(lyrics_text) = lyric {
            tag.insert(TagItem::new(
                ItemKey::Lyrics,
                ItemValue::Text(lyrics_text),
            ));
        }
    }

    // Set custom Netease ID frame/comment
    let _ = tag.insert(TagItem::new(
        ItemKey::Unknown("NETEASE_ID".to_string()),
        ItemValue::Text(song_detail.id.to_string()),
    ));
    // Also save it inside the standard Comment tag as a robust fallback
    let _ = tag.insert(TagItem::new(
        ItemKey::Comment,
        ItemValue::Text(format!("NETEASE_ID:{}", song_detail.id)),
    ));

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
