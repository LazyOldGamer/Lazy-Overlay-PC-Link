//! The socket on Linux: TCP, the system's OpenSSL, PC Link's WebSocket client.

use super::ws::{Stream, Ws};
use super::{Down, Link, Url};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::openssl as tls;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Plain TCP: only to this PC (Url refuses anything else).
struct Plain(TcpStream);

impl Stream for Plain {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        Write::write_all(&mut self.0, buf)
    }
    fn set_read_timeout(&mut self, wait: Duration) -> io::Result<()> {
        self.0.set_read_timeout(Some(wait.max(Duration::from_millis(1))))
    }
}

struct UnixLink<S: Stream>(Ws<S>);

impl<S: Stream> Link for UnixLink<S> {
    fn send(&mut self, text: &str) -> Result<(), Down> {
        self.0.send(text)
    }
    fn recv(&mut self, wait: Duration) -> Result<Option<String>, Down> {
        self.0.recv(wait)
    }
}

fn tcp(url: &Url) -> Result<TcpStream, Down> {
    let addrs = format!("{}:{}", url.host, url.port).to_socket_addrs().map_err(|e| Down::Failed(format!("{}: {e}", url.host)))?;
    let mut last = Down::Failed(format!("{} has no address", url.host));
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(s) => {
                s.set_write_timeout(Some(TIMEOUT)).map_err(|e| Down::Failed(e.to_string()))?;
                return Ok(s);
            }
            Err(e) => last = Down::Failed(format!("could not reach {}: {e}", url.host)),
        }
    }
    Err(last)
}

pub fn connect(url: &Url, key: &str) -> Result<Box<dyn Link>, Down> {
    let s = tcp(url)?;
    if url.secure {
        Ok(Box::new(UnixLink(Ws::connect(tls::connect(&url.host, s)?, url, key)?)))
    } else {
        Ok(Box::new(UnixLink(Ws::connect(Plain(s), url, key)?)))
    }
}
