pub mod ashdi_parser;
pub mod common;
pub mod eneyida;
pub mod hdvb;
pub mod kinoukr;
pub mod kinoukr_db;
pub mod klon;
pub mod moonanime;
pub mod tortuga;
pub mod uaflix;
pub mod uakino_app;
pub mod uakino_best;
pub mod uaserials_com;
pub mod uaserials_my;
pub mod uatut;
pub mod uembed;
pub mod wormhole;

use crate::config::Config;
use crate::tmdb::Meta;
use anyhow::Result;
use reqwest::blocking::Client;

/// Список усіх реалізованих провайдерів (для --list-providers).
pub const ALL_PROVIDERS: &[&str] = &[
    "uaflix",
    "wormhole",
    "uatut",
    "klon",
    "uakino-app",
    "uakino-best",
    "kinoukr",
    "kinoukr-db",
    "eneyida",
    "uaserials-my",
    "uaserials-com",
    "moonanime",
    "uembed",
];

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Stream {
    pub url: String,
    pub title: String,
    pub provider: String,
    pub quality: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub referer: Option<String>,
}

pub trait Provider {
    #[allow(dead_code)]
    fn name(&self) -> &str;
    fn resolve_movie(&self, client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>>;
    fn resolve_tv(
        &self,
        client: &Client,
        cfg: &Config,
        meta: &Meta,
        season: u32,
        episode: u32,
    ) -> Result<Vec<Stream>>;
}

fn run<P: Provider>(
    p: &P,
    client: &Client,
    cfg: &Config,
    meta: &Meta,
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<Vec<Stream>> {
    if meta.media_type == "tv" {
        let s = season.unwrap_or(1);
        let e = episode.unwrap_or(1);
        p.resolve_tv(client, cfg, meta, s, e)
    } else {
        p.resolve_movie(client, cfg, meta)
    }
}

pub fn resolve_all(
    client: &Client,
    cfg: &Config,
    meta: &Meta,
    providers: &[String],
    season: Option<u32>,
    episode: Option<u32>,
) -> Vec<Stream> {
    let names: Vec<&str> = if providers.is_empty() {
        vec!["uaflix"]
    } else {
        providers.iter().map(|s| s.as_str()).collect()
    };

    let mut out = Vec::new();
    for name in names {
        let result = match name {
            "uaflix" => {
                let p = uaflix::Uaflix;
                if meta.media_type == "tv" {
                    let s = season.unwrap_or(1);
                    let e = episode.unwrap_or(1);
                    p.resolve_tv(client, cfg, meta, s, e)
                } else {
                    p.resolve_movie(client, cfg, meta)
                }
            }
            "wormhole" => run(&wormhole::Wormhole, client, cfg, meta, season, episode),
            "uatut" => run(&uatut::Uatut, client, cfg, meta, season, episode),
            "klon" => run(&klon::Klon, client, cfg, meta, season, episode),
            "uakino-app" => run(&uakino_app::UakinoApp, client, cfg, meta, season, episode),
            "uakino-best" => run(&uakino_best::UakinoBest, client, cfg, meta, season, episode),
            "kinoukr" => run(&kinoukr::Kinoukr, client, cfg, meta, season, episode),
            "kinoukr-db" => run(&kinoukr_db::KinoukrDb, client, cfg, meta, season, episode),
            "eneyida" => run(&eneyida::Eneyida, client, cfg, meta, season, episode),
            "uaserials-my" => run(&uaserials_my::UaserialsMy, client, cfg, meta, season, episode),
            "uaserials-com" => run(&uaserials_com::UaserialsCom, client, cfg, meta, season, episode),
            "moonanime" => run(&moonanime::MoonAnime, client, cfg, meta, season, episode),
            "uembed" => run(&uembed::Uembed, client, cfg, meta, season, episode),
            other => {
                eprintln!("[!] Невідомий провайдер: {other}. Див. --list-providers");
                continue;
            }
        };
        match result {
            Ok(mut streams) => out.append(&mut streams),
            Err(e) => eprintln!("[!] {name}: {e}"),
        }
    }
    out
}
