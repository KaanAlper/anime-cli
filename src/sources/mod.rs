pub mod animecix;
pub mod anizle;
pub mod turkanime;

use reqwest::blocking::Client;
use std::time::Duration;

use crate::models::{AnimeSource, SourceKind};

pub fn build_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36")
        .build()
        .expect("failed to build HTTP client")
}

pub fn normalize_tr(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            'ç' | 'Ç' => out.push('c'),
            'ğ' | 'Ğ' => out.push('g'),
            'ı' | 'İ' => out.push('i'),
            'ö' | 'Ö' => out.push('o'),
            'ş' | 'Ş' => out.push('s'),
            'ü' | 'Ü' => out.push('u'),
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_lowercase()),
            c if c.is_whitespace() || c == '-' || c == '_' => out.push(' '),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn slugify_tr(input: &str) -> String {
    normalize_tr(input).replace(' ', "-")
}

pub fn get_source(kind: SourceKind) -> Box<dyn AnimeSource> {
    match kind {
        SourceKind::AnimeciX => Box::new(animecix::AnimeciX::new()),
        SourceKind::Anizle => Box::new(anizle::Anizle::new()),
        SourceKind::TurkAnime => Box::new(turkanime::TurkAnime::new()),
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use std::time::Instant;

    fn test_source_live(kind: SourceKind, query: &str) {
        let t0 = Instant::now();
        let src = get_source(kind);
        let results = src.search(query).expect("search failed");
        println!(
            "[{}] search('{}') -> {} results in {:.2}s (first: {})",
            kind.display_name(),
            query,
            results.len(),
            t0.elapsed().as_secs_f32(),
            results.first().map(|a| a.title.as_str()).unwrap_or("-")
        );
        assert!(!results.is_empty(), "{} returned 0 search results", kind.display_name());

        let anime = results
            .iter()
            .find(|a| !a.is_movie)
            .unwrap_or(&results[0]);
        let t1 = Instant::now();
        let eps = src.get_episodes(anime).expect("get_episodes failed");
        println!(
            "[{}] get_episodes('{}') -> {} eps in {:.2}s (first: {})",
            kind.display_name(),
            anime.title,
            eps.len(),
            t1.elapsed().as_secs_f32(),
            eps.first().map(|e| e.title.as_str()).unwrap_or("-")
        );
        assert!(!eps.is_empty(), "{} returned 0 episodes", kind.display_name());

        let t2 = Instant::now();
        let watch = src
            .get_watch_data(anime, &eps[0], 0)
            .expect("get_watch_data failed");
        println!(
            "[{}] get_watch_data('{}') -> {} streams in {:.2}s (sample: {:?}, fansub: {:?})",
            kind.display_name(),
            eps[0].title,
            watch.streams.len(),
            t2.elapsed().as_secs_f32(),
            watch.streams.first().map(|s| &s.label),
            watch.streams.first().and_then(|s| s.fansub.as_deref()),
        );
        assert!(!watch.streams.is_empty(), "{} returned 0 streams", kind.display_name());
    }

    #[test]
    #[ignore]
    fn live_animecix() {
        test_source_live(SourceKind::AnimeciX, "frieren");
        test_source_live(SourceKind::AnimeciX, "naruto");
    }

    #[test]
    #[ignore]
    fn live_anizle() {
        test_source_live(SourceKind::Anizle, "naruto");
    }

    #[test]
    #[ignore]
    fn live_turkanime() {
        test_source_live(SourceKind::TurkAnime, "naruto");
    }
}
