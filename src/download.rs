use anyhow::{Context, Result, bail};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::config::{log_error, mark_episode_downloaded};
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

fn is_hls_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".m3u8")
        || lower.contains("/m3/")
        || lower.contains("master.txt")
        || lower.contains("puffytr")
}

fn build_ytdlp_cmd(stream: &StreamOption, out_path: &Path, use_aria2c: bool) -> Result<Command> {
    let ytdlp = if has_command("yt-dlp") {
        "yt-dlp"
    } else if has_command("youtube-dl") {
        "youtube-dl"
    } else {
        bail!("İndirme için 'yt-dlp' kurulu olmalı (sudo pacman -S yt-dlp).");
    };

    let mut cmd = Command::new(ytdlp);
    cmd.arg("--no-part")
        .arg("--no-playlist")
        .arg("--newline")
        .arg("--progress")
        .arg("--no-warnings")
        .arg("--progress-template")
        .arg("download:%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s")
        .arg("--user-agent")
        .arg("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/137.0.0.0 Safari/537.36");

    if let Some(referer) = stream.referer.as_deref().filter(|r| !r.is_empty()) {
        cmd.arg("--referer").arg(referer);
    }

    if is_hls_url(&stream.url) {
        cmd.arg("--downloader")
            .arg("ffmpeg")
            .arg("--downloader-args")
            .arg("ffmpeg_i:-allowed_extensions ALL");
    } else if use_aria2c && has_command("aria2c") {
        cmd.arg("--downloader")
            .arg("aria2c")
            .arg("--downloader-args")
            .arg("aria2c:-x 8 -s 8 -k 1M");
    }

    cmd.arg("-o")
        .arg(out_path)
        .arg(&stream.url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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
    pub episode_index: usize,
    pub ep_in_season: usize,
    pub quality: String,
    pub banned_players: Vec<String>,
    pub out_path: PathBuf,
    pub parallel_downloads: usize,
    pub use_aria2c: bool,
}

#[derive(Debug, Default, Clone)]
pub struct DownloadStatus {
    pub queued: usize,
    pub active: bool,
    pub paused: bool,
    pub active_title: Option<String>,
    pub active_progress: Option<String>,
    pub active_items: Vec<(String, Option<String>)>,
    pub queued_titles: Vec<String>,
    pub completed_titles: Vec<String>,
    pub failed_titles: Vec<String>,
}

fn signal_pid(pid: u32, sig: &str) {
    let _ = Command::new("kill")
        .args([sig, &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

struct ActiveDownload {
    child: Child,
    path: PathBuf,
    title: String,
    progress: Arc<Mutex<Option<String>>>,
    last_err: Arc<Mutex<Option<String>>>,
    job: DownloadJob,
}

pub struct DownloadManager {
    queue: VecDeque<DownloadJob>,
    known_paths: HashSet<PathBuf>,
    active_children: Vec<ActiveDownload>,
    completed_titles: Vec<String>,
    failed_titles: Vec<String>,
    playback_depth: usize,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            known_paths: HashSet::new(),
            active_children: Vec::new(),
            completed_titles: Vec::new(),
            failed_titles: Vec::new(),
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
        if self.playback_depth == 1 {
            for act in &self.active_children {
                signal_pid(act.child.id(), "-STOP");
            }
        }
    }

    pub fn on_playback_ended(&mut self) {
        self.playback_depth = self.playback_depth.saturating_sub(1);
        if self.playback_depth == 0 {
            for act in &self.active_children {
                signal_pid(act.child.id(), "-CONT");
            }
        }
    }

    pub fn tick(&mut self) {
        let mut still_running = Vec::new();
        for mut act in self.active_children.drain(..) {
            match act.child.try_wait() {
                Ok(Some(st)) => {
                    self.known_paths.remove(&act.path);
                    if st.success() {
                        let _ = mark_episode_downloaded(
                            act.job.anime.source.display_name(),
                            &act.job.anime.id,
                            &act.job.anime.title,
                            act.job.anime.is_movie,
                            act.job.episode_index,
                            &act.job.episode.title,
                        );
                        self.completed_titles.push(act.title);
                        if self.completed_titles.len() > 10 {
                            self.completed_titles.remove(0);
                        }
                    } else {
                        let detail = act
                            .last_err
                            .lock()
                            .ok()
                            .and_then(|g| g.clone())
                            .unwrap_or_else(|| format!("yt-dlp çıkış kodu: {:?}", st.code()));
                        let msg = format!("{} ({detail})", act.title);
                        log_error(&format!("İndirme başarısız: {msg}"));
                        self.failed_titles.push(msg);
                    }
                }
                Ok(None) => still_running.push(act),
                Err(e) => {
                    self.known_paths.remove(&act.path);
                    let msg = format!("{} ({e})", act.title);
                    log_error(&format!("İndirme süreç hatası: {msg}"));
                    self.failed_titles.push(msg);
                }
            }
        }
        self.active_children = still_running;

        let max_parallel = self
            .queue
            .front()
            .map(|j| j.parallel_downloads.clamp(1, 10))
            .unwrap_or(1);

        while self.playback_depth == 0 && self.active_children.len() < max_parallel {
            let Some(job) = self.queue.pop_front() else {
                break;
            };
            if !job.out_path.exists() {
                let job_title = format!("{} — {}", job.anime.title, job.episode.title);
                let src = get_source(job.anime.source);
                match src.get_watch_data(&job.anime, &job.episode, job.ep_in_season) {
                    Ok(watch) => {
                        let allowed: Vec<_> = watch
                            .streams
                            .into_iter()
                            .filter(|s| {
                                !job.banned_players
                                    .iter()
                                    .any(|b| s.label.to_ascii_lowercase().contains(&b.to_ascii_lowercase()))
                            })
                            .collect();
                        if allowed.is_empty() {
                            let msg = format!("{job_title} (uygun oynatıcı yok)");
                            log_error(&format!("İndirme başarısız: {msg}"));
                            self.failed_titles.push(msg);
                        } else {
                            let idx = pick_preferred_stream_index(&allowed, &job.quality);
                            let stream = &allowed[idx];
                            if let Some(ref sub) = watch.subtitle_url
                                && !sub.is_empty()
                            {
                                save_subtitle(sub, &job.out_path);
                            }
                            match build_ytdlp_cmd(stream, &job.out_path, job.use_aria2c)
                                .and_then(|mut c| c.spawn().map_err(Into::into))
                            {
                                Ok(mut child) => {
                                    let progress =
                                        Arc::new(Mutex::new(Some("Başlatılıyor...".to_string())));
                                    let last_err = Arc::new(Mutex::new(None));
                                    if let Some(stdout) = child.stdout.take() {
                                        let prog = Arc::clone(&progress);
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
                                    if let Some(stderr) = child.stderr.take() {
                                        let err_arc = Arc::clone(&last_err);
                                        thread::spawn(move || {
                                            let reader = BufReader::new(stderr);
                                            for line in reader.lines().map_while(Result::ok) {
                                                let t = line.trim();
                                                if !t.is_empty()
                                                    && let Ok(mut e) = err_arc.lock()
                                                {
                                                    *e = Some(t.chars().take(90).collect());
                                                }
                                            }
                                        });
                                    }
                                    let path = job.out_path.clone();
                                    self.active_children.push(ActiveDownload {
                                        child,
                                        path,
                                        title: job_title,
                                        progress,
                                        last_err,
                                        job,
                                    });
                                    continue;
                                }
                                Err(e) => {
                                    let msg = format!("{job_title} ({e})");
                                    log_error(&format!("İndirme başlatılamadı: {msg}"));
                                    self.failed_titles.push(msg);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("{job_title} ({e})");
                        log_error(&format!("İndirme linki alınamadı: {msg}"));
                        self.failed_titles.push(msg);
                    }
                }
            } else {
                let job_title = format!("{} — {} (zaten var)", job.anime.title, job.episode.title);
                self.completed_titles.push(job_title);
            }
            self.known_paths.remove(&job.out_path);
        }
    }

    pub fn status(&self) -> DownloadStatus {
        let active_items: Vec<(String, Option<String>)> = self
            .active_children
            .iter()
            .map(|a| {
                let prog = a.progress.lock().ok().and_then(|g| g.clone());
                (a.title.clone(), prog)
            })
            .collect();
        let first = active_items.first();
        DownloadStatus {
            queued: self.queue.len(),
            active: !self.active_children.is_empty(),
            paused: self.playback_depth > 0 && !self.active_children.is_empty(),
            active_title: first.map(|(t, _)| t.clone()),
            active_progress: first.and_then(|(_, p)| p.clone()),
            active_items,
            queued_titles: self
                .queue
                .iter()
                .take(8)
                .map(|j| format!("{} — {}", j.anime.title, j.episode.title))
                .collect(),
            completed_titles: self.completed_titles.clone(),
            failed_titles: self.failed_titles.clone(),
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
