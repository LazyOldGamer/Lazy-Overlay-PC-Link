//! Lazy Overlay PC Link: the server may ask, by name, for what `pclink.ini`
//! lists (start or stop a program, pick a file, show a notification) and is
//! told when listed programs or files change. It never sends a path,
//! argument, command or setting.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod app;
pub mod autostart;
pub mod config;
pub mod files;
pub mod journal;
pub mod json;
pub mod link;
pub mod os;
pub mod procs;
pub mod protocol;
pub mod run;
pub mod service;
pub mod toast;
pub mod tray;
