use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::models::StreamOption;
use crate::player::has_command;

pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Parses 1-based range like "5-12" or "5" into 0-based inclusive `(start, end)` indices.
pub fn parse_episode_range(input: &str, total: usize) -> Result<(usize, usize)> {
    if total == 0 {
        bail!("Bölüm listesi boş.");
    }
    let trimmed = input.trim();
    if let Some((a, b)) = trimmed.split_once('-') {
        let start: usize = a.trim().parse().context("Başlangıç sayısı geçersiz")?;
        let end: usize = b.trim().parse().context("Bitiş sayısı geçersiz")?;
        if start == 0 || end == 0 || start > end || start > total {
            bail!("Geçersiz aralık (1-{total} arası olmalı).");
        }
        Ok((start - 1, end.min(total) - 1))
    } else {
        let single: usize = trimmed.parse().context("Geçersiz bölüm numarası")?;
        if single == 0 || single > total {
            bail!("Bölüm numarası 1-{total} arası olmalı.");
        }
        Ok((single - 1, single - 1))
    }
}

pub fn target_file_path(download_dir: &str, anime_title: &str, ep_title: &str) -> PathBuf {
    let dir = Path::new(download_dir).join(sanitize_name(anime_title));
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}.mp4", sanitize_name(ep_title)))
}

fn build_ytdlp_cmd(stream: &StreamOption, out_path: &Path, quiet: bool) -> Result<Command> {
    let ytdlp = if has_command("yt-dlp") {
        "yt-dlp"
    } else if has_command("youtube-dl") {
        "youtube-dl"
    } else {
        bail!("İndirme için 'yt-dlp' kurulu olmalı (sudo pacman -S yt-dlp).");
    };

    let mut cmd = Command::new(ytdlp);
    let referer = stream
        .referer
        .as_deref()
        .unwrap_or("https://yeshi.eu.org/");

    cmd.arg("--no-part")
        .arg("--no-playlist")
        .arg("--referer")
        .arg(referer)
        .arg("--user-agent")
        .arg("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/137.0.0.0 Safari/537.36")
        .arg("--downloader")
        .arg("ffmpeg")
        .arg("--downloader-args")
        .arg("ffmpeg_i:-allowed_extensions ALL")
        .arg("-o")
        .arg(out_path)
        .arg(&stream.url);

    if quiet {
        cmd.arg("--quiet")
            .arg("--no-warnings")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    } else {
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    }

    Ok(cmd)
}

pub fn save_subtitle(sub_url: &str, out_video_path: &Path) {
    let ext = if sub_url.contains(".srt") { "srt" } else { "vtt" };
    let sub_path = out_video_path.with_extension(ext);
    if let Ok(resp) = reqwest::blocking::get(sub_url)
        && resp.status().is_success()
        && let Ok(bytes) = resp.bytes()
    {
        let _ = fs::write(sub_path, bytes);
    }
}

pub fn download_stream(
    stream: &StreamOption,
    subtitle_url: Option<&str>,
    out_path: &Path,
) -> Result<()> {
    if let Some(sub) = subtitle_url
        && !sub.is_empty()
    {
        save_subtitle(sub, out_path);
    }

    let mut cmd = build_ytdlp_cmd(stream, out_path, false)?;
    let status = cmd.status().context("yt-dlp başlatılamadı")?;
    if !status.success() {
        bail!("İndirme başarısız (çıkış kodu: {:?})", status.code());
    }
    Ok(())
}

pub fn spawn_background_download(
    stream: &StreamOption,
    subtitle_url: Option<&str>,
    out_path: &Path,
) -> Result<Child> {
    if let Some(sub) = subtitle_url
        && !sub.is_empty()
    {
        save_subtitle(sub, out_path);
    }
    let mut cmd = build_ytdlp_cmd(stream, out_path, true)?;
    Ok(cmd.spawn().context("Arka plan indirmesi başlatılamadı")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges_correctly() {
        assert_eq!(parse_episode_range("5-12", 24).unwrap(), (4, 11));
        assert_eq!(parse_episode_range("5", 24).unwrap(), (4, 4));
        assert_eq!(parse_episode_range("1-50", 12).unwrap(), (0, 11));
        assert!(parse_episode_range("10-5", 24).is_err());
        assert!(parse_episode_range("0-5", 24).is_err());
    }
}
