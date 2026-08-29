//! Port of uakino-best.js — сайт uakino.best.
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, ORIGIN, REFERER, USER_AGENT};
use scraper::{Html, Selector};
use serde_json::Value;
use std::collections::HashMap;

use super::ashdi_parser::{normalize_url, parse_player};
use super::common::{flatten_playlist, is_imdb_id, PNode};
use super::tortuga::parse_tortuga_vod;
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE: &str = "https://uakino.best";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:151.0) Gecko/20100101 Firefox/151.0";

fn common_headers() -> Result<HeaderMap> {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_static(UA));
    Ok(h)
}

struct SearchItem {
    url: String,
    title: String,
    orig_title: String,
}

fn search(client: &Client, _cfg: &Config, query: &str) -> Result<Vec<SearchItem>> {
    let mut headers = common_headers()?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
    );
    headers.insert("X-Requested-With", HeaderValue::from_static("XMLHttpRequest"));
    headers.insert(ORIGIN, HeaderValue::from_str(BASE)?);
    headers.insert(REFERER, HeaderValue::from_str(&format!("{BASE}/ua/"))?);
    let body = format!(
        "story={}&thisUrl=%2Fua%2F",
        super::common::utf8_percent_encode(query)
    );
    let resp = client
        .post(format!("{BASE}/engine/lazydev/dle_search/ajax.php"))
        .headers(headers)
        .body(body)
        .send()?;
    let text = resp.text()?;
    let json: Value = serde_json::from_str(&text)?;
    let Some(content) = json.get("content").and_then(|x| x.as_str()) else {
        return Ok(vec![]);
    };
    let doc = Html::parse_document(content);
    let sel = Selector::parse("a.search-result-link").unwrap();
    let heading_sel = Selector::parse(".searchheading").unwrap();
    let orig_sel = Selector::parse(".search-orig-title").unwrap();
    let mut out = Vec::new();
    for el in doc.select(&sel) {
        let Some(href) = el.value().attr("href") else { continue };
        let title = el
            .select(&heading_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();
        let title = Regex::new(r"\s+").unwrap().replace_all(title.trim(), " ").to_string();
        let orig_title = el
            .select(&orig_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();
        if !title.is_empty() {
            out.push(SearchItem {
                url: href.to_string(),
                title,
                orig_title,
            });
        }
    }
    Ok(out)
}

fn get_news_id(url: &str) -> Option<String> {
    Regex::new(r"(\d+)-[^/]*\.html")
        .ok()?
        .captures(url)?
        .get(1)
        .map(|m| m.as_str().to_string())
}

fn detect_season(text: &str) -> Option<u32> {
    Regex::new(r"(?i)(\d+)\s*(?:сезон|season)")
        .ok()?
        .captures(text)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

struct Episode {
    file: String,
    episode: u32,
}

fn fetch_playlist(
    client: &Client,
    cfg: &Config,
    news_id: &str,
    page_url: &str,
) -> Result<Option<HashMap<String, Vec<Episode>>>> {
    let referer = if page_url.starts_with("http") {
        page_url.to_string()
    } else {
        format!("{BASE}{page_url}")
    };
    let mut headers = common_headers()?;
    headers.insert(
        "Accept",
        HeaderValue::from_static("application/json, text/javascript, */*; q=0.01"),
    );
    headers.insert("X-Requested-With", HeaderValue::from_static("XMLHttpRequest"));
    headers.insert(REFERER, HeaderValue::from_str(&referer)?);
    let url = format!("{BASE}/engine/ajax/playlists.php?news_id={news_id}&xfield=playlist");
    let resp = http::get(client, &url, cfg, Some(headers))?;
    let text = resp.text()?;
    let json: Value = serde_json::from_str(&text)?;
    if json.get("success").and_then(|x| x.as_bool()) != Some(true) {
        return Ok(None);
    }
    let Some(response_html) = json.get("response").and_then(|x| x.as_str()) else {
        return Ok(None);
    };
    let doc = Html::parse_document(response_html);
    let li_sel = Selector::parse("li[data-file]").unwrap();
    let mut episodes_by_voice: HashMap<String, Vec<Episode>> = HashMap::new();
    for (i, el) in doc.select(&li_sel).enumerate() {
        let Some(file) = el.value().attr("data-file") else { continue };
        let ep_title = el.text().collect::<String>().trim().to_string();
        let voice = el.value().attr("data-voice").unwrap_or("Original").to_string();
        let ep_num = Regex::new(r"\d+")
            .unwrap()
            .find(&ep_title)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or((i + 1) as u32);
        let file = if let Some(rest) = file.strip_prefix("//") {
            format!("https://{rest}")
        } else {
            file.to_string()
        };
        episodes_by_voice.entry(voice).or_default().push(Episode { file, episode: ep_num });
    }
    Ok(Some(episodes_by_voice))
}

enum PlayerKind {
    Ashdi(String),
    Tortuga(String),
}

fn scrape_movie(client: &Client, cfg: &Config, page_url: &str) -> Result<Option<PlayerKind>> {
    let url = if page_url.starts_with("http") {
        page_url.to_string()
    } else {
        format!("{BASE}{page_url}")
    };
    let mut headers = common_headers()?;
    headers.insert(REFERER, HeaderValue::from_str(&format!("{BASE}/"))?);
    let resp = http::get(client, &url, cfg, Some(headers))?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    for sel_str in ["iframe[src*=\"ashdi.vip\"]", "iframe[data-src*=\"ashdi.vip\"]"] {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(el) = doc.select(&sel).next() {
                if let Some(src) = el.value().attr("src").or_else(|| el.value().attr("data-src")) {
                    return Ok(Some(PlayerKind::Ashdi(src.to_string())));
                }
            }
        }
    }
    for sel_str in [
        "iframe[src*=\"tortuga.wtf\"]",
        "iframe[data-src*=\"tortuga.wtf\"]",
        "iframe[src*=\"tortuga.tw\"]",
        "iframe[data-src*=\"tortuga.tw\"]",
    ] {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(el) = doc.select(&sel).next() {
                if let Some(src) = el.value().attr("src").or_else(|| el.value().attr("data-src")) {
                    return Ok(Some(PlayerKind::Tortuga(src.to_string())));
                }
            }
        }
    }
    Ok(None)
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[uakino-best] Пошук: {}", meta.title);
    let query = meta
        .imdb_id
        .clone()
        .filter(|i| is_imdb_id(i))
        .unwrap_or_else(|| meta.title.clone());
    let mut results = search(client, cfg, &query)?;
    if results.is_empty() && meta.imdb_id.is_some() && !meta.title.is_empty() {
        results = search(client, cfg, &meta.title)?;
    }
    if results.is_empty() {
        return Ok(vec![]);
    }

    let norm = |s: &str| s.trim().to_lowercase();
    let nt = norm(&meta.title);
    let no = norm(&meta.original_title);

    // Групування за оригінальною назвою (як у groupResults JS)
    struct SeasonRef {
        season: Option<u32>,
        url: String,
        news_id: Option<String>,
    }
    struct Group {
        orig_title: String,
        seasons: Vec<SeasonRef>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut idx: HashMap<String, usize> = HashMap::new();
    for r in &results {
        let key = if !r.orig_title.is_empty() {
            r.orig_title.to_lowercase()
        } else {
            r.title.to_lowercase()
        };
        let i = *idx.entry(key.clone()).or_insert_with(|| {
            groups.push(Group {
                orig_title: r.orig_title.clone(),
                seasons: Vec::new(),
            });
            groups.len() - 1
        });
        groups[i].seasons.push(SeasonRef {
            season: detect_season(&r.title),
            url: r.url.clone(),
            news_id: get_news_id(&r.url),
        });
    }
    if groups.is_empty() {
        return Ok(vec![]);
    }
    let group = groups
        .iter()
        .find(|g| g.orig_title.to_lowercase() == nt || g.orig_title.to_lowercase() == no)
        .unwrap_or(&groups[0]);

    // Спроба через playlists.php
    let mut voices_map: HashMap<String, Vec<PNode>> = HashMap::new();
    let mut has_data = false;
    for s in &group.seasons {
        let Some(news_id) = &s.news_id else { continue };
        let s_num = s.season.unwrap_or(1);
        let Ok(Some(pl)) = fetch_playlist(client, cfg, news_id, &s.url) else { continue };
        if pl.is_empty() {
            continue;
        }
        has_data = true;
        for (voice_name, items) in pl {
            let entry = voices_map.entry(voice_name).or_default();
            let ep_nodes: Vec<PNode> = items
                .iter()
                .map(|ep| PNode {
                    title: format!("Серія {}", ep.episode),
                    file: Some(ep.file.clone()),
                    season: Some(s_num),
                    episode: Some(ep.episode),
                    ..Default::default()
                })
                .collect();
            entry.push(PNode {
                title: format!("Сезон {s_num}"),
                folder: ep_nodes,
                ..Default::default()
            });
        }
    }

    if has_data {
        let nodes: Vec<PNode> = voices_map
            .into_iter()
            .map(|(name, folder)| PNode::branch(name, folder))
            .collect();
        return Ok(flatten_playlist(&nodes, "uakino-best"));
    }

    // Фолбек: скрейпимо сторінку фільму на предмет плеєра
    let Some(page_url) = group.seasons.first().map(|s| s.url.clone()) else {
        return Ok(vec![]);
    };
    let Some(player) = scrape_movie(client, cfg, &page_url)? else {
        return Ok(vec![]);
    };
    match player {
        PlayerKind::Ashdi(src) => {
            let url = normalize_url(&src);
            let mut headers = common_headers()?;
            headers.insert(REFERER, HeaderValue::from_str(&page_url)?);
            let resp = http::get(client, &url, cfg, Some(headers))?;
            let html = resp.text()?;
            let links = parse_player(&html, &meta.title);
            Ok(links
                .into_iter()
                .map(|l| Stream {
                    url: l.file,
                    title: if l.title.is_empty() { meta.title.clone() } else { l.title },
                    provider: "uakino-best".into(),
                    quality: String::new(),
                    season: l.season,
                    episode: l.episode,
                    referer: Some(url.clone()),
                })
                .collect())
        }
        PlayerKind::Tortuga(src) => {
            let Some(vod) = parse_tortuga_vod(client, cfg, &src)? else {
                return Ok(vec![]);
            };
            Ok(vec![Stream {
                url: vod.file,
                title: "Original".into(),
                provider: "uakino-best".into(),
                quality: String::new(),
                season: None,
                episode: None,
                referer: Some(src),
            }])
        }
    }
}

pub struct UakinoBest;

impl Provider for UakinoBest {
    fn name(&self) -> &str {
        "uakino-best"
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
