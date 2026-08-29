//! Port of tortuga.js — сайт tortuga.tw, XOR-декодування посилань (реверс-інжиніринг
//! з tor.core.min.js: base64 → перший байт як ключ → XOR з (n + i*7 + 13) % 256).
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use regex::Regex;
use reqwest::blocking::Client;
use serde_json::Value;

use super::common::PNode;
use crate::config::Config;
use crate::http;

fn xor_key(n: u8, i: usize) -> u8 {
    ((n as usize + i * 7 + 13) % 256) as u8
}

pub fn decode_tortuga_file(encoded: &str) -> Option<String> {
    let clean = encoded.trim_end_matches('=');
    let raw = STANDARD.decode(clean.trim()).ok().or_else(|| {
        // base64 without proper padding removed above; try padding to multiple of 4
        let mut s = clean.to_string();
        while s.len() % 4 != 0 {
            s.push('=');
        }
        STANDARD.decode(s).ok()
    })?;
    if raw.len() < 2 {
        return None;
    }
    let key = raw[0];
    let mut out = Vec::with_capacity(raw.len() - 1);
    for (i, &b) in raw[1..].iter().enumerate() {
        out.push(b ^ xor_key(key, i));
    }
    // JS does decodeURIComponent(escape(binary_string)) which is a latin1->utf8
    // reinterpretation; the raw bytes ARE the utf-8 bytes already in our case
    // since we decoded straight to a Vec<u8>, so just parse as utf8 (lossy fallback).
    Some(String::from_utf8_lossy(&out).to_string())
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

fn fetch(client: &Client, cfg: &Config, url: &str) -> Result<String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(UA),
    );
    let resp = http::get(client, url, cfg, Some(headers))?;
    Ok(resp.text()?)
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TortugaVod {
    pub file: String,
    pub poster: Option<String>,
}

/// Парсить сторінку фільму `/vod/XXXX` → {file, poster}
pub fn parse_tortuga_vod(client: &Client, cfg: &Config, vod_url: &str) -> Result<Option<TortugaVod>> {
    let html = fetch(client, cfg, vod_url)?;
    let re_file = Regex::new(r#"file\s*:\s*["']([A-Za-z0-9+/=]+)["']"#)?;
    let Some(c) = re_file.captures(&html) else {
        return Ok(None);
    };
    let Some(file) = decode_tortuga_file(&c[1]) else {
        return Ok(None);
    };
    let poster = Regex::new(r#"poster\s*:\s*["']([A-Za-z0-9+/=]+)["']"#)?
        .captures(&html)
        .and_then(|c| decode_tortuga_file(&c[1]));
    Ok(Some(TortugaVod { file, poster }))
}

fn json_to_pnode(v: &Value) -> Option<PNode> {
    match v {
        Value::Object(map) => {
            let title = map.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if let Some(folder) = map.get("folder").and_then(|x| x.as_array()) {
                let children: Vec<PNode> = folder.iter().filter_map(json_to_pnode).collect();
                return Some(PNode::branch(title, children));
            }
            let file = map.get("file").and_then(|x| x.as_str())?;
            Some(PNode {
                title: if title.is_empty() { "Tortuga".to_string() } else { title },
                file: Some(file.to_string()),
                poster: map.get("poster").and_then(|x| x.as_str()).map(String::from),
                subtitle: map.get("subtitle").and_then(|x| x.as_str()).map(String::from),
                ..Default::default()
            })
        }
        _ => None,
    }
}

/// Парсить сторінку серіалу `/embed/XXXX` → повне дерево folder/file
pub fn parse_tortuga_embed(client: &Client, cfg: &Config, embed_url: &str) -> Result<Option<Vec<PNode>>> {
    let html = fetch(client, cfg, embed_url)?;
    let re_file = Regex::new(r#"file\s*:\s*["']([A-Za-z0-9+/=]{20,})["']"#)?;
    let Some(c) = re_file.captures(&html) else {
        return Ok(None);
    };
    let Some(decoded) = decode_tortuga_file(&c[1]) else {
        return Ok(None);
    };
    let Ok(json) = serde_json::from_str::<Value>(&decoded) else {
        return Ok(None);
    };
    let Value::Array(seasons) = json else {
        return Ok(None);
    };
    let nodes: Vec<PNode> = seasons.iter().filter_map(json_to_pnode).collect();
    if nodes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(nodes))
    }
}
