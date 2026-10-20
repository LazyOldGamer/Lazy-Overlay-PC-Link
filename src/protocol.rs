//! The socket's frames. The server may send:
//!
//! ```text
//! {t:'hello', channel, login}                         connected
//! {t:'event', type:'pclink', data:{command, name, id}} command: start or stop
//! {t:'event', type:'pclink', data:{command:'pick', name, id, match}}
//!                                                      a random file from a [folder]
//! {t:'event', type:'pclink', data:{command:'toast', title, text, kind}}
//!                                                      a notification: kind action or error
//! {t:'ack', id, ok, error?}                            an answer to a report
//! {t:'pong'} {t:'error', error}
//! ```
//!
//! Anything else is ignored. This end sends only reports (`pclink.hello`,
//! `.watch`, `.ran`, `.picked`, `.file`) and pings.

use crate::config::{self, Config};
use crate::json::{self, Value, obj};

pub const PROTOCOL: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Start,
    Stop,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Start => "start",
            Kind::Stop => "stop",
        }
    }
}

/// What a frame asks.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Hello {
        login: String,
    },
    Run {
        kind: Kind,
        name: String,
        id: String,
    },
    /// A random file from a [folder], its name having `matching`.
    Pick {
        name: String,
        id: String,
        matching: String,
    },
    Toast {
        title: String,
        text: String,
        error: bool,
    },
    Ack {
        ok: bool,
        error: String,
    },
    Pong,
    Error(String),
    Ignored,
}

// A request's id, echoed back in its report.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Text from elsewhere, safe to show and log: no control, line-breaking,
/// bidi, zero-width or tag characters, at most `max`.
pub fn plain(s: &str, max: usize) -> String {
    let hidden = |c: char| {
        matches!(
            c,
            '\u{ad}'
                | '\u{61c}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e0000}'..='\u{e007f}'
        )
    };
    s.chars().filter(|&c| !c.is_control() && !hidden(c)).take(max).collect()
}

fn text(v: Option<&Value>, max: usize) -> String {
    plain(v.and_then(Value::as_str).unwrap_or(""), max)
}

// An entry's name and the request's id, when both are valid: only a name
// pclink.ini could hold, so nothing odd reaches the window or a terminal.
fn named(data: &Value) -> Option<(String, String)> {
    let name = data.get("name").and_then(Value::as_str).filter(|n| config::valid_name(n))?;
    let id = data.get("id").and_then(Value::as_str).filter(|i| valid_id(i))?;
    Some((name.to_string(), id.to_string()))
}

/// One frame from the server, as what this end may do with it.
pub fn decode(frame: &str) -> Incoming {
    let Ok(v) = json::parse(frame) else { return Incoming::Ignored };
    match v.get("t").and_then(Value::as_str) {
        Some("hello") => Incoming::Hello { login: text(v.get("login"), 40) },
        Some("ack") => Incoming::Ack { ok: v.get("ok").and_then(Value::as_bool).unwrap_or(false), error: text(v.get("error"), 200) },
        Some("pong") => Incoming::Pong,
        Some("error") => Incoming::Error(text(v.get("error"), 200)),
        Some("event") if v.get("type").and_then(Value::as_str) == Some("pclink") => {
            let Some(data) = v.get("data") else { return Incoming::Ignored };
            let run = |kind| named(data).map_or(Incoming::Ignored, |(name, id)| Incoming::Run { kind, name, id });
            match data.get("command").and_then(Value::as_str) {
                Some("start") => run(Kind::Start),
                Some("stop") => run(Kind::Stop),
                Some("pick") => named(data).map_or(Incoming::Ignored, |(name, id)| Incoming::Pick {
                    name,
                    id,
                    matching: text(data.get("match"), 40).trim().to_lowercase(),
                }),
                Some("toast") => {
                    let (title, text) = (text(data.get("title"), 80), text(data.get("text"), 300));
                    if title.trim().is_empty() && text.trim().is_empty() {
                        return Incoming::Ignored;
                    }
                    Incoming::Toast { title, text, error: data.get("kind").and_then(Value::as_str) == Some("error") }
                }
                _ => Incoming::Ignored,
            }
        }
        _ => Incoming::Ignored,
    }
}

fn req(id: u64, op: &str, args: Value) -> String {
    obj(vec![("t", "req".into()), ("id", id.into()), ("op", op.into()), ("args", args)]).to_json()
}

/// What this PC offers: the entries' names (never paths or arguments) and
/// whether each watched program runs.
pub fn hello(id: u64, cfg: &Config, running: &dyn Fn(&str) -> bool) -> String {
    let names = |it: Vec<&String>| Value::Arr(it.into_iter().map(|n| n.as_str().into()).collect());
    let watches =
        cfg.watches.iter().map(|w| obj(vec![("name", w.name.as_str().into()), ("running", running(&w.process).into())])).collect();
    req(
        id,
        "pclink.hello",
        obj(vec![
            ("version", env!("CARGO_PKG_VERSION").into()),
            ("protocol", PROTOCOL.into()),
            ("starts", names(cfg.starts.iter().map(|s| &s.name).collect())),
            ("stops", names(cfg.stops.iter().map(|s| &s.name).collect())),
            ("watches", Value::Arr(watches)),
            ("folders", names(cfg.folders.iter().map(|f| &f.name).collect())),
            ("filewatches", names(cfg.filewatches.iter().map(|f| &f.name).collect())),
            ("toasts", obj(vec![("actions", cfg.toasts.actions.into()), ("errors", cfg.toasts.errors.into())])),
        ]),
    )
}

/// A watched program started or stopped.
pub fn watch(id: u64, name: &str, running: bool) -> String {
    req(id, "pclink.watch", obj(vec![("name", name.into()), ("running", running.into())]))
}

/// What a start or stop the server asked for did.
pub fn ran(id: u64, run: &str, kind: Kind, name: &str, outcome: &Result<String, String>) -> String {
    let (ok, said) = match outcome {
        Ok(s) => (true, s.as_str()),
        Err(e) => (false, e.as_str()),
    };
    let said: String = said.chars().take(200).collect();
    req(
        id,
        "pclink.ran",
        obj(vec![("id", run.into()), ("kind", kind.word().into()), ("name", name.into()), ("ok", ok.into()), ("detail", said.into())]),
    )
}

/// The file picked from a [folder], by its full path, or why none was.
pub fn picked(id: u64, run: &str, name: &str, outcome: &Result<String, String>) -> String {
    let mut args = vec![("id", run.into()), ("name", name.into())];
    match outcome {
        Ok(file) => args.extend([("ok", true.into()), ("file", file.as_str().into())]),
        Err(e) => args.extend([("ok", false.into()), ("detail", e.chars().take(200).collect::<String>().into())]),
    }
    req(id, "pclink.picked", obj(args))
}

/// A watched file created, changed, deleted or renamed (with its old path).
pub fn file(id: u64, name: &str, event: &str, file: &str, old: Option<&str>) -> String {
    let mut args = vec![("name", name.into()), ("event", event.into()), ("file", file.into())];
    if let Some(old) = old {
        args.push(("old", old.into()));
    }
    req(id, "pclink.file", obj(args))
}

pub fn ping() -> String {
    obj(vec![("t", "ping".into())]).to_json()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Folder, Start, Watch};

    #[test]
    fn reads_the_frames_the_server_sends() {
        assert_eq!(decode(r#"{"t":"hello","channel":"1","login":"lazy","kind":"pclink"}"#), Incoming::Hello { login: "lazy".into() });
        assert_eq!(
            decode(r#"{"t":"event","type":"pclink","data":{"command":"stop","name":"Spotify","id":"0a1b-2c"}}"#),
            Incoming::Run { kind: Kind::Stop, name: "Spotify".into(), id: "0a1b-2c".into() }
        );
        assert_eq!(
            decode(r#"{"t":"ack","id":2,"ok":false,"error":"too many reports"}"#),
            Incoming::Ack { ok: false, error: "too many reports".into() }
        );
        assert_eq!(decode(r#"{"t":"pong"}"#), Incoming::Pong);
    }

    #[test]
    fn ignores_anything_else() {
        for f in [
            r#"{"t":"event","type":"pclink","data":{"command":"exec","name":"x","id":"1"}}"#,
            r#"{"t":"event","type":"pclink","data":{"command":"start","name":"x","id":"1","path":"C:\\evil.exe"}}"#,
            r#"{"t":"event","type":"pclink","data":{"command":"start","id":"1"}}"#,
            r#"{"t":"event","type":"pclink","data":{"command":"start","name":"x","id":"a b"}}"#,
            r#"{"t":"event","type":"ha","data":{"command":"start","name":"x","id":"1"}}"#,
            r#"{"t":"config","server":"wss://elsewhere"}"#,
            "{\"t\":\"event\",\"type\":\"pclink\",\"data\":{\"command\":\"start\",\"name\":\"x\\u001b[2J\",\"id\":\"1\"}}",
            "not json",
        ] {
            let got = decode(f);
            // A request with fields it does not know still names only an entry.
            if f.contains("evil") {
                assert_eq!(got, Incoming::Run { kind: Kind::Start, name: "x".into(), id: "1".into() });
            } else {
                assert_eq!(got, Incoming::Ignored, "{f}");
            }
        }
    }

    #[test]
    fn hello_names_the_entries_and_nothing_of_their_paths() {
        let cfg = Config {
            key: Some("secret-key-0123456789".into()),
            runs_per_minute: 20,
            starts: vec![Start {
                name: "App".into(),
                path: "/secret/path/app".into(),
                args: vec!["--token=x".into()],
                dir: None,
                once: false,
                hidden: false,
            }],
            stops: vec![],
            watches: vec![Watch { name: "Game".into(), process: "game.exe".into() }],
            folders: vec![Folder { name: "Clips".into(), path: "/secret/clips".into(), subfolders: false }],
            ..Config::default()
        };
        let text = hello(1, &cfg, &|p| p == "game.exe");
        let v = json::parse(&text).unwrap();
        assert_eq!(v.get("op").and_then(Value::as_str), Some("pclink.hello"));
        let args = v.get("args").unwrap();
        assert_eq!(args.get("starts"), Some(&Value::Arr(vec!["App".into()])));
        assert_eq!(args.get("watches").unwrap().to_json(), r#"[{"name":"Game","running":true}]"#);
        assert_eq!(args.get("folders"), Some(&Value::Arr(vec!["Clips".into()])));
        for secret in ["secret", "token", "game.exe"] {
            assert!(!text.contains(secret), "{secret} sent in {text}");
        }
    }

    #[test]
    fn reads_picks_and_toasts() {
        assert_eq!(
            decode(r#"{"t":"event","type":"pclink","data":{"command":"pick","name":"Clips","id":"p-1","match":" .MP4 "}}"#),
            Incoming::Pick { name: "Clips".into(), id: "p-1".into(), matching: ".mp4".into() }
        );
        assert_eq!(decode(r#"{"t":"event","type":"pclink","data":{"command":"pick","name":"../x","id":"p-1"}}"#), Incoming::Ignored);
        assert_eq!(
            decode(
                "{\"t\":\"event\",\"type\":\"pclink\",\"data\":{\"command\":\"toast\",\"title\":\"Hi\\u001b\",\"text\":\"there\",\"kind\":\"error\"}}"
            ),
            Incoming::Toast { title: "Hi".into(), text: "there".into(), error: true }
        );
        assert_eq!(decode(r#"{"t":"event","type":"pclink","data":{"command":"toast","title":" "}}"#), Incoming::Ignored);
        assert_eq!(plain("a\u{202e}b\u{200b}c\u{7}\u{2028}\u{e0041}d", 10), "abcd");
    }

    #[test]
    fn reports_picks_and_file_changes() {
        let v = json::parse(&picked(5, "p-1", "Clips", &Ok("/clips/a.mp4".into()))).unwrap();
        assert_eq!(v.get("args").unwrap().to_json(), r#"{"id":"p-1","name":"Clips","ok":true,"file":"/clips/a.mp4"}"#);
        let v = json::parse(&file(6, "Shots", "renamed", "/s/b.png", Some("/s/a.png"))).unwrap();
        assert_eq!(v.get("op").and_then(Value::as_str), Some("pclink.file"));
        assert_eq!(v.get("args").unwrap().to_json(), r#"{"name":"Shots","event":"renamed","file":"/s/b.png","old":"/s/a.png"}"#);
    }

    #[test]
    fn reports_what_a_run_did() {
        let v = json::parse(&ran(4, "r-1", Kind::Start, "App", &Err("could not start".into()))).unwrap();
        assert_eq!(v.get("args").unwrap().to_json(), r#"{"id":"r-1","kind":"start","name":"App","ok":false,"detail":"could not start"}"#);
    }
}
