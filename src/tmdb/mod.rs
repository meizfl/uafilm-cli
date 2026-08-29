use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::config::Config;
use crate::http;

#[derive(Debug, Clone)]
pub struct Meta {
    pub id: u64,
    pub media_type: String, // movie | tv
    pub title: String,
    pub original_title: String,
    pub year: String,
    pub imdb_id: Option<String>,
    pub seasons_info: BTreeMap<u32, u32>, // season -> episode_count
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    results: Option<Vec<Value>>,
}

pub fn search(client: &reqwest::blocking::Client, cfg: &Config, query: &str) -> Result<Vec<Value>> {
    if cfg.tmdb_token.is_empty() {
        bail!(
            "TMDB_TOKEN не задано.\n\
             1) https://www.themoviedb.org/settings/api → API Read Access Token\n\
             2) export TMDB_TOKEN='eyJ...'"
        );
    }
    let url = format!(
        "https://api.themoviedb.org/3/search/multi?query={}&language=uk-UA&include_adult=false",
        urlencoding_encode(query)
    );
    let resp = client
        .get(&url)
        .bearer_auth(&cfg.tmdb_token)
        .header("Accept", "application/json")
        .send()
        .context("TMDB search")?;
    if !resp.status().is_success() {
        bail!("TMDB search HTTP {}", resp.status());
    }
    let body: SearchResult = resp.json()?;
    let list = body.results.unwrap_or_default();
    Ok(list
        .into_iter()
        .filter(|v| {
            matches!(
                v.get("media_type").and_then(|x| x.as_str()),
                Some("movie") | Some("tv")
            )
        })
        .collect())
}

pub fn metadata(
    client: &reqwest::blocking::Client,
    cfg: &Config,
    media_type: &str,
    id: u64,
) -> Result<Meta> {
    if cfg.tmdb_token.is_empty() {
        bail!("TMDB_TOKEN не задано");
    }
    let details_url = format!(
        "https://api.themoviedb.org/3/{media_type}/{id}?language=uk-UA"
    );
    let ext_url = format!("https://api.themoviedb.org/3/{media_type}/{id}/external_ids");

    let details: Value = client
        .get(&details_url)
        .bearer_auth(&cfg.tmdb_token)
        .send()?
        .error_for_status()?
        .json()?;
    let ext: Value = client
        .get(&ext_url)
        .bearer_auth(&cfg.tmdb_token)
        .send()?
        .json()
        .unwrap_or(Value::Null);

    let title = details
        .get("title")
        .or_else(|| details.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Без назви")
        .to_string();
    let original_title = details
        .get("original_title")
        .or_else(|| details.get("original_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let date = details
        .get("release_date")
        .or_else(|| details.get("first_air_date"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let year = date.chars().take(4).collect::<String>();

    let mut seasons_info = BTreeMap::new();
    if let Some(arr) = details.get("seasons").and_then(|v| v.as_array()) {
        for s in arr {
            let sn = s.get("season_number").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let ec = s.get("episode_count").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            if sn > 0 && ec > 0 {
                seasons_info.insert(sn, ec);
            }
        }
    }

    Ok(Meta {
        id,
        media_type: media_type.to_string(),
        title,
        original_title,
        year,
        imdb_id: ext
            .get("imdb_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        seasons_info,
    })
}

pub fn title_of(v: &Value) -> String {
    v.get("title")
        .or_else(|| v.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("Без назви")
        .to_string()
}

pub fn year_of(v: &Value) -> String {
    let d = v
        .get("release_date")
        .or_else(|| v.get("first_air_date"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    d.chars().take(4).collect()
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// silence unused import warning for http in this module layout
#[allow(unused_imports)]
use http as _;
