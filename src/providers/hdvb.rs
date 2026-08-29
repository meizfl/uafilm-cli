//! Port of hdvb.js `parseHdvbIframe` — використовується eneyida.tv та uaserials.my.
use anyhow::Result;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, REFERER};
use serde_json::Value;

use super::common::{normalize_proto, PNode};
use crate::config::Config;
use crate::http;

/// Знаходить збалансований JSON-фрагмент (об'єкт чи масив), що починається з
/// першого символу `input`, рахуючи дужки з урахуванням рядків/екранування.
fn extract_balanced(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    let open = *bytes.first()?;
    let close = match open {
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &ch) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == b'\\' {
            escaped = true;
            continue;
        }
        if ch == b'"' {
            in_string = !in_string;
            continue;
        }
        if !in_string {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(&input[..=i]);
                }
            }
        }
    }
    None
}

fn json_to_pnode(v: &Value, fallback: &str) -> Option<PNode> {
    match v {
        Value::Object(map) => {
            let file = map.get("file").and_then(|x| x.as_str());
            let title = map
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or(fallback)
                .to_string();
            let poster = map.get("poster").and_then(|x| x.as_str()).map(normalize_proto);
            let subtitle = map.get("subtitle").and_then(|x| x.as_str()).map(normalize_proto);
            if let Some(f) = file {
                return Some(PNode {
                    title,
                    file: Some(normalize_proto(f)),
                    poster,
                    subtitle,
                    ..Default::default()
                });
            }
            if let Some(folder) = map.get("folder").and_then(|x| x.as_array()) {
                let children: Vec<PNode> = folder
                    .iter()
                    .filter_map(|c| json_to_pnode(c, fallback))
                    .collect();
                if !children.is_empty() {
                    return Some(PNode::branch(title, children));
                }
            }
            None
        }
        Value::Array(_) => None,
        _ => None,
    }
}

pub fn parse_hdvb_html(html: &str, fallback_title: &str) -> Option<Vec<PNode>> {
    let file_re = Regex::new(r"file\s*:\s*").ok()?;
    let m = file_re.find(html)?;
    let after = &html[m.end()..];
    let first = after.trim_start();
    let leading_ws = after.len() - first.len();
    let after = &after[leading_ws..];

    let file_value: String = if after.starts_with('[') || after.starts_with('{') {
        extract_balanced(after)?.to_string()
    } else if after.starts_with('\'') || after.starts_with('"') {
        let quote = after.chars().next().unwrap();
        let rest = &after[1..];
        let end = rest.find(quote)?;
        rest[..end].to_string()
    } else {
        return None;
    };

    if file_value.starts_with('[') || file_value.starts_with('{') {
        let balanced = extract_balanced(&file_value).unwrap_or(&file_value);
        if let Ok(parsed) = serde_json::from_str::<Value>(balanced) {
            let nodes: Vec<PNode> = match &parsed {
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|item| json_to_pnode(item, fallback_title))
                    .collect(),
                Value::Object(_) => json_to_pnode(&parsed, fallback_title).into_iter().collect(),
                _ => vec![],
            };
            if !nodes.is_empty() {
                return Some(nodes);
            }
        }
        return None;
    }

    if file_value.starts_with("http") {
        let poster = Regex::new(r#"poster:\s?['"]([^'"]+)['"]"#)
            .ok()
            .and_then(|re| re.captures(html))
            .map(|c| normalize_proto(&c[1]));
        let subtitle = Regex::new(r#"subtitle:\s?['"]([^'"]+)['"]"#)
            .ok()
            .and_then(|re| re.captures(html))
            .map(|c| normalize_proto(&c[1]));
        return Some(vec![PNode {
            title: fallback_title.to_string(),
            file: Some(normalize_proto(&file_value)),
            poster,
            subtitle,
            ..Default::default()
        }]);
    }

    None
}

pub fn parse_hdvb_iframe(
    client: &Client,
    cfg: &Config,
    iframe_src: &str,
    referer: &str,
    fallback_title: &str,
) -> Result<Option<Vec<PNode>>> {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(referer) {
        headers.insert(REFERER, v);
    }
    let resp = http::get(client, iframe_src, cfg, Some(headers))?;
    let html = resp.text()?;
    Ok(parse_hdvb_html(&html, fallback_title))
}
