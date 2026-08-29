//! Спільні утиліти для портованих провайдерів (аналог ashdi.js хелперів +
//! узагальнене дерево folder/file, яке повертають hdvb/tortuga/uakino/moonanime).
use regex::Regex;
use url::Url;

use super::Stream;

/// Узагальнений вузол плейлиста: або лист (`file` заповнено), або гілка (`folder`).
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct PNode {
    pub title: String,
    pub file: Option<String>,
    pub poster: Option<String>,
    pub subtitle: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub folder: Vec<PNode>,
}

impl PNode {
    pub fn leaf(title: impl Into<String>, file: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            file: Some(file.into()),
            ..Default::default()
        }
    }
    pub fn branch(title: impl Into<String>, folder: Vec<PNode>) -> Self {
        Self {
            title: title.into(),
            folder,
            ..Default::default()
        }
    }
}

fn extract_num(re: &str, text: &str) -> Option<u32> {
    Regex::new(re).ok()?.captures(text)?.get(1)?.as_str().parse().ok()
}

/// Розгортає дерево PNode у плаский список Stream, успадковуючи назву озвучки
/// (dub) та сезон/серію з батьківських вузлів — так само, як `walk()` в ashdi.js.
pub fn flatten_playlist(nodes: &[PNode], provider: &str) -> Vec<Stream> {
    let mut out = Vec::new();
    for n in nodes {
        walk(n, "", None, None, provider, &mut out);
    }
    out
}

fn walk(
    node: &PNode,
    ctx_dub: &str,
    ctx_season: Option<u32>,
    ctx_episode: Option<u32>,
    provider: &str,
    out: &mut Vec<Stream>,
) {
    let mut dub = ctx_dub.to_string();
    let mut season = node.season.or(ctx_season);
    let mut episode = node.episode.or(ctx_episode);
    if !node.title.is_empty() {
        if dub.is_empty() {
            dub = node.title.clone();
        }
        if let Some(s) = extract_num(r"(?i)(?:сезон|season)\s*(\d+)", &node.title) {
            season = Some(s);
        }
        if let Some(e) = extract_num(r"(?i)(?:серія|серiя|episode|ep\.?)\s*(\d+)", &node.title) {
            episode = Some(e);
        }
    }
    if let Some(file) = &node.file {
        out.push(Stream {
            url: file.clone(),
            title: if dub.is_empty() {
                node.title.clone()
            } else {
                dub.clone()
            },
            provider: provider.to_string(),
            quality: String::new(),
            season,
            episode,
            referer: None,
        });
    }
    for child in &node.folder {
        walk(child, &dub, season, episode, provider, out);
    }
}

pub fn clean_title(s: &str) -> String {
    let s = s.to_lowercase().replace(['\'', '`', 'ʼ', '\u{2019}', '"'], "");
    let s = s.replace('ё', "е");
    let re = Regex::new(r"[^a-z0-9а-яіїєґ\s]").unwrap();
    let s = re.replace_all(&s, " ").to_string();
    let re = Regex::new(r"\s+").unwrap();
    re.replace_all(&s, " ").trim().to_string()
}

pub fn extract_year_from_text(text: &str) -> Option<i32> {
    Regex::new(r"\b((?:19|20)\d{2})\b")
        .ok()?
        .captures(text)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

pub fn absolute_url(href: &str, base: &str) -> String {
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    Url::parse(base)
        .ok()
        .and_then(|b| b.join(href).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| format!("{base}{href}"))
}

pub fn normalize_proto(url: &str) -> String {
    if let Some(stripped) = url.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        url.to_string()
    }
}

pub fn is_imdb_id(s: &str) -> bool {
    Regex::new(r"^tt\d+$").map(|re| re.is_match(s)).unwrap_or(false)
}

pub fn utf8_percent_encode(s: &str) -> String {
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
