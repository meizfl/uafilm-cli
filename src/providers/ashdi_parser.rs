//! Port of ashdi.js parsePlayer / getLinksFromAshdiUrl
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use serde_json::Value;

use crate::config::Config;
use crate::http;

#[derive(Debug, Clone)]
pub struct AshdiLink {
    pub title: String,
    pub file: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

pub fn normalize_url(url: &str) -> String {
    let mut u = url.replace("0yql3tj", "oyql3tj").trim().to_string();
    if u.starts_with("//") {
        u = format!("https:{u}");
    }
    u
}

fn safe_parse(raw: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(raw) {
        return Some(v);
    }
    let alt = raw.replace('\'', "\"");
    serde_json::from_str(&alt).ok()
}

pub fn parse_player(html: &str, fallback_title: &str) -> Vec<AshdiLink> {
    let mut results = Vec::new();

    // Extract file: '...' or file: [...]
    let re_str = Regex::new(r#"(?s)file\s*:\s*(['"])((?:\\.|.)*?)\\1"#).ok();
    let mut raw: Option<String> = None;
    if let Some(re) = re_str {
        if let Some(c) = re.captures(html) {
            raw = c.get(2).map(|m| m.as_str().to_string());
        }
    }
    if raw.is_none() {
        if let Ok(re) = Regex::new(r"(?s)file\s*:\s*(\[[\s\S]*?\])\s*[,}]") {
            if let Some(c) = re.captures(html) {
                raw = c.get(1).map(|m| m.as_str().to_string());
            }
        }
    }
    if raw.is_none() {
        if let Ok(re) = Regex::new(r#"(?s)file\s*:\s*(\{[\s\S]*?\})\s*[,}]"#) {
            if let Some(c) = re.captures(html) {
                raw = c.get(1).map(|m| m.as_str().to_string());
            }
        }
    }
    let Some(raw) = raw else {
        // bare m3u8
        if let Ok(re) = Regex::new(r#"https?://[^\s"']+\.m3u8[^\s"']*"#) {
            if let Some(m) = re.find(html) {
                return vec![AshdiLink {
                    title: fallback_title.to_string(),
                    file: normalize_url(m.as_str()),
                    season: None,
                    episode: None,
                }];
            }
        }
        return results;
    };

    let raw = raw.replace("\\'", "'").replace("\\\"", "\"");
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return vec![AshdiLink {
            title: fallback_title.to_string(),
            file: normalize_url(&raw),
            season: None,
            episode: None,
        }];
    }

    let parsed = safe_parse(&raw);
    let Some(parsed) = parsed else {
        return results;
    };

    fn walk(node: &Value, fallback: &str, season: Option<u32>, episode: Option<u32>, out: &mut Vec<AshdiLink>) {
        match node {
            Value::Array(arr) => {
                for n in arr {
                    walk(n, fallback, season, episode, out);
                }
            }
            Value::Object(map) => {
                let mut s = season;
                let mut e = episode;
                if let Some(t) = map.get("title").and_then(|v| v.as_str()) {
                    if let Ok(re) = Regex::new(r"(?i)(?:сезон|season)\s*(\d+)") {
                        if let Some(c) = re.captures(t) {
                            s = c.get(1).and_then(|m| m.as_str().parse().ok());
                        }
                    }
                    if let Ok(re) = Regex::new(r"(?i)(?:серія|сери[яі]|episode|ep\.?)\s*(\d+)") {
                        if let Some(c) = re.captures(t) {
                            e = c.get(1).and_then(|m| m.as_str().parse().ok());
                        }
                    }
                }
                if let Some(n) = map.get("season").and_then(|v| v.as_u64()) {
                    s = Some(n as u32);
                }
                if let Some(n) = map.get("episode").and_then(|v| v.as_u64()) {
                    e = Some(n as u32);
                }
                if let Some(file) = map.get("file").and_then(|v| v.as_str()) {
                    if !file.is_empty() {
                        let title = map
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or(fallback)
                            .to_string();
                        out.push(AshdiLink {
                            title,
                            file: normalize_url(file),
                            season: s,
                            episode: e,
                        });
                    }
                }
                if let Some(folder) = map.get("folder") {
                    walk(folder, fallback, s, e, out);
                }
            }
            _ => {}
        }
    }

    walk(&parsed, fallback_title, None, None, &mut results);
    results
}

pub fn get_links_from_ashdi_url(
    client: &Client,
    cfg: &Config,
    ashdi_url: &str,
    title: &str,
) -> Result<Vec<AshdiLink>> {
    let mut url = normalize_url(ashdi_url);
    if Regex::new(r"(?i)/vod/\d+")?.is_match(&url) && !url.contains("multivoice") {
        url.push_str(if url.contains('?') { "&" } else { "?" });
        url.push_str("multivoice");
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::REFERER,
        reqwest::header::HeaderValue::from_static("https://kinoukr.tv/"),
    );
    let resp = http::get(client, &url, cfg, Some(headers))?;
    let html = resp.text()?;
    if Regex::new(r"(?i)недоступний|заблоковано")?.is_match(&html) {
        return Ok(vec![]);
    }
    Ok(parse_player(&html, title))
}
