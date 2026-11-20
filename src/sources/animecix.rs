use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use serde_json::Value;
use std::collections::HashSet;
use url::Url;

use crate::models::{Anime, AnimeSource, Episode, SourceKind, StreamOption, WatchData};
use crate::sources::{build_client, slugify_tr};

const BASE_URL: &str = "https://animecix.tv/";
const ALT_URL: &str = "https://mangacix.net/";
const TAU_HOST: &str = "tau-video.xyz";

pub struct AnimeciX {
    client: Client,
}

impl AnimeciX {
    pub fn new() -> Self {
        Self {
            client: build_client(),
        }
    }

    fn api_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        h.insert("x-e-h", HeaderValue::from_static("=.a"));
        h
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self
            .client
            .get(url)
            .headers(Self::api_headers())
            .send()
            .with_context(|| format!("AnimeciX isteği başarısız: {url}"))?;
        if !resp.status().is_success() {
            bail!("AnimeciX HTTP {}", resp.status());
        }
        Ok(resp.json()?)
    }

    fn extract_tau_streams(&self, redirect_target: &str) -> Result<Vec<StreamOption>> {
        let resp = self
            .client
            .get(redirect_target)
            .headers(Self::api_headers())
            .send()
            .with_context(|| format!("Video yönlendirme hatası: {redirect_target}"))?;

        if resp.status().as_u16() == 422 {
            bail!("Bölüm verisi beklenen formatta değil (422)");
        }

        let final_url = resp.url().clone();
        let segments: Vec<&str> = final_url.path().split('/').collect();
        if segments.len() < 3 {
            bail!("Beklenmeyen embed URL formatı: {}", final_url);
        }
        let embed_id = segments[2];
        let vid = final_url
            .query_pairs()
            .find(|(k, _)| k == "vid")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        let api_url = format!("https://{TAU_HOST}/api/video/{embed_id}?vid={vid}");
        let video_json: Value = self
            .client
            .get(&api_url)
            .send()?
            .json()
            .context("Tau-video JSON ayrıştırılamadı")?;

        let mut streams = Vec::new();
        if let Some(urls) = video_json.get("urls").and_then(|v| v.as_array()) {
            for item in urls {
                let label = item
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Varsayılan")
                    .to_string();
                if let Some(u) = item.get("url").and_then(|v| v.as_str()) {
                    streams.push(StreamOption {
                        label,
                        url: u.to_string(),
                        referer: Some("https://yeshi.eu.org/".to_string()),
                        fansub: None,
                    });
                }
            }
        }

        if streams.is_empty() {
            bail!("Bu bölüm için oynatılabilir video akışı bulunamadı");
        }
        Ok(streams)
    }

    fn fetch_tr_caption(&self, title_id: &str, season_num: usize, ep_url: &str) -> Option<String> {
        let url = format!(
            "{ALT_URL}secure/related-videos?episode=1&season={season_num}&titleId={title_id}&videoId=637113"
        );
        let data = self.get_json(&url).ok()?;
        let videos = data.get("videos")?.as_array()?;
        let target = videos
            .iter()
            .find(|v| v.get("url").and_then(|u| u.as_str()) == Some(ep_url))
            .or_else(|| videos.first())?;
        let captions = target.get("captions")?.as_array()?;
        for cap in captions {
            if cap.get("language").and_then(|l| l.as_str()) == Some("tr")
                && let Some(u) = cap.get("url").and_then(|u| u.as_str())
            {
                return Some(u.to_string());
            }
        }
        captions
            .first()
            .and_then(|c| c.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
    }
}

impl AnimeSource for AnimeciX {
    fn kind(&self) -> SourceKind {
        SourceKind::AnimeciX
    }

    fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let q = slugify_tr(query);
        let url = format!("{BASE_URL}secure/search/{q}?type=&limit=20");
        let data = self.get_json(&url)?;
        let results = data
            .get("results")
            .and_then(|r| r.as_array())
            .context("AnimeciX arama sonuçları alınamadı")?;

        let mut list = Vec::new();
        for item in results {
            let Some(id) = item.get("id").and_then(|v| v.as_i64()) else {
                continue;
            };
            let Some(title) = item.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let title_type = item
                .get("title_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            list.push(Anime {
                id: id.to_string(),
                title: title.to_string(),
                source: SourceKind::AnimeciX,
                is_movie: title_type.eq_ignore_ascii_case("movie"),
            });
        }
        Ok(list)
    }

    fn get_episodes(&self, anime: &Anime) -> Result<Vec<Episode>> {
        if anime.is_movie {
            return Ok(vec![Episode {
                id: format!("movie:{}", anime.id),
                title: format!("{} (Film)", anime.title),
                number: 1,
                season_num: 1,
            }]);
        }

        let first_url = format!(
            "{ALT_URL}secure/related-videos?episode=1&season=1&titleId={}&videoId=637113",
            anime.id
        );
        let first_data = self.get_json(&first_url)?;
        let season_count = first_data
            .get("videos")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("title"))
            .and_then(|t| t.get("seasons"))
            .and_then(|s| s.as_array())
            .map(|s| s.len())
            .unwrap_or(1)
            .max(1);

        let mut episodes = Vec::new();
        let mut seen = HashSet::new();

        for s in 1..=season_count {
            let data = if s == 1 {
                first_data.clone()
            } else {
                let u = format!(
                    "{ALT_URL}secure/related-videos?episode=1&season={s}&titleId={}&videoId=637113",
                    anime.id
                );
                self.get_json(&u)?
            };

            if let Some(videos) = data.get("videos").and_then(|v| v.as_array()) {
                for v in videos {
                    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("Bölüm");
                    let ep_url = v.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let season_num = v
                        .get("season_num")
                        .and_then(|n| n.as_u64())
                        .unwrap_or(s as u64) as usize;
                    if !ep_url.is_empty() && seen.insert(name.to_string()) {
                        episodes.push(Episode {
                            id: ep_url.to_string(),
                            title: name.to_string(),
                            number: episodes.len() + 1,
                            season_num,
                        });
                    }
                }
            }
        }

        // Fallback for OVAs/Movies tagged with title_type="anime" instead of "movie"
        if episodes.is_empty() {
            let title_url = format!("{BASE_URL}secure/titles/{}?titleId={}", anime.id, anime.id);
            if let Ok(title_data) = self.get_json(&title_url)
                && title_data
                    .get("title")
                    .and_then(|t| t.get("videos"))
                    .and_then(|v| v.as_array())
                    .is_some_and(|arr| !arr.is_empty())
            {
                episodes.push(Episode {
                    id: format!("movie:{}", anime.id),
                    title: format!("{} (Film/OVA)", anime.title),
                    number: 1,
                    season_num: 1,
                });
            }
        }

        Ok(episodes)
    }

    fn get_watch_data(
        &self,
        anime: &Anime,
        episode: &Episode,
        _ep_index_in_season: usize,
    ) -> Result<WatchData> {
        if anime.is_movie || episode.id.starts_with("movie:") {
            let url = format!("{BASE_URL}secure/titles/{}?titleId={}", anime.id, anime.id);
            let data = self.get_json(&url)?;
            let videos = data
                .get("title")
                .and_then(|t| t.get("videos"))
                .and_then(|v| v.as_array())
                .context("Film video listesi bulunamadı")?;

            for v in videos {
                let Some(video_url) = v.get("url").and_then(|u| u.as_str()) else {
                    continue;
                };
                if let Ok(streams) = self.extract_tau_streams(video_url) {
                    let sub = v
                        .get("captions")
                        .and_then(|c| c.as_array())
                        .and_then(|caps| {
                            caps.iter()
                                .find(|c| c.get("language").and_then(|l| l.as_str()) == Some("tr"))
                                .or_else(|| caps.first())
                        })
                        .and_then(|c| c.get("url"))
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string());
                    return Ok(WatchData {
                        streams,
                        subtitle_url: sub,
                    });
                }
            }
            bail!("Film için çalışan video bağlantısı bulunamadı");
        }

        let target = if episode.id.starts_with("http") {
            episode.id.clone()
        } else {
            format!("{BASE_URL}{}", episode.id.trim_start_matches('/'))
        };
        // Validate URL
        let _ = Url::parse(&target)?;
        let streams = self.extract_tau_streams(&target)?;
        let subtitle_url = self.fetch_tr_caption(&anime.id, episode.season_num, &episode.id);

        Ok(WatchData {
            streams,
            subtitle_url,
        })
    }
}
