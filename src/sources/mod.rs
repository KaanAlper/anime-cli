pub mod animecix;
pub mod anizle;
pub mod turkanime;

use reqwest::blocking::Client;
use std::time::Duration;

use crate::models::{AnimeSource, SourceKind};

pub fn build_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(20))
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
