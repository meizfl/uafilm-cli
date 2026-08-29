//! Port of uaserials-com.js — сайт uaserials.com → провайдер Tortuga.
//! Розшифровує AES-256-CBC (CryptoJS-сумісний, PBKDF2-HMAC-SHA512, 999 ітерацій)
//! `data-tagN` атрибути сторінки, потім трансформує у стандартне дерево folder/file.
//!
//! ПРИМІТКА: passphrase (`var dd=...`) в оригіналі іноді буває заплутана через
//! обфускований виклик функції, який в Node виконувався через `eval`. Тут
//! портовано лише "просту" гілку (конкатенація рядкових літералів), яка
//! покриває більшість випадків; повний eval-based фолбек свідомо не портований.

use aes::Aes256;
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, ORIGIN, REFERER, USER_AGENT};
use scraper::{Html, Selector};
use serde_json::Value;
use sha2::Sha512;
use std::sync::Mutex;

use super::common::{clean_title, flatten_playlist};
use super::tortuga::parse_tortuga_vod;
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE: &str = "https://uaserials.com";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:149.0) Gecko/20100101 Firefox/149.0";

static PASSPHRASE_CACHE: Mutex<Option<String>> = Mutex::new(None);

fn headers() -> Result<HeaderMap> {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_static(UA));
    Ok(h)
}

type Aes256CbcDec = cbc::Decryptor<Aes256>;

fn aes_decrypt(passphrase: &str, json_str: &str) -> Result<String> {
    let v: Value = serde_json::from_str(json_str)?;
    let salt = hex::decode(v.get("salt").and_then(|x| x.as_str()).ok_or_else(|| anyhow!("no salt"))?)?;
    let iv = hex::decode(v.get("iv").and_then(|x| x.as_str()).ok_or_else(|| anyhow!("no iv"))?)?;
    let ciphertext = STANDARD.decode(
        v.get("ciphertext")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("no ciphertext"))?,
    )?;
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(passphrase.as_bytes(), &salt, 999, &mut key);

    let mut buf = ciphertext;
    let decryptor = Aes256CbcDec::new_from_slices(&key, &iv)?;
    let decrypted = decryptor
        .decrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| anyhow!("aes decrypt: {e}"))?;
    Ok(String::from_utf8_lossy(decrypted).to_string())
}

fn get_passphrase(client: &Client, cfg: &Config) -> Result<String> {
    if let Some(p) = PASSPHRASE_CACHE.lock().unwrap().clone() {
        return Ok(p);
    }
    let resp = http::get(client, &format!("{BASE}/"), cfg, Some(headers()?))?;
    let html = resp.text()?;
    let src_re = Regex::new(r#"<script[^>]+src="([^"]+\.js[^"]*)""#)?;
    let scripts: Vec<String> = src_re
        .captures_iter(&html)
        .map(|c| {
            let s = &c[1];
            if s.starts_with("http") {
                s.to_string()
            } else if let Some(rest) = s.strip_prefix('/') {
                format!("{BASE}/{rest}")
            } else {
                format!("{BASE}/{s}")
            }
        })
        .collect();

    let dd_re = Regex::new(r"var dd=([^;]+);")?;
    let str_re = Regex::new(r"^'([^']*)'(?:\+'([^']*)')*$")?;

    for url in scripts {
        let Ok(resp) = http::get(client, &url, cfg, Some(headers()?)) else { continue };
        let Ok(js) = resp.text() else { continue };
        let Some(c) = dd_re.captures(&js) else { continue };
        let expr = c[1].trim().to_string();
        if str_re.is_match(&expr) {
            let passphrase = expr.replace('\'', "").replace('+', "");
            *PASSPHRASE_CACHE.lock().unwrap() = Some(passphrase.clone());
            return Ok(passphrase);
        }
        // Obfuscated eval-based passphrase — not ported (see module docs).
    }
    Err(anyhow!("[uaserials-com] passphrase не знайдено"))
}

struct SearchItem {
    title: String,
    oname: String,
    url: String,
}

fn search(client: &Client, _cfg: &Config, query: &str) -> Result<Vec<SearchItem>> {
    let mut headers = headers()?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert(ORIGIN, HeaderValue::from_str(BASE)?);
    headers.insert(REFERER, HeaderValue::from_str(&format!("{BASE}/series/"))?);
    let body = format!(
        "do=search&subaction=search&search_start=0&result_from=1&story={}",
        super::common::utf8_percent_encode(query)
    );
    let resp = client
        .post(format!("{BASE}/index.php?do=search"))
        .headers(headers)
        .body(body)
        .send()?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    let mut out = Vec::new();
    if let Ok(sel) = Selector::parse("a.uas-card[data-uas-type=\"post\"]") {
        let title_sel = Selector::parse(".uas-card__title").unwrap();
        let orig_sel = Selector::parse(".uas-card__orig").unwrap();
        for el in doc.select(&sel) {
            let Some(href) = el.value().attr("href") else { continue };
            let title = el
                .select(&title_sel)
                .next()
                .map(|e| e.text().collect::<String>())
                .unwrap_or_default()
                .trim()
                .to_string();
            let oname = el
                .select(&orig_sel)
                .next()
                .map(|e| e.text().collect::<String>())
                .unwrap_or_default()
                .trim()
                .to_string();
            if !title.is_empty() {
                let url = if href.starts_with("http") {
                    href.to_string()
                } else {
                    format!("{BASE}{href}")
                };
                out.push(SearchItem { title, oname, url });
            }
        }
    }
    if out.is_empty() {
        let re = Regex::new(
            r#"(?s)<a class="short-img[^"]*" href="([^"]+)"[\s\S]*?<div class="th-title[^"]*"[^>]*>([^<]+)</div>\s*<div class="th-title-oname[^"]*"[^>]*>([^<]+)</div>"#,
        )?;
        for c in re.captures_iter(&html) {
            out.push(SearchItem {
                url: c[1].to_string(),
                title: c[2].trim().to_string(),
                oname: c[3].trim().to_string(),
            });
        }
    }
    Ok(out)
}

fn get_vods(client: &Client, cfg: &Config, page_url: &str, passphrase: &str) -> Result<Vec<Value>> {
    let resp = http::get(client, page_url, cfg, Some(headers()?))?;
    let html = resp.text()?;
    let tag_re = Regex::new(r"data-tag\d+='(\{[^']+\})'")?;
    let mut players = Vec::new();
    for c in tag_re.captures_iter(&html) {
        let Ok(decrypted) = aes_decrypt(passphrase, &c[1]) else { continue };
        let cleaned = decrypted.replace('\\', "");
        let Ok(data) = serde_json::from_str::<Value>(&cleaned) else { continue };
        if let Value::Array(arr) = data {
            for mut p in arr {
                if let Some(url) = p.get("url").and_then(|x| x.as_str()) {
                    let fixed = url.replace("/usp/", "/vod/");
                    if let Value::Object(ref mut map) = p {
                        map.insert("url".to_string(), Value::String(fixed));
                    }
                }
                players.push(p);
            }
        }
    }
    let trailer_re = Regex::new(r"(?i)трейлер").unwrap();
    Ok(players
        .into_iter()
        .filter(|p| {
            let name = p.get("tabName").and_then(|x| x.as_str()).unwrap_or("");
            !trailer_re.is_match(name)
        })
        .collect())
}

/// Port of tortuga.js `transformTortugaPlayers` → Vec<PNode> (flattened пізніше).
fn transform_players(client: &Client, cfg: &Config, players: &[Value]) -> Vec<super::common::PNode> {
    use super::common::PNode;
    let mut out = Vec::new();
    for player in players {
        let tab_name = player
            .get("tabName")
            .and_then(|x| x.as_str())
            .unwrap_or("Tortuga")
            .to_string();

        let seasons = player.get("seasons").and_then(|x| x.as_array());
        let episodes = player.get("episodes").and_then(|x| x.as_array());

        if let Some(seasons) = seasons.filter(|s| !s.is_empty()) {
            let season_nodes: Vec<PNode> = seasons
                .iter()
                .map(|season| {
                    let s_title = season.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let eps = season.get("episodes").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                    let ep_nodes: Vec<PNode> = eps
                        .iter()
                        .map(|ep| {
                            let e_title = ep.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let sounds = ep.get("sounds").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                            let sound_nodes: Vec<PNode> = sounds
                                .iter()
                                .filter_map(|s| {
                                    let title = s.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    let url = s.get("url").and_then(|x| x.as_str())?;
                                    Some(PNode::leaf(title, url))
                                })
                                .collect();
                            PNode::branch(e_title, sound_nodes)
                        })
                        .collect();
                    PNode::branch(s_title, ep_nodes)
                })
                .collect();
            out.push(PNode::branch(tab_name, season_nodes));
        } else if let Some(episodes) = episodes.filter(|e| !e.is_empty()) {
            let has_multi = episodes.iter().any(|ep| {
                ep.get("sounds")
                    .and_then(|x| x.as_array())
                    .map(|a| a.len() > 1)
                    .unwrap_or(false)
            });
            if has_multi {
                let ep_nodes: Vec<PNode> = episodes
                    .iter()
                    .map(|ep| {
                        let e_title = ep.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let sounds = ep.get("sounds").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                        let sound_nodes: Vec<PNode> = sounds
                            .iter()
                            .filter_map(|s| {
                                let title = s.get("title").and_then(|x| x.as_str()).unwrap_or(&e_title).to_string();
                                let url = s.get("url").and_then(|x| x.as_str())?;
                                Some(PNode::leaf(title, url))
                            })
                            .collect();
                        PNode::branch(e_title, sound_nodes)
                    })
                    .collect();
                out.push(PNode::branch(tab_name, ep_nodes));
            } else {
                let ep_nodes: Vec<PNode> = episodes
                    .iter()
                    .filter_map(|ep| {
                        let e_title = ep.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let url = ep
                            .get("sounds")
                            .and_then(|x| x.as_array())
                            .and_then(|a| a.first())
                            .and_then(|s| s.get("url"))
                            .and_then(|x| x.as_str())
                            .or_else(|| ep.get("url").and_then(|x| x.as_str()))?;
                        Some(PNode::leaf(e_title, url))
                    })
                    .collect();
                out.push(PNode::branch(tab_name, ep_nodes));
            }
        } else if let Some(url) = player.get("url").and_then(|x| x.as_str()) {
            if let Ok(Some(vod)) = parse_tortuga_vod(client, cfg, url) {
                out.push(PNode::leaf(tab_name, vod.file));
            }
        }
    }
    out
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[uaserials-com] Пошук: {}", meta.title);
    let passphrase = get_passphrase(client, cfg)?;
    let query = if !meta.title.is_empty() { &meta.title } else { &meta.original_title };
    if query.is_empty() {
        return Ok(vec![]);
    }
    let mut results = search(client, cfg, query)?;
    if results.is_empty() && !meta.original_title.is_empty() && meta.original_title != meta.title {
        results = search(client, cfg, &meta.original_title)?;
    }
    if results.is_empty() {
        return Ok(vec![]);
    }
    let norm = clean_title;
    let nt = norm(&meta.title);
    let no = norm(&meta.original_title);
    let target = results
        .iter()
        .find(|r| norm(&r.title) == nt || (!no.is_empty() && norm(&r.oname) == no))
        .or_else(|| {
            results
                .iter()
                .find(|r| norm(&r.title).contains(&nt) || (!no.is_empty() && norm(&r.oname).contains(&no)))
        })
        .or_else(|| results.first());
    let Some(target) = target else { return Ok(vec![]) };

    let players = get_vods(client, cfg, &target.url, &passphrase)?;
    if players.is_empty() {
        return Ok(vec![]);
    }
    let nodes = transform_players(client, cfg, &players);
    Ok(flatten_playlist(&nodes, "uaserials-com"))
}

pub struct UaserialsCom;

impl Provider for UaserialsCom {
    fn name(&self) -> &str {
        "uaserials-com"
    }
    fn resolve_movie(&self, client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
        // Оригінал: тільки фільми (TV повертає VOD-и, які тут не програються).
        resolve(client, cfg, meta)
    }
    fn resolve_tv(
        &self,
        _client: &Client,
        _cfg: &Config,
        _meta: &Meta,
        _season: u32,
        _episode: u32,
    ) -> Result<Vec<Stream>> {
        Ok(vec![])
    }
}
