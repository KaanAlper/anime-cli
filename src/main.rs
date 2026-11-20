mod config;
mod models;
mod player;
mod sources;
mod ui;

use anyhow::Result;
use clap::Parser;
use std::fs;

use crate::config::{
    AppConfig, HistoryEntry, cache_dir, load_config, load_history, save_config, save_history_entry,
};
use crate::models::{Anime, Episode, SourceKind, WatchData};
use crate::player::{
    has_ytdlp, is_direct_stream_url, pick_preferred_stream_index, play_mpv,
};
use crate::sources::get_source;

#[derive(Parser, Debug)]
#[command(
    name = "anime",
    version,
    about = "Terminalde Türkçe altyazılı anime arama ve izleme aracı"
)]
struct Cli {
    /// Doğrudan son izlenen animeyi devam ettir
    #[arg(long)]
    go: bool,

    /// Başlangıç kaynağı (animecix, anizle, turkanime)
    #[arg(long, short)]
    source: Option<String>,
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
        let active_q = manual_quality.as_deref().unwrap_or(&cfg.preferred_quality);
        let subtitle = format!("{}  •  Kalite: {}", anime.source.display_name(), active_q);
        let title = format!("{} ( {} )", anime.title, ep.title);

        let mut options = vec!["İzle".to_string()];
        if !anime.is_movie {
            options.extend([
                "Sonraki bölüm".into(),
                "Önceki bölüm".into(),
                "Bölüm seç".into(),
            ]);
        }
        options.extend([
            "".into(),
            "Çözünürlük seç".into(),
            "Geri".into(),
        ]);

        let Some(picked_idx) = ui::select(&title, &subtitle, &options, 0)? else {
            return Ok(());
        };

        match options[picked_idx].as_str() {
            "İzle" => {
                let res = ui::with_spinner("Akış hazırlanıyor...", || {
                    fetch_watch_cached(anime.source, anime, episodes, ep_idx)
                });
                match res {
                    Ok(watch) => {
                        let q_pref = manual_quality.as_deref().unwrap_or(&cfg.preferred_quality);
                        let primary_idx = pick_preferred_stream_index(&watch.streams, q_pref);
                        let primary = &watch.streams[primary_idx];

                        if !is_direct_stream_url(&primary.url) && !has_ytdlp() {
                            let choices = vec![
                                "Evet, 'sudo pacman -S yt-dlp' ile kur".to_string(),
                                "Hayır, vazgeç".to_string(),
                            ];
                            let picked = ui::select(
                                "yt-dlp kurulu değil",
                                "Bu kaynak embed oynatıcı kullanıyor; izlemek için yt-dlp gerekli.",
                                &choices,
                                0,
                            )?;
                            if picked == Some(0) {
                                ui::suspend();
                                let status = std::process::Command::new("sudo")
                                    .args(["pacman", "-S", "yt-dlp"])
                                    .status();
                                let _ = ui::resume();
                                if !matches!(status, Ok(s) if s.success()) {
                                    ui::show_error("yt-dlp kurulumu tamamlanamadı.");
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        }

                        // Build candidate order: preferred first, then fallback to other streams if manual_quality wasn't locked
                        let mut candidate_indices = vec![primary_idx];
                        if manual_quality.is_none() {
                            for i in 0..watch.streams.len() {
                                if i != primary_idx && candidate_indices.len() < 6 {
                                    candidate_indices.push(i);
                                }
                            }
                        }

                        let mpv_title = format!("{} - {}", anime.title, ep.title);
                        let total = candidate_indices.len();
                        let mut last_err = None;
                        let mut played = false;
                        for (n, idx) in candidate_indices.into_iter().enumerate() {
                            if player::take_interrupted() {
                                break;
                            }
                            let stream = &watch.streams[idx];
                            println!(
                                "\x1b[38;2;228;92;192m▶\x1b[0m Oynatılıyor ({}/{}): {}",
                                n + 1,
                                total,
                                stream.label
                            );
                            match play_mpv(stream, watch.subtitle_url.as_deref(), &mpv_title) {
                                Ok(()) => {
                                    played = true;
                                    break;
                                }
                                Err(e) => {
                                    if player::take_interrupted() {
                                        played = true;
                                        break;
                                    }
                                    last_err = Some(e);
                                }
                            }
                        }

                        if !played {
                            if let Some(e) = last_err {
                                ui::show_error(&format!("Oynatma hatası: {e}"));
                            }
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
                    Err(e) => ui::show_error(&format!("Bölüm oynatılamadı: {e}")),
                }
            }
            "Sonraki bölüm" => {
                if ep_idx + 1 < episodes.len() {
                    ep_idx += 1;
                } else {
                    ui::show_error("Zaten son bölümdesiniz.");
                }
            }
            "Önceki bölüm" => {
                if ep_idx > 0 {
                    ep_idx -= 1;
                } else {
                    ui::show_error("Zaten ilk bölümdesiniz.");
                }
            }
            "Bölüm seç" => {
                let ep_titles: Vec<String> = episodes.iter().map(|e| e.title.clone()).collect();
                if let Some(idx) = ui::select("Bölüm seç", &anime.title, &ep_titles, ep_idx)? {
                    ep_idx = idx;
                }
            }
            "Çözünürlük seç" => {
                let res = ui::with_spinner("Çözünürlükler yükleniyor...", || {
                    fetch_watch_cached(anime.source, anime, episodes, ep_idx)
                });
                match res {
                    Ok(watch) => {
                        let labels: Vec<String> =
                            watch.streams.iter().map(|s| s.label.clone()).collect();
                        if let Some(q_idx) =
                            ui::select("Çözünürlük seç", &ep.title, &labels, 0)?
                        {
                            manual_quality = Some(labels[q_idx].clone());
                        }
                    }
                    Err(e) => ui::show_error(&format!("Çözünürlükler alınamadı: {e}")),
                }
            }
            _ => return Ok(()),
        }
    }
}

fn search_and_watch(cfg: &mut AppConfig, source_kind: SourceKind) -> Result<()> {
    let Some(query) = ui::input(
        &format!("Anime Ara ({})", source_kind.display_name()),
        "Anime adını yazın...",
    )?
    else {
        return Ok(());
    };

    let src = get_source(source_kind);
    let results = match ui::with_spinner("Aranıyor...", || src.search(&query)) {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => {
            ui::show_error("Sonuç bulunamadı.");
            return Ok(());
        }
        Err(e) => {
            ui::show_error(&format!("Arama hatası: {e}"));
            return Ok(());
        }
    };

    let titles: Vec<String> = results
        .iter()
        .map(|a| {
            if a.is_movie {
                format!("{} [Film]", a.title)
            } else {
                a.title.clone()
            }
        })
        .collect();

    let Some(anime_idx) = ui::select(
        "Anime seç",
        &format!("{} sonuç ({})", results.len(), source_kind.display_name()),
        &titles,
        0,
    )?
    else {
        return Ok(());
    };
    let selected = &results[anime_idx];

    let episodes = match ui::with_spinner("Bölümler yükleniyor...", || src.get_episodes(selected))
    {
        Ok(eps) if !eps.is_empty() => eps,
        Ok(_) => {
            ui::show_error("Bölüm bulunamadı.");
            return Ok(());
        }
        Err(e) => {
            ui::show_error(&format!("Bölümler alınamadı: {e}"));
            return Ok(());
        }
    };

    let ep_idx = if selected.is_movie || episodes.len() == 1 {
        0
    } else {
        let ep_titles: Vec<String> = episodes.iter().map(|e| e.title.clone()).collect();
        match ui::select("Bölüm seç", &selected.title, &ep_titles, 0)? {
            Some(i) => i,
            None => return Ok(()),
        }
    };

    watch_loop(cfg, selected, &episodes, ep_idx)
}

fn resume_last(cfg: &mut AppConfig) -> Result<()> {
    let history = load_history();
    let Some(last) = history.first() else {
        ui::show_error("Henüz izleme geçmişi yok.");
        return Ok(());
    };
    let source_kind = SourceKind::from_str(&last.source).unwrap_or(SourceKind::AnimeciX);
    let anime = Anime {
        id: last.anime_id.clone(),
        title: last.anime_title.clone(),
        source: source_kind,
        is_movie: last.is_movie,
    };
    let src = get_source(source_kind);
    let episodes = ui::with_spinner(
        &format!("Yükleniyor: {}...", anime.title),
        || src.get_episodes(&anime),
    )?;
    if episodes.is_empty() {
        ui::show_error("Bölüm bulunamadı.");
        return Ok(());
    }
    let idx = last.episode_index.min(episodes.len().saturating_sub(1));
    watch_loop(cfg, &anime, &episodes, idx)
}

fn settings_menu(cfg: &mut AppConfig, active_source: &mut SourceKind) -> Result<()> {
    loop {
        let items = vec![
            format!("Varsayılan Kaynak ({})", active_source.display_name()),
            format!("Varsayılan Kalite ({})", cfg.preferred_quality),
            "Önbelleği Temizle".into(),
            "Geri".into(),
        ];

        let Some(idx) = ui::select("Ayarlar", "Tercihlerinizi düzenleyin", &items, 0)? else {
            return Ok(());
        };

        match idx {
            0 => {
                let sources: Vec<String> = SourceKind::all()
                    .iter()
                    .map(|s| s.display_name().to_string())
                    .collect();
                if let Some(s_idx) =
                    ui::select("Varsayılan Kaynak", active_source.display_name(), &sources, 0)?
                    && let Some(kind) = SourceKind::from_str(&sources[s_idx])
                {
                    *active_source = kind;
                    cfg.default_source = kind.display_name().to_string();
                    let _ = save_config(cfg);
                }
            }
            1 => {
                let qualities: Vec<String> = ["best", "1080p", "720p", "480p", "360p"]
                    .into_iter()
                    .map(String::from)
                    .collect();
                if let Some(q_idx) = ui::select(
                    "Varsayılan Kalite",
                    &format!("Aktif: {}", cfg.preferred_quality),
                    &qualities,
                    0,
                )? {
                    cfg.preferred_quality = qualities[q_idx].clone();
                    let _ = save_config(cfg);
                }
            }
            2 => {
                let _ = fs::remove_dir_all(cache_dir());
                ui::show_error("Önbellek temizlendi.");
            }
            _ => return Ok(()),
        }
    }
}

fn main() -> Result<()> {
    let _ = ctrlc::set_handler(|| {
        player::INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let _ui = ui::UiSession::start()?;
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
            menu.push(format!("Son İzlenene Devam Et ({})", last.anime_title));
        }
        menu.extend([
            "Kaynak Değiştir".into(),
            "Ayarlar".into(),
            "Çık".into(),
        ]);

        let subtitle = format!(
            "{}  •  Kalite: {}",
            active_source.display_name(),
            cfg.preferred_quality
        );

        let Some(idx) = ui::select("anime-cli", &subtitle, &menu, 0)? else {
            break;
        };

        let choice = &menu[idx];
        if choice == "Anime Ara" {
            let _ = search_and_watch(&mut cfg, active_source);
        } else if choice.starts_with("Son İzlenene") {
            if let Err(e) = resume_last(&mut cfg) {
                ui::show_error(&format!("Devam edilemedi: {e}"));
            }
        } else if choice == "Kaynak Değiştir" {
            let sources: Vec<String> = SourceKind::all()
                .iter()
                .map(|s| s.display_name().to_string())
                .collect();
            if let Some(s_idx) =
                ui::select("Kaynak seç", active_source.display_name(), &sources, 0)?
                && let Some(kind) = SourceKind::from_str(&sources[s_idx])
            {
                active_source = kind;
                cfg.default_source = kind.display_name().to_string();
                let _ = save_config(&cfg);
            }
        } else if choice == "Ayarlar" {
            let _ = settings_menu(&mut cfg, &mut active_source);
        } else {
            break;
        }
    }

    Ok(())
}
