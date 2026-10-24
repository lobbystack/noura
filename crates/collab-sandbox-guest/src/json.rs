//! Checks that bytes are RFC 8785 canonical JSON: the JSON Canonicalization
//! Scheme's serialization of the value they hold, byte for byte.
//!
//! The check reads the bytes once and compares each token with its canonical
//! form instead of building the value. It keeps an explicit stack, so nesting
//! depth costs heap memory and never a deep call stack. Strings outside
//! object keys aren't collected, and a number with at most 15 significant
//! digits and an exponent of at most 300 is checked by its digits alone, so
//! the verdict's fuel per byte stays low for any property an honest note
//! holds (`crates/collab-sandbox/tests/budget.rs` measures it).

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
                            let key = self.key()?;
                            self.eat(b':')?;
                            stack.push(Frame::Object {
                                last_key: Some(key),
                            });
                            continue;
                        }
                    }
                    b'"' => self.string(None)?,
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
                        let key = self.key()?;
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

    /// An object key in canonical form, as UTF-16 code units.
    fn key(&mut self) -> Option<Vec<u16>> {
        let mut units = Vec::new();
        self.string(Some(&mut units))?;
        Some(units)
    }

    /// A string in canonical form, pushing its UTF-16 code units to `units`
    /// when given.
    fn string(&mut self, mut units: Option<&mut Vec<u16>>) -> Option<()> {
        self.eat(b'"')?;
        loop {
            let byte = self.peek()?;
            let unit = match byte {
                b'"' => {
                    self.at += 1;
                    return Some(());
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
                    unit
                }
                0x00..=0x1f => return None,
                0x20..=0x7f => {
                    self.at += 1;
                    u16::from(byte)
                }
                _ => {
                    // One character: find its UTF-8 length from the lead byte.
                    let len = match byte {
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
                    self.at = end;
                    if let Some(units) = units.as_deref_mut() {
                        let mut buffer = [0u16; 2];
                        units.extend_from_slice(c.encode_utf16(&mut buffer));
                    }
                    continue;
                }
            };
            if let Some(units) = units.as_deref_mut() {
                units.push(unit);
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
        let text = &self.bytes[start..self.at];
        match short_number_is_canonical(text) {
            Some(canonical) => canonical.then_some(()),
            None => {
                let text = std::str::from_utf8(text).ok()?;
                let value: f64 = text.parse().ok()?;
                (value.is_finite() && ecmascript_number(value) == text).then_some(())
            }
        }
    }

    fn digits(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
    }
}

/// The most significant digits a number may have for
/// [`short_number_is_canonical`] to judge it: any decimal of 15 digits or
/// fewer survives a round trip through a double, so its shortest digits are
/// its own.
const SHORT_DIGITS: usize = 15;
/// The largest exponent magnitude [`short_number_is_canonical`] judges,
/// which keeps it among normal doubles, far from overflow and from the
/// subnormals that hold fewer than 15 digits.
const SHORT_EXPONENT: i32 = 300;

/// Whether `text`, a JSON number token, is the canonical form of its value,
/// judged from its digits alone, or `None` when it has more than
/// [`SHORT_DIGITS`] significant digits or an exponent past
/// [`SHORT_EXPONENT`], which the full check judges.
///
/// The canonical form of a nonzero value with significant digits `d`, of
/// which there are `k`, and decimal exponent `n` (the value is
/// `0.d × 10^n`) is ECMAScript's: `d` and `n − k` zeros when `k ≤ n ≤ 21`,
/// `d` with a point after `n` digits when `0 < n ≤ 21`, `0.`, `−n` zeros and
/// `d` when `−6 < n ≤ 0`, and otherwise the first digit, the rest after a
/// point when there is a rest, `e`, the sign of `n − 1` and its magnitude.
/// Each form writes `d` without trailing zeros, which the first form adds
/// back as its own.
pub fn short_number_is_canonical(text: &[u8]) -> Option<bool> {
    let (negative, body) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, text),
    };
    let (mantissa, exponent) = match body.iter().position(|byte| matches!(byte, b'e' | b'E')) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let (whole, fraction) = match mantissa.iter().position(|byte| *byte == b'.') {
        Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
        None => (mantissa, &[][..]),
    };
    if exponent.is_none() && whole.len() <= SHORT_DIGITS {
        // The common forms, without building the expected text. The token's
        // grammar already rules out a leading zero before other digits.
        if fraction.is_empty() {
            // An integer: canonical unless it's -0.
            return Some(!(negative && whole == b"0"));
        }
        let ends_in_zero = fraction.last() == Some(&b'0');
        if whole != b"0" {
            // `n` is the whole part's length, at most 15, so the form puts
            // the point after it: canonical when no zero ends the fraction.
            if whole.len() + fraction.len() <= SHORT_DIGITS {
                return Some(!ends_in_zero);
            }
        } else if fraction.len() <= SHORT_DIGITS {
            // 0.000ddd: canonical below six leading zeros, for a nonzero
            // value whose last digit isn't a zero.
            let zeros = fraction.iter().take_while(|digit| **digit == b'0').count();
            return Some(!ends_in_zero && zeros < 6);
        }
    }
    // The exponent as written, or 0.
    let written = match exponent {
        None => 0,
        Some(exponent) => {
            let (sign, digits) = match exponent.split_first() {
                Some((b'+', rest)) => (1, rest),
                Some((b'-', rest)) => (-1, rest),
                _ => (1, exponent),
            };
            if digits.len() > 3 {
                return None;
            }
            let magnitude = digits
                .iter()
                .fold(0i32, |value, digit| value * 10 + i32::from(digit - b'0'));
            sign * magnitude
        }
    };
    // The significant digits: every digit from the first nonzero one to the
    // last nonzero one.
    let all = || whole.iter().chain(fraction.iter());
    let Some(first) = all().position(|digit| *digit != b'0') else {
        // Zero: its one form is `0`.
        return Some(text == b"0");
    };
    let nonzero = |digit: &u8| *digit != b'0';
    let last = match fraction.iter().rposition(nonzero) {
        Some(at) => whole.len() + at,
        None => whole.iter().rposition(nonzero)?,
    };
    let k = last - first + 1;
    if k > SHORT_DIGITS || written.abs() > SHORT_EXPONENT {
        return None;
    }
    // The value is 0.d × 10^n.
    let n = whole.len() as i32 - first as i32 + written;
    let digits = || all().skip(first).take(k).copied();
    let mut expected = Vec::with_capacity(text.len());
    if negative {
        expected.push(b'-');
    }
    let k = k as i32;
    if k <= n && n <= 21 {
        expected.extend(digits());
        expected.extend(std::iter::repeat_n(b'0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        expected.extend(digits().take(n as usize));
        expected.push(b'.');
        expected.extend(digits().skip(n as usize));
    } else if -6 < n && n <= 0 {
        expected.extend_from_slice(b"0.");
        expected.extend(std::iter::repeat_n(b'0', (-n) as usize));
        expected.extend(digits());
    } else {
        expected.extend(digits().take(1));
        if k > 1 {
            expected.push(b'.');
            expected.extend(digits().skip(1));
        }
        expected.push(b'e');
        let e = n - 1;
        expected.push(if e >= 0 { b'+' } else { b'-' });
        expected.extend_from_slice(e.abs().to_string().as_bytes());
    }
    Some(expected == text)
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

    /// The full check of a number token, which the fast one must match.
    fn full_check(text: &str) -> bool {
        text.parse::<f64>()
            .is_ok_and(|value| value.is_finite() && ecmascript_number(value) == text)
    }

    #[test]
    fn the_short_number_check_agrees_with_the_full_one() {
        let mut tokens: Vec<String> = Vec::new();
        for sign in ["", "-"] {
            for whole in [
                "0",
                "1",
                "10",
                "120",
                "999999999999999",
                "100000000000000000000",
            ] {
                for fraction in ["", ".5", ".05", ".50", ".0", ".000001", ".0000001", ".25"] {
                    for exponent in [
                        "", "e1", "e+1", "e-1", "e+20", "e+21", "e-6", "e-7", "e+300", "e-300",
                        "e+07", "E+21",
                    ] {
                        tokens.push(format!("{sign}{whole}{fraction}{exponent}"));
                    }
                }
            }
        }
        for token in [
            "1e+21",
            "1.5e+21",
            "1e-7",
            "1.25e-7",
            "0.000001",
            "123456789012345",
            "1234567890123456",
            "0.1",
            "12.5",
            "100",
            "1e+2",
            "0e+5",
            "-0",
            "0",
        ] {
            tokens.push(token.to_owned());
        }
        let mut judged = 0;
        for token in &tokens {
            if let Some(fast) = short_number_is_canonical(token.as_bytes()) {
                judged += 1;
                assert_eq!(fast, full_check(token), "{token}");
            }
        }
        assert!(judged > tokens.len() / 2, "{judged} of {}", tokens.len());
    }

    #[test]
    fn the_short_number_check_agrees_on_every_small_decimal() {
        // Every token of up to four digits and a point, with exponents.
        let mut count = 0;
        for value in 0..10_000u32 {
            let digits = format!("{value}");
            for point in 0..=digits.len() {
                let (whole, fraction) = digits.split_at(point);
                let whole = if whole.is_empty() { "0" } else { whole };
                if whole.len() > 1 && whole.starts_with('0') {
                    continue;
                }
                let mantissa = if fraction.is_empty() {
                    whole.to_owned()
                } else {
                    format!("{whole}.{fraction}")
                };
                for exponent in ["", "e+21", "e-7", "e+5", "e-3", "e+22"] {
                    let token = format!("{mantissa}{exponent}");
                    if let Some(fast) = short_number_is_canonical(token.as_bytes()) {
                        assert_eq!(fast, full_check(&token), "{token}");
                        count += 1;
                    }
                }
            }
        }
        assert!(count > 100_000);
    }
}
