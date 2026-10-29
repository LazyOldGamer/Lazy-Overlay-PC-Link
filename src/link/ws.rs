//! A WebSocket client (RFC 6455) just big enough for PC Link: one handshake,
//! masked text frames out; text, ping and close in. Frames are buffered, so a
//! wait that ends partway through one loses nothing.

use super::hash;
use super::{Down, PATH, Url};
use crate::json::MAX_TEXT;
use std::io::{self, ErrorKind, Read};
use std::sync::OnceLock;
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
    // Whether a fragmented message, text or binary, is still being continued.
    open: bool,
}

fn failed(e: impl std::fmt::Display) -> Down {
    Down::Failed(e.to_string())
}

fn quiet(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Random bytes for the handshake key and frame masks; none is no connection.
/// /dev/urandom is opened once and kept: every frame sent takes a mask.
fn random<const N: usize>() -> Result<[u8; N], Down> {
    static URANDOM: OnceLock<std::fs::File> = OnceLock::new();
    let said = |e: io::Error| failed(format!("could not read /dev/urandom: {e}"));
    let file = match URANDOM.get() {
        Some(f) => f,
        None => {
            let f = std::fs::File::open("/dev/urandom").map_err(said)?;
            URANDOM.get_or_init(|| f)
        }
    };
    let mut out = [0u8; N];
    (&*file).read_exact(&mut out).map_err(said)?;
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
        Ok(Ws { s, buf: buf[end + 4..].to_vec(), msg: Vec::new(), text: false, open: false })
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

    /// The connection failed as RFC 6455 7.1.7 has it: a close saying 1002
    /// (protocol error), then down.
    fn broken(&mut self, why: &str) -> Down {
        let _ = self.frame(8, &1002u16.to_be_bytes());
        failed(format!("the server broke the WebSocket protocol: {why}"))
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
        // No extension was agreed, so no RSV bit may be set (5.2); a control
        // frame is whole and at most 125 bytes (5.5).
        let (fin, opcode) = (b[0] & 0x80 != 0, b[0] & 0x0f);
        if b[0] & 0x70 != 0 {
            return Err(self.broken("a reserved bit is set"));
        }
        if opcode >= 8 && (!fin || b[1] & 0x7f > 125) {
            return Err(self.broken("a control frame is fragmented or too long"));
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
        let frame = (fin, opcode, b[at..end].to_vec());
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
                        // A continuation continues a message, and a new message
                        // waits for the last one's end (5.4).
                        if opcode == 0 && !self.open {
                            return Err(self.broken("a continuation with nothing to continue"));
                        }
                        if opcode != 0 && self.open {
                            return Err(self.broken("a new message before the last one ended"));
                        }
                        self.open = !fin;
                        if opcode != 0 {
                            self.msg.clear();
                            self.text = opcode == 1;
                        }
                        // Binary is dropped.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // A server that has sent `input` already, read back a few bytes at a time,
    // and keeps what the client writes.
    struct Fake {
        input: VecDeque<u8>,
        sent: Vec<u8>,
    }

    impl Stream for Fake {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.input.is_empty() {
                return Err(io::Error::from(ErrorKind::WouldBlock));
            }
            let n = buf.len().min(self.input.len()).min(7);
            for b in buf.iter_mut().take(n) {
                *b = self.input.pop_front().expect("counted");
            }
            Ok(n)
        }
        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            self.sent.extend_from_slice(buf);
            Ok(())
        }
        fn set_read_timeout(&mut self, _: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    fn ws(input: &[u8]) -> Ws<Fake> {
        Ws {
            s: Fake { input: input.iter().copied().collect(), sent: Vec::new() },
            buf: Vec::new(),
            msg: Vec::new(),
            text: false,
            open: false,
        }
    }

    // A server's frame: unmasked, as RFC 6455 has them.
    fn frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            n if n < 126 => out.push(n as u8),
            n if n < 65536 => {
                out.push(126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        out.extend_from_slice(payload);
        out
    }

    const WAIT: Duration = Duration::from_millis(10);

    fn failed_with(r: Result<Option<String>, Down>, words: &str) {
        match r {
            Err(Down::Failed(why)) => assert!(why.contains(words), "{why}"),
            other => panic!("wanted a failure saying {words:?}, got {other:?}"),
        }
    }

    #[test]
    fn a_text_frame_split_across_reads_is_one_message() {
        let mut w = ws(&frame(true, 1, "hello there, PC Link".as_bytes()));
        assert_eq!(w.recv(WAIT), Ok(Some("hello there, PC Link".into())));
        assert_eq!(w.recv(WAIT), Ok(None), "and nothing after it");
    }

    #[test]
    fn half_a_frame_waits_for_the_rest() {
        let whole = frame(true, 1, b"later");
        let mut w = ws(&whole[..3]);
        assert_eq!(w.recv(WAIT), Ok(None));
        w.s.input.extend(&whole[3..]);
        assert_eq!(w.recv(WAIT), Ok(Some("later".into())));
    }

    #[test]
    fn fragments_are_joined_and_binary_dropped() {
        let mut input = frame(true, 2, b"\x00\x01binary");
        input.extend(frame(false, 2, b"more "));
        input.extend(frame(true, 0, b"binary"));
        input.extend(frame(false, 1, b"one "));
        input.extend(frame(true, 9, b"a ping between fragments"));
        input.extend(frame(false, 0, b"two "));
        input.extend(frame(true, 0, b"three"));
        let mut w = ws(&input);
        assert_eq!(w.recv(WAIT), Ok(Some("one two three".into())));
    }

    // Each is a protocol error (RFC 6455 5.2, 5.4, 5.5): a close saying 1002, then down.
    #[test]
    fn a_broken_frame_fails_the_connection_with_1002() {
        let reserved = {
            let mut f = frame(true, 1, b"x");
            f[0] |= 0x40;
            f
        };
        let mut new_mid_message = frame(false, 1, b"one ");
        new_mid_message.extend(frame(true, 1, b"two"));
        for (bytes, words) in [
            (frame(true, 0, b"nothing to continue"), "nothing to continue"),
            (new_mid_message, "before the last one ended"),
            (reserved, "reserved bit"),
            (frame(false, 9, b"half a ping"), "control frame"),
            (frame(true, 9, &[b'p'; 126]), "control frame"),
        ] {
            let mut w = ws(&bytes);
            failed_with(w.recv(WAIT), words);
            assert_eq!(&w.s.sent[..2], &[0x88, 0x82], "a masked close of two bytes");
            let payload: Vec<u8> = w.s.sent[6..8].iter().enumerate().map(|(i, b)| b ^ w.s.sent[2 + i]).collect();
            assert_eq!(payload, 1002u16.to_be_bytes(), "{words}");
        }
    }

    #[test]
    fn a_ping_is_answered_with_its_payload_and_an_unknown_opcode_passed_over() {
        let mut input = frame(true, 9, b"are you there");
        input.extend(frame(true, 0xb, b"reserved"));
        input.extend(frame(true, 1, b"after"));
        let mut w = ws(&input);
        assert_eq!(w.recv(WAIT), Ok(Some("after".into())));
        let pong = &w.s.sent;
        assert_eq!(pong[0], 0x8a, "a final pong");
        assert_eq!(pong[1], 0x80 | 13, "masked, the ping's length");
        let mask = &pong[2..6];
        let payload: Vec<u8> = pong[6..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        assert_eq!(payload, b"are you there");
    }

    #[test]
    fn a_close_is_echoed_and_says_why() {
        let mut payload = 4001u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"replaced");
        let mut w = ws(&frame(true, 8, &payload));
        assert_eq!(w.recv(WAIT), Err(Down::Closed(4001, "replaced".into())));
        assert_eq!(w.s.sent[0], 0x88, "the close sent back");

        let mut bare = ws(&frame(true, 8, b""));
        assert_eq!(bare.recv(WAIT), Err(Down::Closed(1005, String::new())), "no code is 1005");
    }

    #[test]
    fn a_masked_frame_is_refused() {
        let mut bytes = frame(true, 1, b"hi");
        bytes[1] |= 0x80;
        bytes.splice(2..2, [1, 2, 3, 4]);
        failed_with(ws(&bytes).recv(WAIT), "masked");
    }

    #[test]
    fn too_much_is_refused_before_it_arrives() {
        // A header claiming more than a message may be, with none of it sent.
        let mut huge = vec![0x81, 127];
        huge.extend_from_slice(&u64::MAX.to_be_bytes());
        failed_with(ws(&huge).recv(WAIT), "too much");

        // Fragments, each small, adding up to too much.
        let piece = vec![b'a'; MAX_TEXT / 2 + 1];
        let mut input = frame(false, 1, &piece);
        input.extend(frame(true, 0, &piece));
        failed_with(ws(&input).recv(WAIT), "too much");
    }

    #[test]
    fn text_that_is_not_utf8_is_refused() {
        failed_with(ws(&frame(true, 1, &[0xff, 0xfe, b'x'])).recv(WAIT), "UTF-8");
    }

    #[test]
    fn a_closed_socket_says_so() {
        struct Shut;
        impl Stream for Shut {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Ok(0)
            }
            fn write_all(&mut self, _: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn set_read_timeout(&mut self, _: Duration) -> io::Result<()> {
                Ok(())
            }
        }
        let mut w = Ws { s: Shut, buf: Vec::new(), msg: Vec::new(), text: false, open: false };
        failed_with(w.recv(WAIT), "closed");
    }
}
