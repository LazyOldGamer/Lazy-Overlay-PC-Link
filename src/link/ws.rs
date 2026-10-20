//! A WebSocket client (RFC 6455) just big enough for PC Link: one handshake,
//! masked text frames out; text, ping and close in. Frames are buffered, so a
//! wait that ends partway through one loses nothing.

use super::hash;
use super::{Down, PATH, Url};
use crate::json::MAX_TEXT;
use std::io::{self, ErrorKind, Read};
use std::time::{Duration, Instant};

/// Plain TCP, or TLS on it.
pub trait Stream {
    /// 0 when closed; WouldBlock or TimedOut when nothing came in time.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    fn set_read_timeout(&mut self, wait: Duration) -> io::Result<()>;
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HEADERS: usize = 16 * 1024;

pub struct Ws<S: Stream> {
    s: S,
    buf: Vec<u8>,
    msg: Vec<u8>,
    // Whether the message being continued is text (binary is dropped).
    text: bool,
}

fn failed(e: impl std::fmt::Display) -> Down {
    Down::Failed(e.to_string())
}

fn quiet(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Random bytes for the handshake key and frame masks; none is no connection.
fn random<const N: usize>() -> Result<[u8; N], Down> {
    let mut out = [0u8; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut out))
        .map_err(|e| failed(format!("could not read /dev/urandom: {e}")))?;
    Ok(out)
}

impl<S: Stream> Ws<S> {
    /// The handshake, with the key as a bearer token.
    pub fn connect(mut s: S, url: &Url, key: &str) -> Result<Ws<S>, Down> {
        let nonce = hash::base64(&random::<16>()?);
        let host = if url.port == if url.secure { 443 } else { 80 } { url.host.clone() } else { format!("{}:{}", url.host, url.port) };
        let request = format!(
            "GET {PATH} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {nonce}\r\n\
             Sec-WebSocket-Version: 13\r\nAuthorization: Bearer {key}\r\nUser-Agent: LazyOverlayPCLink/{}\r\n\r\n",
            env!("CARGO_PKG_VERSION")
        );
        s.write_all(request.as_bytes()).map_err(failed)?;

        let until = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut buf = Vec::new();
        let end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            if buf.len() > MAX_HEADERS {
                return Err(failed("the server's answer is too long"));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(failed("the server did not answer in time"));
            }
            s.set_read_timeout(left).map_err(failed)?;
            let mut chunk = [0u8; 2048];
            match s.read(&mut chunk) {
                Ok(0) => return Err(failed("the server closed the connection")),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if quiet(&e) => {}
                Err(e) => return Err(failed(e)),
            }
        };
        let head = String::from_utf8_lossy(&buf[..end]).to_string();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
        if status != "101" {
            return Err(failed(format!("the server answered {status}")));
        }
        let answer = lines
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept"))
            .map(|(_, v)| v.trim().to_string());
        if answer.as_deref() != Some(hash::accept(&nonce).as_str()) {
            return Err(failed("the server's answer was not a WebSocket's"));
        }
        Ok(Ws { s, buf: buf[end + 4..].to_vec(), msg: Vec::new(), text: false })
    }

    fn frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), Down> {
        let mut out = Vec::with_capacity(payload.len() + 14);
        out.push(0x80 | opcode);
        match payload.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n < 65536 => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = random::<4>()?;
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.s.write_all(&out).map_err(failed)
    }

    pub fn send(&mut self, text: &str) -> Result<(), Down> {
        self.frame(1, text.as_bytes())
    }

    // One whole frame from the buffer, if it holds one: (fin, opcode, payload).
    fn take(&mut self) -> Result<Option<(bool, u8, Vec<u8>)>, Down> {
        let b = &self.buf;
        if b.len() < 2 {
            return Ok(None);
        }
        if b[1] & 0x80 != 0 {
            return Err(failed("the server masked a frame"));
        }
        let (len, at) = match b[1] & 0x7f {
            126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4),
            127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().expect("8 bytes")), 10),
            126 | 127 => return Ok(None),
            n => (n as u64, 2),
        };
        if len > MAX_TEXT as u64 {
            return Err(failed("the server sent too much at once"));
        }
        let end = at + len as usize;
        if b.len() < end {
            return Ok(None);
        }
        let frame = (b[0] & 0x80 != 0, b[0] & 0x0f, b[at..end].to_vec());
        self.buf.drain(..end);
        Ok(Some(frame))
    }

    /// The next text frame, waiting at most `wait`.
    pub fn recv(&mut self, wait: Duration) -> Result<Option<String>, Down> {
        let until = Instant::now() + wait;
        loop {
            while let Some((fin, opcode, payload)) = self.take()? {
                match opcode {
                    0..=2 => {
                        if opcode != 0 {
                            self.msg.clear();
                            self.text = opcode == 1;
                        }
                        // Binary is dropped, and so is a continuation with nothing to continue.
                        if !self.text {
                            continue;
                        }
                        self.msg.extend_from_slice(&payload);
                        if self.msg.len() > MAX_TEXT {
                            return Err(failed("the server sent too much at once"));
                        }
                        if fin {
                            self.text = false;
                            let text = String::from_utf8(std::mem::take(&mut self.msg))
                                .map_err(|_| failed("the server sent text that is not UTF-8"))?;
                            return Ok(Some(text));
                        }
                    }
                    8 => {
                        let code = if payload.len() >= 2 { u16::from_be_bytes([payload[0], payload[1]]) } else { 1005 };
                        let why = String::from_utf8_lossy(payload.get(2..).unwrap_or(&[])).to_string();
                        let _ = self.frame(8, &payload[..payload.len().min(2)]);
                        return Err(Down::Closed(code, why));
                    }
                    9 => self.frame(10, &payload)?,
                    _ => {}
                }
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.s.set_read_timeout(left).map_err(failed)?;
            let mut chunk = [0u8; 8192];
            match self.s.read(&mut chunk) {
                Ok(0) => return Err(failed("the server closed the connection")),
                Ok(n) => {
                    if self.buf.len() + n > 2 * MAX_TEXT {
                        return Err(failed("the server sent too much at once"));
                    }
                    self.buf.extend_from_slice(&chunk[..n]);
                }
                Err(e) if quiet(&e) => return Ok(None),
                Err(e) => return Err(failed(e)),
            }
        }
    }
}
