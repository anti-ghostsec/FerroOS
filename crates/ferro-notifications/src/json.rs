//! Just enough JSON to read `dunstctl history`: objects, arrays, strings
//! (with their escapes), numbers, true, false and null.

#[derive(Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Option<Value> {
    let mut p = Parser { s: text.as_bytes(), at: 0 };
    let v = p.value()?;
    p.space();
    (p.at == p.s.len()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.s.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, b: u8) -> Option<()> {
        self.space();
        (self.s.get(self.at) == Some(&b)).then(|| self.at += 1)
    }

    fn value(&mut self) -> Option<Value> {
        self.space();
        match *self.s.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Value::Object(fields));
                }
                loop {
                    self.space();
                    let k = self.string()?;
                    self.eat(b':')?;
                    fields.push((k, self.value()?));
                    if self.eat(b',').is_none() {
                        self.eat(b'}')?;
                        return Some(Value::Object(fields));
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Value::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    if self.eat(b',').is_none() {
                        self.eat(b']')?;
                        return Some(Value::Array(items));
                    }
                }
            }
            b'"' => self.string().map(Value::Str),
            b't' => self.word("true", Value::Bool(true)),
            b'f' => self.word("false", Value::Bool(false)),
            b'n' => self.word("null", Value::Null),
            _ => {
                let start = self.at;
                while self.s.get(self.at).is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c)) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.s[start..self.at]).ok()?.parse().ok().map(Value::Number)
            }
        }
    }

    fn word(&mut self, w: &str, v: Value) -> Option<Value> {
        self.s[self.at..].starts_with(w.as_bytes()).then(|| {
            self.at += w.len();
            v
        })
    }

    fn string(&mut self) -> Option<String> {
        if self.s.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut out = Vec::new();
        loop {
            match *self.s.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return String::from_utf8(out).ok();
                }
                b'\\' => {
                    self.at += 1;
                    let e = *self.s.get(self.at)?;
                    self.at += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' | b'f' => {}
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.at..self.at + 4)?).ok()?;
                            self.at += 4;
                            let mut code = u32::from_str_radix(hex, 16).ok()?;
                            // A pair for what's beyond the first 65536.
                            if (0xd800..0xdc00).contains(&code) && self.s.get(self.at..self.at + 2) == Some(b"\\u") {
                                let low = u32::from_str_radix(std::str::from_utf8(self.s.get(self.at + 2..self.at + 6)?).ok()?, 16).ok()?;
                                self.at += 6;
                                code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                            }
                            let c = char::from_u32(code).unwrap_or('\u{fffd}');
                            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                c => {
                    out.push(c);
                    self.at += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_should() {
        let v = parse(r#" {"a": [1, -2.5e1, true, null], "b": "x\"é\n😀"} "#).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Array(vec![Value::Number(1.0), Value::Number(-25.0), Value::Bool(true), Value::Null])));
        assert_eq!(v.get("b").and_then(Value::as_str), Some("x\"é\n😀"));
        assert_eq!(parse("[1,"), None);
        assert_eq!(parse("{} x"), None);
    }
}
