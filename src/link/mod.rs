//! The socket to the server: one outbound connection, TLS always (plain
//! only to this PC, for tests), the key in a header. No listening port.

use std::fmt;
use std::time::Duration;

#[cfg(unix)]
mod dl;
pub mod hash;
#[cfg(unix)]
mod openssl;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod winhttp;
#[cfg(unix)]
mod ws;

/// The socket's path on the server.
pub const PATH: &str = "/ws/pclink";

/// Where the socket goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
}

impl Url {
    /// `https://host[:port]`; `http://` only to this PC, for tests.
    pub fn parse(address: &str) -> Result<Url, String> {
        let a = address.trim().trim_end_matches('/');
        let (secure, rest) = match (a.strip_prefix("https://"), a.strip_prefix("http://")) {
            (Some(r), _) => (true, r),
            (_, Some(r)) => (false, r),
            _ => return Err("the server's address starts with https://".into()),
        };
        let (host, port) = rest.rsplit_once(':').map_or((rest, None), |(h, p)| (h, Some(p)));
        let port = match port {
            Some(p) => p.parse::<u16>().ok().filter(|p| *p > 0).ok_or("bad port")?,
            None if secure => 443,
            None => 80,
        };
        let host = host.to_ascii_lowercase();
        if host.is_empty() || host.len() > 253 || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            return Err("bad host in the server's address".into());
        }
        if !secure && !matches!(host.as_str(), "localhost" | "127.0.0.1") {
            return Err("the server's address must start with https://".into());
        }
        Ok(Url { secure, host, port })
    }

    pub fn ws(&self) -> String {
        format!("{}://{}:{}{PATH}", if self.secure { "wss" } else { "ws" }, self.host, self.port)
    }
}

/// Why the socket is down.
#[derive(Debug, Clone, PartialEq)]
pub enum Down {
    /// Closed by the server, with its code and reason.
    Closed(u16, String),
    /// Could not connect, or the connection broke.
    Failed(String),
}

impl fmt::Display for Down {
    // The server's words, shown plain.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let clean = |s: &str| crate::protocol::plain(s, 200);
        match self {
            Down::Closed(code, why) if why.is_empty() => write!(f, "closed by the server ({code})"),
            Down::Closed(code, why) => write!(f, "closed by the server ({code}: {})", clean(why)),
            Down::Failed(why) => write!(f, "{}", clean(why)),
        }
    }
}

/// An open socket.
pub trait Link {
    /// One text frame.
    fn send(&mut self, text: &str) -> Result<(), Down>;
    /// The next text frame, waiting at most `wait`; None when none came.
    fn recv(&mut self, wait: Duration) -> Result<Option<String>, Down>;
}

/// Opens the socket, sending `key` as a bearer token.
pub fn connect(url: &Url, key: &str) -> Result<Box<dyn Link>, Down> {
    #[cfg(windows)]
    return winhttp::connect(url, key).map(|l| Box::new(l) as Box<dyn Link>);
    #[cfg(unix)]
    return unix::connect(url, key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_servers_address() {
        assert_eq!(Url::parse("https://LazyOverlay.com/").unwrap().ws(), "wss://lazyoverlay.com:443/ws/pclink");
        assert_eq!(Url::parse("https://o.example:8443").unwrap().ws(), "wss://o.example:8443/ws/pclink");
        assert_eq!(Url::parse("http://127.0.0.1:8080").unwrap().ws(), "ws://127.0.0.1:8080/ws/pclink");
    }

    #[test]
    fn refuses_plain_text_off_this_pc_and_odd_addresses() {
        for bad in [
            "http://lazyoverlay.com",
            "http://192.168.1.2",
            "ftp://x",
            "lazyoverlay.com",
            "https://user:pw@lazyoverlay.com",
            "https://lazyoverlay.com:0",
            "https://lazyoverlay.com:99999",
            "https://bad host",
            "https://lazyoverlay.com/a",
            "https://",
            "https://[::1]",
            "https://a\r\nX-Evil: 1",
        ] {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
    }
}
