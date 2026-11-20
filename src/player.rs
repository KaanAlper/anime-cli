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
    if !pref.is_empty()
        && pref != "best"
        && pref != "auto"
        && let Some(idx) = streams
            .iter()
            .position(|s| s.label.to_lowercase().contains(&pref))
    {
        return idx;
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

/// Builds the full playback fallback order grouped by fansub:
/// 1. If `selected_fansub` is Some(f), try all players of `f` first (preferred player/quality first),
///    then all players of every other fansub in order.
/// 2. If `selected_fansub` is None ("Tümü"), iterate fansub by fansub, trying all players of each
///    fansub (preferred player/quality first within each fansub) before moving to the next fansub.
pub fn build_playback_order(
    streams: &[StreamOption],
    selected_fansub: Option<&str>,
    preferred_quality: &str,
) -> Vec<usize> {
    if streams.is_empty() {
        return Vec::new();
    }

    // Collect distinct fansub groups in appearance order
    let mut fansub_groups: Vec<Option<&str>> = Vec::new();
    for s in streams {
        let key = s.fansub.as_deref();
        if !fansub_groups.contains(&key) {
            fansub_groups.push(key);
        }
    }

    // Move selected_fansub to the front if specified
    if let Some(sel) = selected_fansub
        && let Some(pos) = fansub_groups.iter().position(|&k| k == Some(sel))
    {
        let chosen = fansub_groups.remove(pos);
        fansub_groups.insert(0, chosen);
    }

    let mut order = Vec::with_capacity(streams.len());
    for group in fansub_groups {
        let group_indices: Vec<usize> = streams
            .iter()
            .enumerate()
            .filter(|(_, s)| s.fansub.as_deref() == group)
            .map(|(i, _)| i)
            .collect();
        if group_indices.is_empty() {
            continue;
        }
        let group_streams: Vec<StreamOption> =
            group_indices.iter().map(|&i| streams[i].clone()).collect();
        let best_local = pick_preferred_stream_index(&group_streams, preferred_quality);
        let best_global = group_indices[best_local];

        order.push(best_global);
        for idx in group_indices {
            if idx != best_global {
                order.push(idx);
            }
        }
    }

    order
}

pub fn resolve_embed_with_ytdlp(url: &str, referer: Option<&str>) -> Result<String> {
    let ytdlp = if has_command("yt-dlp") {
        "yt-dlp"
    } else {
        "youtube-dl"
    };

    let mut args = vec![
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
    ];
    if let Some(r) = referer {
        args.push("--referer");
        args.push(r);
    }
    args.push(url);

    let output = Command::new("timeout")
        .args(&args)
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
        let u = resolve_embed_with_ytdlp(&stream.url, stream.referer.as_deref())?;
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
        .arg("--force-seekable=yes")
        .arg("--hr-seek=yes")
        .arg("--demuxer-readahead-secs=60")
        .arg("--demuxer-max-bytes=150MiB")
        .arg("--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=5");

    if is_hls {
        cmd.arg("--demuxer=lavf")
            .arg("--demuxer-lavf-format=hls")
            .arg("--demuxer-lavf-o=allowed_extensions=ALL");
    }
    if resolved_embed {
        cmd.arg("--ytdl=no");
    }

    cmd.arg(format!("--title={title}"))
        .arg(format!("--force-media-title={title}"))
        .arg("--user-agent=Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/137.0.0.0 Safari/537.36");

    if let Some(referer) = stream.referer.as_deref()
        && !referer.is_empty()
    {
        cmd.arg(format!("--referrer={referer}"));
    }

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
            StreamOption { label: "480p".into(), url: "u1".into(), referer: None, fansub: None },
            StreamOption { label: "1080p".into(), url: "u2".into(), referer: None, fansub: None },
            StreamOption { label: "720p".into(), url: "u3".into(), referer: None, fansub: None },
        ];
        assert_eq!(pick_preferred_stream_index(&streams, "best"), 1);
        assert_eq!(pick_preferred_stream_index(&streams, "720p"), 2);

        let ordered_labels = vec![
            StreamOption { label: "Stream A".into(), url: "u1".into(), referer: None, fansub: None },
            StreamOption { label: "Stream B".into(), url: "u2".into(), referer: None, fansub: None },
            StreamOption { label: "Stream C".into(), url: "u3".into(), referer: None, fansub: None },
        ];
        assert_eq!(pick_preferred_stream_index(&ordered_labels, "best"), 0);
    }

    #[test]
    fn builds_fansub_grouped_fallback_order() {
        let streams = vec![
            StreamOption { label: "Player1".into(), url: "u1".into(), referer: None, fansub: Some("Fansub A".into()) },
            StreamOption { label: "Player1".into(), url: "u2".into(), referer: None, fansub: Some("Fansub B".into()) },
            StreamOption { label: "Player2".into(), url: "u3".into(), referer: None, fansub: Some("Fansub A".into()) },
            StreamOption { label: "Player2".into(), url: "u4".into(), referer: None, fansub: Some("Fansub B".into()) },
        ];
        // Selected Fansub B + Player2 -> tries (B, Player2), (B, Player1), then (A, Player2), (A, Player1)
        let order = build_playback_order(&streams, Some("Fansub B"), "Player2");
        assert_eq!(order, vec![3, 1, 2, 0]);

        // All fansubs + best -> tries all of A first, then all of B
        let all_order = build_playback_order(&streams, None, "best");
        assert_eq!(all_order, vec![0, 2, 1, 3]);
    }
}
