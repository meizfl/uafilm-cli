mod config;
mod http;
mod player;
mod providers;
mod tmdb;

use anyhow::{bail, Context, Result};
use clap::Parser;
use config::Config;
use player::PlayerKind;
use std::io::{self, Write};

#[derive(Parser, Debug)]
#[command(name = "ashdi", about = "ASHDI CLI (Rust) — TMDB + scrapers, one binary")]
struct Cli {
    /// Назва або TMDB ID
    query: Option<String>,
    #[arg(long, value_parser = ["movie", "tv"])]
    r#type: Option<String>,
    #[arg(short = 's', long)]
    season: Option<u32>,
    #[arg(short = 'e', long)]
    episode: Option<u32>,
    /// auto | mpv | vlc | ffplay | builtin | system | none
    #[arg(long, default_value = "auto")]
    player: String,
    #[arg(long, default_value_t = 1)]
    source: usize,
    #[arg(long = "provider")]
    providers: Vec<String>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    list_providers: bool,
}

fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn choose_index(n: usize, label: &str) -> Result<usize> {
    if n == 1 {
        return Ok(0);
    }
    loop {
        let s = prompt(&format!("Оберіть {label}: "))?;
        if let Ok(i) = s.parse::<usize>() {
            if i >= 1 && i <= n {
                return Ok(i - 1);
            }
        }
        println!("[!] Неправильний вибір.");
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.list_providers {
        println!("uaflix");
        return Ok(());
    }

    let cfg = Config::from_env();
    let client = http::client(&cfg)?;

    let mut query = cli.query.clone().unwrap_or_default();
    if query.is_empty() {
        query = prompt("Фільм / серіал / TMDB ID: ")?;
    }
    if query.is_empty() {
        bail!("Порожній запит");
    }

    let mut media_type = cli.r#type.clone();
    let meta = if query.chars().all(|c| c.is_ascii_digit()) {
        let id: u64 = query.parse()?;
        println!("[+] TMDB ID: {id}");
        if media_type.is_none() {
            println!("\n[1] Фільм\n[2] Серіал");
            let c = prompt("Тип [1/2]: ")?;
            media_type = Some(if c == "2" { "tv" } else { "movie" }.into());
        }
        tmdb::metadata(&client, &cfg, media_type.as_deref().unwrap(), id)?
    } else {
        println!("[+] Пошук TMDB: {query}");
        let results = tmdb::search(&client, &cfg, &query)?;
        if results.is_empty() {
            bail!("Нічого не знайдено");
        }
        println!();
        for (i, item) in results.iter().enumerate() {
            let title = tmdb::title_of(item);
            let year = tmdb::year_of(item);
            let mt = item
                .get("media_type")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("[{}] {title} ({year}) [{mt}] [TMDB {id}]", i + 1);
            if let Some(orig) = item
                .get("original_title")
                .or_else(|| item.get("original_name"))
                .and_then(|v| v.as_str())
            {
                if orig != title {
                    println!("    Original: {orig}");
                }
            }
        }
        println!();
        let idx = choose_index(results.len(), "варіант")?;
        let item = &results[idx];
        let id = item.get("id").and_then(|v| v.as_u64()).context("id")?;
        let mt = media_type
            .clone()
            .unwrap_or_else(|| {
                item.get("media_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("movie")
                    .to_string()
            });
        let m = tmdb::metadata(&client, &cfg, &mt, id)?;
        println!("\n{}", "=".repeat(60));
        println!("{}", m.title);
        if !m.year.is_empty() {
            println!("Рік: {}", m.year);
        }
        println!("TMDB: {}", m.id);
        if let Some(ref imdb) = m.imdb_id {
            println!("IMDB: {imdb}");
        }
        println!("{}", "=".repeat(60));
        m
    };

    let (season, episode) = if meta.media_type == "tv" {
        println!("\n[+] Сезони/серії...");
        let season = if let Some(s) = cli.season {
            s
        } else if !meta.seasons_info.is_empty() {
            let seasons: Vec<u32> = meta.seasons_info.keys().copied().collect();
            if seasons.len() == 1 {
                seasons[0]
            } else {
                println!();
                for s in &seasons {
                    println!("[{s}] Сезон {s}");
                }
                println!();
                let i = choose_index(seasons.len(), "сезон")?;
                seasons[i]
            }
        } else {
            prompt("Номер сезону: ")?.parse()?
        };
        println!("[+] Сезон: {season}");

        let episode = if let Some(e) = cli.episode {
            e
        } else if let Some(&count) = meta.seasons_info.get(&season) {
            let eps: Vec<u32> = (1..=count).collect();
            if eps.len() == 1 {
                eps[0]
            } else {
                println!();
                for e in &eps {
                    println!("[{e}] {e}");
                }
                println!();
                let i = choose_index(eps.len(), "серію")?;
                eps[i]
            }
        } else {
            prompt("Номер серії: ")?.parse()?
        };
        println!("[+] Серія: {episode}");
        (Some(season), Some(episode))
    } else {
        (None, None)
    };

    println!("[+] Резолвлю джерела...");
    let streams = providers::resolve_all(
        &client,
        &cfg,
        &meta,
        &cli.providers,
        season,
        episode,
    );

    if cli.json {
        // simple JSON dump
        let arr: Vec<_> = streams
            .iter()
            .map(|s| {
                serde_json::json!({
                    "url": s.url,
                    "title": s.title,
                    "provider": s.provider,
                    "season": s.season,
                    "episode": s.episode,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    if streams.is_empty() {
        bail!("Джерела не знайдено");
    }

    println!("\n[+] Джерел: {}", streams.len());
    for (i, s) in streams.iter().enumerate() {
        println!("[{}] {} - {}", i + 1, s.provider, s.title);
    }

    let idx = cli.source;
    if idx < 1 || idx > streams.len() {
        bail!("Джерело {idx} не існує");
    }
    let stream = &streams[idx - 1];
    println!("\n[+] Провайдер: {}", stream.provider);
    println!("[+] Джерело: {}", stream.title);

    let kind = PlayerKind::parse(&cli.player);
    player::play(
        &stream.url,
        kind,
        stream.referer.as_deref(),
    )?;
    Ok(())
}
