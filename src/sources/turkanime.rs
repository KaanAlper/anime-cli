use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::time::SystemTime;
use url::Url;

use crate::config::cache_dir;
use crate::models::{Anime, AnimeSource, Episode, SourceKind, StreamOption, WatchData};
use crate::sources::{build_client, normalize_tr};

const MIRRORS: &[&str] = &[
    "https://raw.githubusercontent.com/barkeser2002/turkanime-gui/main/arsiv",
    "https://gitlab.com/AnimeDepo/animedepo/-/raw/master",
];
const CACHE_TTL_SECS: u64 = 24 * 3600;

pub struct TurkAnime {
    client: Client,
}

impl TurkAnime {
    pub fn new() -> Self {
        Self {
            client: build_client(),
        }
    }

    fn fetch_json(&self, rel_path: &str) -> Result<Value> {
        let clean = rel_path.trim_start_matches('/');
        for base in MIRRORS {
            let url = format!("{base}/{clean}");
            if let Ok(resp) = self.client.get(&url).send()
                && resp.status().is_success()
                && let Ok(val) = resp.json::<Value>()
            {
                return Ok(val);
            }
        }
        bail!("TürkAnime Arşiv dosyası okunamadı: {clean}");
    }

    fn load_dizin(&self) -> Result<Value> {
        let cache_file = cache_dir().join("turkanime_dizin.json");
        if let Ok(meta) = fs::metadata(&cache_file)
            && let Ok(modified) = meta.modified()
            && let Ok(age) = SystemTime::now().duration_since(modified)
            && age.as_secs() < CACHE_TTL_SECS
            && let Ok(bytes) = fs::read(&cache_file)
            && let Ok(val) = serde_json::from_slice::<Value>(&bytes)
        {
            return Ok(val);
        }

        let val = self.fetch_json("dizin.json")?;
        let _ = fs::create_dir_all(cache_dir());
        if let Ok(bytes) = serde_json::to_vec(&val) {
            let _ = fs::write(&cache_file, bytes);
        }
        Ok(val)
    }

    fn clean_url(raw: &str) -> Option<String> {
        let mut u = raw.trim().to_string();
        for _ in 0..5 {
            if u.starts_with("//") {
                u = format!("https:{u}");
            }
            let lower = u.to_ascii_lowercase();
            if lower.starts_with("https:http://") || lower.starts_with("https:https://") {
                u = u[6..].to_string();
                continue;
            }
            if lower.starts_with("http:http://") || lower.starts_with("http:https://") {
                u = u[5..].to_string();
                continue;
            }
            if let Ok(parsed) = Url::parse(&u) {
                let host = parsed.host_str().unwrap_or("").to_lowercase();
                if (host == "href.li" || host == "www.href.li")
                    && let Some(q) = parsed.query()
                {
                    u = urlencoding::decode(q)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| q.to_string());
                    continue;
                }
            }
            break;
        }

        let parsed = Url::parse(&u).ok()?;
        let host = parsed.host_str()?.to_lowercase();
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return None;
        }
        if host.contains("turkanime") {
            if parsed.path().trim_end_matches('/').ends_with("/pixeldrain.php")
                && let Some((_, id)) = parsed.query_pairs().find(|(k, _)| k == "id")
            {
                return Some(format!("https://pixeldrain.com/u/{id}"));
            }
            return None;
        }
        Some(u)
    }
}

impl AnimeSource for TurkAnime {
    fn kind(&self) -> SourceKind {
        SourceKind::TurkAnime
    }

    fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let dizin = self.load_dizin()?;
        let index = dizin
            .get("index")
            .and_then(|v| v.as_object())
            .context("TürkAnime dizin formatı geçersiz")?;

        let q_norm = normalize_tr(query);
        let words: Vec<&str> = q_norm.split_whitespace().collect();

        let mut scored: Vec<(i32, Anime)> = Vec::new();
        for (_group, items) in index {
            let Some(map) = items.as_object() else {
                continue;
            };
            for (slug, info) in map {
                let title = info
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or(slug.as_str());
                let t_norm = normalize_tr(title);
                let s_norm = normalize_tr(slug);

                let score = if t_norm == q_norm {
                    1000
                } else if t_norm.starts_with(&q_norm) {
                    800
                } else if t_norm.contains(&q_norm) || s_norm.contains(&q_norm) {
                    600
                } else if !words.is_empty() && words.iter().all(|w| t_norm.contains(w) || s_norm.contains(w)) {
                    400
                } else {
                    continue;
                };

                scored.push((
                    score,
                    Anime {
                        id: slug.clone(),
                        title: title.to_string(),
                        source: SourceKind::TurkAnime,
                        is_movie: false,
                    },
                ));
            }
        }

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.len().cmp(&b.1.title.len())));
        Ok(scored.into_iter().take(25).map(|(_, a)| a).collect())
    }

    fn get_episodes(&self, anime: &Anime) -> Result<Vec<Episode>> {
        let data = self.fetch_json(&format!("animeler/{}/bolumler.json", anime.id))?;
        let arr = data
            .as_array()
            .context("TürkAnime bölüm listesi geçersiz")?;

        let mut episodes = Vec::new();
        for (i, entry) in arr.iter().enumerate() {
            let Some(pair) = entry.as_array() else {
                continue;
            };
            let ep_slug = pair.first().and_then(|v| v.as_str()).unwrap_or("");
            let ep_title = pair.get(1).and_then(|v| v.as_str()).unwrap_or("Bölüm");
            if !ep_slug.is_empty() {
                episodes.push(Episode {
                    id: format!("{}/{}", anime.id, ep_slug),
                    title: ep_title.to_string(),
                    number: i + 1,
                    season_num: 1,
                });
            }
        }
        Ok(episodes)
    }

    fn get_watch_data(
        &self,
        _anime: &Anime,
        episode: &Episode,
        _ep_index_in_season: usize,
    ) -> Result<WatchData> {
        let (anime_slug, ep_slug) = episode
            .id
            .split_once('/')
            .context("Geçersiz TürkAnime bölüm kimliği")?;
        let data = self.fetch_json(&format!("animeler/{anime_slug}/{ep_slug}.json"))?;
        let arr = data.as_array().context("Bölüm akış verisi bulunamadı")?;

        let mut streams_with_prio: Vec<(usize, StreamOption)> = Vec::new();
        let mut seen = HashSet::new();

        for item in arr {
            let player = item
                .get("player")
                .and_then(|v| v.as_str())
                .unwrap_or("Oynatıcı")
                .trim()
                .to_uppercase();
            if player.starts_with("DEAD_") {
                continue;
            }
            if item.get("alive").and_then(|v| v.as_bool()) == Some(false) {
                continue;
            }
            let Some(raw_url) = item.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(url) = Self::clean_url(raw_url) else {
                continue;
            };

            let fansub_raw = item
                .get("fansub")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let fansub_name = if fansub_raw.is_empty() {
                "Varsayılan"
            } else {
                fansub_raw
            };

            if !seen.insert((url.clone(), fansub_name.to_string())) {
                continue;
            }

            let label = player.clone();

            let referer = item
                .get("referer")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    if url.contains("sibnet.ru") {
                        Some("https://video.sibnet.ru/".to_string())
                    } else if url.contains(".mp4") || url.contains(".m3u8") {
                        Some("https://www.turkanime.co/".to_string())
                    } else {
                        None
                    }
                });

            let prio = player_priority(&player);
            streams_with_prio.push((
                prio,
                StreamOption {
                    label,
                    url,
                    referer,
                    fansub: Some(fansub_name.to_string()),
                },
            ));
        }

        streams_with_prio.sort_by_key(|(prio, _)| *prio);
        let streams: Vec<StreamOption> = streams_with_prio.into_iter().map(|(_, s)| s).collect();

        if streams.is_empty() {
            bail!("Bu bölüm için çalışan arşiv bağlantısı bulunamadı");
        }

        Ok(WatchData {
            streams,
            subtitle_url: None,
        })
    }
}

fn player_priority(player: &str) -> usize {
    let norm = match player {
        "OK.RU" => "ODNOKLASSNIKI",
        "XDVID" => "HDVID",
        other => other,
    };
    const SUPPORTED: &[&str] = &[
        "YADISK",
        "ALUCARD(BETA)",
        "GDRIVE",
        "MAIL",
        "PIXELDRAIN",
        "AMATERASU(BETA)",
        "HDVID",
        "ODNOKLASSNIKI",
        "DAILYMOTION",
        "SIBNET",
        "VK",
        "VIDMOLY",
        "SENDVID",
        "MYVI",
    ];
    if let Some(pos) = SUPPORTED.iter().position(|&p| p == norm) {
        return pos;
    }
    const LAST: &[&str] = &["MP4UPLOAD", "CLONE", "STREAMSB", "YOURUPLOAD", "UQLOAD"];
    if LAST.contains(&norm) {
        return 1000;
    }
    500
}
