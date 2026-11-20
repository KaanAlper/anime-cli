use anyhow::{Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{REFERER, USER_AGENT};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;
use url::Url;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/137.0.0.0 Safari/537.36";

#[derive(Clone)]
struct Segment {
    cum_secs: f64,
    dur: f64,
    url: String,
}

pub struct HlsProxy {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Drop for HlsProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start_hls_proxy(playlist_url: &str, referer: Option<&str>) -> Result<HlsProxy> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent(UA)
        .build()?;

    let ref_str = referer.unwrap_or("https://pl.puffytr.tr/").to_string();
    let body = client
        .get(playlist_url)
        .header(REFERER, &ref_str)
        .send()?
        .text()
        .context("HLS playlist okunamadı")?;

    let base_url = Url::parse(playlist_url)?;
    let mut segs = Vec::new();
    let mut cur_dur = 2.0_f64;
    let mut cum = 0.0_f64;

    for line in body.lines() {
        let s = line.trim();
        if let Some(rest) = s.strip_prefix("#EXTINF:") {
            if let Ok(d) = rest.split(',').next().unwrap_or("2.0").trim().parse::<f64>() {
                cur_dur = d;
            }
        } else if !s.is_empty() && !s.starts_with('#') {
            let abs = if s.starts_with("http") {
                s.to_string()
            } else {
                base_url.join(s).map(|u| u.to_string()).unwrap_or_default()
            };
            if !abs.is_empty() {
                segs.push(Segment {
                    cum_secs: cum,
                    dur: cur_dur,
                    url: abs,
                });
                cum += cur_dur;
            }
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);
    let segs = Arc::new(segs);

    thread::spawn(move || {
        while !stop_clone.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let segs_c = Arc::clone(&segs);
                    let client_c = client.clone();
                    let ref_c = ref_str.clone();
                    thread::spawn(move || {
                        let _ = handle_conn(stream, &segs_c, &client_c, &ref_c);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });

    Ok(HlsProxy { port, stop })
}

fn handle_conn(
    mut stream: TcpStream,
    segs: &[Segment],
    client: &Client,
    referer: &str,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut req_line = String::new();
    reader.read_line(&mut req_line)?;
    let path = req_line.split_whitespace().nth(1).unwrap_or("/");

    // Drain headers
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
    }

    if let Some(rest) = path.strip_prefix("/from/") {
        let sec: f64 = rest
            .split('.')
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);
        let start_i = segs
            .iter()
            .position(|s| s.cum_secs + s.dur > sec)
            .unwrap_or(0);

        let mut out = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n",
        );
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{start_i}\n"));
        for (i, seg) in segs.iter().enumerate().skip(start_i) {
            out.push_str(&format!("#EXTINF:{:.6},\n/seg/{i}.ts\n", seg.dur));
        }
        out.push_str("#EXT-X-ENDLIST\n");

        let bytes = out.as_bytes();
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        stream.write_all(hdr.as_bytes())?;
        stream.write_all(bytes)?;
    } else if let Some(rest) = path.strip_prefix("/seg/") {
        let idx: usize = rest
            .split('.')
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        if let Some(seg) = segs.get(idx) {
            let resp = client
                .get(&seg.url)
                .header(REFERER, referer)
                .header(USER_AGENT, UA)
                .send()?;
            let bytes = resp.bytes()?;
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(hdr.as_bytes());
            let _ = stream.write_all(&bytes);
        } else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    } else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    }

    Ok(())
}
