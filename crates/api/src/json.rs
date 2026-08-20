//! A JSON writer sized to what this API emits: objects, arrays, strings,
//! numbers, booleans.

/// Builds a JSON document.
#[derive(Debug, Default)]
pub struct Json {
    out: String,
    needs_comma: bool,
}

impl Json {
    /// An empty document.
    pub fn new() -> Json {
        Json { out: String::new(), needs_comma: false }
    }

    /// Finished bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.out.into_bytes()
    }

    /// Finished text.
    pub fn into_string(self) -> String {
        self.out
    }

    /// Open an object.
    pub fn begin_object(&mut self) -> &mut Json {
        self.punctuate();
        self.out.push('{');
        self.needs_comma = false;
        self
    }

    /// Close an object.
    pub fn end_object(&mut self) -> &mut Json {
        self.out.push('}');
        self.needs_comma = true;
        self
    }

    /// Open an array.
    pub fn begin_array(&mut self) -> &mut Json {
        self.punctuate();
        self.out.push('[');
        self.needs_comma = false;
        self
    }

    /// Close an array.
    pub fn end_array(&mut self) -> &mut Json {
        self.out.push(']');
        self.needs_comma = true;
        self
    }

    /// Write an object key. The next value belongs to it.
    pub fn key(&mut self, name: &str) -> &mut Json {
        self.punctuate();
        escape_into(&mut self.out, name);
        self.out.push(':');
        self.needs_comma = false;
        self
    }

    /// Write a string value.
    pub fn string(&mut self, value: &str) -> &mut Json {
        self.punctuate();
        escape_into(&mut self.out, value);
        self.needs_comma = true;
        self
    }

    /// Write an integer value.
    pub fn number(&mut self, value: u64) -> &mut Json {
        self.punctuate();
        self.out.push_str(&value.to_string());
        self.needs_comma = true;
        self
    }

    /// Write a boolean value.
    pub fn bool(&mut self, value: bool) -> &mut Json {
        self.punctuate();
        self.out.push_str(if value { "true" } else { "false" });
        self.needs_comma = true;
        self
    }

    /// Write `null`.
    pub fn null(&mut self) -> &mut Json {
        self.punctuate();
        self.out.push_str("null");
        self.needs_comma = true;
        self
    }

    /// A string field.
    pub fn field(&mut self, name: &str, value: &str) -> &mut Json {
        self.key(name).string(value)
    }

    /// An integer field.
    pub fn field_number(&mut self, name: &str, value: u64) -> &mut Json {
        self.key(name).number(value)
    }

    /// A boolean field.
    pub fn field_bool(&mut self, name: &str, value: bool) -> &mut Json {
        self.key(name).bool(value)
    }

    fn punctuate(&mut self) {
        if self.needs_comma {
            self.out.push(',');
        }
    }
}

fn escape_into(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_nested_documents() {
        let mut j = Json::new();
        j.begin_object();
        j.field("name", "cairn");
        j.field_number("count", 2);
        j.key("items").begin_array();
        j.string("a");
        j.begin_object().field("b", "c").end_object();
        j.end_array();
        j.field_bool("ok", true);
        j.end_object();
        assert_eq!(
            j.into_string(),
            r#"{"name":"cairn","count":2,"items":["a",{"b":"c"}],"ok":true}"#
        );
    }

    #[test]
    fn escapes_control_and_quote() {
        let mut j = Json::new();
        j.string("a\"b\\c\nd\u{1}e");
        assert_eq!(j.into_string(), "\"a\\\"b\\\\c\\nd\\u0001e\"");
    }
}
