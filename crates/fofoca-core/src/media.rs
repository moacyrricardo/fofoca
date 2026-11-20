//! Replay audio over plain HTTP on 127.0.0.1 (fix for spec 014).
//!
//! WebKitGTK hands `<audio>` to GStreamer, which can't stream from Tauri's custom `asset://`
//! scheme ("NotSupportedError"). GStreamer streams `http://` fine, so the app serves the meeting
//! audio itself: loopback only, a random per-run token in every path, and only the two audio
//! files of a real meeting folder (resolved through [`Store::audio_tracks`]). `Range` requests are
//! honoured so the player can seek without reading the whole file.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::store::Store;

pub struct MediaServer {
    port: u16,
    token: String,
}

impl MediaServer {
    /// Bind an ephemeral loopback port and serve in background threads for the app's lifetime.
    pub fn start(store: Store) -> Result<MediaServer> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).context("binding the replay audio server")?;
        let port = listener.local_addr()?.port();
        let token = random_token()?;
        let t = token.clone();
        std::thread::Builder::new().name("media-server".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let (store, token) = (store.clone(), t.clone());
                let _ = std::thread::Builder::new().name("media-conn".into()).spawn(move || {
                    let _ = handle(stream, &store, &token);
                });
            }
        })?;
        Ok(MediaServer { port, token })
    }

    /// URL of a meeting's track (`"mic"` or `"call"`).
    pub fn url(&self, id: &str, track: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{id}/{track}.wav", self.port, self.token)
    }

    /// The origin to allow in the CSP's `media-src`.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn random_token() -> Result<String> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn handle(stream: TcpStream, store: &Store, token: &str) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut range = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    let mut out = stream;
    if method != "GET" && method != "HEAD" {
        return status(&mut out, "405 Method Not Allowed");
    }
    let Some(file) = resolve(store, token, path) else {
        return status(&mut out, "404 Not Found");
    };
    let mut f = File::open(file)?;
    let len = f.metadata()?.len();
    let (start, end, code) = match range.as_deref().map(|r| parse_range(r, len)) {
        None => (0, len.saturating_sub(1), "200 OK"),
        Some(Some((s, e))) => (s, e, "206 Partial Content"),
        Some(None) => {
            let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return out.write_all(head.as_bytes());
        }
    };
    let body_len = if len == 0 { 0 } else { end - start + 1 };
    let mut head = format!(
        "HTTP/1.1 {code}\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Length: {body_len}\r\nCache-Control: no-store\r\nConnection: close\r\n"
    );
    if code.starts_with("206") {
        head += &format!("Content-Range: bytes {start}-{end}/{len}\r\n");
    }
    head += "\r\n";
    out.write_all(head.as_bytes())?;
    if method == "GET" && body_len > 0 {
        f.seek(SeekFrom::Start(start))?;
        std::io::copy(&mut f.take(body_len), &mut out)?;
    }
    out.flush()
}

fn status(out: &mut TcpStream, code: &str) -> std::io::Result<()> {
    out.write_all(format!("HTTP/1.1 {code}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
}

/// `/<token>/<meeting-id>/<mic|call>.wav` → the file, if the token matches and the meeting has it.
fn resolve(store: &Store, token: &str, path: &str) -> Option<PathBuf> {
    let mut seg = path.strip_prefix('/')?.split('/');
    let (t, id, name) = (seg.next()?, seg.next()?, seg.next()?);
    if seg.next().is_some() || t != token {
        return None;
    }
    let tracks = store.audio_tracks(id).ok()?;
    match name {
        "mic.wav" => tracks.mic,
        "call.wav" => tracks.call,
        _ => None,
    }
}

/// Parse a single `bytes=` range into an inclusive `(start, end)`; `None` if unsatisfiable.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-500", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-2", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }

    fn get(url: &str, range: Option<&str>) -> (u16, Option<String>, Vec<u8>) {
        let client = reqwest::blocking::Client::new();
        let mut req = client.get(url);
        if let Some(r) = range {
            req = req.header("Range", r);
        }
        let res = req.send().unwrap();
        let cr = res.headers().get("content-range").map(|v| v.to_str().unwrap().to_string());
        (res.status().as_u16(), cr, res.bytes().unwrap().to_vec())
    }

    #[test]
    fn serves_only_meeting_audio_with_ranges() {
        let dir = std::env::temp_dir().join(format!("fofoca-media-{}", std::process::id()));
        let store = Store::new(dir.clone()).unwrap();
        let meta = store.create(&crate::StartInfo::default()).unwrap();
        let body: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        std::fs::write(dir.join(&meta.id).join("system.wav"), &body).unwrap();
        let server = MediaServer::start(store).unwrap();

        let (code, _, all) = get(&server.url(&meta.id, "call"), None);
        assert_eq!((code, all.len()), (200, 1000));
        let (code, cr, part) = get(&server.url(&meta.id, "call"), Some("bytes=100-199"));
        assert_eq!((code, cr.as_deref(), &part[..]), (206, Some("bytes 100-199/1000"), &body[100..200]));
        assert_eq!(get(&server.url(&meta.id, "call"), Some("bytes=2000-")).0, 416);

        // missing track, wrong token, other files, path tricks
        assert_eq!(get(&server.url(&meta.id, "mic"), None).0, 404);
        let bad_token = server.url(&meta.id, "call").replace(&server.token, "0000");
        assert_eq!(get(&bad_token, None).0, 404);
        let base = format!("{}/{}", server.origin(), server.token);
        assert_eq!(get(&format!("{base}/{}/meta.json", meta.id), None).0, 404);
        assert_eq!(get(&format!("{base}/..%2F..%2Fetc/call.wav"), None).0, 404);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
