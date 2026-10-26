//! Reading HTTP answers with a size limit. `ureq`'s `into_json` reads as much
//! as the server sends, so every answer goes through here instead.

use std::io::Read;

use anyhow::{Result, ensure};
use serde::de::DeserializeOwned;

/// GitHub's release list and compare answers.
pub const GITHUB_MAX: u64 = 1024 * 1024;
/// Last.fm and ListenBrainz answers are small JSON objects.
pub const SCROBBLE_MAX: u64 = 256 * 1024;
/// LRCLIB search results can carry many full lyrics.
pub const LRCLIB_MAX: u64 = 2 * 1024 * 1024;

/// Parse a response body as JSON, refusing one over `max` bytes.
pub fn read_json<T: DeserializeOwned>(r: ureq::Response, max: u64) -> Result<T> {
    let mut body = Vec::new();
    r.into_reader().take(max + 1).read_to_end(&mut body)?;
    ensure!(body.len() as u64 <= max, "the answer was over {} KB", max / 1024);
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// Serves one answer of `len` bytes of JSON per connection, without a
    /// Content-Length, the way a misbehaving server could.
    fn serve(len: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut req = [0u8; 1024];
                let _ = std::io::Read::read(&mut s, &mut req);
                let body = format!("\"{}\"", "x".repeat(len.saturating_sub(2)));
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}");
            }
        });
        url
    }

    #[test]
    fn small_answers_are_read() {
        let url = serve(1000);
        let v: String = read_json(ureq::get(&url).call().unwrap(), 2048).unwrap();
        assert_eq!(v.len(), 998);
    }

    #[test]
    fn oversized_answers_are_refused() {
        let url = serve(3 * 1024 * 1024);
        let err = read_json::<String>(ureq::get(&url).call().unwrap(), SCROBBLE_MAX).unwrap_err();
        assert!(err.to_string().contains("over 256 KB"), "{err}");
    }
}
