//! Checks that bytes are RFC 8785 canonical JSON: the JSON Canonicalization
//! Scheme's serialization of the value they hold, byte for byte.
//!
//! The check reads the bytes once and compares each token with its canonical
//! form instead of building the value. It keeps an explicit stack, so nesting
//! depth costs heap memory and never a deep call stack.

/// Whether `bytes` is the RFC 8785 canonical JSON of the value it holds.
pub fn is_canonical(bytes: &[u8]) -> bool {
    Checker { bytes, at: 0 }.check().is_some()
}

/// The JSON string of `text` under RFC 8785.
pub fn canonical_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

enum Frame {
    /// Inside an array.
    Array,
    /// Inside an object, holding the last key's UTF-16 units.
    Object { last_key: Option<Vec<u16>> },
}

struct Checker<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Checker<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.at += 1)
    }

    fn check(mut self) -> Option<()> {
        let mut stack: Vec<Frame> = Vec::new();
        // Whether the next token is a value, as opposed to the close of the
        // container on top of the stack or a separator.
        let mut want_value = true;
        loop {
            if want_value {
                match self.peek()? {
                    b'[' => {
                        self.at += 1;
                        if self.eat(b']').is_none() {
                            stack.push(Frame::Array);
                            continue;
                        }
                    }
                    b'{' => {
                        self.at += 1;
                        if self.eat(b'}').is_none() {
                            let key = self.string()?;
                            self.eat(b':')?;
                            stack.push(Frame::Object {
                                last_key: Some(key),
                            });
                            continue;
                        }
                    }
                    b'"' => {
                        self.string()?;
                    }
                    b't' => self.literal(b"true")?,
                    b'f' => self.literal(b"false")?,
                    b'n' => self.literal(b"null")?,
                    _ => self.number()?,
                }
                want_value = false;
            }
            // A value just ended: close containers or move to the next
            // element.
            match stack.last_mut() {
                None => return (self.at == self.bytes.len()).then_some(()),
                Some(Frame::Array) => match self.peek()? {
                    b',' => {
                        self.at += 1;
                        want_value = true;
                    }
                    b']' => {
                        self.at += 1;
                        stack.pop();
                    }
                    _ => return None,
                },
                Some(Frame::Object { last_key }) => match self.peek()? {
                    b',' => {
                        self.at += 1;
                        let key = self.string()?;
                        // Keys sort by UTF-16 code units, each above the last.
                        if last_key.as_ref().is_some_and(|last| *last >= key) {
                            return None;
                        }
                        *last_key = Some(key);
                        self.eat(b':')?;
                        want_value = true;
                    }
                    b'}' => {
                        self.at += 1;
                        stack.pop();
                    }
                    _ => return None,
                },
            }
        }
    }

    fn literal(&mut self, word: &[u8]) -> Option<()> {
        let end = self.at.checked_add(word.len())?;
        (self.bytes.get(self.at..end)? == word).then(|| self.at = end)
    }

    /// A string in canonical form, returning its UTF-16 code units.
    fn string(&mut self) -> Option<Vec<u16>> {
        self.eat(b'"')?;
        let mut units = Vec::new();
        loop {
            let byte = self.peek()?;
            match byte {
                b'"' => {
                    self.at += 1;
                    return Some(units);
                }
                b'\\' => {
                    self.at += 1;
                    let unit = match self.peek()? {
                        b'"' => 0x22,
                        b'\\' => 0x5c,
                        b'b' => 0x08,
                        b't' => 0x09,
                        b'n' => 0x0a,
                        b'f' => 0x0c,
                        b'r' => 0x0d,
                        b'u' => {
                            let hex = self.bytes.get(self.at + 1..self.at + 5)?;
                            if &hex[..2] != b"00" {
                                return None;
                            }
                            let digit = |d: u8| match d {
                                b'0'..=b'9' => Some(d - b'0'),
                                b'a'..=b'f' => Some(d - b'a' + 10),
                                _ => None,
                            };
                            let value = digit(hex[2])? << 4 | digit(hex[3])?;
                            // Only control characters without a short escape
                            // take the \u form.
                            if value >= 0x20 || matches!(value, 0x08 | 0x09 | 0x0a | 0x0c | 0x0d) {
                                return None;
                            }
                            self.at += 4;
                            u16::from(value)
                        }
                        _ => return None,
                    };
                    self.at += 1;
                    units.push(unit);
                }
                0x00..=0x1f => return None,
                _ => {
                    // One character: find its UTF-8 length from the lead byte.
                    let len = match byte {
                        0x00..=0x7f => 1,
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => return None,
                    };
                    let end = self.at.checked_add(len)?;
                    let c = std::str::from_utf8(self.bytes.get(self.at..end)?)
                        .ok()?
                        .chars()
                        .next()?;
                    let mut buffer = [0u16; 2];
                    units.extend_from_slice(c.encode_utf16(&mut buffer));
                    self.at = end;
                }
            }
        }
    }

    /// A number whose text is the canonical serialization of its value.
    fn number(&mut self) -> Option<()> {
        let start = self.at;
        self.eat(b'-');
        match self.peek()? {
            b'0' => self.at += 1,
            b'1'..=b'9' => self.digits(),
            _ => return None,
        }
        if self.eat(b'.').is_some() {
            if !self.peek()?.is_ascii_digit() {
                return None;
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !self.peek()?.is_ascii_digit() {
                return None;
            }
            self.digits();
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).ok()?;
        let value: f64 = text.parse().ok()?;
        (value.is_finite() && ecmascript_number(value) == text).then_some(())
    }

    fn digits(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
    }
}

/// ECMAScript's `Number.prototype.toString` of a finite `value`, which
/// RFC 8785 uses for numbers: the shortest digits that round-trip, placed
/// by the exponent.
pub fn ecmascript_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust's {:e} format always has an exponent");
    let exponent: i32 = exponent
        .parse()
        .expect("Rust's {:e} exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (whole, fraction) = digits.split_at(n as usize);
        format!("{whole}.{fraction}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e >= 0 { '+' } else { '-' };
        let (first, rest) = digits.split_at(1);
        if rest.is_empty() {
            format!("{first}e{sign}{}", e.abs())
        } else {
            format!("{first}.{rest}e{sign}{}", e.abs())
        }
    };
    if value < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_values_pass() {
        for json in [
            "null",
            "true",
            "false",
            "0",
            "-1",
            "1.5",
            "1e+21",
            "1e-7",
            "0.000001",
            "123456789012345680000",
            "\"\"",
            "\"a\\nb\\u001f\"",
            "\"é\"",
            "[]",
            "{}",
            "[1,[2,{\"a\":[]}]]",
            "{\"a\":1,\"b\":2}",
            "{\"\\u0001\":0,\"a\":0}",
        ] {
            assert!(is_canonical(json.as_bytes()), "{json}");
        }
    }

    #[test]
    fn other_forms_fail() {
        for json in [
            "",
            " 1",
            "1 ",
            "1.0",
            "-0",
            "01",
            "1E21",
            "1e21",
            "1e400",
            "\"\\u0041\"",
            "\"\\/\"",
            "\"\\u000A\"",
            "\"\\u000a\"",
            "\"\u{1}\"",
            "{\"b\":1,\"a\":2}",
            "{\"a\":1,\"a\":2}",
            "[1,]",
            "[1 ]",
            "{\"a\" :1}",
            "tru",
            "[",
            "[[",
            "\"\\ud800\"",
        ] {
            assert!(!is_canonical(json.as_bytes()), "{json:?}");
        }
    }

    #[test]
    fn deep_nesting_needs_no_call_stack() {
        let depth = 200_000;
        let json = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(is_canonical(json.as_bytes()));
    }

    #[test]
    fn keys_sort_by_utf16_code_units() {
        // U+FF61 sorts after U+1F600's high surrogate D83D in UTF-16, though
        // its code point is lower.
        assert!(is_canonical("{\"😀\":0,\"｡\":0}".as_bytes()));
        assert!(!is_canonical("{\"｡\":0,\"😀\":0}".as_bytes()));
    }

    #[test]
    fn strings_serialize_canonically() {
        assert_eq!(
            canonical_string("a\"\\\n\u{1}é"),
            "\"a\\\"\\\\\\n\\u0001é\""
        );
    }
}
