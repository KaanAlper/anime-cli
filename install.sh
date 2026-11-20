#!/usr/bin/env bash
set -euo pipefail

REPO_URL="https://github.com/KaanAlper/anime-cli.git"
RELEASE_BIN_URL="https://github.com/KaanAlper/anime-cli/releases/latest/download/anime-linux-x86_64"
INSTALL_DIR="$HOME/.local/bin"
BIN_PATH="$INSTALL_DIR/anime"

echo "✦ anime-cli kurulumu başlatılıyor..."

# 1. Dağıtım tespiti (Arch vs Debian/Ubuntu)
DISTRO="unknown"
if command -v pacman >/dev/null 2>&1; then
    DISTRO="arch"
elif command -v apt-get >/dev/null 2>&1; then
    DISTRO="debian"
fi

# 2. Görüntü sunucusu tespiti (Wayland vs X11)
SESSION_DISPLAY="x11"
if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]] || [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
    SESSION_DISPLAY="wayland"
fi

echo "→ Sistem: $DISTRO | Oturum: $SESSION_DISPLAY"

install_arch_deps() {
    local pkgs=(mpv yt-dlp ffmpeg gum git)
    if [[ "$SESSION_DISPLAY" == "wayland" ]]; then
        pkgs+=(wl-clipboard)
    fi
    if [[ -n "${KITTY_WINDOW_ID:-}" ]]; then
        pkgs+=(librsvg)
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        pkgs+=(rust)
    fi
    echo "→ Arch Linux paketleri kuruluyor: ${pkgs[*]}"
    sudo pacman -S --needed --noconfirm "${pkgs[@]}"
}

install_debian_deps() {
    local pkgs=(mpv ffmpeg curl git ca-certificates)
    if [[ "$SESSION_DISPLAY" == "wayland" ]]; then
        pkgs+=(wl-clipboard)
    fi
    if [[ -n "${KITTY_WINDOW_ID:-}" ]]; then
        pkgs+=(librsvg2-bin)
    fi
    echo "→ Debian/Ubuntu paketleri güncelleniyor..."
    sudo apt-get update -qq
    sudo apt-get install -y "${pkgs[@]}"

    if ! command -v yt-dlp >/dev/null 2>&1; then
        if apt-cache show yt-dlp >/dev/null 2>&1; then
            sudo apt-get install -y yt-dlp
        else
            echo "→ yt-dlp ikilisi indiriliyor..."
            sudo curl -fsSL "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp" -o /usr/local/bin/yt-dlp
            sudo chmod a+rx /usr/local/bin/yt-dlp
        fi
    fi

    if ! command -v gum >/dev/null 2>&1; then
        if apt-cache show gum >/dev/null 2>&1; then
            sudo apt-get install -y gum
        else
            echo "→ Charm gum deposu ekleniyor..."
            sudo mkdir -p /etc/apt/keyrings
            curl -fsSL https://repo.charm.sh/apt/gpg.key | sudo gpg --dearmor -o /etc/apt/keyrings/charm.gpg
            echo "deb [signed-by=/etc/apt/keyrings/charm.gpg] https://repo.charm.sh/apt/ * *" | sudo tee /etc/apt/sources.list.d/charm.list >/dev/null
            sudo apt-get update -qq && sudo apt-get install -y gum
        fi
    fi
}

case "$DISTRO" in
    arch) install_arch_deps ;;
    debian) install_debian_deps ;;
    *)
        echo "[!] Desteklenen paket yöneticisi (pacman/apt-get) bulunamadı. mpv, yt-dlp, ffmpeg ve gum'ı manuel kurun."
        ;;
esac

mkdir -p "$INSTALL_DIR"

# 3. Önce hazır release ikilisini dene (x86_64), yoksa kaynaktan derle
ARCH="$(uname -m)"
if [[ "$ARCH" == "x86_64" ]] && curl -fsSL -o "$BIN_PATH.tmp" "$RELEASE_BIN_URL" 2>/dev/null; then
    chmod 755 "$BIN_PATH.tmp"
    mv "$BIN_PATH.tmp" "$BIN_PATH"
    echo "✔ Hazır sürüm indirildi: $BIN_PATH"
else
    rm -f "$BIN_PATH.tmp"
    if ! command -v cargo >/dev/null 2>&1; then
        echo "→ Rust (rustup) kuruluyor..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT
    echo "→ Kaynak kod indiriliyor ve derleniyor..."
    git clone --depth 1 "$REPO_URL" "$TMP_DIR/anime-cli"
    (cd "$TMP_DIR/anime-cli" && cargo build --release)
    install -Dm755 "$TMP_DIR/anime-cli/target/release/anime" "$BIN_PATH"
    echo "✔ Derlendi ve kuruldu: $BIN_PATH"
fi

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "ℹ '$INSTALL_DIR' PATH içinde değil. Kalıcı eklemek için:"
    echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc"
fi

echo ""
echo "✦ Kurulum tamamlandı! Başlatmak için: anime"
