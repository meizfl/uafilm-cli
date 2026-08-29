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

## Build

```bash
TMDB_TOKEN='eyJ...' cargo build --release
./target/release/ashdi --help
```
## Install from GitHub (Linux)
``` bash
curl -fsSL https://raw.githubusercontent.com/meizfl/uafilm-cli/refs/heads/main/installer/install-uafilm-cli.sh | sudo bash
```

## Remove
``` bash
curl -fsSL https://raw.githubusercontent.com/meizfl/uafilm-cli/refs/heads/main/installer/install-uafilm-cli.sh | sudo bash -s -- --remove
```
The installer installs the program to /opt/uafilm-cli/ and creates symbolic links to /bin and /usr/share/applications/
