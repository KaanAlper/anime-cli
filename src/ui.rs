use anyhow::{Context, Result, bail};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use crate::config::cache_dir;
use crate::player::has_command;

const LOGO_SVG_BYTES: &[u8] = include_bytes!("../assets/logo.svg");
static LOGO_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn term_logo_path() -> Option<&'static PathBuf> {
    LOGO_PATH
        .get_or_init(|| {
            let dir = cache_dir();
            let _ = fs::create_dir_all(&dir);
            let svg_path = dir.join("logo.svg");
            if fs::write(&svg_path, LOGO_SVG_BYTES).is_err() {
                return None;
            }
            if has_command("rsvg-convert") {
                let png_path = dir.join("logo_term.png");
                if Command::new("rsvg-convert")
                    .args(["-w", "340", "-o"])
                    .arg(&png_path)
                    .arg(&svg_path)
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
                {
                    return Some(png_path);
                }
            }
            Some(svg_path)
        })
        .as_ref()
}

pub struct UiSession;

impl UiSession {
    pub fn start() -> Result<Self> {
        if !has_command("gum") {
            println!("\x1b[38;2;228;92;192m[!]\x1b[0m Arayüz için 'gum' kurulu değil.");
            print!("'sudo pacman -S gum' ile kurulsun mu? [E/h]: ");
            let _ = io::stdout().flush();
            let mut ans = String::new();
            io::stdin().read_line(&mut ans)?;
            let ans = ans.trim().to_lowercase();
            if ans.is_empty() || ans == "e" || ans == "y" {
                let status = Command::new("sudo")
                    .args(["pacman", "-S", "gum"])
                    .status()
                    .context("sudo pacman çalıştırılamadı")?;
                if !status.success() {
                    bail!("gum kurulumu başarısız oldu.");
                }
            } else {
                bail!("gum olmadan başlatılamıyor.");
            }
        }
        Ok(Self)
    }
}

impl Drop for UiSession {
    fn drop(&mut self) {
        clear_screen();
    }
}

pub fn suspend() {}
pub fn resume() -> Result<()> {
    Ok(())
}

fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

fn print_header(title: &str, subtitle: &str) {
    clear_screen();
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        && has_command("kitten")
        && let Some(logo) = term_logo_path()
    {
        let _ = Command::new("kitten")
            .args(["icat", "--align", "left", "--stdin", "no"])
            .arg(logo)
            .stderr(Stdio::null())
            .status();
    }
    let mut lines = vec![format!("\x1b[1;38;2;228;92;192m✦ {title}\x1b[0m")];
    if !subtitle.is_empty() {
        lines.push(format!("\x1b[38;2;136;153;170m{subtitle}\x1b[0m"));
    }
    let _ = Command::new("gum")
        .args([
            "style",
            "--border",
            "rounded",
            "--border-foreground",
            "#e45cc0",
            "--padding",
            "0 2",
            "--margin",
            "0 0 1 0",
        ])
        .arg(lines.join("\n"))
        .status();
}

pub fn with_spinner<T, F>(label: &str, f: F) -> T
where
    F: FnOnce() -> T,
{
    clear_screen();
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = Arc::clone(&done);
    let msg = label.to_string();

    let handle = thread::spawn(move || {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let mut idx = 0;
        while !done_clone.load(Ordering::Relaxed) {
            print!(
                "\r\x1b[38;2;228;92;192m{}\x1b[0m \x1b[38;2;170;187;204m{}\x1b[0m",
                frames[idx], msg
            );
            let _ = io::stdout().flush();
            idx = (idx + 1) % frames.len();
            thread::sleep(Duration::from_millis(80));
        }
    });

    let result = f();
    done.store(true, Ordering::Relaxed);
    let _ = handle.join();
    print!("\r\x1b[2K");
    let _ = io::stdout().flush();
    result
}

pub fn show_error(msg: &str) {
    crate::config::log_error(msg);
    clear_screen();
    let _ = Command::new("gum")
        .args([
            "style",
            "--border",
            "rounded",
            "--border-foreground",
            "#ff5f5f",
            "--foreground",
            "#ff5f5f",
            "--padding",
            "0 1",
            &format!("[!] {msg}"),
        ])
        .status();
    println!("\n\x1b[38;2;102;102;102mDevam etmek için Enter'a basın...\x1b[0m");
    let mut buf = String::new();
    let _ = io::stdin().read_line(&mut buf);
}

struct RawTtyGuard {
    saved_mode: Option<String>,
}

impl RawTtyGuard {
    fn enter() -> Result<Self> {
        let out = Command::new("stty")
            .args(["-F", "/dev/tty", "-g"])
            .output()
            .context("stty okunamadı")?;
        let saved_mode = if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            None
        };
        let _ = Command::new("stty")
            .args(["-F", "/dev/tty", "raw", "-echo"])
            .status();
        Ok(Self { saved_mode })
    }
}

impl Drop for RawTtyGuard {
    fn drop(&mut self) {
        if let Some(ref mode) = self.saved_mode {
            let _ = Command::new("stty").args(["-F", "/dev/tty", mode]).status();
        } else {
            let _ = Command::new("stty")
                .args(["-F", "/dev/tty", "sane"])
                .status();
        }
        let _ = io::stdout().write_all(b"\x1b[?25h\r\n");
        let _ = io::stdout().flush();
    }
}

fn highlight_non_selected_match(text: &str, query: &str) -> String {
    if query.is_empty() {
        return format!("\x1b[38;2;255;255;255m{text}\x1b[0m");
    }
    let lower_t = text.to_lowercase();
    let lower_q = query.to_lowercase();
    if let Some(byte_start) = lower_t.find(&lower_q) {
        let byte_end = (byte_start + lower_q.len()).min(text.len());
        if text.is_char_boundary(byte_start) && text.is_char_boundary(byte_end) {
            let before = &text[..byte_start];
            let matched = &text[byte_start..byte_end];
            let after = &text[byte_end..];
            return format!(
                "\x1b[38;2;255;255;255m{before}\x1b[38;2;139;178;127m{matched}\x1b[38;2;255;255;255m{after}\x1b[0m"
            );
        }
    }
    format!("\x1b[38;2;255;255;255m{text}\x1b[0m")
}

pub fn select_highlighted(
    title: &str,
    subtitle: &str,
    items: &[String],
    initial: usize,
) -> Result<Option<usize>> {
    let clean_items: Vec<(usize, &String)> = items
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.trim().is_empty())
        .collect();
    if clean_items.is_empty() {
        return Ok(None);
    }

    print_header(title, subtitle);

    let mut tty = fs::OpenOptions::new()
        .read(true)
        .open("/dev/tty")
        .context("/dev/tty açılamadı")?;
    let _guard = RawTtyGuard::enter()?;

    let mut query = String::new();
    let mut cursor = initial.min(clean_items.len().saturating_sub(1));
    let mut scroll = 0usize;
    let max_vis = 15usize;
    let total_rows = max_vis + 2; // prompt + items + help
    let mut first_draw = true;

    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x1b[?25l");

    loop {
        let q_lower = query.to_lowercase();
        let filtered: Vec<(usize, &String, i32)> = if q_lower.is_empty() {
            clean_items.iter().map(|&(i, s)| (i, s, 0)).collect()
        } else {
            let words: Vec<&str> = q_lower.split_whitespace().collect();
            let mut matched: Vec<(usize, &String, i32)> = clean_items
                .iter()
                .filter_map(|&(i, s)| {
                    let sl = s.to_lowercase();
                    if sl.starts_with(&q_lower) {
                        Some((i, s, 0))
                    } else if sl.contains(&q_lower) {
                        Some((i, s, 1))
                    } else if words.iter().all(|w| sl.contains(w)) {
                        Some((i, s, 2))
                    } else {
                        None
                    }
                })
                .collect();
            matched.sort_by_key(|&(_, _, score)| score);
            matched
        };

        if filtered.is_empty() {
            cursor = 0;
            scroll = 0;
        } else {
            cursor = cursor.min(filtered.len() - 1);
            if cursor < scroll {
                scroll = cursor;
            } else if cursor >= scroll + max_vis {
                scroll = cursor + 1 - max_vis;
            }
        }

        let mut buf = String::with_capacity(2048);
        if !first_draw {
            let _ = write!(buf, "\x1b[{total_rows}A\r");
        }
        first_draw = false;

        if query.is_empty() {
            let _ = write!(
                buf,
                "\x1b[38;2;196;180;139m> \x1b[38;2;136;153;170mFiltrele...\x1b[0m\x1b[K\r\n"
            );
        } else {
            let _ = write!(
                buf,
                "\x1b[38;2;196;180;139m> \x1b[38;2;255;255;255m{query}\x1b[48;2;244;214;236m \x1b[0m\x1b[K\r\n"
            );
        }

        for row in 0..max_vis {
            let idx = scroll + row;
            if let Some(&(_, item, _)) = filtered.get(idx) {
                if idx == cursor {
                    let _ = write!(
                        buf,
                        "\x1b[38;2;228;92;192m» \x1b[48;2;244;214;236;38;2;0;0;0;1m {item} \x1b[0m\x1b[K\r\n"
                    );
                } else {
                    let rendered = highlight_non_selected_match(item, &query);
                    let _ = write!(buf, "   {rendered}\x1b[K\r\n");
                }
            } else if row == 0 && filtered.is_empty() {
                let _ = write!(buf, "   \x1b[38;2;136;153;170m(Eşleşme yok)\x1b[0m\x1b[K\r\n");
            } else {
                buf.push_str("\x1b[K\r\n");
            }
        }

        let _ = write!(
            buf,
            "\x1b[38;2;102;102;102m↑↓ gezin • yazarak filtrele • enter seç • esc geri\x1b[0m\x1b[K\r\n"
        );

        let _ = stdout.write_all(buf.as_bytes());
        let _ = stdout.flush();

        let mut b = [0u8; 16];
        let n = tty.read(&mut b)?;
        if n == 0 {
            return Ok(None);
        }
        let slice = &b[..n];
        if slice == [3] || slice == [27] {
            return Ok(None);
        }
        if slice == [13] || slice == [10] {
            return Ok(filtered.get(cursor).map(|&(orig_idx, _, _)| orig_idx));
        }
        if slice == [127] || slice == [8] {
            query.pop();
            cursor = 0;
            scroll = 0;
            continue;
        }
        if slice.starts_with(b"\x1b[A") || slice.starts_with(b"\x1bOA") {
            cursor = cursor.saturating_sub(1);
            continue;
        }
        if slice.starts_with(b"\x1b[B") || slice.starts_with(b"\x1bOB") {
            if !filtered.is_empty() {
                cursor = (cursor + 1).min(filtered.len() - 1);
            }
            continue;
        }
        if slice.starts_with(b"\x1b[5~") {
            cursor = cursor.saturating_sub(max_vis);
            continue;
        }
        if slice.starts_with(b"\x1b[6~") {
            if !filtered.is_empty() {
                cursor = (cursor + max_vis).min(filtered.len() - 1);
            }
            continue;
        }
        if let Ok(s) = std::str::from_utf8(slice) {
            let mut changed = false;
            for ch in s.chars() {
                if !ch.is_control() {
                    query.push(ch);
                    changed = true;
                }
            }
            if changed {
                cursor = 0;
                scroll = 0;
            }
        }
    }
}

pub fn select(title: &str, subtitle: &str, items: &[String], initial: usize) -> Result<Option<usize>> {
    let clean_items: Vec<(usize, &String)> = items
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.trim().is_empty())
        .collect();
    if clean_items.is_empty() {
        return Ok(None);
    }

    print_header(title, subtitle);

    let input_data = clean_items
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let use_filter = clean_items.len() > 8;
    let mut cmd = Command::new("gum");
    if use_filter {
        cmd.args([
            "filter",
            "--placeholder",
            "Filtrele...",
            "--placeholder.foreground",
            "#8899aa",
            "--indicator",
            "» ",
            "--indicator.foreground",
            "#e45cc0",
            "--selected-prefix",
            "",
            "--unselected-prefix",
            "",
            "--cursor-text.foreground",
            "#e45cc0",
            "--text.foreground",
            "#ffffff",
            "--match.foreground",
            "#8bb27f",
            "--prompt.foreground",
            "#c4b48b",
            "--height",
            "18",
        ]);
    } else {
        cmd.args([
            "choose",
            "--cursor",
            "» ",
            "--cursor.foreground",
            "#e45cc0",
            "--item.foreground",
            "#ffffff",
            "--selected.foreground",
            "#e45cc0",
            "--height",
            "18",
        ]);
        if initial > 0
            && let Some((_, init_item)) = clean_items.get(initial)
        {
            cmd.arg("--selected").arg(init_item.as_str());
        }
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("gum çalıştırılamadı")?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_data.as_bytes());
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Ok(None);
    }

    let picked = strip_ansi(&String::from_utf8_lossy(&output.stdout)).trim().to_string();
    if picked.is_empty() {
        return Ok(None);
    }

    Ok(clean_items
        .into_iter()
        .find(|(_, s)| strip_ansi(s).trim() == picked)
        .map(|(idx, _)| idx))
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for nc in chars.by_ref() {
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn input(title: &str, placeholder: &str) -> Result<Option<String>> {
    print_header(title, "");

    let child = Command::new("gum")
        .args([
            "input",
            "--placeholder",
            placeholder,
            "--placeholder.foreground",
            "#8899aa",
            "--prompt",
            "» ",
            "--prompt.foreground",
            "#c4b48b",
            "--cursor.foreground",
            "#e45cc0",
        ])
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .spawn()
        .context("gum input çalıştırılamadı")?;

    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

pub fn multi_select(
    title: &str,
    subtitle: &str,
    items: &[String],
    preselected: &[String],
) -> Result<Option<Vec<String>>> {
    if items.is_empty() {
        return Ok(Some(Vec::new()));
    }

    print_header(title, subtitle);

    let input_data = items.join("\n");
    let mut cmd = Command::new("gum");
    cmd.args([
        "choose",
        "--no-limit",
        "--show-help",
        "--cursor",
        "» ",
        "--cursor.foreground",
        "#e45cc0",
        "--item.foreground",
        "#ffffff",
        "--selected.foreground",
        "#e45cc0",
        "--height",
        "18",
    ]);

    if !preselected.is_empty() {
        cmd.arg("--selected").arg(preselected.join(","));
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("gum choose çalıştırılamadı")?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_data.as_bytes());
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Ok(None);
    }

    let selected: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(Some(selected))
}
