//! Just enough JSON for the socket's frames: parsed with depth and size
//! limits, written with every string escaped.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

const MAX_DEPTH: usize = 16;
/// Keys an object may have: a repeated key is looked for among them, so a
/// frame of thousands cannot cost millions of comparisons.
const MAX_FIELDS: usize = 256;
/// Also the socket's frame cap.
pub const MAX_TEXT: usize = 64 * 1024;

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as JSON text.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self);
        out
    }
}

/// An object from pairs.
pub fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::Obj(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<u64> for Value {
    fn from(n: u64) -> Self {
        Value::Num(n as f64)
    }
}

fn write_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Num(n) if n.is_finite() => {
            if n.fract() == 0.0 && n.abs() < 9e15 {
                let _ = write!(out, "{}", *n as i64);
            } else {
                let _ = write!(out, "{n}");
            }
        }
        Value::Num(_) => out.push_str("null"),
        Value::Str(s) => write_str(out, s),
        Value::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Obj(fields) => {
            out.push('{');
            for (i, (k, item)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, k);
                out.push(':');
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parses one JSON text, or says why not.
pub fn parse(text: &str) -> Result<Value, String> {
    if text.len() > MAX_TEXT {
        return Err("too long".into());
    }
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("unexpected text at {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn eat(&mut self, word: &str) -> Result<(), String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(())
        } else {
            Err(format!("expected {word} at {}", self.i))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deep".into());
        }
        self.ws();
        match self.s.get(self.i) {
            None => Err("unexpected end".into()),
            Some(b'n') => self.eat("null").map(|_| Value::Null),
            Some(b't') => self.eat("true").map(|_| Value::Bool(true)),
            Some(b'f') => self.eat("false").map(|_| Value::Bool(false)),
            Some(b'"') => self.string().map(Value::Str),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Value::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Arr(items));
                        }
                        _ => return Err(format!("expected , or ] at {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut fields: Vec<(String, Value)> = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Value::Obj(fields));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(format!("expected a key at {}", self.i));
                    }
                    if fields.len() >= MAX_FIELDS {
                        return Err("too many keys".into());
                    }
                    let key = self.string()?;
                    self.ws();
                    self.eat(":")?;
                    let v = self.value(depth + 1)?;
                    // The first of a repeated key wins, as get() reads it.
                    if !fields.iter().any(|(k, _)| *k == key) {
                        fields.push((key, v));
                    }
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Obj(fields));
                        }
                        _ => return Err(format!("expected , or }} at {}", self.i)),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(format!("unexpected character at {}", self.i)),
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        while let Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') = self.s.get(self.i) {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "bad number")?;
        match text.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Value::Num(n)),
            _ => Err(format!("bad number at {start}")),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.i..self.i + 4).ok_or("unexpected end")?;
        // Four hex digits, never a sign.
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(format!("bad escape at {}", self.i));
        }
        let text = std::str::from_utf8(digits).map_err(|_| "bad escape")?;
        let n = u32::from_str_radix(text, 16).map_err(|_| format!("bad escape at {}", self.i))?;
        self.i += 4;
        Ok(n)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(&b) = self.s.get(self.i) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "not UTF-8")?);
            match self.s.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = *self.s.get(self.i).ok_or("unexpected end")?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                self.eat("\\u")?;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("bad surrogate pair".into());
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code).ok_or("bad escape")?);
                        }
                        _ => return Err(format!("bad escape at {}", self.i - 1)),
                    }
                }
                Some(_) => return Err(format!("control character in a string at {}", self.i)),
                None => return Err("unexpected end".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_server_sends() {
        let v = parse(r#"{"t":"event","type":"pclink","data":{"command":"start","name":"Spotify","id":"a-1"}}"#).unwrap();
        assert_eq!(v.get("t").and_then(Value::as_str), Some("event"));
        let data = v.get("data").unwrap();
        assert_eq!(data.get("name").and_then(Value::as_str), Some("Spotify"));
        let ack = parse(r#" { "t" : "ack", "id" : 3, "ok" : false, "error" : "too \"many\" \u00e9\ud83d\ude00" } "#).unwrap();
        assert_eq!(ack.get("id"), Some(&Value::Num(3.0)));
        assert_eq!(ack.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(ack.get("error").and_then(Value::as_str), Some("too \"many\" \u{e9}\u{1f600}"));
        assert_eq!(
            parse("[1,-2.5e2,null,true,[]]").unwrap(),
            Value::Arr(vec![Value::Num(1.0), Value::Num(-250.0), Value::Null, Value::Bool(true), Value::Arr(vec![])])
        );
    }

    #[test]
    fn refuses_what_is_not_json() {
        for bad in [
            "",
            "{",
            "{\"a\"}",
            "{\"a\":1,}",
            "[1 2]",
            "\"open",
            "nul",
            "01x",
            "{\"a\":\"\u{1}\"}",
            "\"\\ud800\"",
            "\"\\x\"",
            "1 2",
            "NaN",
            "\"\\u+04a\"",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
        let wide = format!("{{{}}}", (0..MAX_FIELDS + 1).map(|i| format!("\"k{i}\":0")).collect::<Vec<_>>().join(","));
        assert!(parse(&wide).is_err());
        let deep = "[".repeat(40) + &"]".repeat(40);
        assert!(parse(&deep).is_err());
        assert!(parse(&format!("\"{}\"", "a".repeat(MAX_TEXT))).is_err());
    }

    #[test]
    fn the_first_of_a_repeated_key_wins() {
        let v = parse(r#"{"name":"a","name":"b"}"#).unwrap();
        assert_eq!(v.get("name").and_then(Value::as_str), Some("a"));
    }

    #[test]
    fn writes_what_reads_back() {
        let v = obj(vec![
            ("t", "req".into()),
            ("id", 7u64.into()),
            ("ok", true.into()),
            ("s", "quote \" slash \\ line\n\u{1} \u{2028}".into()),
            ("n", Value::Num(1.5)),
            ("a", Value::Arr(vec![Value::Null])),
        ]);
        let text = v.to_json();
        assert_eq!(text, r#"{"t":"req","id":7,"ok":true,"s":"quote \" slash \\ line\n\u0001 \u2028","n":1.5,"a":[null]}"#);
        assert_eq!(parse(&text).unwrap(), v);
    }
}
