//! Port of uaserials-my.js — сайт uaserials.my → провайдер HDVB.
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};

use super::common::flatten_playlist;
use super::hdvb::parse_hdvb_html;
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE: &str = "https://uaserials.my";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:149.0) Gecko/20100101 Firefox/149.0";

fn headers() -> Result<HeaderMap> {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_static(UA));
    Ok(h)
}

struct SearchItem {
    title: String,
    oname: String,
    url: String,
}

fn search(client: &Client, cfg: &Config, query: &str) -> Result<Vec<SearchItem>> {
    let url = format!(
        "{BASE}/index.php?do=search&subaction=search&search_start=0&full_search=0&story={}",
        super::common::utf8_percent_encode(query)
    );
    let resp = http::get(client, &url, cfg, Some(headers()?))?;
    let html = resp.text()?;
    let re = Regex::new(
        r#"(?s)<a class="short-img[^"]*" href="([^"]+)"[\s\S]*?<div class="th-title[^"]*"[^>]*>([^<]+)</div>\s*<div class="th-title-oname[^"]*"[^>]*>([^<]+)</div>"#,
    )?;
    Ok(re
        .captures_iter(&html)
        .map(|c| SearchItem {
            url: c[1].to_string(),
            title: c[2].trim().to_string(),
            oname: c[3].trim().to_string(),
        })
        .collect())
}

fn get_vods(client: &Client, cfg: &Config, page_url: &str) -> Result<Vec<(String, String)>> {
    let resp = http::get(client, page_url, cfg, Some(headers()?))?;
    let html = resp.text()?;
    let mut iframes = Vec::new();

    let re1 = Regex::new(
        r#"(?s)<div class="video_box tabs_b[^"]*">[\s\S]*?<iframe[^>]+(?:data-src|src)="([^"]+)"[^>]*title="([^"]*)""#,
    )?;
    for c in re1.captures_iter(&html) {
        iframes.push((c[1].to_string(), c[2].to_string()));
    }

    if iframes.is_empty() {
        let re2 = Regex::new(r#"<iframe[^>]+(?:data-src|src)="([^"]+)"[^>]*title="([^"]*)""#)?;
        for c in re2.captures_iter(&html) {
            if c[1].contains("hdvb") || c[1].contains("hdvbua") {
                iframes.push((c[1].to_string(), c[2].to_string()));
            }
        }
    }

    if iframes.is_empty() {
        let re3 = Regex::new(r#"https?://[^"'\s]*hdvb[^"'\s]+"#)?;
        for m in re3.find_iter(&html) {
            iframes.push((m.as_str().to_string(), "HDVB".to_string()));
        }
    }

    let trailer_re = Regex::new(r"(?i)трейлер").unwrap();
    let trailer_url_re = Regex::new(r"(?i)/trailer/").unwrap();
    Ok(iframes
        .into_iter()
        .filter(|(u, l)| !trailer_re.is_match(l) && !trailer_url_re.is_match(u))
        .collect())
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[uaserials-my] Пошук: {}", meta.title);
    if meta.title.is_empty() {
        return Ok(vec![]);
    }
    let mut results = search(client, cfg, &meta.title)?;
    if results.is_empty() && !meta.original_title.is_empty() && meta.original_title != meta.title {
        results = search(client, cfg, &meta.original_title)?;
    }
    if results.is_empty() {
        return Ok(vec![]);
    }
    let norm = |s: &str| s.trim().to_lowercase();
    let nt = norm(&meta.title);
    let no = norm(&meta.original_title);
    let target = results
        .iter()
        .find(|r| norm(&r.title) == nt)
        .or_else(|| {
            if !meta.original_title.is_empty() {
                results.iter().find(|r| norm(&r.oname) == no)
            } else {
                None
            }
        })
        .or_else(|| {
            results
                .iter()
                .find(|r| norm(&r.title).contains(&nt) || (!no.is_empty() && norm(&r.oname).contains(&no)))
        })
        .or_else(|| results.first());
    let Some(target) = target else { return Ok(vec![]) };

    let iframes = get_vods(client, cfg, &target.url)?;
    if iframes.is_empty() {
        return Ok(vec![]);
    }

    for (url, _label) in &iframes {
        let referer = format!("{BASE}/");
        let mut h = HeaderMap::new();
        h.insert(reqwest::header::REFERER, HeaderValue::from_str(&referer)?);
        if let Ok(resp) = http::get(client, url, cfg, Some(h)) {
            if let Ok(html) = resp.text() {
                if let Some(nodes) = parse_hdvb_html(&html, &meta.title) {
                    return Ok(flatten_playlist(&nodes, "uaserials-my"));
                }
            }
        }
    }
    Ok(vec![])
}

pub struct UaserialsMy;

impl Provider for UaserialsMy {
    fn name(&self) -> &str {
        "uaserials-my"
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
