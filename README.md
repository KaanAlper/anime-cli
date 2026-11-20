# anime-cli

Rust ile yazılmış, terminal üzerinden Türkçe altyazılı anime arama ve izleme aracı (`anime` komutu).

## Desteklenen Kaynaklar

- **AnimeciX** — JSON API, çoklu çözünürlük ve Türkçe altyazı desteği
- **Anizle** — Aynalar (`anizle.co`, `anizm.com.tr`, `anizm.pro`), disk önbellekli katalog ve HLS (`1080p` / `720p` / `480p`) çözünürlük ayrıştırma
- **TürkAnime Arşiv** — AnimeDepo statik JSON arşivi (GitLab & GitHub aynaları)

## Özellikler

- **Otomatik En Yüksek Çözünürlük:** Varsayılan olarak `best` (en yüksek çözünürlük: `1080p > 720p > 480p`) seçilir; menüden veya ayarlardan her an değiştirilebilir.
- **Kaldığın Yerden Devam:** `mpv --save-position-on-quit` ve yerel izleme geçmişi (`anime --go`).
- **Hızlı Yerel Önbellek:** Anizle ve TürkAnime dizinleri 24 saat boyunca `~/.cache/anime-cli/` altında önbelleklenir.

## Kurulum

Gereksinimler: `rust`/`cargo`, `mpv`, `gum` (ve TürkAnime Arşiv embed linkleri için `yt-dlp`)

```bash
cargo build --release
install -Dm755 target/release/anime ~/.local/bin/anime
```

## Kullanım

```bash
anime              # İnteraktif TUI menüsü
anime --go         # Son izlenen anime/bölümden devam et
anime -s anizle    # Belirli kaynak ile başlat (animecix, anizle, turkanime)
```
