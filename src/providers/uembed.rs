//! Port of uembed.js — cinepro.aartzz.pp.ua API (лише фільми, за TMDB ID).
use anyhow::Result;
use reqwest::blocking::Client;
use serde_json::Value;

use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE_URL: &str = "https://cinepro.aartzz.pp.ua";

pub struct Uembed;

impl Provider for Uembed {
    fn name(&self) -> &str {
        "uembed"
    }

    fn resolve_movie(&self, client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
        eprintln!("[uembed] TMDB ID: {}", meta.id);
        let url = format!("{BASE_URL}/movie/{}?providers=uembed", meta.id);
        let resp = http::get(client, &url, cfg, None)?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let v: Value = resp.json()?;
        let Some(files) = v.get("files").and_then(|x| x.as_array()) else {
            return Ok(vec![]);
        };
        Ok(files
            .iter()
            .filter_map(|f| {
                let file = f.get("file").and_then(|x| x.as_str())?;
                let quality = f
                    .get("quality")
                    .and_then(|x| x.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| {
                        if f.get("type").and_then(|x| x.as_str()) == Some("hls") {
                            "Auto".into()
                        } else {
                            "Unknown".into()
                        }
                    });
                Some(Stream {
                    url: file.to_string(),
                    title: "English".into(),
                    provider: "uembed".into(),
                    quality,
                    season: None,
                    episode: None,
                    referer: None,
                })
            })
            .collect())
    }

    fn resolve_tv(
        &self,
        _client: &Client,
        _cfg: &Config,
        _meta: &Meta,
        _season: u32,
        _episode: u32,
    ) -> Result<Vec<Stream>> {
        // uembed підтримує лише фільми (як і в оригінальному backend)
        Ok(vec![])
    }
}
