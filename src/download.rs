use anyhow::{Context, Result, bail};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::models::{Anime, Episode, StreamOption};
use crate::player::{has_command, pick_preferred_stream_index};
use crate::sources::get_source;

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

fn build_ytdlp_cmd(stream: &StreamOption, out_path: &Path) -> Result<Command> {
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
        .arg("--newline")
        .arg("--progress")
        .arg("--no-warnings")
        .arg("--progress-template")
        .arg("download:%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s")
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
        .arg(&stream.url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

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

#[derive(Debug, Clone)]
pub struct DownloadJob {
    pub anime: Anime,
    pub episode: Episode,
    pub ep_in_season: usize,
    pub quality: String,
    pub banned_players: Vec<String>,
    pub out_path: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub struct DownloadStatus {
    pub queued: usize,
    pub active: bool,
    pub paused: bool,
    pub active_title: Option<String>,
    pub active_progress: Option<String>,
    pub queued_titles: Vec<String>,
    pub completed_titles: Vec<String>,
}

fn signal_pid(pid: u32, sig: &str) {
    let _ = Command::new("kill")
        .args([sig, &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub struct DownloadManager {
    queue: VecDeque<DownloadJob>,
    known_paths: HashSet<PathBuf>,
    active_child: Option<(Child, PathBuf, String)>,
    active_progress: Arc<Mutex<Option<String>>>,
    completed_titles: Vec<String>,
    playback_depth: usize,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            known_paths: HashSet::new(),
            active_child: None,
            active_progress: Arc::new(Mutex::new(None)),
            completed_titles: Vec::new(),
            playback_depth: 0,
        }
    }

    pub fn enqueue(&mut self, job: DownloadJob) {
        if !job.out_path.exists() && self.known_paths.insert(job.out_path.clone()) {
            self.queue.push_back(job);
        }
    }

    pub fn on_playback_started(&mut self) {
        self.playback_depth += 1;
        if self.playback_depth == 1
            && let Some((ref child, _, _)) = self.active_child
        {
            signal_pid(child.id(), "-STOP");
        }
    }

    pub fn on_playback_ended(&mut self) {
        self.playback_depth = self.playback_depth.saturating_sub(1);
        if self.playback_depth == 0
            && let Some((ref child, _, _)) = self.active_child
        {
            signal_pid(child.id(), "-CONT");
        }
    }

    pub fn tick(&mut self) {
        if let Some((mut child, path, title)) = self.active_child.take() {
            match child.try_wait() {
                Ok(Some(st)) => {
                    self.known_paths.remove(&path);
                    if st.success() {
                        self.completed_titles.push(title);
                        if self.completed_titles.len() > 10 {
                            self.completed_titles.remove(0);
                        }
                    }
                }
                Ok(None) => {
                    self.active_child = Some((child, path, title));
                }
                Err(_) => {
                    self.known_paths.remove(&path);
                }
            }
        }

        if self.playback_depth == 0
            && self.active_child.is_none()
            && let Some(job) = self.queue.pop_front()
        {
            if !job.out_path.exists() {
                let src = get_source(job.anime.source);
                if let Ok(watch) = src.get_watch_data(&job.anime, &job.episode, job.ep_in_season) {
                    let allowed: Vec<_> = watch
                        .streams
                        .into_iter()
                        .filter(|s| {
                            !job.banned_players
                                .iter()
                                .any(|b| s.label.to_ascii_lowercase().contains(&b.to_ascii_lowercase()))
                        })
                        .collect();
                    if !allowed.is_empty() {
                        let idx = pick_preferred_stream_index(&allowed, &job.quality);
                        let stream = &allowed[idx];
                        if let Some(ref sub) = watch.subtitle_url
                            && !sub.is_empty()
                        {
                            save_subtitle(sub, &job.out_path);
                        }
                        if let Ok(mut cmd) = build_ytdlp_cmd(stream, &job.out_path)
                            && let Ok(mut child) = cmd.spawn()
                        {
                            if let Ok(mut p) = self.active_progress.lock() {
                                *p = Some("Başlatılıyor...".to_string());
                            }
                            if let Some(stdout) = child.stdout.take() {
                                let prog = Arc::clone(&self.active_progress);
                                thread::spawn(move || {
                                    let reader = BufReader::new(stdout);
                                    for line in reader.lines().map_while(Result::ok) {
                                        let t = line.trim();
                                        if t.contains('|') {
                                            let parts: Vec<&str> = t.split('|').collect();
                                            if parts.len() >= 3
                                                && let Ok(mut p) = prog.lock()
                                            {
                                                *p = Some(format!(
                                                    "{}  •  {}  •  ETA {}",
                                                    parts[0].trim(),
                                                    parts[1].trim(),
                                                    parts[2].trim()
                                                ));
                                            }
                                        }
                                    }
                                    if let Ok(mut p) = prog.lock() {
                                        *p = None;
                                    }
                                });
                            }
                            let title = format!("{} — {}", job.anime.title, job.episode.title);
                            self.active_child = Some((child, job.out_path, title));
                            return;
                        }
                    }
                }
            }
            self.known_paths.remove(&job.out_path);
        }
    }

    pub fn status(&self) -> DownloadStatus {
        DownloadStatus {
            queued: self.queue.len(),
            active: self.active_child.is_some(),
            paused: self.playback_depth > 0 && self.active_child.is_some(),
            active_title: self.active_child.as_ref().map(|(_, _, t)| t.clone()),
            active_progress: self
                .active_progress
                .lock()
                .ok()
                .and_then(|g| g.clone()),
            queued_titles: self
                .queue
                .iter()
                .take(8)
                .map(|j| format!("{} — {}", j.anime.title, j.episode.title))
                .collect(),
            completed_titles: self.completed_titles.clone(),
        }
    }
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
