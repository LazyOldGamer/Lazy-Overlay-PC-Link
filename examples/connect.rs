//! PC Link's console against a server of the test's choosing, for
//! scripts/tls-check.sh: the release program connects only to lazyoverlay.com.
//!
//!   connect <https://host:port> <pclink.ini>

use pclink::link::Url;
use pclink::service;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, settings] = args.as_slice() else {
        eprintln!("usage: connect <https://host:port> <pclink.ini>");
        std::process::exit(2)
    };
    let url = Url::parse(url).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    service::run_to(&url, settings.into(), Arc::new(service::print))
}
