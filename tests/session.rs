// A whole connection against a stand-in server: hello, start, refusal,
// watch, pick, notification, rename and a revoked key.

mod ws;

use pclink::app::{App, Event, REVOKED};
use pclink::config;
use pclink::json::{self, Value};
use pclink::link::{self, Down, Url};
use std::sync::{Arc, Mutex};

const KEY: &str = "test-key-0123456789abcdefghijklmnopqrstuvw";

#[cfg(windows)]
fn program() -> (String, String) {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    (format!(r"{root}\System32\cmd.exe"), "/c exit 0".into())
}
#[cfg(not(windows))]
fn program() -> (String, String) {
    ("/bin/sh".into(), "-c true".into())
}

// This test's own process, as a watch sees it.
fn me() -> String {
    std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().to_string()
}

fn args(frame: &str) -> (String, Value) {
    let v = json::parse(frame).unwrap();
    assert_eq!(v.get("t").and_then(Value::as_str), Some("req"), "{frame}");
    (v.get("op").and_then(Value::as_str).unwrap().to_string(), v.get("args").unwrap().clone())
}

#[test]
fn a_whole_session() {
    let (path, arg) = program();
    let (flag, rest) = arg.split_once(' ').unwrap();
    let dir = std::env::temp_dir().join(format!("pclink-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.mp4"), b"clip").unwrap();
    let text = format!(
        "[start Echo]\npath = {path}\narg = {flag}\narg = {rest}\n[stop Nothing]\nprocess = no-such-thing\n\
                        [watch Me]\nprocess = {}\n[folder Clips]\npath = {}\n[filewatch Drop]\npath = {}\n\
                        [toasts]\nerrors = no\n",
        me(),
        dir.display(),
        dir.display()
    );
    let (clip, moved) = (dir.join("a.mp4"), dir.join("b.mp4"));
    let cfg = config::parse(&text).unwrap();

    let server = ws::Server::start();
    let port = server.port;
    let handshake = Arc::new(Mutex::new(String::new()));
    let seen = handshake.clone();
    let peer = std::thread::spawn(move || {
        let mut c = server.accept();
        *seen.lock().unwrap() = c.request.clone();
        c.send(r#"{"t":"hello","channel":"1","login":"tester","kind":"pclink"}"#);

        let (op, hello) = args(&c.recv().unwrap());
        assert_eq!(op, "pclink.hello");
        assert_eq!(hello.get("starts").unwrap().to_json(), r#"["Echo"]"#);
        assert_eq!(hello.get("stops").unwrap().to_json(), r#"["Nothing"]"#);
        assert_eq!(hello.get("watches").unwrap().to_json(), r#"[{"name":"Me","running":true}]"#);

        c.send(r#"{"t":"event","type":"pclink","data":{"command":"start","name":"Echo","id":"run-1"}}"#);
        let (op, ran) = args(&c.recv().unwrap());
        assert_eq!(op, "pclink.ran");
        assert_eq!(ran.to_json(), r#"{"id":"run-1","kind":"start","name":"Echo","ok":true,"detail":"started"}"#);

        // Not in the file, a command it does not know, and a stop of what is not running.
        c.send(r#"{"t":"event","type":"pclink","data":{"command":"start","name":"Nope","id":"run-2"}}"#);
        let (_, ran) = args(&c.recv().unwrap());
        assert_eq!(ran.get("ok"), Some(&Value::Bool(false)));
        assert!(ran.get("detail").and_then(Value::as_str).unwrap().contains("Nope is not allowed to start"));
        c.send(r#"{"t":"event","type":"pclink","data":{"command":"exec","name":"Echo","id":"run-3"}}"#);
        c.send(r#"{"t":"event","type":"pclink","data":{"command":"stop","name":"Nothing","id":"run-4"}}"#);
        let (_, ran) = args(&c.recv().unwrap());
        assert_eq!(ran.get("id").and_then(Value::as_str), Some("run-4"), "exec was ignored");
        assert_eq!(ran.get("detail").and_then(Value::as_str), Some("not running"));

        c.send(r#"{"t":"event","type":"pclink","data":{"command":"pick","name":"Clips","id":"pick-1","match":".mp4"}}"#);
        let (op, picked) = args(&c.recv().unwrap());
        assert_eq!(op, "pclink.picked");
        assert_eq!(picked.get("file").and_then(Value::as_str), clip.to_str());
        c.send(r#"{"t":"event","type":"pclink","data":{"command":"toast","title":"Oops","text":"failed","kind":"error"}}"#);

        // After the first look at the folder (every 2 seconds), a rename.
        std::thread::sleep(std::time::Duration::from_millis(3000));
        std::fs::rename(&clip, &moved).unwrap();
        let (op, file) = args(&c.recv().unwrap());
        assert_eq!(op, "pclink.file");
        assert_eq!(file.get("event").and_then(Value::as_str), Some("renamed"));
        assert_eq!(file.get("file").and_then(Value::as_str), moved.to_str());
        assert_eq!(file.get("old").and_then(Value::as_str), clip.to_str());

        c.send(r#"{"t":"ack","id":2,"ok":true}"#);
        c.close(REVOKED, "unauthorized");
    });

    let url = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
    let mut l = link::connect(&url, KEY).unwrap();
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = said.clone();
    let mut app = App::new(
        std::env::temp_dir().join("pclink-test-none.ini"),
        cfg,
        Arc::new(move |e: Event| {
            if let Event::Log(m) = e {
                log.lock().unwrap().push(m)
            }
        }),
    );
    let down = app.session(&mut *l);
    drop(l);
    peer.join().unwrap();

    assert_eq!(down, Down::Closed(REVOKED, "unauthorized".into()));
    let request = handshake.lock().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /ws/pclink http/1.1"), "{request}");
    assert!(request.contains(&format!("authorization: bearer {}", KEY.to_ascii_lowercase())), "{request}");
    assert!(!request.contains("origin:"), "{request}");
    let said = said.lock().unwrap().join("\n");
    assert!(said.contains("Connected to tester's overlay"), "{said}");
    assert!(said.contains("Your overlay asked to start Echo: started"), "{said}");
    assert!(said.contains("asked to start Nope, but it was not done"), "{said}");
    assert!(said.contains("not shown (turned off here): Oops failed"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_upgrade_is_a_failure_naming_the_status() {
    let server = ws::Server::start();
    let port = server.port;
    let peer = std::thread::spawn(move || server.refuse(403));
    let url = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
    let got = link::connect(&url, KEY).err().expect("refused");
    peer.join().unwrap();
    assert!(matches!(&got, Down::Failed(why) if why.contains("403")), "{got:?}");
}

#[test]
fn nothing_listening_is_a_failure() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let url = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
    assert!(matches!(link::connect(&url, KEY).err(), Some(Down::Failed(_))));
}
