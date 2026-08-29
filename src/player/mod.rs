//! Playback cascade:
//! - mpv / vlc / ffplay from PATH
//! - **builtin**: ffmpeg-sidecar (downloads static ffmpeg once next to the binary)
//! - system `open` (may use a browser)
//! - none: print URL

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerKind {
    Auto,
    Mpv,
    Vlc,
    Ffplay,
    System,
    Builtin,
    None,
}

impl PlayerKind {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "mpv" => Self::Mpv,
            "vlc" => Self::Vlc,
            "ffplay" => Self::Ffplay,
            "system" | "default" | "open" => Self::System,
            "builtin" | "internal" | "ff" | "ffmpeg" => Self::Builtin,
            "none" => Self::None,
            "auto" | "" => Self::Auto,
            _ => Self::Auto,
        }
    }
}

fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
        let exe = dir.join(format!("{cmd}.exe"));
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

fn run_external(bin: PathBuf, args: &[String]) -> Result<()> {
    let st = Command::new(&bin)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawn {}", bin.display()))?;
    if !st.success() {
        bail!("{} exited with {st}", bin.display());
    }
    Ok(())
}

fn ensure_sidecar_ffmpeg() -> Result<PathBuf> {
    use ffmpeg_sidecar::command::ffmpeg_is_installed;
    use ffmpeg_sidecar::download::auto_download;
    use ffmpeg_sidecar::paths::ffmpeg_path;

    if !ffmpeg_is_installed() {
        eprintln!("[player] Завантажую ffmpeg у кеш біля бінарника (один раз)...");
        // Keep ffplay if the package provides it (do not set KEEP_ONLY_FFMPEG)
        auto_download().map_err(|e| anyhow::anyhow!("ffmpeg download: {e}"))?;
    }
    let p = ffmpeg_path();
    if !p.is_file() && which("ffmpeg").is_none() {
        // ffmpeg_path may return "ffmpeg" expecting PATH — resolve
        if let Some(w) = which("ffmpeg") {
            return Ok(w);
        }
        bail!("ffmpeg sidecar missing at {}", p.display());
    }
    if p.is_file() {
        eprintln!("[player] ffmpeg: {}", p.display());
        Ok(p)
    } else {
        which("ffmpeg").ok_or_else(|| anyhow::anyhow!("ffmpeg not found"))
    }
}

fn play_builtin(url: &str, referer: Option<&str>) -> Result<()> {
    let ffmpeg = ensure_sidecar_ffmpeg()?;

    // Prefer ffplay next to ffmpeg
    if let Some(dir) = ffmpeg.parent() {
        for name in ["ffplay", "ffplay.exe"] {
            let p = dir.join(name);
            if p.is_file() {
                eprintln!("[+] Builtin ffplay...");
                let mut args = vec![
                    "-autoexit".into(),
                    "-loglevel".into(),
                    "error".into(),
                ];
                if let Some(r) = referer {
                    args.push("-headers".into());
                    args.push(format!("Referer: {r}\r\n"));
                }
                args.push(url.into());
                return run_external(p, &args);
            }
        }
    }
    // Also try PATH ffplay after sidecar install
    if let Some(p) = which("ffplay") {
        eprintln!("[+] ffplay (PATH)...");
        let mut args = vec!["-autoexit".into(), "-loglevel".into(), "error".into()];
        if let Some(r) = referer {
            args.push("-headers".into());
            args.push(format!("Referer: {r}\r\n"));
        }
        args.push(url.into());
        return run_external(p, &args);
    }

    eprintln!("[+] Builtin ffmpeg + SDL...");
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    if let Some(r) = referer {
        args.push("-headers".into());
        args.push(format!("Referer: {r}\r\n"));
    }
    args.extend([
        "-i".into(),
        url.into(),
        "-f".into(),
        "sdl".into(),
        "ashdi".into(),
    ]);
    if run_external(ffmpeg.clone(), &args).is_ok() {
        return Ok(());
    }

    eprintln!("[player] SDL недоступний — пробую аудіо-вивід...");

    #[cfg(target_os = "windows")]
    {
        let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];
        if let Some(r) = referer {
            args.push("-headers".into());
            args.push(format!("Referer: {r}\r\n"));
        }
        args.extend([
            "-re".into(),
            "-i".into(),
            url.into(),
            "-f".into(),
            "winwave".into(),
            "default".into(),
        ]);
        if run_external(ffmpeg.clone(), &args).is_ok() {
            return Ok(());
        }
    }

    #[cfg(target_os = "linux")]
    {
        for sink in ["pulse", "alsa"] {
            let mut args: Vec<String> =
                vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];
            if let Some(r) = referer {
                args.push("-headers".into());
                args.push(format!("Referer: {r}\r\n"));
            }
            args.extend([
                "-re".into(),
                "-i".into(),
                url.into(),
                "-vn".into(),
                "-f".into(),
                sink.into(),
                "default".into(),
            ]);
            if run_external(ffmpeg.clone(), &args).is_ok() {
                return Ok(());
            }
        }
    }

    bail!(
        "Вбудований плеєр не відкрив вікно.\n\
         ffmpeg: {}\n\
         Встанови mpv або використай --player none.",
        ffmpeg.display()
    )
}

pub fn play_auto(url: &str, referer: Option<&str>) -> Result<()> {
    if which("mpv").is_some() {
        return play(url, PlayerKind::Mpv, referer);
    }
    if which("vlc").is_some() {
        return play(url, PlayerKind::Vlc, referer);
    }
    if which("ffplay").is_some() {
        return play(url, PlayerKind::Ffplay, referer);
    }
    match play(url, PlayerKind::Builtin, referer) {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("[player] builtin failed: {e}");
            eprintln!("[player] system open...");
            play(url, PlayerKind::System, referer)
        }
    }
}

pub fn play(url: &str, kind: PlayerKind, referer: Option<&str>) -> Result<()> {
    match kind {
        PlayerKind::Auto => play_auto(url, referer),
        PlayerKind::None => {
            println!("{url}");
            Ok(())
        }
        PlayerKind::System => {
            eprintln!("[+] System open...");
            open::that(url).map_err(|e| anyhow::anyhow!("open: {e}"))
        }
        PlayerKind::Mpv => {
            let bin = which("mpv").ok_or_else(|| anyhow::anyhow!("mpv не в PATH"))?;
            eprintln!("[+] mpv...");
            let mut args = Vec::new();
            if let Some(r) = referer {
                args.push(format!("--referrer={r}"));
                args.push(format!("--http-header-fields=Referer: {r}"));
            }
            args.push(url.to_string());
            run_external(bin, &args)
        }
        PlayerKind::Vlc => {
            let bin = which("vlc").ok_or_else(|| anyhow::anyhow!("vlc не в PATH"))?;
            eprintln!("[+] vlc...");
            run_external(bin, &[url.to_string()])
        }
        PlayerKind::Ffplay => {
            let bin = which("ffplay").ok_or_else(|| anyhow::anyhow!("ffplay не в PATH"))?;
            eprintln!("[+] ffplay...");
            run_external(bin, &["-autoexit".into(), url.to_string()])
        }
        PlayerKind::Builtin => play_builtin(url, referer),
    }
}
