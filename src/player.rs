use anyhow::{Context, Result, bail};
use regex::Regex;
use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::models::StreamOption;

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn take_interrupted() -> bool {
    INTERRUPTED.swap(false, Ordering::SeqCst)
}

pub fn has_command(name: &str) -> bool {
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let full = dir.join(name);
            if full.is_file() {
                return true;
            }
        }
    }
    false
}

pub fn has_ytdlp() -> bool {
    has_command("yt-dlp") || has_command("youtube-dl")
}

pub fn is_direct_stream_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".mp4")
        || lower.contains(".m3u8")
        || lower.contains("/mn/")
        || lower.contains("master.txt")
}

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

    // Default ("best"): pick the stream with the highest numeric resolution (e.g., 1080 > 720 > 480).
    // On ties (e.g. player-name labels with score 0), keep the earliest item in priority order.
    streams
        .iter()
        .enumerate()
        .max_by_key(|(i, s)| (extract_resolution_score(&s.label), std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub fn resolve_embed_with_ytdlp(url: &str) -> Result<String> {
    let ytdlp = if has_command("yt-dlp") {
        "yt-dlp"
    } else {
        "youtube-dl"
    };

    let output = Command::new("timeout")
        .args([
            "8",
            ytdlp,
            "--no-warnings",
            "--no-playlist",
            "--socket-timeout",
            "5",
            "--retries",
            "1",
            "--extractor-retries",
            "1",
            "-f",
            "best",
            "-g",
            url,
        ])
        .output()
        .context("yt-dlp çalıştırılamadı")?;

    if !output.status.success() {
        bail!("Embed bağlantısı çözülemedi (ölü veya zaman aşımı)");
    }

    let resolved = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    if resolved.is_empty() {
        bail!("yt-dlp boş URL döndürdü");
    }
    Ok(resolved)
}

pub fn play_mpv(
    stream: &StreamOption,
    subtitle_url: Option<&str>,
    title: &str,
) -> Result<()> {
    let resolved_embed = !is_direct_stream_url(&stream.url) && has_ytdlp();
    let target_url = if resolved_embed {
        let u = resolve_embed_with_ytdlp(&stream.url)?;
        println!("  \x1b[38;2;139;178;127m✔\x1b[0m Bağlantı çözüldü, mpv açılıyor...");
        u
    } else {
        stream.url.clone()
    };

    let is_hls = {
        let lower = target_url.to_ascii_lowercase();
        lower.contains(".m3u8")
            || lower.contains("/mn/")
            || lower.contains("master.txt")
            || lower.contains("puffytr")
    };

    let mut cmd = Command::new("mpv");
    cmd.arg("--fullscreen")
        .arg("--save-position-on-quit")
        .arg("--really-quiet")
        .arg("--no-terminal")
        .arg("--cache=yes")
        .arg("--demuxer-readahead-secs=60")
        .arg("--demuxer-max-bytes=150MiB");

    if is_hls {
        cmd.arg("--demuxer-lavf-o=allowed_extensions=ALL");
    }
    if resolved_embed {
        cmd.arg("--ytdl=no");
    }

    cmd.arg(format!("--title={title}"))
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

    cmd.arg(&target_url);

    let status = cmd
        .status()
        .context("mpv başlatılamadı (sisteminizde mpv yüklü mü?)")?;

    if take_interrupted() || status.code().is_none() {
        return Ok(());
    }

    if !status.success() && status.code() != Some(4) {
        let is_direct = is_direct_stream_url(&stream.url);
        if !is_direct && !has_ytdlp() {
            bail!(
                "mpv oynatamadı (kod: {:?}). Embed bağlantıları için 'yt-dlp' kurulu olmalı (sudo pacman -S yt-dlp).",
                status.code()
            );
        }
        bail!(
            "Video açılamadı (mpv çıkış kodu: {:?}) — bağlantı ölü veya engelli olabilir.",
            status.code()
        );
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

        let ordered_labels = vec![
            StreamOption { label: "Stream A".into(), url: "u1".into(), referer: None },
            StreamOption { label: "Stream B".into(), url: "u2".into(), referer: None },
            StreamOption { label: "Stream C".into(), url: "u3".into(), referer: None },
        ];
        assert_eq!(pick_preferred_stream_index(&ordered_labels, "best"), 0);
    }
}
