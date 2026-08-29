//! Port of uatut.js — сайт tv.uatut.fun → провайдер Ashdi.
use anyhow::Result;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::Value;

use super::ashdi_parser::get_links_from_ashdi_url;
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const UATUT_URL: &str = "https://tv.uatut.fun/watch";

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    let query = meta.imdb_id.clone().unwrap_or_else(|| meta.title.clone());
    if query.is_empty() {
        return Ok(vec![]);
    }
    eprintln!("[uatut] Пошук: {query}");
    let url = format!(
        "{UATUT_URL}/search.php?q={}",
        super::common::utf8_percent_encode(&query)
    );
    let resp = http::get(client, &url, cfg, None)?;
    let text = resp.text()?;
    let Ok(results) = serde_json::from_str::<Value>(&text) else {
        return Ok(vec![]);
    };
    let Value::Array(results) = results else {
        return Ok(vec![]);
    };
    if results.is_empty() {
        return Ok(vec![]);
    }
    let matched = meta
        .imdb_id
        .as_ref()
        .and_then(|imdb| {
            results
                .iter()
                .find(|r| r.get("imdb_id").and_then(|v| v.as_str()) == Some(imdb.as_str()))
        })
        .or_else(|| results.first());
    let Some(matched) = matched else { return Ok(vec![]) };
    let Some(id) = matched.get("id") else { return Ok(vec![]) };
    let id_str = match id {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Ok(vec![]),
    };
    let page_url = format!("{UATUT_URL}/{id_str}");
    let resp = http::get(client, &page_url, cfg, None)?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    let sel = Selector::parse("iframe").unwrap();
    let ashdi_src = doc
        .select(&sel)
        .filter_map(|el| el.value().attr("src"))
        .find(|s| s.contains("ashdi.vip"));
    let Some(ashdi_src) = ashdi_src else {
        return Ok(vec![]);
    };
    let title = if meta.title.is_empty() {
        matched.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else {
        meta.title.clone()
    };
    let links = get_links_from_ashdi_url(client, cfg, ashdi_src, &title)?;
    Ok(links
        .into_iter()
        .map(|l| Stream {
            url: l.file,
            title: if l.title.is_empty() { title.clone() } else { l.title },
            provider: "uatut".into(),
            quality: String::new(),
            season: l.season,
            episode: l.episode,
            referer: Some(ashdi_src.to_string()),
        })
        .collect())
}

pub struct Uatut;

impl Provider for Uatut {
    fn name(&self) -> &str {
        "uatut"
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
