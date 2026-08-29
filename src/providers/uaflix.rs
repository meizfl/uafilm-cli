use anyhow::{Context, Result};
use regex::Regex;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use url::Url;

use super::ashdi_parser::{get_links_from_ashdi_url, normalize_url};
use super::{Provider, Stream};
use crate::config::Config;
use crate::http;
use crate::tmdb::Meta;

const BASE: &str = "https://uafix.net";

pub struct Uaflix;

fn norm(s: &str) -> String {
    let s = s.to_lowercase().replace('ё', "е");
    let re = Regex::new(r"['`ʼ’]").unwrap();
    let s = re.replace_all(&s, "'");
    let re = Regex::new(r"[^a-z0-9а-яіїєґ'\s]").unwrap();
    let s = re.replace_all(&s, "");
    let re = Regex::new(r"\s+").unwrap();
    re.replace_all(&s, " ").trim().to_string()
}

fn search(client: &Client, cfg: &Config, query: &str) -> Result<Vec<(String, String, Option<String>)>> {
    let url = format!(
        "{BASE}/index.php?do=search&subaction=search&story={}",
        utf8_percent_encode(query)
    );
    let resp = http::get(client, &url, cfg, None)?;
    let html = resp.text()?;
    let doc = Html::parse_document(&html);
    let sel = Selector::parse(".sres-wrap").unwrap();
    let h2_sel = Selector::parse(".sres-text h2").unwrap();
    let mut out = Vec::new();
    for el in doc.select(&sel) {
        let title = el
            .select(&h2_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();
        let href = el
            .value()
            .attr("href")
            .or_else(|| {
                el.select(&Selector::parse("a").unwrap())
                    .next()
                    .and_then(|a| a.value().attr("href"))
            })
            .map(|h| join_url(BASE, h))
            .unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let text = el.text().collect::<String>();
        let year = Regex::new(r"\b((?:19|20)\d{2})\b")
            .ok()
            .and_then(|re| re.captures(&text))
            .map(|c| c[1].to_string());
        out.push((title, href, year));
    }
    eprintln!("[uaflix] search '{query}' → {} результат(ів)", out.len());
    Ok(out)
}

fn pick(
    candidates: &[(String, String, Option<String>)],
    title: &str,
    orig: &str,
    year: &str,
) -> Option<(String, String)> {
    let tu = norm(title);
    let te = norm(orig);
    let y: Option<i32> = year.parse().ok();

    let year_ok = |c: &Option<String>| -> bool {
        match (y, c.as_ref().and_then(|s| s.parse::<i32>().ok())) {
            (Some(y), Some(cy)) => (cy - y).abs() <= 1,
            _ => true,
        }
    };

    for (raw, link, yr) in candidates {
        if !year_ok(yr) {
            continue;
        }
        let parts: Vec<_> = raw.split('/').map(norm).collect();
        if parts.iter().any(|p| p == &tu || (!te.is_empty() && p == &te)) {
            return Some((raw.clone(), link.clone()));
        }
    }
    for (raw, link, yr) in candidates {
        if !year_ok(yr) {
            continue;
        }
        let n = norm(raw);
        if (!tu.is_empty() && n.contains(&tu)) || (!te.is_empty() && n.contains(&te)) {
            return Some((raw.clone(), link.clone()));
        }
    }
    candidates
        .first()
        .map(|(a, b, _)| (a.clone(), b.clone()))
}

fn is_geo_blocked(html: &str) -> bool {
    Regex::new(r"(?i)тільки трейлер|пройдіть авторизацію|змініть країну|VPN")
        .map(|re| re.is_match(html))
        .unwrap_or(false)
}

fn extract_players(html: &str) -> Vec<(String, String)> {
    let doc = Html::parse_document(html);
    let mut players = Vec::new();
    let tabs_sel = Selector::parse(".tabs-sel .tabs-link").unwrap();
    let box_sel = Selector::parse(".tabs-b.video-box").unwrap();
    let tabs: Vec<_> = doc.select(&tabs_sel).collect();
    let boxes: Vec<_> = doc.select(&box_sel).collect();
    if !tabs.is_empty() && !boxes.is_empty() {
        for (i, tab) in tabs.iter().enumerate() {
            let name = tab.text().collect::<String>().trim().to_string();
            if let Some(div) = boxes.get(i) {
                if let Some(iframe) = div.select(&Selector::parse("iframe").unwrap()).next() {
                    if let Some(src) = iframe.value().attr("src") {
                        if !src.contains("youtube.com") && !src.contains("youtu.be") {
                            players.push((
                                if name.is_empty() {
                                    format!("Player {}", i + 1)
                                } else {
                                    name
                                },
                                src.to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }
    if players.is_empty() {
        let iframe_sel = Selector::parse(".video-box iframe, iframe").unwrap();
        for iframe in doc.select(&iframe_sel) {
            if let Some(src) = iframe.value().attr("src") {
                if !src.contains("youtube.com") && !src.contains("youtu.be") {
                    players.push(("Дивитись онлайн".into(), src.to_string()));
                }
            }
        }
    }
    players
}

fn collect_episodes(html: &str, base: &str) -> Vec<(u32, u32, String)> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("#sers-wr .video-item").unwrap();
    let a_sel = Selector::parse("a[href]").unwrap();
    let mut eps = Vec::new();
    for el in doc.select(&sel) {
        let Some(a) = el.select(&a_sel).next() else {
            continue;
        };
        let href = a.value().attr("href").unwrap_or("");
        let url = join_url(base, href);
        let text = el.text().collect::<String>();
        let season = Regex::new(r"(?i)Сезон\s+(\d+)")
            .ok()
            .and_then(|re| re.captures(&text))
            .and_then(|c| c[1].parse().ok())
            .unwrap_or(1);
        let episode = Regex::new(r"(?i)Серія\s+(\d+)")
            .ok()
            .and_then(|re| re.captures(&text))
            .and_then(|c| c[1].parse().ok())
            .unwrap_or((eps.len() + 1) as u32);
        eps.push((season, episode, url));
    }
    eps
}

fn extract_m3u8(client: &Client, cfg: &Config, iframe_src: &str, referer: &str) -> Option<String> {
    let headers = http::referer_headers(referer);
    let resp = http::get(client, iframe_src, cfg, Some(headers)).ok()?;
    let html = resp.text().ok()?;
    if let Ok(re) = Regex::new(r#"file\s*:\s*["']([^"']+\.m3u8[^"']*)["']"#) {
        if let Some(c) = re.captures(&html) {
            return Some(normalize_url(&c[1]));
        }
    }
    if let Ok(re) = Regex::new(r#"https?://[^\s"']+\.m3u8[^\s"']*"#) {
        if let Some(m) = re.find(&html) {
            return Some(normalize_url(m.as_str()));
        }
    }
    // zetvideo /vod/ pages often embed player with file:
    if let Ok(re) = Regex::new(r#"(?s)file\s*:\s*["']([^"']+)["']"#) {
        if let Some(c) = re.captures(&html) {
            let u = normalize_url(&c[1]);
            if u.starts_with("http") {
                return Some(u);
            }
        }
    }
    None
}

fn players_to_streams(
    client: &Client,
    cfg: &Config,
    players: &[(String, String)],
    page_url: &str,
    title: &str,
    season: Option<u32>,
    episode: Option<u32>,
) -> Vec<Stream> {
    let mut streams = Vec::new();
    for (name, src) in players {
        let src = normalize_url(src);
        if src.to_lowercase().contains("ashdi") {
            let clean = src.split('?').next().unwrap_or(&src);
            if let Ok(links) = get_links_from_ashdi_url(client, cfg, clean, title) {
                for l in links {
                    if let (Some(s), Some(e), Some(want_s), Some(want_e)) =
                        (l.season, l.episode, season, episode)
                    {
                        if s != want_s || e != want_e {
                            continue;
                        }
                    }
                    streams.push(Stream {
                        url: l.file,
                        title: if l.title.is_empty() {
                            name.clone()
                        } else {
                            l.title
                        },
                        provider: "ashdi".into(),
                        quality: String::new(),
                        season: l.season.or(season),
                        episode: l.episode.or(episode),
                        referer: Some(src.clone()),
                    });
                }
            }
            continue;
        }
        if let Some(m3u8) = extract_m3u8(client, cfg, &src, page_url) {
            streams.push(Stream {
                url: m3u8,
                title: name.clone(),
                provider: "uaflix".into(),
                quality: String::new(),
                season,
                episode,
                referer: Some(src.clone()),
            });
        } else {
            // iframe URL itself (mpv may follow / some pages are direct)
            streams.push(Stream {
                url: src.clone(),
                title: format!("{name} (iframe)"),
                provider: "uaflix".into(),
                quality: String::new(),
                season,
                episode,
                referer: Some(page_url.to_string()),
            });
        }
    }
    streams
}

fn find_page(client: &Client, cfg: &Config, meta: &Meta) -> Result<Option<String>> {
    let mut candidates = search(client, cfg, &meta.title)?;
    if candidates.is_empty() && !meta.original_title.is_empty() {
        candidates = search(client, cfg, &meta.original_title)?;
    }
    if let Some((raw, link)) = pick(&candidates, &meta.title, &meta.original_title, &meta.year) {
        eprintln!("[uaflix] Сторінка: {raw}");
        eprintln!("[uaflix] URL: {link}");
        Ok(Some(link))
    } else {
        eprintln!("[uaflix] Нічого не знайдено в пошуку");
        Ok(None)
    }
}

impl Provider for Uaflix {
    fn name(&self) -> &str {
        "uaflix"
    }

    fn resolve_movie(&self, client: &Client, cfg: &Config, meta: &Meta) -> Result<Vec<Stream>> {
        let Some(page) = find_page(client, cfg, meta)? else {
            return Ok(vec![]);
        };
        let resp = http::get(client, &page, cfg, None)?;
        let html = resp.text()?;
        if is_geo_blocked(&html) {
            eprintln!("[uaflix] ⚠ Трейлер/geo — VPN або UAFLIX_COOKIE");
        }
        let players = extract_players(&html);
        eprintln!("[uaflix] Плеєрів: {}", players.len());
        Ok(players_to_streams(
            client,
            cfg,
            &players,
            &page,
            &meta.title,
            None,
            None,
        ))
    }

    fn resolve_tv(
        &self,
        client: &Client,
        cfg: &Config,
        meta: &Meta,
        season: u32,
        episode: u32,
    ) -> Result<Vec<Stream>> {
        let Some(page) = find_page(client, cfg, meta)? else {
            return Ok(vec![]);
        };
        let resp = http::get(client, &page, cfg, None)?;
        let html = resp.text().context("series page")?;
        let eps = collect_episodes(&html, &page);
        eprintln!("[uaflix] Серій у списку: {}", eps.len());

        let mut ep_url = eps
            .iter()
            .find(|(s, e, _)| *s == season && *e == episode)
            .map(|(_, _, u)| u.clone());
        if ep_url.is_none() {
            let fallback = format!(
                "{}/season-{:02}-episode-{:02}/",
                page.trim_end_matches('/'),
                season,
                episode
            );
            eprintln!("[uaflix] Fallback URL: {fallback}");
            ep_url = Some(fallback);
        } else {
            eprintln!("[uaflix] URL серії: {}", ep_url.as_ref().unwrap());
        }
        let ep_url = ep_url.unwrap();

        let headers = http::referer_headers(&page);
        let resp = http::get(client, &ep_url, cfg, Some(headers))?;
        let html = resp.text()?;
        if is_geo_blocked(&html) {
            eprintln!(
                "[uaflix] ⚠ uafix.net віддає лише трейлер (geo/auth).\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20VPN (Україна) або export UAFLIX_COOKIE='...'"
            );
            return Ok(vec![]);
        }
        let players = extract_players(&html);
        eprintln!("[uaflix] Плеєрів на серії: {}", players.len());
        for (n, s) in &players {
            eprintln!("         - {n}: {}", &s[..s.len().min(80)]);
        }
        Ok(players_to_streams(
            client,
            cfg,
            &players,
            &ep_url,
            &meta.title,
            Some(season),
            Some(episode),
        ))
    }
}

fn join_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    Url::parse(base)
        .ok()
        .and_then(|b| b.join(href).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| format!("{base}{href}"))
}

fn utf8_percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
