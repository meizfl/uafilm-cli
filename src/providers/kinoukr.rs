//! Port of kinoukr.js — сайт kinoukr.tv → Ashdi + Tortuga посилання.
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT};
use scraper::{Html, Selector};

use super::ashdi_parser::get_links_from_ashdi_url;
use super::common::flatten_playlist;
use super::tortuga::{parse_tortuga_embed, parse_tortuga_vod};
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE: &str = "https://kinoukr.tv";
const COOKIES: &str = "onlyforkinoukr=1; lampac-off=1";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:149.0) Gecko/20100101 Firefox/149.0";

fn base_headers() -> Result<HeaderMap> {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_static(UA));
    h.insert(COOKIE, HeaderValue::from_static(COOKIES));
    Ok(h)
}

fn get_dle_hash(client: &Client, cfg: &Config) -> Result<Option<String>> {
    let resp = http::get(client, &format!("{BASE}/main/"), cfg, Some(base_headers()?))?;
    let html = resp.text()?;
    let re1 = Regex::new(r"dle_login_hash\s*=\s*'([^']+)'")?;
    if let Some(c) = re1.captures(&html) {
        return Ok(Some(c[1].to_string()));
    }
    let re2 = Regex::new(r"user_hash\s*=\s*'([^']+)'")?;
    Ok(re2.captures(&html).map(|c| c[1].to_string()))
}

struct SearchItem {
    title: String,
    url: String,
}

fn search(client: &Client, _cfg: &Config, query: &str, dle_hash: &str) -> Result<Vec<SearchItem>> {
    let form = format!(
        "story={}&dle_hash={}&thisUrl=%2Fmain%2F",
        super::common::utf8_percent_encode(query),
        super::common::utf8_percent_encode(dle_hash)
    );
    let mut headers = base_headers()?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
    );
    headers.insert("X-Requested-With", HeaderValue::from_static("XMLHttpRequest"));
    headers.insert(ORIGIN, HeaderValue::from_str(BASE)?);
    headers.insert(REFERER, HeaderValue::from_str(&format!("{BASE}/main/"))?);
    let resp = client
        .post(format!("{BASE}/engine/lazydev/dle_search/ajax.php"))
        .headers(headers)
        .body(form)
        .send()?;
    let text = resp.text()?;
    // Може повернутись або сирий HTML, або {"content": "..."}
    let html = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        v.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string()
    } else {
        text
    };
    if html.is_empty() {
        return Ok(vec![]);
    }
    let doc = Html::parse_document(&html);
    let a_sel = Selector::parse("a").unwrap();
    let heading_sel = Selector::parse(".searchheading").unwrap();
    let mut out = Vec::new();
    for el in doc.select(&a_sel) {
        let Some(href) = el.value().attr("href") else { continue };
        let title = el
            .select(&heading_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("{BASE}{href}")
        };
        out.push(SearchItem { title, url });
    }
    Ok(out)
}

fn pick_best<'a>(results: &'a [SearchItem], title: &str, year: &str) -> Option<&'a SearchItem> {
    let norm = |s: &str| s.trim().to_lowercase();
    let nt = norm(title);
    if let Some(r) = results.iter().find(|r| norm(&r.title) == nt) {
        return Some(r);
    }
    if !year.is_empty() {
        let year_re = Regex::new(&format!(r"\b{}\b", regex::escape(year))).ok();
        if let Some(re) = &year_re {
            if let Some(r) = results
                .iter()
                .find(|r| norm(&r.title).contains(&nt) && re.is_match(&r.title))
            {
                return Some(r);
            }
        }
    }
    if let Some(r) = results.iter().find(|r| norm(&r.title).contains(&nt)) {
        return Some(r);
    }
    results.first()
}

struct Vod {
    label: String,
    url: String,
}

fn get_vods(client: &Client, cfg: &Config, page_url: &str) -> Result<Vec<Vod>> {
    let resp = http::get(client, page_url, cfg, Some(base_headers()?))?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);

    let mut tab_names = Vec::new();
    if let Ok(sel) = Selector::parse(".fplayer .tabs-sel span, .tabs-sel span") {
        for el in doc.select(&sel) {
            let t = el.text().collect::<String>().trim().to_string();
            if !t.is_empty() {
                tab_names.push(t);
            }
        }
    }

    let mut vods = Vec::new();
    if let Ok(sel) = Selector::parse(".fplayer .tabs-b.video-box, .tabs-b.video-box") {
        let iframe_sel = Selector::parse("iframe").unwrap();
        for (i, content) in doc.select(&sel).enumerate() {
            let Some(iframe) = content.select(&iframe_sel).next() else { continue };
            let Some(src) = iframe.value().attr("src") else { continue };
            let label = tab_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("Player {}", i + 1));
            if Regex::new(r"(?i)трейлер").unwrap().is_match(&label) {
                continue;
            }
            vods.push(Vod {
                label,
                url: src.to_string(),
            });
        }
    }
    Ok(vods)
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[kinoukr] Пошук: {}", meta.title);
    let Some(dle_hash) = get_dle_hash(client, cfg)? else {
        return Ok(vec![]);
    };
    let query = meta
        .imdb_id
        .clone()
        .filter(|i| super::common::is_imdb_id(i))
        .unwrap_or_else(|| meta.title.clone());
    if query.is_empty() {
        return Ok(vec![]);
    }
    let mut results = search(client, cfg, &query, &dle_hash)?;
    if results.is_empty() && meta.imdb_id.is_some() {
        results = search(client, cfg, &meta.title, &dle_hash)?;
    }
    if results.is_empty() {
        return Ok(vec![]);
    }
    let Some(target) = pick_best(&results, &meta.title, &meta.year) else {
        return Ok(vec![]);
    };
    let vods = get_vods(client, cfg, &target.url)?;
    if vods.is_empty() {
        return Ok(vec![]);
    }

    let mut streams = Vec::new();
    for vod in &vods {
        if vod.url.to_lowercase().contains("ashdi") {
            if let Ok(links) = get_links_from_ashdi_url(client, cfg, &vod.url, &meta.title) {
                for l in links {
                    streams.push(Stream {
                        url: l.file,
                        title: if l.title.is_empty() { vod.label.clone() } else { l.title },
                        provider: "kinoukr".into(),
                        quality: String::new(),
                        season: l.season,
                        episode: l.episode,
                        referer: Some(vod.url.clone()),
                    });
                }
            }
        } else if vod.url.to_lowercase().contains("tortuga") {
            if Regex::new(r"(?i)tortuga\.tw/embed/").unwrap().is_match(&vod.url) {
                if let Ok(Some(nodes)) = parse_tortuga_embed(client, cfg, &vod.url) {
                    streams.extend(flatten_playlist(&nodes, "kinoukr"));
                }
            } else if let Ok(Some(v)) = parse_tortuga_vod(client, cfg, &vod.url) {
                streams.push(Stream {
                    url: v.file,
                    title: vod.label.clone(),
                    provider: "kinoukr".into(),
                    quality: String::new(),
                    season: None,
                    episode: None,
                    referer: Some(vod.url.clone()),
                });
            }
        }
    }
    Ok(streams)
}

pub struct Kinoukr;

impl Provider for Kinoukr {
    fn name(&self) -> &str {
        "kinoukr"
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
