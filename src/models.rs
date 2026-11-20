use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    AnimeciX,
    Anizle,
    TurkAnime,
}

impl SourceKind {
    pub fn display_name(self) -> &'static str {
        match self {
            SourceKind::AnimeciX => "AnimeciX",
            SourceKind::Anizle => "Anizle",
            SourceKind::TurkAnime => "TürkAnime Arşiv",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        let lower = s.trim().to_lowercase();
        match lower.as_str() {
            "animecix" => Some(SourceKind::AnimeciX),
            "anizle" | "anizm" => Some(SourceKind::Anizle),
            "turkanime" | "türkanime" | "turkanime arşiv" | "türkanime arşiv" | "animedepo" => {
                Some(SourceKind::TurkAnime)
            }
            _ => None,
        }
    }

    pub fn all() -> &'static [SourceKind] {
        &[SourceKind::AnimeciX, SourceKind::Anizle, SourceKind::TurkAnime]
    }
}

#[derive(Debug, Clone)]
pub struct Anime {
    pub id: String,
    pub title: String,
    pub source: SourceKind,
    pub is_movie: bool,
}

#[derive(Debug, Clone)]
pub struct Episode {
    pub id: String,
    pub title: String,
    #[allow(dead_code)]
    pub number: usize,
    pub season_num: usize,
}

#[derive(Debug, Clone)]
pub struct StreamOption {
    pub label: String,
    pub url: String,
    pub referer: Option<String>,
    pub fansub: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WatchData {
    pub streams: Vec<StreamOption>,
    pub subtitle_url: Option<String>,
}

pub trait AnimeSource: Send + Sync {
    #[allow(dead_code)]
    fn kind(&self) -> SourceKind;
    fn search(&self, query: &str) -> Result<Vec<Anime>>;
    fn get_episodes(&self, anime: &Anime) -> Result<Vec<Episode>>;
    fn get_watch_data(&self, anime: &Anime, episode: &Episode, ep_index_in_season: usize) -> Result<WatchData>;
}
