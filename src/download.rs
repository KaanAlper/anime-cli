use anyhow::{Context, Result, bail};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

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
        .arg("--quiet")
        .arg("--no-warnings")
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
        .stdout(Stdio::null())
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
    pub out_path: PathBuf,
}

#[derive(Debug)]
pub enum DownloadEvent {
    Enqueue(DownloadJob),
    PlaybackStarted,
    PlaybackEnded,
}

#[derive(Debug, Default, Clone)]
pub struct BusStatus {
    pub queued: usize,
    pub active: bool,
    pub paused: bool,
}

struct EventBus {
    tx: Sender<DownloadEvent>,
    status: Arc<Mutex<BusStatus>>,
}

static BUS: OnceLock<EventBus> = OnceLock::new();

fn signal_pid(pid: u32, sig: &str) {
    let _ = Command::new("kill")
        .args([sig, &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn init_bus() -> &'static EventBus {
    BUS.get_or_init(|| {
        let (tx, rx): (Sender<DownloadEvent>, Receiver<DownloadEvent>) = mpsc::channel();
        let status = Arc::new(Mutex::new(BusStatus::default()));
        let status_clone = Arc::clone(&status);

        thread::spawn(move || {
            let mut queue: VecDeque<DownloadJob> = VecDeque::new();
            let mut known_paths: HashSet<PathBuf> = HashSet::new();
            let mut active_child: Option<(Child, PathBuf)> = None;
            let mut playback_depth: usize = 0;

            loop {
                while let Ok(ev) = rx.try_recv() {
                    match ev {
                        DownloadEvent::Enqueue(job) => {
                            if !job.out_path.exists() && known_paths.insert(job.out_path.clone()) {
                                queue.push_back(job);
                            }
                        }
                        DownloadEvent::PlaybackStarted => {
                            playback_depth += 1;
                            if playback_depth == 1
                                && let Some((ref child, _)) = active_child
                            {
                                signal_pid(child.id(), "-STOP");
                            }
                        }
                        DownloadEvent::PlaybackEnded => {
                            playback_depth = playback_depth.saturating_sub(1);
                            if playback_depth == 0
                                && let Some((ref child, _)) = active_child
                            {
                                signal_pid(child.id(), "-CONT");
                            }
                        }
                    }
                }

                // Reap finished child if any
                if let Some((mut child, path)) = active_child.take() {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            known_paths.remove(&path);
                        }
                        Ok(None) => {
                            active_child = Some((child, path));
                        }
                        Err(_) => {
                            known_paths.remove(&path);
                        }
                    }
                }

                // Start next job only when not paused and no active child
                if playback_depth == 0
                    && active_child.is_none()
                    && let Some(job) = queue.pop_front()
                {
                    if !job.out_path.exists() {
                        let src = get_source(job.anime.source);
                        if let Ok(watch) =
                            src.get_watch_data(&job.anime, &job.episode, job.ep_in_season)
                            && !watch.streams.is_empty()
                        {
                            let idx =
                                pick_preferred_stream_index(&watch.streams, &job.quality);
                            let stream = &watch.streams[idx];
                            if let Some(ref sub) = watch.subtitle_url
                                && !sub.is_empty()
                            {
                                save_subtitle(sub, &job.out_path);
                            }
                            if let Ok(mut cmd) = build_ytdlp_cmd(stream, &job.out_path)
                                && let Ok(child) = cmd.spawn()
                            {
                                active_child = Some((child, job.out_path.clone()));
                            } else {
                                known_paths.remove(&job.out_path);
                            }
                        } else {
                            known_paths.remove(&job.out_path);
                        }
                    } else {
                        known_paths.remove(&job.out_path);
                    }
                }

                if let Ok(mut st) = status_clone.lock() {
                    st.queued = queue.len();
                    st.active = active_child.is_some();
                    st.paused = playback_depth > 0 && active_child.is_some();
                }

                thread::sleep(Duration::from_millis(150));
            }
        });

        EventBus { tx, status }
    })
}

pub fn emit(event: DownloadEvent) {
    let bus = init_bus();
    let _ = bus.tx.send(event);
}

pub fn get_status() -> BusStatus {
    let bus = init_bus();
    bus.status.lock().map(|s| s.clone()).unwrap_or_default()
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
