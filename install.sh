#!/usr/bin/env bash
set -euo pipefail

REPO_URL="https://github.com/KaanAlper/anime-cli.git"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "✦ anime-cli kuruluyor..."

for cmd in git cargo mpv; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "[!] Eksik bağımlılık: $cmd (lütfen önce kurun)"
    exit 1
  fi
done

git clone --depth 1 "$REPO_URL" "$TMP_DIR/anime-cli"
cd "$TMP_DIR/anime-cli"
cargo build --release

mkdir -p "$HOME/.local/bin"
install -Dm755 target/release/anime "$HOME/.local/bin/anime"

echo "✔ Kurulum tamamlandı: $HOME/.local/bin/anime"
if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
  echo "[!] '$HOME/.local/bin' PATH içinde değil; kabuğunuza eklemeyi unutmayın."
fi
echo "Başlatmak için: anime"
