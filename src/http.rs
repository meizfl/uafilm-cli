use anyhow::{Context, Result};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE, REFERER, USER_AGENT};
use std::time::Duration;

use crate::config::Config;

pub fn client(cfg: &Config) -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .user_agent(&cfg.user_agent)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("http client")
}

pub fn get(
    client: &Client,
    url: &str,
    cfg: &Config,
    extra: Option<HeaderMap>,
) -> Result<Response> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_str(&cfg.user_agent)?);
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/json,*/*;q=0.8"),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("uk-UA,uk;q=0.9,en-US;q=0.8,en;q=0.7"),
    );
    if let Some(ref c) = cfg.uaflix_cookie {
        if let Ok(v) = HeaderValue::from_str(c) {
            headers.insert(COOKIE, v);
        }
    }
    if let Some(h) = extra {
        headers.extend(h);
    }
    client
        .get(url)
        .headers(headers)
        .send()
        .with_context(|| format!("GET {url}"))
}

pub fn referer_headers(referer: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(referer) {
        h.insert(REFERER, v);
    }
    h
}
