//! Port of kinoukr-db.js — завантажує kinoukr.json (каталог lampac), шукає в пам'яті
//! (замість SQLite — для одноразового CLI-запуску пошук по Vec цілком ефективний;
//! каталог кешується у файл на 24 год, як і в оригіналі).
use anyhow::Result;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::time::Duration;

use super::ashdi_parser::get_links_from_ashdi_url;
use super::common::flatten_playlist;
use super::tortuga::{parse_tortuga_embed, parse_tortuga_vod};
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const JSON_URL: &str = "https://raw.githubusercontent.com/lampac-nextgen/lampac/refs/heads/main/Core/data/kinoukr.json";
const ASHDI_BASE: &str = "https://ashdi.vip";
const TORTUGA_BASE: &str = "https://tortuga.tw";
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Deserialize, Clone, Default)]
#[allow(dead_code)]
struct RawItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    eng_name: String,
    #[serde(default)]
    year: String,
    #[serde(default)]
    kp_id: String,
    #[serde(default)]
    imdb_id: String,
    #[serde(default)]
    ashdi: String,
    #[serde(default)]
    tortuga: String,
}

struct Row {
    title: String,
    eng_name: String,
    search_title: String,
    search_eng: String,
    year: String,
    imdb_id: String,
    ashdi_path: String,
    tortuga_path: String,
}

fn norm(s: &str) -> String {
    s.replace('ґ', "г").replace('є', "е")
}

fn cache_path() -> std::path::PathBuf {
    std::env::temp_dir().join("uafilm-cli-kinoukr.json")
}

fn load_catalog(client: &Client, cfg: &Config) -> Result<Vec<Row>> {
    let path = cache_path();
    let fresh = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or(Duration::MAX) < TTL)
        .unwrap_or(false);

    let text = if fresh {
        eprintln!("[kinoukr-db] Каталог з кешу: {}", path.display());
        fs::read_to_string(&path)?
    } else {
        eprintln!("[kinoukr-db] Завантаження kinoukr.json...");
        let resp = http::get(client, JSON_URL, cfg, None)?;
        let t = resp.text()?;
        let _ = fs::write(&path, &t);
        t
    };

    let v: Value = serde_json::from_str(&text)?;
    let Value::Object(map) = v else {
        return Ok(vec![]);
    };
    let mut rows = Vec::with_capacity(map.len());
    for (_slug, item) in map {
        let raw: RawItem = serde_json::from_value(item).unwrap_or_default();
        let title = raw.name.trim().to_string();
        let eng_name = raw.eng_name.trim().to_string();
        rows.push(Row {
            search_title: norm(&title.to_lowercase()),
            search_eng: norm(&eng_name.to_lowercase()),
            title,
            eng_name,
            year: raw.year,
            imdb_id: raw.imdb_id,
            ashdi_path: raw.ashdi,
            tortuga_path: raw.tortuga,
        });
    }
    eprintln!("[kinoukr-db] Записів: {}", rows.len());
    Ok(rows)
}

fn search<'a>(rows: &'a [Row], title: &str, eng: &str, year: &str) -> Option<&'a Row> {
    let nt = norm(&title.trim().to_lowercase());
    let ne = norm(&eng.trim().to_lowercase());
    if nt.is_empty() && ne.is_empty() {
        return None;
    }

    if !year.is_empty() {
        if let Some(r) = rows
            .iter()
            .find(|r| (r.search_title == nt || r.search_eng == nt) && r.year == year)
        {
            return Some(r);
        }
        if !ne.is_empty() && ne != nt {
            if let Some(r) = rows.iter().find(|r| r.search_eng == ne && r.year == year) {
                return Some(r);
            }
        }
    }

    if let Some(r) = rows.iter().find(|r| r.search_title == nt || r.search_eng == nt) {
        return Some(r);
    }
    if !ne.is_empty() && ne != nt {
        if let Some(r) = rows.iter().find(|r| r.search_eng == ne) {
            return Some(r);
        }
    }

    let year_ok = |r: &&Row| year.is_empty() || r.year.is_empty() || r.year == year;
    if let Some(r) = rows
        .iter()
        .filter(year_ok)
        .find(|r| r.search_title.contains(&nt) || r.search_eng.contains(&nt))
    {
        return Some(r);
    }
    if !ne.is_empty() {
        if let Some(r) = rows.iter().filter(year_ok).find(|r| r.search_eng.contains(&ne)) {
            return Some(r);
        }
    }

    let words: Vec<&str> = nt.split_whitespace().filter(|w| w.len() > 2).collect();
    if !words.is_empty() {
        if let Some(r) = rows.iter().filter(year_ok).find(|r| {
            words
                .iter()
                .any(|w| r.search_title.contains(w) || r.search_eng.contains(w))
        }) {
            return Some(r);
        }
    }

    None
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[kinoukr-db] Пошук: {}", meta.title);
    let rows = load_catalog(client, cfg)?;
    if rows.is_empty() {
        return Ok(vec![]);
    }

    let row = meta
        .imdb_id
        .as_ref()
        .and_then(|imdb| rows.iter().find(|r| &r.imdb_id == imdb))
        .or_else(|| search(&rows, &meta.title, &meta.original_title, &meta.year));

    let Some(row) = row else {
        eprintln!("[kinoukr-db] Не знайдено: {}", meta.title);
        return Ok(vec![]);
    };
    eprintln!("[kinoukr-db] Знайдено: {} ({})", row.title, row.eng_name);

    let mut streams = Vec::new();

    if !row.ashdi_path.is_empty() {
        let url = format!("{ASHDI_BASE}/{}", row.ashdi_path);
        if let Ok(links) = get_links_from_ashdi_url(client, cfg, &url, &meta.title) {
            for l in links {
                streams.push(Stream {
                    url: l.file,
                    title: if l.title.is_empty() { row.title.clone() } else { l.title },
                    provider: "kinoukr-db".into(),
                    quality: String::new(),
                    season: l.season,
                    episode: l.episode,
                    referer: Some(url.clone()),
                });
            }
        }
    }

    if !row.tortuga_path.is_empty() {
        let url = format!("{TORTUGA_BASE}/{}", row.tortuga_path);
        if row.tortuga_path.starts_with("embed/") {
            if let Ok(Some(nodes)) = parse_tortuga_embed(client, cfg, &url) {
                streams.extend(flatten_playlist(&nodes, "kinoukr-db"));
            }
        } else if let Ok(Some(vod)) = parse_tortuga_vod(client, cfg, &url) {
            streams.push(Stream {
                url: vod.file,
                title: row.title.clone(),
                provider: "kinoukr-db".into(),
                quality: String::new(),
                season: None,
                episode: None,
                referer: Some(url),
            });
        }
    }

    Ok(streams)
}

pub struct KinoukrDb;

impl Provider for KinoukrDb {
    fn name(&self) -> &str {
        "kinoukr-db"
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
