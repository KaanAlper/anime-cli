mod config;
mod models;
mod player;
mod sources;

use anyhow::Result;
use clap::Parser;
use inquire::{Select, Text};
use std::fmt;

use crate::config::{AppConfig, HistoryEntry, load_config, load_history, save_config, save_history_entry};
use crate::models::{Anime, Episode, SourceKind, WatchData};
use crate::player::{pick_preferred_stream_index, play_mpv};
use crate::sources::get_source;

#[derive(Parser, Debug)]
#[command(name = "anime", version, about = "Terminalde Türkçe altyazılı anime arama ve izleme aracı")]
struct Cli {
    /// Doğrudan son izlenen animeyi devam ettir
    #[arg(long)]
    go: bool,

    /// Başlangıç kaynağı (animecix, anizle, turkanime)
    #[arg(long, short)]
    source: Option<String>,
}

struct AnimeItem(Anime);
impl fmt::Display for AnimeItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_movie {
            write!(f, "{} [Film]", self.0.title)
        } else {
            write!(f, "{}", self.0.title)
        }
    }
}

struct EpisodeItem(Episode);
impl fmt::Display for EpisodeItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.title)
    }
}

fn fetch_watch_cached(
    source_kind: SourceKind,
    anime: &Anime,
    episodes: &[Episode],
    ep_idx: usize,
) -> Result<WatchData> {
    let ep = &episodes[ep_idx];
    let ep_in_season = episodes[..ep_idx]
        .iter()
        .filter(|e| e.season_num == ep.season_num)
        .count();
    let src = get_source(source_kind);
    src.get_watch_data(anime, ep, ep_in_season)
}

fn watch_loop(
    cfg: &mut AppConfig,
    anime: &Anime,
    episodes: &[Episode],
    mut ep_idx: usize,
) -> Result<()> {
    let mut manual_quality: Option<String> = None;

    loop {
        let ep = &episodes[ep_idx];
        let active_q = manual_quality
            .as_deref()
            .unwrap_or(&cfg.preferred_quality);
        let prompt = format!(
            "{} — {} [{} | Kalite: {}]",
            anime.title,
            ep.title,
            anime.source.display_name(),
            active_q
        );

        let mut options = vec!["İzle"];
        if !anime.is_movie {
            options.extend(["Sonraki bölüm", "Önceki bölüm", "Bölüm seç"]);
        }
        options.extend(["Çözünürlük / Kaynak seç", "Geri"]);

        let choice = match Select::new(&prompt, options).with_page_size(10).prompt() {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        match choice {
            "İzle" => {
                println!("Akış hazırlanıyor...");
                match fetch_watch_cached(anime.source, anime, episodes, ep_idx) {
                    Ok(watch) => {
                        let q_pref = manual_quality
                            .as_deref()
                            .unwrap_or(&cfg.preferred_quality);
                        let stream_idx = pick_preferred_stream_index(&watch.streams, q_pref);
                        let stream = &watch.streams[stream_idx];
                        println!("Oynatılıyor: {} ({})", ep.title, stream.label);
                        let mpv_title = format!("{} - {}", anime.title, ep.title);
                        if let Err(e) = play_mpv(stream, watch.subtitle_url.as_deref(), &mpv_title) {
                            eprintln!("[!] Oynatma hatası: {e}");
                        } else {
                            let _ = save_history_entry(HistoryEntry {
                                source: anime.source.display_name().to_string(),
                                anime_id: anime.id.clone(),
                                anime_title: anime.title.clone(),
                                is_movie: anime.is_movie,
                                episode_index: ep_idx,
                                episode_title: ep.title.clone(),
                            });
                        }
                    }
                    Err(e) => eprintln!("[!] Bölüm oynatılamadı: {e}"),
                }
            }
            "Sonraki bölüm" => {
                if ep_idx + 1 < episodes.len() {
                    ep_idx += 1;
                } else {
                    println!("Zaten son bölümdesiniz.");
                }
            }
            "Önceki bölüm" => {
                if ep_idx > 0 {
                    ep_idx -= 1;
                } else {
                    println!("Zaten ilk bölümdesiniz.");
                }
            }
            "Bölüm seç" => {
                let items: Vec<EpisodeItem> = episodes.iter().cloned().map(EpisodeItem).collect();
                if let Ok(selected) = Select::new("Bölüm seç:", items)
                    .with_page_size(15)
                    .with_starting_cursor(ep_idx)
                    .prompt()
                {
                    if let Some(pos) = episodes.iter().position(|e| e.id == selected.0.id && e.number == selected.0.number) {
                        ep_idx = pos;
                    }
                }
            }
            "Çözünürlük / Kaynak seç" => {
                println!("Çözünürlükler yükleniyor...");
                match fetch_watch_cached(anime.source, anime, episodes, ep_idx) {
                    Ok(watch) => {
                        let labels: Vec<String> =
                            watch.streams.iter().map(|s| s.label.clone()).collect();
                        if let Ok(picked) = Select::new("Çözünürlük seç:", labels).prompt() {
                            manual_quality = Some(picked);
                        }
                    }
                    Err(e) => eprintln!("[!] Çözünürlükler alınamadı: {e}"),
                }
            }
            "Geri" | _ => return Ok(()),
        }
    }
}

fn search_and_watch(cfg: &mut AppConfig, source_kind: SourceKind) -> Result<()> {
    let query = match Text::new(&format!("{} — Anime ara:", source_kind.display_name())).prompt() {
        Ok(q) if !q.trim().is_empty() => q,
        _ => return Ok(()),
    };

    println!("Aranıyor...");
    let src = get_source(source_kind);
    let results = match src.search(&query) {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => {
            println!("Sonuç bulunamadı.");
            return Ok(());
        }
        Err(e) => {
            eprintln!("[!] Arama hatası: {e}");
            return Ok(());
        }
    };

    let items: Vec<AnimeItem> = results.into_iter().map(AnimeItem).collect();
    let selected = match Select::new("Anime seç:", items).with_page_size(15).prompt() {
        Ok(a) => a.0,
        Err(_) => return Ok(()),
    };

    println!("Bölümler yükleniyor...");
    let episodes = match src.get_episodes(&selected) {
        Ok(eps) if !eps.is_empty() => eps,
        Ok(_) => {
            println!("Bölüm bulunamadı.");
            return Ok(());
        }
        Err(e) => {
            eprintln!("[!] Bölümler alınamadı: {e}");
            return Ok(());
        }
    };

    let ep_idx = if selected.is_movie || episodes.len() == 1 {
        0
    } else {
        let ep_items: Vec<EpisodeItem> = episodes.iter().cloned().map(EpisodeItem).collect();
        match Select::new("Bölüm seç:", ep_items).with_page_size(15).prompt() {
            Ok(ep) => episodes
                .iter()
                .position(|e| e.id == ep.0.id && e.number == ep.0.number)
                .unwrap_or(0),
            Err(_) => return Ok(()),
        }
    };

    watch_loop(cfg, &selected, &episodes, ep_idx)
}

fn resume_last(cfg: &mut AppConfig) -> Result<()> {
    let history = load_history();
    let Some(last) = history.first() else {
        println!("Henüz izleme geçmişi yok.");
        return Ok(());
    };
    let source_kind = SourceKind::from_str(&last.source).unwrap_or(SourceKind::AnimeciX);
    let anime = Anime {
        id: last.anime_id.clone(),
        title: last.anime_title.clone(),
        source: source_kind,
        is_movie: last.is_movie,
    };
    println!("Son izlenen yükleniyor: {} ({})", anime.title, last.episode_title);
    let src = get_source(source_kind);
    let episodes = src.get_episodes(&anime)?;
    if episodes.is_empty() {
        println!("Bölüm bulunamadı.");
        return Ok(());
    }
    let idx = last.episode_index.min(episodes.len().saturating_sub(1));
    watch_loop(cfg, &anime, &episodes, idx)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = load_config();

    let mut active_source = cli
        .source
        .as_deref()
        .and_then(SourceKind::from_str)
        .or_else(|| SourceKind::from_str(&cfg.default_source))
        .unwrap_or(SourceKind::AnimeciX);

    if cli.go {
        return resume_last(&mut cfg);
    }

    loop {
        let history = load_history();
        let mut menu = vec!["Anime Ara".to_string()];
        if let Some(last) = history.first() {
            menu.push(format!("Devam Et ({} — {})", last.anime_title, last.episode_title));
        }
        menu.push(format!("Kaynak Değiştir (Aktif: {})", active_source.display_name()));
        menu.push(format!("Varsayılan Kalite (Aktif: {})", cfg.preferred_quality));
        menu.push("Çık".to_string());

        let choice = match Select::new("anime-cli", menu).prompt() {
            Ok(c) => c,
            Err(_) => break,
        };

        if choice == "Anime Ara" {
            let _ = search_and_watch(&mut cfg, active_source);
        } else if choice.starts_with("Devam Et") {
            if let Err(e) = resume_last(&mut cfg) {
                eprintln!("[!] Devam edilemedi: {e}");
            }
        } else if choice.starts_with("Kaynak Değiştir") {
            let sources: Vec<&str> = SourceKind::all().iter().map(|s| s.display_name()).collect();
            if let Ok(picked) = Select::new("Kaynak seç:", sources).prompt()
                && let Some(kind) = SourceKind::from_str(picked)
            {
                active_source = kind;
                cfg.default_source = kind.display_name().to_string();
                let _ = save_config(&cfg);
            }
        } else if choice.starts_with("Varsayılan Kalite") {
            let qualities = vec!["best", "1080p", "720p", "480p", "360p"];
            if let Ok(picked) = Select::new("Varsayılan çözünürlük tercihi:", qualities).prompt() {
                cfg.preferred_quality = picked.to_string();
                let _ = save_config(&cfg);
            }
        } else {
            break;
        }
    }

    Ok(())
}
