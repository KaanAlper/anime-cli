mod bus;
mod config;
mod download;
mod hls_proxy;
mod models;
mod player;
mod sources;
mod ui;

use anyhow::Result;
use clap::Parser;
use std::fs;

use crate::config::{
    AppConfig, HistoryEntry, cache_dir, load_config, load_history, save_config,
};
use crate::models::{Anime, Episode, SourceKind, WatchData};
use crate::player::{
    build_playback_order, has_ytdlp, is_direct_stream_url, play_mpv,
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

fn is_player_banned(label: &str, banned: &[String]) -> bool {
    let lower = label.to_ascii_lowercase();
    banned
        .iter()
        .any(|b| !b.is_empty() && lower.contains(&b.to_ascii_lowercase()))
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
    let mut selected_fansub: Option<String> = None;

    loop {
        let ep = &episodes[ep_idx];
        let active_q = manual_quality.as_deref().unwrap_or(&cfg.preferred_quality);
        let fansub_badge = selected_fansub
            .as_deref()
            .map(|f| format!("  •  Fansub: {f}"))
            .unwrap_or_default();
        let subtitle = format!(
            "{}  •  Kalite: {}{}",
            anime.source.display_name(),
            active_q,
            fansub_badge
        );
        let title = format!("{} ( {} )", anime.title, ep.title);

        let mut options = vec!["İzle".to_string()];
        if !anime.is_movie {
            options.extend([
                "Sonraki bölüm".into(),
                "Önceki bölüm".into(),
                "Bölüm seç".into(),
            ]);
        }
        let quality_label = if anime.source == SourceKind::TurkAnime {
            "Oynatıcı seç"
        } else {
            "Çözünürlük seç"
        };
        options.push("".into());
        if anime.source != SourceKind::AnimeciX {
            options.push("Fansub seç".into());
        }
        options.extend([quality_label.into(), "İndir".into(), "Geri".into()]);

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
                        let allowed_streams: Vec<_> = watch
                            .streams
                            .iter()
                            .filter(|s| !is_player_banned(&s.label, &cfg.banned_players))
                            .cloned()
                            .collect();
                        if allowed_streams.is_empty() {
                            ui::show_error("Banlı oynatıcılar hariç oynatılabilir akış bulunamadı.");
                            continue;
                        }
                        let q_pref = manual_quality.as_deref().unwrap_or(&cfg.preferred_quality);
                        let candidate_indices = build_playback_order(
                            &allowed_streams,
                            selected_fansub.as_deref(),
                            q_pref,
                        );
                        let primary_idx = candidate_indices[0];
                        let primary = &allowed_streams[primary_idx];

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

                        if cfg.download_while_watching {
                            let ep_in_season = episodes[..ep_idx]
                                .iter()
                                .filter(|e| e.season_num == ep.season_num)
                                .count();
                            bus::emit(bus::AppEvent::EnqueueDownload(download::DownloadJob {
                                anime: anime.clone(),
                                episode: ep.clone(),
                                ep_in_season,
                                quality: q_pref.to_string(),
                                banned_players: cfg.banned_players.clone(),
                                out_path: download::target_file_path(
                                    &cfg.download_dir,
                                    &anime.title,
                                    &ep.title,
                                ),
                            }));
                        }

                        let mpv_title = format!("{} - {}", anime.title, ep.title);
                        let total = candidate_indices.len();
                        let mut last_err = None;
                        let mut played = false;

                        bus::emit(bus::AppEvent::PlaybackStarted);
                        for (n, idx) in candidate_indices.into_iter().enumerate() {
                            if player::take_interrupted() {
                                break;
                            }
                            let stream = &allowed_streams[idx];
                            let fs_tag = stream
                                .fansub
                                .as_deref()
                                .map(|f| format!(" [{f}]"))
                                .unwrap_or_default();
                            println!(
                                "\x1b[38;2;228;92;192m▶\x1b[0m Oynatılıyor ({}/{}): {}{}",
                                n + 1,
                                total,
                                stream.label,
                                fs_tag
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

                        let history_entry = if played {
                            Some(HistoryEntry {
                                source: anime.source.display_name().to_string(),
                                anime_id: anime.id.clone(),
                                anime_title: anime.title.clone(),
                                is_movie: anime.is_movie,
                                episode_index: ep_idx,
                                episode_title: ep.title.clone(),
                            })
                        } else {
                            None
                        };
                        bus::emit(bus::AppEvent::PlaybackEnded {
                            history: history_entry,
                        });

                        if !played
                            && let Some(e) = last_err
                        {
                            ui::show_error(&format!("Oynatma hatası: {e}"));
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
            "Fansub seç" => {
                let res = ui::with_spinner("Fansublar yükleniyor...", || {
                    fetch_watch_cached(anime.source, anime, episodes, ep_idx)
                });
                match res {
                    Ok(watch) => {
                        let mut fansubs: Vec<String> = Vec::new();
                        for s in &watch.streams {
                            if let Some(ref f) = s.fansub
                                && !fansubs.contains(f)
                            {
                                fansubs.push(f.clone());
                            }
                        }
                        if fansubs.is_empty() {
                            ui::show_error("Bu bölüm için ayrı fansub bilgisi yok.");
                        } else {
                            let mut choices = vec!["Tümü (Otomatik)".to_string()];
                            choices.extend(fansubs);
                            if let Some(f_idx) =
                                ui::select("Fansub seç", &ep.title, &choices, 0)?
                            {
                                selected_fansub = if f_idx == 0 {
                                    None
                                } else {
                                    Some(choices[f_idx].clone())
                                };

                                // Fansub seçildikten sonra o fansub'ın oynatıcı/çözünürlük seçimini aç
                                let mut labels: Vec<String> = vec!["Tümü (Otomatik)".to_string()];
                                for s in &watch.streams {
                                    let matches_fs = selected_fansub
                                        .as_deref()
                                        .is_none_or(|f| s.fansub.as_deref() == Some(f));
                                    if matches_fs && !labels.contains(&s.label) {
                                        labels.push(s.label.clone());
                                    }
                                }
                                if labels.len() > 1
                                    && let Some(q_idx) =
                                        ui::select(quality_label, &ep.title, &labels, 0)?
                                {
                                    manual_quality = if q_idx == 0 {
                                        None
                                    } else {
                                        Some(labels[q_idx].clone())
                                    };
                                }
                            }
                        }
                    }
                    Err(e) => ui::show_error(&format!("Fansublar alınamadı: {e}")),
                }
            }
            "Çözünürlük seç" | "Oynatıcı seç" => {
                let res = ui::with_spinner("Seçenekler yükleniyor...", || {
                    fetch_watch_cached(anime.source, anime, episodes, ep_idx)
                });
                match res {
                    Ok(watch) => {
                        let mut labels: Vec<String> = vec!["Tümü (Otomatik)".to_string()];
                        for s in &watch.streams {
                            let matches_fs = selected_fansub
                                .as_deref()
                                .is_none_or(|f| s.fansub.as_deref() == Some(f));
                            if matches_fs && !labels.contains(&s.label) {
                                labels.push(s.label.clone());
                            }
                        }
                        if labels.len() <= 1 {
                            ui::show_error("Seçili fansub için akış bulunamadı.");
                        } else if let Some(q_idx) =
                            ui::select(quality_label, &ep.title, &labels, 0)?
                        {
                            manual_quality = if q_idx == 0 {
                                None
                            } else {
                                Some(labels[q_idx].clone())
                            };
                        }
                    }
                    Err(e) => ui::show_error(&format!("Seçenekler alınamadı: {e}")),
                }
            }
            "İndir" => {
                download_menu(cfg, anime, episodes, ep_idx, manual_quality.as_deref())?;
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

fn download_range(
    cfg: &AppConfig,
    anime: &Anime,
    episodes: &[Episode],
    start_idx: usize,
    end_idx: usize,
    manual_quality: Option<&str>,
) {
    let q_pref = manual_quality.unwrap_or(&cfg.preferred_quality);
    let total = end_idx - start_idx + 1;

    for idx in start_idx..=end_idx {
        let ep = &episodes[idx];
        let ep_in_season = episodes[..idx]
            .iter()
            .filter(|e| e.season_num == ep.season_num)
            .count();
        bus::emit(bus::AppEvent::EnqueueDownload(download::DownloadJob {
            anime: anime.clone(),
            episode: ep.clone(),
            ep_in_season,
            quality: q_pref.to_string(),
            banned_players: cfg.banned_players.clone(),
            out_path: download::target_file_path(&cfg.download_dir, &anime.title, &ep.title),
        }));
    }

    println!(
        "\n\x1b[38;2;139;178;127m✔\x1b[0m {total} bölüm indirme kuyruğuna eklendi -> {}",
        cfg.download_dir
    );
    println!("\x1b[38;2;102;102;102mİzlerken otomatik duraklar, bitince devam eder. Enter'a basın...\x1b[0m");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}

fn download_menu(
    cfg: &AppConfig,
    anime: &Anime,
    episodes: &[Episode],
    ep_idx: usize,
    manual_quality: Option<&str>,
) -> Result<()> {
    if anime.is_movie || episodes.len() == 1 {
        download_range(cfg, anime, episodes, 0, 0, manual_quality);
        return Ok(());
    }

    let options = vec![
        format!("Bu bölümü indir ({})", episodes[ep_idx].title),
        format!("Hepsini indir (1-{})", episodes.len()),
        "Bölüm aralığı indir (ör. 5-12)".to_string(),
        "Geri".to_string(),
    ];

    let Some(choice) = ui::select(
        "İndir",
        &format!("Klasör: {}", cfg.download_dir),
        &options,
        0,
    )?
    else {
        return Ok(());
    };

    match choice {
        0 => download_range(cfg, anime, episodes, ep_idx, ep_idx, manual_quality),
        1 => download_range(
            cfg,
            anime,
            episodes,
            0,
            episodes.len().saturating_sub(1),
            manual_quality,
        ),
        2 => {
            if let Some(range_str) = ui::input(
                &format!("Bölüm Aralığı (1-{})", episodes.len()),
                "ör. 5-12 veya 5",
            )? {
                match download::parse_episode_range(&range_str, episodes.len()) {
                    Ok((start, end)) => {
                        download_range(cfg, anime, episodes, start, end, manual_quality);
                    }
                    Err(e) => ui::show_error(&format!("{e}")),
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn settings_menu(cfg: &mut AppConfig, active_source: &mut SourceKind) -> Result<()> {
    loop {
        let dl_watch_box = if cfg.download_while_watching {
            "[x] Açık"
        } else {
            "[ ] Kapalı"
        };
        let items = vec![
            format!("Varsayılan Kaynak ({})", active_source.display_name()),
            format!("Varsayılan Kalite ({})", cfg.preferred_quality),
            format!("İndirme Dizini ({})", cfg.download_dir),
            format!("İzlerken İndir ({dl_watch_box})"),
            format!("Banlı Oynatıcılar ({} seçili)", cfg.banned_players.len()),
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
                if let Some(new_dir) = ui::input("İndirme Dizini", &cfg.download_dir)? {
                    cfg.download_dir = new_dir;
                    let _ = save_config(cfg);
                }
            }
            3 => {
                cfg.download_while_watching = !cfg.download_while_watching;
                let _ = save_config(cfg);
            }
            4 => {
                let mut known: Vec<String> = [
                    "GDRIVE",
                    "MAIL",
                    "ODNOKLASSNIKI",
                    "OK.RU",
                    "SIBNET",
                    "VK",
                    "YADISK",
                    "PIXELDRAIN",
                    "HDVID",
                    "VIDMOLY",
                    "SENDVID",
                    "MYVI",
                    "MP4UPLOAD",
                    "UQLOAD",
                    "DOODSTREAM",
                    "STREAMSB",
                    "DAILYMOTION",
                    "Beta Player",
                    "Aincrad",
                ]
                .into_iter()
                .map(String::from)
                .collect();
                for existing in &cfg.banned_players {
                    if !known.contains(existing) {
                        known.push(existing.clone());
                    }
                }
                if let Some(picked) = ui::multi_select(
                    "Banlı Oynatıcılar",
                    "Space ile seç/kaldır, Enter ile kaydet (denemelerde otomatik atlanır)",
                    &known,
                    &cfg.banned_players,
                )? {
                    cfg.banned_players = picked;
                    let _ = save_config(cfg);
                }
            }
            5 => {
                let _ = fs::remove_dir_all(cache_dir());
                bus::emit(bus::AppEvent::CacheCleared);
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
            menu.push(format!(
                "Son İzlenene Devam Et ({} — {})",
                last.anime_title, last.source
            ));
        }
        menu.extend([
            "Kaynak Değiştir".into(),
            "Ayarlar".into(),
            "Çık".into(),
        ]);

        let dl_status = bus::status();
        let dl_badge = if dl_status.active_download || dl_status.queued_downloads > 0 {
            let state = if dl_status.download_paused {
                "Duraklatıldı"
            } else {
                "İndiriliyor"
            };
            format!("  •  {state} (+{} kuyruk)", dl_status.queued_downloads)
        } else {
            String::new()
        };
        let subtitle = format!(
            "{}  •  Kalite: {}{}",
            active_source.display_name(),
            cfg.preferred_quality,
            dl_badge
        );

        let Some(idx) = ui::select("anime-cli", &subtitle, &menu, 0)? else {
            if player::take_interrupted() {
                let confirm = vec!["Evet, çık".to_string(), "Hayır, menüde kal".to_string()];
                match ui::select(
                    "Çıkmak istiyor musunuz?",
                    "Çıkmak için tekrar Ctrl+C yapın veya Evet'i seçin",
                    &confirm,
                    1,
                )? {
                    None | Some(0) => break,
                    _ => continue,
                }
            }
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
