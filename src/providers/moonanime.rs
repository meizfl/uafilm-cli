//! Port of moonanime.js (пошук через API) + вбудованого /api/moonanime/stream
//! резолвера з index.js (подвійне XOR-декодування прямого посилання на CDN).
//! На відміну від Node-бекенду, який проксіює HLS через власний сервер, тут
//! повертається пряме посилання на CDN — плеєр (mpv/vlc) звертається до нього
//! напряму з потрібними заголовками (Referer/Origin).
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ORIGIN, REFERER, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;

use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const API_KEY: &str = "865fEF-E2e1Bc-2ca431-e6A150-780DFD-737C6B";
const API_HOST: &str = "https://api.moonanime.art";
const MOON_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

fn moon_outer_decode(html: &str) -> Option<String> {
    let re = Regex::new(r#"var _\w+=atob\("([^"]+)"\)"#).ok()?;
    let b64 = &re.captures(html)?[1];
    let b = STANDARD.decode(b64).ok()?;
    if b.len() < 32 {
        return None;
    }
    let key = &b[..32];
    let mut out = Vec::with_capacity(b.len() - 32);
    for (i, &byte) in b[32..].iter().enumerate() {
        out.push(byte ^ key[i % 32]);
    }
    Some(String::from_utf8_lossy(&out).to_string())
}

fn moon_inner_decode(encoded: &str) -> Option<String> {
    let key = b"mAnK";
    let b = STANDARD.decode(encoded).ok()?;
    let mut out = Vec::with_capacity(b.len());
    for (i, &byte) in b.iter().enumerate() {
        out.push(byte ^ key[i % key.len()]);
    }
    Some(String::from_utf8_lossy(&out).to_string())
}

/// Резолвить vod id → список (якість, пряме посилання на CDN)
fn resolve_vod(client: &Client, cfg: &Config, vod_id: &str) -> Result<Vec<(String, String)>> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(MOON_UA));
    let url = format!("https://moonanime.art/vod/{vod_id}");
    let resp = http::get(client, &url, cfg, Some(headers))?;
    let html = resp.text()?;
    let decoded_js = moon_outer_decode(&html).ok_or_else(|| anyhow!("outer XOR decode failed"))?;
    let file_re = Regex::new(r#"file:\s*_0xd\("([^"]+)"\)"#)?;
    let Some(c) = file_re.captures(&decoded_js) else {
        return Ok(vec![]);
    };
    let Some(file_value) = moon_inner_decode(&c[1]) else {
        return Ok(vec![]);
    };

    if file_value.contains('[') && file_value.contains(']') {
        let part_re = Regex::new(r"\[([^\]]+)\](https?://\S+)")?;
        let mut out = Vec::new();
        for part in file_value.split(',') {
            if let Some(m) = part_re.captures(part) {
                out.push((m[1].trim().to_string(), m[2].trim().to_string()));
            }
        }
        Ok(out)
    } else if file_value.starts_with("http") {
        Ok(vec![("Auto".to_string(), file_value)])
    } else {
        Ok(vec![])
    }
}

#[derive(Debug, Clone)]
struct EpisodeRef {
    vod: String,
    episode: u32,
    title: String,
}

fn fetch_videos(
    client: &Client,
    cfg: &Config,
    anime_id: i64,
) -> Result<HashMap<String, HashMap<u32, Vec<EpisodeRef>>>> {
    let url = format!("{API_HOST}/api/2.0/title/{anime_id}/videos?api_key={API_KEY}");
    let resp = http::get(client, &url, cfg, None)?;
    let v: Value = resp.json()?;
    let Value::Array(items) = v else {
        return Ok(HashMap::new());
    };
    let mut out: HashMap<String, HashMap<u32, Vec<EpisodeRef>>> = HashMap::new();
    for trans_obj in items {
        let Value::Object(map) = trans_obj else { continue };
        let Some((trans_name, seasons)) = map.into_iter().next() else { continue };
        let Value::Object(seasons) = seasons else { continue };
        if seasons.is_empty() {
            continue;
        }
        let mut season_map: HashMap<u32, Vec<EpisodeRef>> = HashMap::new();
        for (s_num_str, episodes) in seasons {
            let s_num: u32 = s_num_str.parse().unwrap_or(1);
            let Value::Array(episodes) = episodes else { continue };
            let mut eps = Vec::new();
            for ep in episodes {
                let vod = ep.get("vod").and_then(|x| x.as_str()).unwrap_or("");
                let vod_id = vod.split('/').filter(|p| !p.is_empty()).last().unwrap_or("").to_string();
                if vod_id.is_empty() {
                    continue;
                }
                let episode_num = ep
                    .get("episode")
                    .and_then(|x| x.as_u64())
                    .unwrap_or((eps.len() + 1) as u64) as u32;
                let title = ep
                    .get("title")
                    .and_then(|x| x.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| format!("{episode_num} серія"));
                eps.push(EpisodeRef { vod: vod_id, episode: episode_num, title });
            }
            if !eps.is_empty() {
                season_map.insert(s_num, eps);
            }
        }
        if !season_map.is_empty() {
            out.insert(trans_name, season_map);
        }
    }
    Ok(out)
}

fn find_anime(client: &Client, cfg: &Config, meta: &Meta) -> Result<Option<i64>> {
    let mut headers = HeaderMap::new();
    headers.insert(ORIGIN, HeaderValue::from_static("http://lampa.mx"));
    let imdb = meta.imdb_id.clone().unwrap_or_default();
    let url = format!(
        "{API_HOST}/api/2.0/titles?api_key={API_KEY}&imdbid={}&search={}",
        super::common::utf8_percent_encode(&imdb),
        super::common::utf8_percent_encode(&meta.title)
    );
    let resp = http::get(client, &url, cfg, Some(headers))?;
    let v: Value = resp.json()?;
    let Some(list) = v.get("anime_list").and_then(|x| x.as_array()) else {
        return Ok(None);
    };
    if list.is_empty() {
        return Ok(None);
    }
    let year: Option<i64> = meta.year.parse().ok();
    let matched = year
        .and_then(|y| {
            list.iter()
                .find(|i| i.get("year").and_then(|v| v.as_i64()) == Some(y))
        })
        .or_else(|| list.first());
    Ok(matched.and_then(|i| i.get("id").and_then(|x| x.as_i64())))
}

fn resolve(
    client: &Client,
    cfg: &Config,
    meta: &Meta,
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<Vec<Stream>> {
    eprintln!("[moonanime] Пошук: {}", meta.title);
    let Some(anime_id) = find_anime(client, cfg, meta)? else {
        eprintln!("[moonanime] Не знайдено");
        return Ok(vec![]);
    };
    let translations = fetch_videos(client, cfg, anime_id)?;
    if translations.is_empty() {
        return Ok(vec![]);
    }

    let want_season = season.unwrap_or(1);
    let want_episode = episode.unwrap_or(1);

    let mut streams = Vec::new();
    let mut referer_headers = HeaderMap::new();
    referer_headers.insert(REFERER, HeaderValue::from_static("https://moonanime.art/"));

    for (dub, seasons) in &translations {
        let Some(eps) = seasons.get(&want_season).or_else(|| seasons.values().next()) else {
            continue;
        };
        let Some(ep) = eps
            .iter()
            .find(|e| e.episode == want_episode)
            .or_else(|| eps.first())
        else {
            continue;
        };
        match resolve_vod(client, cfg, &ep.vod) {
            Ok(qualities) => {
                for (quality, url) in qualities {
                    streams.push(Stream {
                        url,
                        title: format!("{dub} — {}", ep.title),
                        provider: "moonanime".into(),
                        quality,
                        season: Some(want_season),
                        episode: Some(ep.episode),
                        referer: Some("https://moonanime.art/".to_string()),
                    });
                }
            }
            Err(e) => eprintln!("[moonanime] {dub}: {e}"),
        }
    }
    Ok(streams)
}

pub struct MoonAnime;

impl Provider for MoonAnime {
    fn name(&self) -> &str {
        "moonanime"
    }
    fn resolve_movie(&self, client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
        resolve(client, cfg, meta, None, None)
    }
    fn resolve_tv(
        &self,
        client: &Client,
        cfg: &Config,
        meta: &Meta,
        season: u32,
        episode: u32,
    ) -> Result<Vec<Stream>> {
        resolve(client, cfg, meta, Some(season), Some(episode))
    }
}
