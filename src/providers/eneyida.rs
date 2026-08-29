//! Port of eneyida.js — сайт eneyida.tv → провайдер HDVB.
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use scraper::{Html, Selector};

use super::common::flatten_playlist;
use super::hdvb::parse_hdvb_iframe;
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE: &str = "https://eneyida.tv";

pub struct Eneyida;

fn find_movie_link(client: &Client, cfg: &Config, title: &str, year: &str) -> Result<Option<String>> {
    let url = format!("{BASE}/index.php?do=search");
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    let client_ref = client;
    let body = format!(
        "do=search&subaction=search&story={}",
        super::common::utf8_percent_encode(title)
    );
    let resp = client_ref
        .post(&url)
        .headers({
            let mut h = headers.clone();
            h.insert(
                reqwest::header::USER_AGENT,
                reqwest::header::HeaderValue::from_str(&cfg.user_agent)?,
            );
            h
        })
        .body(body)
        .send()?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    let art_sel = Selector::parse("article").unwrap();
    let a_sel = Selector::parse("a").unwrap();
    let title_sel = Selector::parse(".short-title, h2, a").unwrap();
    let year_re = if !year.is_empty() {
        Regex::new(&format!(r"(?:^|\D){}(?:\D|$)", regex::escape(year))).ok()
    } else {
        None
    };
    let title_lc = title.to_lowercase();
    for el in doc.select(&art_sel) {
        let Some(a) = el.select(&a_sel).next() else { continue };
        let Some(href) = a.value().attr("href") else { continue };
        let title_text = el.select(&title_sel).next().map(|e| e.text().collect::<String>()).unwrap_or_default().to_lowercase();
        let text_content = el.text().collect::<String>();
        let has_year = match &year_re {
            Some(re) => re.is_match(&text_content),
            None => true,
        };
        let has_title = title_text.contains(&title_lc);
        if has_year && has_title {
            return Ok(Some(href.to_string()));
        }
    }
    Ok(None)
}

fn resolve(client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
    eprintln!("[eneyida] Пошук: {} {}", meta.title, meta.year);
    let Some(link) = find_movie_link(client, cfg, &meta.title, &meta.year)? else {
        eprintln!("[eneyida] Нічого не знайдено");
        return Ok(vec![]);
    };
    let resp = http::get(client, &link, cfg, None)?;
    let html = resp.text()?;
    let re = Regex::new(r#"src="(https?://[^/]+/[^"]+/[0-9]+)""#)?;
    let Some(c) = re.captures(&html) else {
        return Ok(vec![]);
    };
    let iframe_src = c[1].to_string();
    let referer = format!("{BASE}/");
    let Some(nodes) = parse_hdvb_iframe(client, cfg, &iframe_src, &referer, &meta.title)? else {
        return Ok(vec![]);
    };
    Ok(flatten_playlist(&nodes, "eneyida"))
}

impl Provider for Eneyida {
    fn name(&self) -> &str {
        "eneyida"
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
        let mut streams = resolve(client, cfg, meta)?;
        for s in &mut streams {
            if s.season.is_none() {
                s.season = Some(season);
            }
            if s.episode.is_none() {
                s.episode = Some(episode);
            }
        }
        Ok(streams
            .into_iter()
            .filter(|s| s.season == Some(season) && s.episode == Some(episode))
            .collect())
    }
}
