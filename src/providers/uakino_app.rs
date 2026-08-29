//! Port of uakino-app.js — api.uakino.app, автентифікація через mTLS клієнтський
//! сертифікат. Замість SQLite (Node-версія) тут повний каталог кешується у JSON-
//! файл на диску (TTL 24 год) і шукається лінійно в пам'яті — прийнятно для
//! одноразового CLI-запуску.
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use super::ashdi_parser::get_links_from_ashdi_url;
use super::common::{flatten_playlist, PNode};
use super::{Provider, Stream};
use crate::config::Config;
use crate::tmdb::Meta;

const API_HOST: &str = "https://api.uakino.app";
const CERT_PEM: &str = include_str!("uakino_certs/cert.pem");
const CERT_KEY: &str = include_str!("uakino_certs/cert.key");
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Deserialize)]
struct FilterItem {
    id: i64,
    title: Option<String>,
    #[serde(default)]
    xfields: String,
}

struct Row {
    id: i64,
    title: String,
    origname: String,
    ashdivip: String,
    playlist: String,
}

fn mtls_client(cfg: &Config) -> Result<Client> {
    let combined = format!("{CERT_PEM}\n{CERT_KEY}");
    let identity = reqwest::Identity::from_pem(combined.as_bytes()).context("uakino-app mTLS identity")?;
    Client::builder()
        .identity(identity)
        .timeout(Duration::from_secs(120))
        .user_agent("ktor-client")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .context("uakino-app https client")
        .map(|c| {
            let _ = cfg; // config not otherwise needed for this dedicated client
            c
        })
}

fn parse_xfields(xf: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for part in xf.split("||") {
        if let Some(idx) = part.find('|') {
            let key = part[..idx].trim().to_string();
            let val = part[idx + 1..].trim().to_string();
            m.insert(key, val);
        }
    }
    m
}

fn cache_path() -> std::path::PathBuf {
    std::env::temp_dir().join("uafilm-cli-uakino.json")
}

fn load_catalog(cfg: &Config) -> Result<Vec<Row>> {
    let path = cache_path();
    let fresh = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or(Duration::MAX) < TTL)
        .unwrap_or(false);

    let text = if fresh {
        eprintln!("[uakino-app] Каталог з кешу: {}", path.display());
        fs::read_to_string(&path)?
    } else {
        eprintln!("[uakino-app] Завантаження повного каталогу (limit=99999)...");
        let client = mtls_client(cfg)?;
        let resp = client
            .get(format!("{API_HOST}/api/v1/filter?limit=99999"))
            .send()
            .context("uakino-app filter request")?;
        let t = resp.text()?;
        let _ = fs::write(&path, &t);
        t
    };

    let items: Vec<FilterItem> = serde_json::from_str(&text).context("uakino-app filter parse")?;
    let rows: Vec<Row> = items
        .into_iter()
        .map(|item| {
            let xf = parse_xfields(&item.xfields);
            Row {
                id: item.id,
                title: item.title.unwrap_or_default().trim().to_string(),
                origname: xf.get("origname").cloned().unwrap_or_default().trim().to_string(),
                ashdivip: xf.get("ashdivip").cloned().unwrap_or_default(),
                playlist: xf.get("playlist").cloned().unwrap_or_default(),
            }
        })
        .collect();
    eprintln!("[uakino-app] Записів: {}", rows.len());
    Ok(rows)
}

fn search<'a>(rows: &'a [Row], title: &str, orig_title: &str) -> Option<&'a Row> {
    let nt = title.trim().to_lowercase();
    let no = orig_title.trim().to_lowercase();
    if nt.is_empty() && no.is_empty() {
        return None;
    }
    if let Some(r) = rows.iter().find(|r| {
        r.title.to_lowercase() == nt || r.origname.to_lowercase() == nt || r.origname.to_lowercase() == no
    }) {
        return Some(r);
    }
    if let Some(r) = rows
        .iter()
        .find(|r| r.title.to_lowercase().contains(&nt) || r.origname.to_lowercase().contains(&nt))
    {
        return Some(r);
    }
    if !no.is_empty() {
        if let Some(r) = rows.iter().find(|r| r.origname.to_lowercase().contains(&no)) {
            return Some(r);
        }
    }
    let words: Vec<&str> = nt.split_whitespace().filter(|w| w.len() > 2).collect();
    if !words.is_empty() {
        return rows.iter().find(|r| {
            let t = r.title.to_lowercase();
            let o = r.origname.to_lowercase();
            words.iter().any(|w| t.contains(w) || o.contains(w))
        });
    }
    None
}

fn fix_playlist_proto(v: &mut Value) {
    match v {
        Value::Array(arr) => arr.iter_mut().for_each(fix_playlist_proto),
        Value::Object(map) => {
            if let Some(Value::String(file)) = map.get("file").cloned() {
                if let Some(rest) = file.strip_prefix("//") {
                    map.insert("file".into(), Value::String(format!("https:{rest}")));
                }
            }
            if let Some(folder) = map.get_mut("folder") {
                fix_playlist_proto(folder);
            }
        }
        _ => {}
    }
}

fn json_to_pnode(v: &Value) -> Option<PNode> {
    let map = v.as_object()?;
    let title = map.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if let Some(file) = map.get("file").and_then(|x| x.as_str()) {
        return Some(PNode::leaf(title, file));
    }
    if let Some(folder) = map.get("folder").and_then(|x| x.as_array()) {
        let children: Vec<PNode> = folder.iter().filter_map(json_to_pnode).collect();
        return Some(PNode::branch(title, children));
    }
    None
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    let _ = client; // сайт вимагає окремий mTLS-клієнт, звичайний HTTP-клієнт тут не використовується
    eprintln!("[uakino-app] Пошук: {}", meta.title);
    let rows = load_catalog(cfg)?;
    let Some(row) = search(&rows, &meta.title, &meta.original_title) else {
        eprintln!("[uakino-app] Не знайдено: {}", meta.title);
        return Ok(vec![]);
    };
    eprintln!("[uakino-app] Знайдено: {} (id: {})", row.title, row.id);

    if !row.ashdivip.is_empty() {
        let url = if let Some(rest) = row.ashdivip.strip_prefix("//") {
            format!("https:{rest}")
        } else {
            row.ashdivip.clone()
        };
        if let Ok(links) = get_links_from_ashdi_url(client, cfg, &url, &meta.title) {
            if !links.is_empty() {
                return Ok(links
                    .into_iter()
                    .map(|l| Stream {
                        url: l.file,
                        title: if l.title.is_empty() { row.title.clone() } else { l.title },
                        provider: "uakino-app".into(),
                        quality: String::new(),
                        season: l.season,
                        episode: l.episode,
                        referer: Some(url.clone()),
                    })
                    .collect());
            }
        }
    }

    if !row.playlist.is_empty() {
        let cleaned = row.playlist.replace("\\\"", "\"");
        if let Ok(mut parsed) = serde_json::from_str::<Value>(&cleaned) {
            fix_playlist_proto(&mut parsed);
            let nodes: Vec<PNode> = match &parsed {
                Value::Array(arr) => arr.iter().filter_map(json_to_pnode).collect(),
                Value::Object(_) => json_to_pnode(&parsed).into_iter().collect(),
                _ => vec![],
            };
            if !nodes.is_empty() {
                return Ok(flatten_playlist(&nodes, "uakino-app"));
            }
        }
    }

    eprintln!("[uakino-app] Немає ashdivip/playlist для id: {}", row.id);
    Ok(vec![])
}

pub struct UakinoApp;

impl Provider for UakinoApp {
    fn name(&self) -> &str {
        "uakino-app"
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
