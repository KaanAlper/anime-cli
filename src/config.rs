use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_source")]
    pub default_source: String,
    #[serde(default = "default_quality")]
    pub preferred_quality: String,
    #[serde(default = "default_download_dir")]
    pub download_dir: String,
    #[serde(default)]
    pub download_while_watching: bool,
    #[serde(default)]
    pub banned_players: Vec<String>,
    #[serde(default)]
    pub player_priority: Vec<String>,
    #[serde(default = "default_parallel_downloads")]
    pub parallel_downloads: usize,
    #[serde(default)]
    pub use_aria2c: bool,
    #[serde(default = "default_true")]
    pub remember_position: bool,
    #[serde(default = "default_true")]
    pub show_watched_icon: bool,
}

fn default_source() -> String {
    "AnimeciX".to_string()
}

fn default_quality() -> String {
    "best".to_string()
}

fn default_parallel_downloads() -> usize {
    3
}

fn default_true() -> bool {
    true
}

fn default_download_dir() -> String {
    dirs::download_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("anime-cli")
        .to_string_lossy()
        .to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default_source: default_source(),
            preferred_quality: default_quality(),
            download_dir: default_download_dir(),
            download_while_watching: false,
            banned_players: Vec::new(),
            player_priority: Vec::new(),
            parallel_downloads: default_parallel_downloads(),
            use_aria2c: false,
            remember_position: true,
            show_watched_icon: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub source: String,
    pub anime_id: String,
    pub anime_title: String,
    pub is_movie: bool,
    pub episode_index: usize,
    pub episode_title: String,
    #[serde(default)]
    pub watched_episodes: Vec<usize>,
    #[serde(default)]
    pub downloaded_episodes: Vec<usize>,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("anime-cli")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("anime-cli")
}

fn backup_corrupt(path: &Path) {
    if !path.exists() {
        return;
    }
    let base = format!("{}.bozuk", path.display());
    let mut candidate = PathBuf::from(&base);
    let mut i = 1usize;
    while candidate.exists() {
        candidate = PathBuf::from(format!("{base}.{i}"));
        i += 1;
    }
    let _ = fs::rename(path, candidate);
}

pub fn log_error(msg: &str) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("error.log"))
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

pub fn load_config() -> AppConfig {
    let path = config_dir().join("config.json");
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
            return cfg;
        }
        backup_corrupt(&path);
    }
    let cfg = AppConfig::default();
    let _ = save_config(&cfg);
    cfg
}

pub fn save_config(cfg: &AppConfig) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let content = serde_json::to_string_pretty(cfg)?;
    fs::write(dir.join("config.json"), content)?;
    Ok(())
}

pub fn load_history() -> Vec<HistoryEntry> {
    let path = config_dir().join("history.json");
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(entries) = serde_json::from_str::<Vec<HistoryEntry>>(&content) {
            return entries;
        }
        backup_corrupt(&path);
    }
    Vec::new()
}

pub fn watched_for_anime(source: &str, anime_id: &str) -> (Vec<usize>, Option<usize>) {
    let entries = load_history();
    if let Some(e) = entries
        .into_iter()
        .find(|e| e.source == source && e.anime_id == anime_id)
    {
        let mut watched = e.watched_episodes;
        if !watched.contains(&e.episode_index) {
            watched.push(e.episode_index);
        }
        return (watched, Some(e.episode_index));
    }
    (Vec::new(), None)
}

pub fn downloaded_for_anime(source: &str, anime_id: &str) -> Vec<usize> {
    load_history()
        .into_iter()
        .find(|e| e.source == source && e.anime_id == anime_id)
        .map(|e| e.downloaded_episodes)
        .unwrap_or_default()
}

pub fn mark_episode_downloaded(
    source: &str,
    anime_id: &str,
    anime_title: &str,
    is_movie: bool,
    ep_idx: usize,
    ep_title: &str,
) -> Result<()> {
    let mut entries = load_history();
    if let Some(existing) = entries
        .iter_mut()
        .find(|e| e.source == source && e.anime_id == anime_id)
    {
        if !existing.downloaded_episodes.contains(&ep_idx) {
            existing.downloaded_episodes.push(ep_idx);
        }
    } else {
        entries.push(HistoryEntry {
            source: source.to_string(),
            anime_id: anime_id.to_string(),
            anime_title: anime_title.to_string(),
            is_movie,
            episode_index: ep_idx,
            episode_title: ep_title.to_string(),
            watched_episodes: Vec::new(),
            downloaded_episodes: vec![ep_idx],
        });
    }
    entries.truncate(50);
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("history.json"), serde_json::to_string_pretty(&entries)?)?;
    Ok(())
}

pub fn save_history_entry(mut entry: HistoryEntry) -> Result<()> {
    let mut entries = load_history();
    let prev = entries
        .iter()
        .find(|e| e.source == entry.source && e.anime_id == entry.anime_id)
        .cloned();
    let mut watched = prev
        .as_ref()
        .map(|e| {
            let mut w = e.watched_episodes.clone();
            if !w.contains(&e.episode_index) {
                w.push(e.episode_index);
            }
            w
        })
        .unwrap_or_default();
    if !watched.contains(&entry.episode_index) {
        watched.push(entry.episode_index);
    }
    entry.watched_episodes = watched;
    if let Some(p) = prev {
        for d in p.downloaded_episodes {
            if !entry.downloaded_episodes.contains(&d) {
                entry.downloaded_episodes.push(d);
            }
        }
    }

    entries.retain(|e| !(e.source == entry.source && e.anime_id == entry.anime_id));
    entries.insert(0, entry);
    entries.truncate(50);

    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("history.json"), serde_json::to_string_pretty(&entries)?)?;
    Ok(())
}
