// A WebSocket server just big enough for the tests, in plain std.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub struct Server {
    listener: TcpListener,
    pub port: u16,
}

pub struct Conn {
    stream: TcpStream,
    pub request: String,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        Server { listener, port }
    }

    fn request(&self) -> (TcpStream, String) {
        let (stream, _) = self.listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            request.push_str(&line);
        }
        (stream, request)
    }

    pub fn accept(self) -> Conn {
        let (mut stream, request) = self.request();
        let key = request
            .lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-key")).map(|(_, v)| v.trim().to_string()))
            .expect("a websocket key");
        let accept = pclink::link::hash::accept(&key);
        write!(
            stream,
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        )
        .unwrap();
        Conn { stream, request }
    }

    pub fn refuse(self, status: u16) {
        let (mut stream, _) = self.request();
        write!(stream, "HTTP/1.1 {status} No\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    }
}

impl Conn {
    fn frame(&mut self, opcode: u8, payload: &[u8]) {
        let mut out = vec![0x80 | opcode];
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
        self.stream.write_all(&out).unwrap();
    }

    pub fn send(&mut self, text: &str) {
        self.frame(1, text.as_bytes());
    }

    pub fn close(&mut self, code: u16, reason: &str) {
        let mut p = code.to_be_bytes().to_vec();
        p.extend_from_slice(reason.as_bytes());
        self.frame(8, &p);
        // The other end's close, or its going away.
        let _ = self.recv();
    }

    /// The next text frame; pings answered, None once closed.
    pub fn recv(&mut self) -> Option<String> {
        loop {
            let mut head = [0u8; 2];
            self.stream.read_exact(&mut head).ok()?;
            let opcode = head[0] & 0x0f;
            let mut len = (head[1] & 0x7f) as u64;
            if len == 126 {
                let mut b = [0u8; 2];
                self.stream.read_exact(&mut b).ok()?;
                len = u16::from_be_bytes(b) as u64;
            } else if len == 127 {
                let mut b = [0u8; 8];
                self.stream.read_exact(&mut b).ok()?;
                len = u64::from_be_bytes(b);
            }
            assert!(head[1] & 0x80 != 0, "a client's frames are masked");
            let mut mask = [0u8; 4];
            self.stream.read_exact(&mut mask).ok()?;
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload).ok()?;
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
            match opcode {
                1 => return Some(String::from_utf8(payload).unwrap()),
                8 => return None,
                9 => self.frame(10, &payload),
                _ => {}
            }
        }
    }
}
