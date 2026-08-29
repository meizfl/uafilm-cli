pub mod ashdi_parser;
pub mod uaflix;

use crate::config::Config;
use crate::tmdb::Meta;
use anyhow::Result;
use reqwest::blocking::Client;

#[derive(Debug, Clone)]
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
            other => {
                eprintln!("[!] Невідомий / ще не портований провайдер: {other}");
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
