use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_source")]
    pub default_source: String,
    #[serde(default = "default_quality")]
    pub preferred_quality: String,
    #[serde(default = "default_download_dir")]
    pub download_dir: String,
}

fn default_source() -> String {
    "AnimeciX".to_string()
}

fn default_quality() -> String {
    "best".to_string()
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

pub fn load_config() -> AppConfig {
    let path = config_dir().join("config.json");
    if let Ok(content) = fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str::<AppConfig>(&content)
    {
        return cfg;
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
    if let Ok(content) = fs::read_to_string(path)
        && let Ok(entries) = serde_json::from_str::<Vec<HistoryEntry>>(&content)
    {
        return entries;
    }
    Vec::new()
}

pub fn save_history_entry(entry: HistoryEntry) -> Result<()> {
    let mut entries = load_history();
    entries.retain(|e| !(e.source == entry.source && e.anime_id == entry.anime_id));
    entries.insert(0, entry);
    entries.truncate(50);

    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("history.json"), serde_json::to_string_pretty(&entries)?)?;
    Ok(())
}
