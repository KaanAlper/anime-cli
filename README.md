<p align="center">
  <img src="assets/logo.svg" width="280" alt="anime-cli" />
</p>

# ✦ anime-cli

Terminal üzerinden Türkçe altyazılı anime arama, izleme ve indirme aracı (**Rust** + **Charm `gum`** + **`mpv`**).

## Özellikler

- **Çoklu Kaynak Desteği:**
  - **AnimeciX** — JSON API, çözünürlük seçimi (`1080p`/`720p`/`480p`) ve otomatik Türkçe altyazı
  - **Anizle** — Aynalar (`anizle.co`, `anizm.com.tr`, `anizm.pro`), 24 saatlik katalog önbelleği, çevirmen/fansub seçimi ve HLS çözünürlük ayrıştırma
  - **TürkAnime Arşiv** — AnimeDepo statik JSON arşivi (GitHub & GitLab aynaları), ayrı fansub ve oynatıcı seçimi, ölü bağlantılarda otomatik yedek oynatıcıya geçiş
- **Akıllı Çözünürlük Seçimi:** Varsayılan olarak en yüksek çözünürlüğü (`best`: `1080p > 720p > 480p`) otomatik seçer; menüden veya ayarlardan değiştirilebilir.
- **Arka Plan İndirme & Event Bus:**
  - Tek bölüm, tüm sezon veya özel aralık (`5-12`) indirme
  - Ayarlarda **"İzlerken İndir"** kutucuğu ve özelleştirilebilir **İndirme Dizini**
  - Merkezi **Event Bus** sayesinde izleme başladığında aktif indirme `SIGSTOP` ile duraklatılır, izleme bitince `SIGCONT` ile kaldığı yerden devam eder
- **Kaldığın Yerden Devam:** `mpv --save-position-on-quit` ve `--go` ile son izlenen bölümden tek komutta devam
- **Güvenli Çıkış & Kontrol:** `mpv` açıkken `Ctrl+C` ile yalnızca oynatıcı kapanıp menüye döner; ana menüde `Ctrl+C` çıkış onayı ister

---

## Kurulum

### Gereksinimler
- `mpv`
- `gum` (yoksa uygulama başlangıçta `sudo pacman -S gum` ile kurmayı teklif eder)
- `yt-dlp` & `ffmpeg` (indirme ve TürkAnime Arşiv embed bağlantıları için)
- `rust` / `cargo` (kaynaktan derleme için)

### Tek Satırda Kurulum

**Linux (Arch / Debian / Ubuntu — Wayland & X11 otomatik algılanır):**
```bash
bash -c "$(curl -fsSL https://raw.githubusercontent.com/KaanAlper/anime-cli/main/install.sh)"
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/KaanAlper/anime-cli/main/install.ps1 | iex
```

**Hazır İkili / Taşınabilir `.exe` Paketi (Kurulum Gerektirmez):**
[GitHub Releases](https://github.com/KaanAlper/anime-cli/releases/latest) sayfasından:
- **Windows (Hepsi İçinde Taşınabilir ZIP):** `anime-windows-x86_64-bundle.zip` (`anime.exe` + `mpv` + `yt-dlp` + `ffmpeg` + `gum` dahil — indirip çıkarın ve `anime.exe`ye tıklayın)
- **Tek Dosya:** `anime-windows-x86_64.exe` veya `anime-linux-x86_64` (ilk açılışta eksik bağımlılıklar otomatik kurulur)

### `git clone` ile Kurulum

```bash
git clone https://github.com/KaanAlper/anime-cli.git
cd anime-cli
cargo build --release
install -Dm755 target/release/anime ~/.local/bin/anime
```

*(Sistem geneline kurmak isterseniz: `sudo install -Dm755 target/release/anime /usr/local/bin/anime`)*

---

## Kullanım

```bash
anime              # İnteraktif menüyü başlatır
anime --go         # Son izlenen anime ve bölümden devam eder
anime -s anizle    # Belirli kaynak ile başlatır (animecix, anizle, turkanime)
```
