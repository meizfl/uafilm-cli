//! Port of klon.js — сайт klon.fun (DLE) → провайдер Ashdi.
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, ORIGIN, REFERER, USER_AGENT};
use scraper::{Html, Selector};

use super::ashdi_parser::{normalize_url, parse_player};
use super::common::{absolute_url, clean_title, extract_year_from_text, is_imdb_id};
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE_URL: &str = "https://klon.fun";

fn nav_headers(cfg: &Config, referer: &str) -> Result<HeaderMap> {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_str(&cfg.user_agent)?);
    h.insert(REFERER, HeaderValue::from_str(referer)?);
    Ok(h)
}

fn fetch_user_hash(client: &Client, cfg: &Config) -> Result<Option<String>> {
    let headers = nav_headers(cfg, &format!("{BASE_URL}/"))?;
    let resp = http::get(client, &format!("{BASE_URL}/"), cfg, Some(headers))?;
    let html = resp.text()?;
    let re1 = Regex::new(r"(?:dle_login_hash|user_hash)\s*=\s*'([^']+)'")?;
    if let Some(c) = re1.captures(&html) {
        return Ok(Some(c[1].to_string()));
    }
    let re2 = Regex::new(r#"name="user_hash"\s+value="([^"]+)""#)?;
    if let Some(c) = re2.captures(&html) {
        return Ok(Some(c[1].to_string()));
    }
    Ok(None)
}

struct SearchItem {
    title: String,
    url: String,
}

fn search_title(client: &Client, cfg: &Config, query: &str) -> Result<Vec<SearchItem>> {
    let Some(user_hash) = fetch_user_hash(client, cfg)? else {
        return Ok(vec![]);
    };
    let search_url = format!("{BASE_URL}/engine/ajax/controller.php?mod=search");
    let form = format!(
        "query={}&skin=klontv&user_hash={}",
        super::common::utf8_percent_encode(query),
        super::common::utf8_percent_encode(&user_hash)
    );
    let mut headers = nav_headers(cfg, &format!("{BASE_URL}/"))?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
    );
    headers.insert("X-Requested-With", HeaderValue::from_static("XMLHttpRequest"));
    headers.insert(ORIGIN, HeaderValue::from_str(BASE_URL)?);
    let resp = client
        .post(&search_url)
        .headers(headers)
        .body(form)
        .send()?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    let a_sel = Selector::parse("a[href]").unwrap();
    let heading_sel = Selector::parse(".searchheading").unwrap();
    let href_html_re = Regex::new(r"(?i)\.html(?:\?|$)").unwrap();
    let skip_re = Regex::new(r"(?i)do=search|subaction=search|mode=advanced").unwrap();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for el in doc.select(&a_sel) {
        let Some(href) = el.value().attr("href") else { continue };
        if !href_html_re.is_match(href) || skip_re.is_match(href) {
            continue;
        }
        let title = el
            .select(&heading_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();
        let title = if title.trim().is_empty() {
            el.text().collect::<String>()
        } else {
            title
        };
        let title = title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = absolute_url(href, BASE_URL);
        if seen.insert(url.clone()) {
            out.push(SearchItem { title, url });
        }
    }
    Ok(out)
}

fn score(item: &SearchItem, fallback_title: &str, year: &str, imdb_id: Option<&str>) -> i32 {
    let mut score = 0i32;
    let item_title = clean_title(&item.title);
    let target_title = clean_title(fallback_title);
    let item_year = extract_year_from_text(&item.title).or_else(|| extract_year_from_text(&item.url));
    if imdb_id.map(is_imdb_id).unwrap_or(false) {
        score += 100;
    }
    if !target_title.is_empty() && item_title == target_title {
        score += 80;
    } else if !target_title.is_empty() && item_title.contains(&target_title) {
        score += 45;
    } else if !target_title.is_empty() {
        let words: Vec<&str> = target_title.split(' ').filter(|w| !w.is_empty()).collect();
        score += words.iter().filter(|w| item_title.contains(*w)).count() as i32 * 6;
    }
    if let (Ok(y), Some(iy)) = (year.parse::<i32>(), item_year) {
        if y == iy {
            score += 35;
        }
    }
    if Regex::new(r"(?i)/serialy/").unwrap().is_match(&item.url) {
        score += 3;
    }
    if Regex::new(r"(?i)/filmy/").unwrap().is_match(&item.url) {
        score += 3;
    }
    if Regex::new(r"(?i)/multfilmy/").unwrap().is_match(&item.url) {
        score += 3;
    }
    score
}

fn find_best_post_url(client: &Client, cfg: &Config, meta: &Meta) -> Result<Option<String>> {
    let query = if meta.imdb_id.as_deref().map(is_imdb_id).unwrap_or(false) {
        meta.imdb_id.clone().unwrap()
    } else {
        meta.title.trim().to_string()
    };
    if query.is_empty() {
        return Ok(None);
    }
    let mut results = search_title(client, cfg, &query)?;
    if results.is_empty() {
        return Ok(None);
    }
    results.sort_by(|a, b| {
        score(b, &meta.title, &meta.year, meta.imdb_id.as_deref())
            .cmp(&score(a, &meta.title, &meta.year, meta.imdb_id.as_deref()))
    });
    Ok(results.into_iter().next().map(|r| r.url))
}

fn get_iframe(client: &Client, cfg: &Config, post_url: &str) -> Result<Option<String>> {
    let headers = nav_headers(cfg, &format!("{BASE_URL}/"))?;
    let resp = http::get(client, post_url, cfg, Some(headers))?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    let candidates = [
        "iframe[data-src*=\"ashdi.vip\"]",
        "iframe[src*=\"ashdi.vip\"]",
        "iframe[data-src*=\"ashdi\"]",
        "iframe[src*=\"ashdi\"]",
        "iframe[data-src]",
        "iframe[src]",
    ];
    for sel_str in candidates {
        let Ok(sel) = Selector::parse(sel_str) else { continue };
        if let Some(el) = doc.select(&sel).next() {
            let src = el
                .value()
                .attr("data-src")
                .or_else(|| el.value().attr("src"));
            if let Some(src) = src {
                let mut abs = absolute_url(src, post_url);
                if Regex::new(r"(?i)/vod/\d+").unwrap().is_match(&abs) && !abs.contains("multivoice") {
                    abs.push_str(if abs.contains('?') { "&" } else { "?" });
                    abs.push_str("multivoice");
                }
                return Ok(Some(abs));
            }
        }
    }
    Ok(None)
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[klon] Пошук: {} {}", meta.title, meta.year);
    let Some(post_url) = find_best_post_url(client, cfg, meta)? else {
        return Ok(vec![]);
    };
    let Some(iframe) = get_iframe(client, cfg, &post_url)? else {
        return Ok(vec![]);
    };
    let iframe = normalize_url(&iframe);
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_str(&cfg.user_agent)?);
    let resp = http::get(client, &iframe, cfg, Some(headers))?;
    let html = resp.text()?;
    let links = parse_player(&html, &meta.title);
    Ok(links
        .into_iter()
        .map(|l| Stream {
            url: l.file,
            title: if l.title.is_empty() {
                meta.title.clone()
            } else {
                l.title
            },
            provider: "klon".into(),
            quality: String::new(),
            season: l.season,
            episode: l.episode,
            referer: Some(iframe.clone()),
        })
        .collect())
}

pub struct Klon;

impl Provider for Klon {
    fn name(&self) -> &str {
        "klon"
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
