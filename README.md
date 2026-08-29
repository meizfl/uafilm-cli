# UaFlim CLI

## Compile-time TMDB token

```bash
TMDB_TOKEN='eyJ...' cargo build --release
# or
ASHDI_TMDB_TOKEN='eyJ...' cargo build --release
```

Runtime `export TMDB_TOKEN=...` overrides the baked-in value.

## Builtin player

Default `--player auto`:

1. mpv / vlc / ffplay from PATH  
2. **builtin** — [ffmpeg-sidecar](https://crates.io/crates/ffmpeg-sidecar) downloads a static ffmpeg into user cache on first run (no system package, no browser)  
3. OS `open`  

```bash
./target/release/uafilm-cli "Flower of Evil" --player builtin
./target/release/uafilm-cli "Flower of Evil" --player auto
```

> Pure-Rust H.264/HLS GUI player still is not practical. Sidecar ffmpeg is the reliable zero-install approach.

## Build

```bash
TMDB_TOKEN='eyJ...' cargo build --release
./target/release/ashdi --help
```
