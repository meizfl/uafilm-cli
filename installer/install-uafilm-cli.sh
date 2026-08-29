#!/usr/bin/env bash

set -euo pipefail

APP_NAME="uafilm-cli"
INSTALL_DIR="/opt/uafilm-cli"
BIN_LINK="/bin/uafilm-cli"
DESKTOP_LINK="/usr/share/applications/uafilm-cli.desktop"

REPO="meizfl/uafilm-cli"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

SVG_CONTENT='<svg width="31" height="32" viewBox="0 0 31 32" fill="none" xmlns="http://www.w3.org/2000/svg">
    <defs>
        <linearGradient id="grad" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stop-color="#C4B5FD"/>
            <stop offset="50%" stop-color="#8B5CF6"/>
            <stop offset="100%" stop-color="#6D28D9"/>
        </linearGradient>
        <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
            <feDropShadow dx="1.2" dy="1.5" stdDeviation="1.2" flood-color="#4C1D95" flood-opacity="0.45"/>
        </filter>
    </defs>
    <path d="M0.557816 9.24832L29.4367 0.035822C30.033 -0.154392 30.6108 0.380053 30.4678 0.989283L23.505 30.6436C23.3493 31.3074 22.4888 31.4853 22.0783 30.9384L13.0188 18.8739C12.961 18.7969 12.89 18.7308 12.809 18.6788L0.371912 10.6918C-0.201881 10.3233 -0.0899653 9.45505 0.557816 9.24832Z"
          fill="url(#grad)"
          filter="url(#shadow)"/>
    </svg>'

DESKTOP_CONTENT='[Desktop Entry]
Comment=The best way to search for movies
Exec=/opt/uafilm-cli/uafilm-cli
Icon=/opt/uafilm-cli/uafilm-cli.svg
Name=UaFilm CLI
GenericName=Media player
Categories=AudioVideo;Video;Player;
NoDisplay=false
Path=
PrefersNonDefaultGPU=false
StartupNotify=true
Terminal=true
TerminalOptions=
Type=Application
X-KDE-SubstituteUID=false
X-KDE-Username=
'

die() {
    echo "Error: $*" >&2
    exit 1
}

require_root() {
    if [[ $EUID -ne 0 ]]; then
        die "this script must be run as root (e.g. sudo $0)"
    fi
}

remove() {
    require_root

    echo "Removing UaFilm CLI..."

    rm -f "$BIN_LINK"
    rm -f "$DESKTOP_LINK"
    rm -rf "$INSTALL_DIR"

    echo "UaFilm CLI has been successfully removed."
}

install() {
    require_root

    command -v curl >/dev/null 2>&1 ||
        die "curl is not installed"

    echo "Fetching the latest release..."

    local tag
    tag="$(
        curl -fsSL "$API_URL" |
        sed -n 's/.*"tag_name": "\(.*\)",/\1/p' |
        head -n1
    )"

    [[ -n "$tag" ]] ||
        die "failed to determine the latest release"

    echo "Latest release: $tag"

    local download_url
    download_url="https://github.com/${REPO}/releases/download/${tag}/uafilm-cli-linux-amd64"

    echo "Downloading uafilm-cli..."

    mkdir -p "$INSTALL_DIR"

    curl -fL --progress-bar \
        "$download_url" \
        -o "$INSTALL_DIR/uafilm-cli" ||
        die "failed to download uafilm-cli"

    chmod 755 "$INSTALL_DIR/uafilm-cli"

    echo "Installing SVG icon..."

    printf '%s\n' "$SVG_CONTENT" > "$INSTALL_DIR/uafilm-cli.svg"
    chmod 644 "$INSTALL_DIR/uafilm-cli.svg"

    echo "Installing desktop entry..."

    printf '%s' "$DESKTOP_CONTENT" > "$INSTALL_DIR/uafilm-cli.desktop"
    chmod 644 "$INSTALL_DIR/uafilm-cli.desktop"

    echo "Creating symbolic links..."

    ln -sfn "$INSTALL_DIR/uafilm-cli" "$BIN_LINK"
    ln -sfn "$INSTALL_DIR/uafilm-cli.desktop" "$DESKTOP_LINK"

    echo
    echo "========================================"
    echo " UaFilm CLI has been successfully installed!"
    echo "========================================"
    echo
    echo "Version: $tag"
    echo "Binary:  $INSTALL_DIR/uafilm-cli"
    echo "Command: $BIN_LINK"
    echo
    echo "To remove:"
    echo "  sudo $0 --remove"
}

main() {
    case "${1:-}" in
        "")
            install
            ;;
        "--remove")
            remove
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
}

main "$@"
