use anyhow::{Context, Result, bail};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, REFERER};
use reqwest::redirect::Policy;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::time::{Duration, SystemTime};
use url::Url;

use crate::config::cache_dir;
use crate::models::{Anime, AnimeSource, Episode, SourceKind, StreamOption, WatchData};
use crate::sources::{build_client, normalize_tr};

const MIRRORS: &[&str] = &["https://anizle.co", "https://anizm.com.tr", "https://anizm.pro"];
const CACHE_TTL_SECS: u64 = 24 * 3600;

pub struct Anizle {
    client: Client,
    no_redirect_client: Client,
}

impl Anizle {
    pub fn new() -> Self {
        let no_redirect_client = Client::builder()
            .timeout(Duration::from_secs(12))
            .redirect(Policy::none())
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36")
            .build()
            .expect("failed to build no-redirect client");

        Self {
            client: build_client(),
            no_redirect_client,
        }
    }

    fn fetch_from_mirrors(&self, path: &str) -> Result<(String, String)> {
        let clean_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

        for base in MIRRORS {
            let url = format!("{base}{clean_path}");
            if let Ok(resp) = self.client.get(&url).send()
                && resp.status().is_success()
                && let Ok(text) = resp.text()
                && !text.contains("Just a moment")
                && !text.contains("Checking your browser")
            {
                return Ok((base.to_string(), text));
            }
        }
        bail!("Anizle aynalarına ulaşılamadı: {clean_path}");
    }

    fn load_catalog(&self) -> Result<Vec<Value>> {
        let cache_file = cache_dir().join("anizle_db.json");
        if let Ok(meta) = fs::metadata(&cache_file)
            && let Ok(modified) = meta.modified()
            && let Ok(age) = SystemTime::now().duration_since(modified)
            && age.as_secs() < CACHE_TTL_SECS
            && let Ok(bytes) = fs::read(&cache_file)
            && let Ok(val) = serde_json::from_slice::<Vec<Value>>(&bytes)
            && !val.is_empty()
        {
            return Ok(val);
        }

        let (_, body) = self.fetch_from_mirrors("/getAnimeListForSearch")?;
        let parsed: Vec<Value> =
            serde_json::from_str(&body).context("Anizle katalog JSON ayrıştırılamadı")?;

        let _ = fs::create_dir_all(cache_dir());
        let _ = fs::write(&cache_file, body.as_bytes());
        Ok(parsed)
    }

    fn ajax_headers(referer: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("X-Requested-With", HeaderValue::from_static("XMLHttpRequest"));
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        if let Ok(val) = HeaderValue::from_str(referer) {
            h.insert(REFERER, val);
        }
        h
    }

    fn extract_hls_variants(
        &self,
        m3u8_url: &str,
        referer: &str,
        label_prefix: &str,
        fansub: Option<&str>,
    ) -> Vec<StreamOption> {
        let Ok(resp) = self
            .client
            .get(m3u8_url)
            .header(REFERER, referer)
            .header(ACCEPT, "*/*")
            .send()
        else {
            return Vec::new();
        };

        if !resp.status().is_success() {
            return Vec::new();
        }
        let Ok(body) = resp.text() else {
            return Vec::new();
        };
        if !body.trim_start().starts_with("#EXTM3U") {
            return Vec::new();
        }

        let base_url = Url::parse(m3u8_url).ok();
        let res_re = Regex::new(r#"RESOLUTION=\d+x(\d+)"#).unwrap();
        let name_re = Regex::new(r#"NAME="([^"]+)""#).unwrap();

        let mut variants = Vec::new();
        let lines: Vec<&str> = body.lines().collect();
        for i in 0..lines.len() {
            let line = lines[i].trim();
            if line.starts_with("#EXT-X-STREAM-INF:") {
                let next_uri = lines.get(i + 1).map(|s| s.trim()).unwrap_or("");
                if next_uri.is_empty() || next_uri.starts_with('#') {
                    continue;
                }
                let resolved = if next_uri.starts_with("http") {
                    next_uri.to_string()
                } else if let Some(ref b) = base_url {
                    b.join(next_uri).map(|u| u.to_string()).unwrap_or_default()
                } else {
                    continue;
                };
                if resolved.is_empty() {
                    continue;
                }

                let quality = name_re
                    .captures(line)
                    .map(|c| c[1].to_string())
                    .or_else(|| res_re.captures(line).map(|c| format!("{}p", &c[1])))
                    .unwrap_or_else(|| "HLS".to_string());

                variants.push(StreamOption {
                    label: format!("{quality} ({label_prefix})"),
                    url: resolved,
                    referer: Some(referer.to_string()),
                    fansub: fansub.map(|s| s.to_string()),
                });
            }
        }

        if variants.is_empty() {
            variants.push(StreamOption {
                label: format!("Auto ({label_prefix})"),
                url: m3u8_url.to_string(),
                referer: Some(referer.to_string()),
                fansub: fansub.map(|s| s.to_string()),
            });
        }
        variants
    }

    fn resolve_video(
        &self,
        origin: &str,
        video_id: &str,
        label_prefix: &str,
        fansub: Option<&str>,
    ) -> Vec<StreamOption> {
        let video_url = format!("{origin}/video/{video_id}");
        let Ok(resp) = self
            .client
            .get(&video_url)
            .headers(Self::ajax_headers(&format!("{origin}/")))
            .send()
        else {
            return Vec::new();
        };
        let Ok(json) = resp.json::<Value>() else {
            return Vec::new();
        };
        let player_html = json.get("player").and_then(|p| p.as_str()).unwrap_or("");
        let pid_re = Regex::new(r"/player/(\d+)").unwrap();
        let Some(cap) = pid_re.captures(player_html) else {
            return Vec::new();
        };
        let player_id = &cap[1];
        let player_url = format!("{origin}/player/{player_id}");

        // Check redirect without following unknown third-party embeds
        let Ok(first) = self
            .no_redirect_client
            .get(&player_url)
            .header(REFERER, format!("{origin}/"))
            .send()
        else {
            return Vec::new();
        };

        let (final_player_url, page_html) = if first.status().is_redirection() {
            let Some(loc) = first.headers().get("location").and_then(|v| v.to_str().ok()) else {
                return Vec::new();
            };
            let target = Url::parse(&player_url)
                .and_then(|b| b.join(loc))
                .map(|u| u.to_string())
                .unwrap_or_else(|_| loc.to_string());

            let is_anizm_player = target.contains("puffytr")
                || target.contains("anizmplayer")
                || MIRRORS.iter().any(|m| target.starts_with(m));
            if !is_anizm_player {
                return Vec::new();
            }

            let Ok(r2) = self.client.get(&target).header(REFERER, format!("{origin}/")).send() else {
                return Vec::new();
            };
            let final_u = r2.url().to_string();
            let Ok(txt) = r2.text() else {
                return Vec::new();
            };
            (final_u, txt)
        } else if first.status().is_success() {
            let Ok(txt) = first.text() else {
                return Vec::new();
            };
            (player_url, txt)
        } else {
            return Vec::new();
        };

        let master_re = Regex::new(r#"(?:nativeMasterUrl|masterUrl)\s*=\s*"([^"]+)""#).unwrap();
        for cap in master_re.captures_iter(&page_html) {
            let path = &cap[1];
            let m3u8_url = if path.starts_with("http") {
                path.to_string()
            } else if let Ok(base) = Url::parse(&final_player_url) {
                base.join(path).map(|u| u.to_string()).unwrap_or_default()
            } else {
                continue;
            };
            if m3u8_url.is_empty() {
                continue;
            }
            let variants =
                self.extract_hls_variants(&m3u8_url, &final_player_url, label_prefix, fansub);
            if !variants.is_empty() {
                return variants;
            }
        }

        Vec::new()
    }
}

impl AnimeSource for Anizle {
    fn kind(&self) -> SourceKind {
        SourceKind::Anizle
    }

    fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let catalog = self.load_catalog()?;
        let q_norm = normalize_tr(query);
        let words: Vec<&str> = q_norm.split_whitespace().collect();

        let mut scored: Vec<(i32, Anime)> = Vec::new();
        for item in &catalog {
            let title = item.get("info_title").and_then(|v| v.as_str()).unwrap_or("");
            let slug = item.get("info_slug").and_then(|v| v.as_str()).unwrap_or("");
            if title.is_empty() || slug.is_empty() {
                continue;
            }

            let t_norm = normalize_tr(title);
            let orig_norm = item
                .get("info_titleoriginal")
                .and_then(|v| v.as_str())
                .map(normalize_tr)
                .unwrap_or_default();

            let score = if t_norm == q_norm || orig_norm == q_norm {
                1000
            } else if t_norm.starts_with(&q_norm) {
                800
            } else if t_norm.contains(&q_norm) || orig_norm.contains(&q_norm) {
                600
            } else if !words.is_empty() && words.iter().all(|w| t_norm.contains(w) || orig_norm.contains(w)) {
                400
            } else {
                continue;
            };

            scored.push((
                score,
                Anime {
                    id: slug.to_string(),
                    title: title.to_string(),
                    source: SourceKind::Anizle,
                    is_movie: false,
                },
            ));
        }

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.len().cmp(&b.1.title.len())));
        Ok(scored.into_iter().take(25).map(|(_, a)| a).collect())
    }

    fn get_episodes(&self, anime: &Anime) -> Result<Vec<Episode>> {
        let (_, html) = self.fetch_from_mirrors(&format!("/{}", anime.id))?;
        let re = Regex::new(r#"href="(?:https?://[^/]+)?/([^"]+?-(\d+)-bolum[^"]*)""#).unwrap();

        let mut eps: Vec<(usize, String, String)> = Vec::new();
        let mut seen = HashSet::new();

        for cap in re.captures_iter(&html) {
            let ep_slug = cap[1].trim_matches('/').to_string();
            if let Ok(num) = cap[2].parse::<usize>()
                && seen.insert(num)
            {
                eps.push((num, ep_slug, format!("{num}. Bölüm")));
            }
        }

        eps.sort_by_key(|(num, _, _)| *num);
        if eps.is_empty() {
            bail!("Anizle üzerinde bölüm bulunamadı: {}", anime.title);
        }

        Ok(eps
            .into_iter()
            .map(|(num, id, title)| Episode {
                id,
                title,
                number: num,
                season_num: 1,
            })
            .collect())
    }

    fn get_watch_data(
        &self,
        _anime: &Anime,
        episode: &Episode,
        _ep_index_in_season: usize,
    ) -> Result<WatchData> {
        let (origin, html) = self.fetch_from_mirrors(&format!("/{}", episode.id))?;

        // v3 translator buttons
        let v3_re = Regex::new(r#"data-translator-id="(\d+)"[^>]*data-sources="([^"]+/bolum/\d+/cevirmen/\d+)""#).unwrap();
        let mut translators: Vec<(String, String)> = v3_re
            .captures_iter(&html)
            .map(|c| (format!("Çevirmen #{}", &c[1]), c[2].to_string()))
            .collect();

        if translators.is_empty() {
            let v3_alt = Regex::new(r#"data-sources="([^"]+/bolum/\d+/cevirmen/\d+)""#).unwrap();
            translators = v3_alt
                .captures_iter(&html)
                .enumerate()
                .map(|(i, c)| (format!("Çevirmen #{}", i + 1), c[1].to_string()))
                .collect();
        }

        if translators.is_empty() {
            // Legacy fallback
            let old_re = Regex::new(r#"translator="([^"]+)""#).unwrap();
            translators = old_re
                .captures_iter(&html)
                .enumerate()
                .map(|(i, c)| (format!("Fansub #{}", i + 1), c[1].to_string()))
                .collect();
        }

        let mut streams = Vec::new();
        for (fansub_name, tr_url) in translators.iter().take(4) {
            let Ok(resp) = self
                .client
                .get(tr_url)
                .headers(Self::ajax_headers(&format!("{origin}/")))
                .send()
            else {
                continue;
            };
            let Ok(json) = resp.json::<Value>() else {
                continue;
            };

            if let Some(videos) = json.get("videos").and_then(|v| v.as_array()) {
                for v in videos {
                    let Some(vid_id) = v.get("id").and_then(|i| i.as_i64()) else {
                        continue;
                    };
                    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("Player");
                    let lower_name = name.to_lowercase();
                    // Prefer Anizm's own players first (fast, direct HLS)
                    if lower_name.contains("beta")
                        || lower_name.contains("aincrad")
                        || lower_name.contains("anizm")
                        || lower_name.contains("sistenn")
                    {
                        let mut resolved = self.resolve_video(
                            &origin,
                            &vid_id.to_string(),
                            name,
                            Some(fansub_name.as_str()),
                        );
                        streams.append(&mut resolved);
                    }
                }
            }
        }

        if streams.is_empty() {
            bail!("Anizle bu bölüm için oynatılabilir HLS akışı bulamadı");
        }

        Ok(WatchData {
            streams,
            subtitle_url: None,
        })
    }
}
