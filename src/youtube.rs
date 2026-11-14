//! Fetches playlist metadata by shelling out to `yt-dlp` (no API key needed).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::ErrorKind;
use std::process::Command;

use crate::model::Video;

pub struct Fetched {
    pub id: String,
    pub title: String,
    pub videos: Vec<Video>,
    /// Private/deleted/unknown-length entries that were left out.
    pub skipped: usize,
}

#[derive(Deserialize)]
struct RawPlaylist {
    id: Option<String>,
    title: Option<String>,
    entries: Option<Vec<RawEntry>>,
}

#[derive(Deserialize)]
struct RawEntry {
    id: Option<String>,
    title: Option<String>,
    duration: Option<f64>,
}

pub fn fetch_playlist(url: &str) -> Result<Fetched> {
    let out = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--dump-single-json",
            "--yes-playlist",
            "--no-warnings",
            url,
        ])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow::anyhow!(
                    "yt-dlp was not found in PATH. Install it first (e.g. `pip install -U yt-dlp` or your package manager)."
                )
            } else {
                anyhow::Error::new(e).context("failed to run yt-dlp")
            }
        })?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("yt-dlp failed for {url}:\n{}", err.trim());
    }
    parse_playlist_json(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_playlist_json(json: &str) -> Result<Fetched> {
    let raw: RawPlaylist = serde_json::from_str(json).context("unexpected yt-dlp output")?;
    let Some(entries) = raw.entries else {
        bail!("that URL is a single video, not a playlist");
    };
    let id = raw.id.context("playlist has no id")?;
    let title = raw.title.unwrap_or_else(|| id.clone());

    let mut videos = Vec::new();
    let mut skipped = 0;
    for e in entries {
        let title = e.title.unwrap_or_default();
        let private = title == "[Private video]" || title == "[Deleted video]";
        match (e.id, e.duration) {
            (Some(id), Some(d)) if !private && d >= 1.0 => videos.push(Video {
                id,
                title,
                duration_secs: d.round() as u64,
                watched: false,
            }),
            _ => skipped += 1,
        }
    }
    if videos.is_empty() {
        bail!("no usable videos found in this playlist");
    }
    Ok(Fetched {
        id,
        title,
        videos,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_skips_unusable_entries() {
        let json = r#"{
            "id": "PL123", "title": "Rust 101",
            "entries": [
              {"id": "a", "title": "Intro", "duration": 600.0},
              {"id": "b", "title": "[Private video]", "duration": null},
              {"id": "c", "title": "Live stream", "duration": null},
              {"id": "d", "title": "Ownership", "duration": 1200}
            ]}"#;
        let f = parse_playlist_json(json).unwrap();
        assert_eq!(f.id, "PL123");
        assert_eq!(f.videos.len(), 2);
        assert_eq!(f.skipped, 2);
        assert_eq!(f.videos[1].duration_secs, 1200);
    }

    #[test]
    fn rejects_single_video() {
        assert!(parse_playlist_json(r#"{"id":"x","title":"t"}"#).is_err());
    }
}
