//! Port of wormhole.js — wh.lme.isroot.in → провайдер Ashdi (лише за IMDB ID).
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::Value;

use super::ashdi_parser::get_links_from_ashdi_url;
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const WORMHOLE_URL: &str = "https://wh.lme.isroot.in";

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    let Some(imdb) = &meta.imdb_id else {
        eprintln!("[wormhole] Немає IMDB ID — пропуск");
        return Ok(vec![]);
    };
    eprintln!("[wormhole] Пошук: {imdb}");
    let url = format!("{WORMHOLE_URL}/?imdb_id={imdb}");
    let resp = http::get(client, &url, cfg, None)?;
    let text = resp.text()?;
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Ok(vec![]);
    };
    let Some(ashdi_url) = v.get("play").and_then(|x| x.as_str()) else {
        return Ok(vec![]);
    };
    if !ashdi_url.contains("ashdi") {
        return Ok(vec![]);
    }
    let links = get_links_from_ashdi_url(client, cfg, ashdi_url, &meta.title)?;
    Ok(links
        .into_iter()
        .map(|l| Stream {
            url: l.file,
            title: if l.title.is_empty() {
                meta.title.clone()
            } else {
                l.title
            },
            provider: "wormhole".into(),
            quality: String::new(),
            season: l.season,
            episode: l.episode,
            referer: Some(ashdi_url.to_string()),
        })
        .collect())
}

pub struct Wormhole;

impl Provider for Wormhole {
    fn name(&self) -> &str {
        "wormhole"
    }
    fn resolve_movie(&self, client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
        resolve(client, cfg, meta)
    }
    fn resolve_tv(
        &self,
        client: &Client,
        cfg: &Config,
        meta: &Meta,
        season: u32,
        episode: u32,
    ) -> Result<Vec<Stream>> {
        let streams = resolve(client, cfg, meta)?;
        Ok(streams
            .into_iter()
            .filter(|s| {
                (s.season.is_none() && s.episode.is_none())
                    || (s.season == Some(season) && s.episode == Some(episode))
            })
            .collect())
    }
}
