//! A JSON reader for the daemon's own answers.
//!
//! The writer lives in `api`; this is the other half, and only this end needs
//! it. Numbers keep their source text rather than becoming floats: cairnd emits
//! integers, and a count round-tripped through `f64` is a count that can come
//! back wrong.
//!
//! The daemon is trusted and local, but the parser is bounded anyway
//! ([`MAX_DEPTH`]) because a bound that only exists when someone remembers to
//! add it is not a bound.

use std::fmt::Write as _;

/// Nesting the parser will follow before refusing. The API's deepest document
/// is four.
const MAX_DEPTH: usize = 32;

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number as written, so integers stay exact.
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// Object members in document order, which is the order they are shown in.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// A member of an object, or `None` for anything else.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(members) => Some(members),
            _ => None,
        }
    }

    /// One line of display text: strings unquoted, everything else as written.
    ///
    /// Not sanitised. Callers put this through `text::line` before it reaches a
    /// terminal.
    pub fn to_line(&self) -> String {
        match self {
            Value::Null => "-".to_owned(),
            Value::Bool(true) => "yes".to_owned(),
            Value::Bool(false) => "no".to_owned(),
            Value::Number(n) => n.clone(),
            Value::String(s) => s.clone(),
            Value::Array(items) => {
                let mut out = String::new();
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&item.to_line());
                }
                out
            }
            Value::Object(members) => {
                let mut out = String::new();
                for (i, (k, v)) in members.iter().enumerate() {
                    let sep = if i > 0 { ", " } else { "" };
                    let _ = write!(out, "{sep}{k}={}", v.to_line());
                }
                out
            }
        }
    }
}

/// Parse one JSON document. Trailing text is an error.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser {
        rest: text,
        depth: 0,
    };
    let value = p.value()?;
    p.space();
    if !p.rest.is_empty() {
        return Err("trailing text after the document".to_owned());
    }
    Ok(value)
}

struct Parser<'a> {
    rest: &'a str,
    depth: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        self.rest = self.rest.trim_start_matches([' ', '\t', '\n', '\r']);
    }

    fn eat(&mut self, c: char) -> bool {
        match self.rest.strip_prefix(c) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("nested too deeply".to_owned());
        }
        self.space();
        let value = match self.rest.as_bytes().first() {
            Some(b'{') => self.object()?,
            Some(b'[') => self.array()?,
            Some(b'"') => Value::String(self.string()?),
            Some(b't') if self.rest.starts_with("true") => {
                self.rest = &self.rest[4..];
                Value::Bool(true)
            }
            Some(b'f') if self.rest.starts_with("false") => {
                self.rest = &self.rest[5..];
                Value::Bool(false)
            }
            Some(b'n') if self.rest.starts_with("null") => {
                self.rest = &self.rest[4..];
                Value::Null
            }
            Some(b'-' | b'0'..=b'9') => self.number()?,
            _ => return Err("expected a value".to_owned()),
        };
        self.depth -= 1;
        Ok(value)
    }

    fn object(&mut self) -> Result<Value, String> {
        self.rest = &self.rest[1..];
        let mut members = Vec::new();
        self.space();
        if self.eat('}') {
            return Ok(Value::Object(members));
        }
        loop {
            self.space();
            let key = self.string()?;
            self.space();
            if !self.eat(':') {
                return Err("expected ':' after an object key".to_owned());
            }
            members.push((key, self.value()?));
            self.space();
            if self.eat(',') {
                continue;
            }
            if self.eat('}') {
                return Ok(Value::Object(members));
            }
            return Err("expected ',' or '}' in an object".to_owned());
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.rest = &self.rest[1..];
        let mut items = Vec::new();
        self.space();
        if self.eat(']') {
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.space();
            if self.eat(',') {
                continue;
            }
            if self.eat(']') {
                return Ok(Value::Array(items));
            }
            return Err("expected ',' or ']' in an array".to_owned());
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if !self.eat('"') {
            return Err("expected a string".to_owned());
        }
        let mut out = String::new();
        let mut chars = self.rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.rest = &self.rest[i + 1..];
                    return Ok(out);
                }
                '\\' => {
                    let (_, esc) = chars.next().ok_or("string ended inside an escape")?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let start = chars.next().ok_or("truncated \\u escape")?.0;
                            for _ in 0..3 {
                                chars.next().ok_or("truncated \\u escape")?;
                            }
                            let hex = self
                                .rest
                                .get(start..start + 4)
                                .ok_or("truncated \\u escape")?;
                            let code =
                                u32::from_str_radix(hex, 16).map_err(|_| "bad \\u escape")?;
                            // Lone surrogates cannot be represented; the writer
                            // never emits one, and a replacement character says
                            // more than a parse failure would.
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        other => return Err(format!("unknown escape \\{other}")),
                    }
                }
                c => out.push(c),
            }
        }
        Err("string was not closed".to_owned())
    }

    fn number(&mut self) -> Result<Value, String> {
        let end = self
            .rest
            .find(|c: char| !matches!(c, '-' | '+' | '.' | 'e' | 'E' | '0'..='9'))
            .unwrap_or(self.rest.len());
        let (text, rest) = self.rest.split_at(end);
        if text.parse::<f64>().is_err() {
            return Err(format!("bad number {text:?}"));
        }
        self.rest = rest;
        Ok(Value::Number(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_the_api_emits() {
        let v = parse(r#"{"archives":[{"uuid":"a","entry_count":9086,"suggest":true}]}"#).unwrap();
        let first = &v.get("archives").unwrap().as_array().unwrap()[0];
        assert_eq!(first.get("uuid").unwrap().as_str(), Some("a"));
        assert_eq!(first.get("entry_count").unwrap().as_u64(), Some(9086));
        assert_eq!(first.get("suggest").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn counts_stay_exact() {
        // Beyond f64's integer range, which is why numbers keep their text.
        let v = parse(r#"{"n":9007199254740993}"#).unwrap();
        assert_eq!(v.get("n").unwrap().as_u64(), Some(9_007_199_254_740_993));
    }

    #[test]
    fn member_order_is_document_order() {
        let v = parse(r#"{"b":1,"a":2,"c":3}"#).unwrap();
        let keys: Vec<&str> = v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["b", "a", "c"]);
    }

    #[test]
    fn unescapes_strings() {
        let v = parse(r#"{"s":"a\"b\\c\nd\u001be\u00e9"}"#).unwrap();
        assert_eq!(
            v.get("s").unwrap().as_str(),
            Some("a\"b\\c\nd\u{1b}e\u{e9}")
        );
    }

    #[test]
    fn refuses_malformed_documents() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\"}",
            "{\"a\":}",
            "tru",
            "{} junk",
            "\"unclosed",
            "{\"a\":\"\\q\"}",
            "[1 2]",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!("{}{}", "[".repeat(64), "]".repeat(64));
        assert!(parse(&deep).is_err());
        let fine = format!("{}{}", "[".repeat(8), "]".repeat(8));
        assert!(parse(&fine).is_ok());
    }

    #[test]
    fn empty_containers_parse() {
        assert_eq!(parse("{}").unwrap(), Value::Object(Vec::new()));
        assert_eq!(parse("[]").unwrap(), Value::Array(Vec::new()));
        assert_eq!(
            parse(r#"{"archives":[]}"#)
                .unwrap()
                .get("archives")
                .unwrap()
                .as_array(),
            Some(&[][..])
        );
    }
}
