use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;

use crate::models::StreamOption;

pub fn extract_resolution_score(label: &str) -> u32 {
    let re = Regex::new(r"(\d{3,4})\s*p?").unwrap();
    if let Some(cap) = re.captures(label)
        && let Ok(val) = cap[1].parse::<u32>()
        && (144..=4320).contains(&val)
    {
        return val;
    }
    0
}

/// Picks the best stream index according to `preferred_quality` ("best", "1080p", "720p", etc.).
/// Falls back to the highest numeric resolution available.
pub fn pick_preferred_stream_index(streams: &[StreamOption], preferred_quality: &str) -> usize {
    if streams.is_empty() {
        return 0;
    }

    let pref = preferred_quality.trim().to_lowercase();
    if !pref.is_empty() && pref != "best" && pref != "auto" {
        if let Some(idx) = streams
            .iter()
            .position(|s| s.label.to_lowercase().contains(&pref))
        {
            return idx;
        }
    }

    // Default ("best"): pick the stream with the highest numeric resolution (e.g., 1080 > 720 > 480)
    streams
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| extract_resolution_score(&s.label))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub fn play_mpv(
    stream: &StreamOption,
    subtitle_url: Option<&str>,
    title: &str,
) -> Result<()> {
    let mut cmd = Command::new("mpv");
    cmd.arg("--fullscreen")
        .arg("--save-position-on-quit")
        .arg(format!("--title={title}"))
        .arg(format!("--force-media-title={title}"))
        .arg("--user-agent=Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/137.0.0.0 Safari/537.36");

    let referer = stream
        .referer
        .as_deref()
        .unwrap_or("https://yeshi.eu.org/");
    cmd.arg(format!("--referrer={referer}"));

    if let Some(sub) = subtitle_url
        && !sub.is_empty()
    {
        cmd.arg(format!("--sub-file={sub}"));
    }

    cmd.arg(&stream.url);

    let status = cmd
        .status()
        .context("mpv başlatılamadı (sisteminizde mpv yüklü mü?)")?;

    if !status.success() && status.code() != Some(4) {
        // mpv exit code 4 is quit via key/window close in some setups, usually 0
        eprintln!("[!] mpv çıkış kodu: {:?}", status.code());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_highest_resolution_for_best() {
        let streams = vec![
            StreamOption { label: "480p".into(), url: "u1".into(), referer: None },
            StreamOption { label: "1080p".into(), url: "u2".into(), referer: None },
            StreamOption { label: "720p".into(), url: "u3".into(), referer: None },
        ];
        assert_eq!(pick_preferred_stream_index(&streams, "best"), 1);
        assert_eq!(pick_preferred_stream_index(&streams, "720p"), 2);
    }
}
