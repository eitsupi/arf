//! Strict JSON text parser.
//!
//! `serde_json` cannot be used to build a [`Value`] directly for this
//! conversion: it silently keeps only the last of several members with the
//! same key, and it turns an integer too large for 64 bits into an
//! approximate double without saying so. Both must be rejected, so the text
//! is parsed here, with the limits applied as the document is read, and only
//! the finished tree is handed on.

use super::error::{JsonError, JsonErrorKind};
use super::limits::{Budget, MAX_EXACT_INTEGER, MAX_TEXT_BYTES, PathStack, check_depth};
use serde_json::{Map, Number, Value};

/// Parse `text` as a single JSON value (RFC 8259), enforcing the crate's
/// limits and rejecting duplicate object keys.
pub(super) fn parse(text: &str) -> Result<Value, JsonError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(JsonError::at_root(JsonErrorKind::TextTooLarge {
            len: text.len(),
            limit: MAX_TEXT_BYTES,
        }));
    }
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        budget: Budget::default(),
        path: PathStack::default(),
    };
    parser.skip_whitespace();
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();
    if parser.pos != parser.bytes.len() {
        return Err(parser.syntax("unexpected data after the JSON value"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    budget: Budget,
    path: PathStack,
}

impl Parser<'_> {
    fn syntax(&self, message: &'static str) -> JsonError {
        self.path.error(JsonErrorKind::Syntax {
            offset: self.pos,
            message,
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8, message: &'static str) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.syntax(message))
        }
    }

    /// Parse a value. `depth` is the nesting level of the enclosing container.
    fn parse_value(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.budget
            .count_node()
            .map_err(|kind| self.path.error(kind))?;
        match self.peek() {
            None => Err(self.syntax("unexpected end of input")),
            Some(b'n') => self.parse_literal("null", Value::Null),
            Some(b't') => self.parse_literal("true", Value::Bool(true)),
            Some(b'f') => self.parse_literal("false", Value::Bool(false)),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(self.syntax("unexpected character")),
        }
    }

    fn parse_literal(&mut self, word: &'static str, value: Value) -> Result<Value, JsonError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.syntax("invalid literal"))
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, JsonError> {
        check_depth(depth).map_err(|kind| self.path.error(kind))?;
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.path.push_index(items.len());
            self.skip_whitespace();
            items.push(self.parse_value(depth)?);
            self.path.pop();
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.syntax("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, JsonError> {
        check_depth(depth).map_err(|kind| self.path.error(kind))?;
        self.pos += 1; // '{'
        let mut members = Map::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.syntax("expected a string key in object"));
            }
            let key = self.parse_string()?;
            self.path.push_key(&key);
            if members.contains_key(&key) {
                return Err(self.path.error(JsonErrorKind::DuplicateKey));
            }
            self.skip_whitespace();
            self.expect(b':', "expected ':' after object key")?;
            self.skip_whitespace();
            let value = self.parse_value(depth)?;
            members.insert(key, value);
            self.path.pop();
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(self.syntax("expected ',' or '}' in object")),
            }
        }
    }

    fn parse_number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => self.skip_digits(),
            _ => return Err(self.syntax("invalid number")),
        }
        let digits_end = self.pos;
        let mut integral = true;
        if self.peek() == Some(b'.') {
            integral = false;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.syntax("expected digits after the decimal point"));
            }
            self.skip_digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            integral = false;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.syntax("expected digits in the exponent"));
            }
            self.skip_digits();
        }

        let token = &self.text[start..self.pos];
        if integral {
            let negative = token.starts_with('-');
            let digits = &token[usize::from(negative)..digits_end - start];
            // 2^53 has 16 digits, so any longer integer is out of range.
            let magnitude = if digits.len() > 16 {
                None
            } else {
                digits.parse::<u64>().ok()
            };
            return match magnitude {
                Some(m) if m <= MAX_EXACT_INTEGER => {
                    if negative && m == 0 {
                        // "-0" is a distinct double; keep its sign.
                        Ok(float_value(-0.0))
                    } else if negative {
                        Ok(Value::Number(Number::from(-(m as i64))))
                    } else {
                        Ok(Value::Number(Number::from(m)))
                    }
                }
                _ => Err(self.path.error(JsonErrorKind::IntegerOutOfRange)),
            };
        }

        match token.parse::<f64>() {
            Ok(number) if number.is_finite() => Ok(float_value(number)),
            _ => Err(self.path.error(JsonErrorKind::NonFiniteNumber)),
        }
    }

    fn skip_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }

    /// Parse a string token starting at the opening quote.
    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.pos += 1; // opening '"'
        let mut out = String::new();
        loop {
            // Copy the run of ordinary characters in one step. The run ends
            // at an ASCII byte, so it is a whole number of UTF-8 characters.
            let run_start = self.pos;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(&self.text[run_start..self.pos]);
            self.budget
                .count_string(self.pos - run_start)
                .map_err(|kind| self.path.error(kind))?;

            match self.peek() {
                None => return Err(self.syntax("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let ch = self.parse_escape()?;
                    self.budget
                        .count_string(ch.len_utf8())
                        .map_err(|kind| self.path.error(kind))?;
                    out.push(ch);
                }
                Some(_) => return Err(self.syntax("control character in string")),
            }
        }
    }

    /// Parse the part of an escape sequence after the backslash.
    fn parse_escape(&mut self) -> Result<char, JsonError> {
        let Some(byte) = self.peek() else {
            return Err(self.syntax("unterminated escape sequence"));
        };
        self.pos += 1;
        Ok(match byte {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{08}',
            b'f' => '\u{0c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let first = self.parse_hex4()?;
                match first {
                    0xD800..=0xDBFF => {
                        if !self.bytes[self.pos..].starts_with(b"\\u") {
                            return Err(self.syntax("unpaired surrogate in escape sequence"));
                        }
                        self.pos += 2;
                        let second = self.parse_hex4()?;
                        if !(0xDC00..=0xDFFF).contains(&second) {
                            return Err(self.syntax("unpaired surrogate in escape sequence"));
                        }
                        let code = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                        char::from_u32(code)
                            .ok_or_else(|| self.syntax("invalid escape sequence"))?
                    }
                    0xDC00..=0xDFFF => {
                        return Err(self.syntax("unpaired surrogate in escape sequence"));
                    }
                    code => char::from_u32(code)
                        .ok_or_else(|| self.syntax("invalid escape sequence"))?,
                }
            }
            _ => return Err(self.syntax("invalid escape sequence")),
        })
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let Some(digits) = self.bytes.get(self.pos..self.pos + 4) else {
            return Err(self.syntax("incomplete \\u escape"));
        };
        let mut code = 0u32;
        for &digit in digits {
            let value = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return Err(self.syntax("invalid hexadecimal digit in \\u escape")),
            };
            code = code * 16 + u32::from(value);
        }
        self.pos += 4;
        Ok(code)
    }
}

/// A finite double as a JSON number. Callers guarantee finiteness.
fn float_value(number: f64) -> Value {
    Number::from_f64(number)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
