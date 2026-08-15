//! 正規化 JSON「c1 形式」(SPEC §4.1)とオブジェクトID(SPEC §4.2)。
//!
//! 入力の解釈は通常の JSON(空白・全エスケープ形式を受理、ただし整数のみ・重複キー拒否)で
//! 行い、ハッシュ計算は常に正規形への再直列化を通す。正規形は: UTF-8・1行・キーはバイト順・
//! 空白なし・整数のみ・最小限のエスケープ(`\"` `\\` と制御文字の `\u00xx` 小文字4桁)。

use crate::sha2::sha256;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Text(String),
    Array(Vec<Value>),
    /// BTreeMap の順序はバイト辞書順であり、正規形のキー順序と一致する。
    Object(BTreeMap<String, Value>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 入力先頭からのバイト位置。
    pub position: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "byte {}: {}", self.position, self.message)
    }
}

const MAX_DEPTH: usize = 128;

pub fn parse(input: &str) -> Result<Value, ParseError> {
    let bytes = input.as_bytes();
    let mut parser = Parser { bytes, position: 0 };
    parser.skip_whitespace();
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();
    if parser.position != bytes.len() {
        return Err(parser.error("値の後に余分な入力がある"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, message: &str) -> ParseError {
        ParseError { position: self.position, message: message.to_string() }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.position += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ParseError> {
        if self.peek() == Some(expected) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.error(&format!("'{}' を期待した", expected as char)))
        }
    }

    fn consume_literal(&mut self, literal: &str) -> bool {
        if self.bytes[self.position..].starts_with(literal.as_bytes()) {
            self.position += literal.len();
            true
        } else {
            false
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("入れ子が深すぎる"));
        }
        match self.peek() {
            None => Err(self.error("値を期待したが入力が尽きた")),
            Some(b'n') => {
                if self.consume_literal("null") {
                    Ok(Value::Null)
                } else {
                    Err(self.error("null を期待した"))
                }
            }
            Some(b't') => {
                if self.consume_literal("true") {
                    Ok(Value::Bool(true))
                } else {
                    Err(self.error("true を期待した"))
                }
            }
            Some(b'f') => {
                if self.consume_literal("false") {
                    Ok(Value::Bool(false))
                } else {
                    Err(self.error("false を期待した"))
                }
            }
            Some(b'"') => Ok(Value::Text(self.parse_string()?)),
            Some(b'[') => self.parse_array(depth),
            Some(b'{') => self.parse_object(depth),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_integer(),
            Some(other) => Err(self.error(&format!("解釈できないバイト 0x{other:02x}"))),
        }
    }

    fn parse_integer(&mut self) -> Result<Value, ParseError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        let digits_start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        if self.position == digits_start {
            return Err(self.error("数字を期待した"));
        }
        // 小数・指数は仕様で禁止(SPEC §4.1)。
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err(self.error("浮動小数点は c1 では使えない(整数のみ)"));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .expect("数字と'-'のみなので常に UTF-8");
        let digits = &self.bytes[digits_start..self.position];
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(self.error("先頭ゼロは許されない"));
        }
        match text.parse::<i64>() {
            Ok(v) => Ok(Value::Integer(v)),
            Err(_) => Err(self.error("64bit 符号付き整数の範囲外")),
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let b = match self.peek() {
                None => return Err(self.error("文字列が閉じていない")),
                Some(b) => b,
            };
            match b {
                b'"' => {
                    self.position += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.position += 1;
                    let escaped = match self.peek() {
                        None => return Err(self.error("エスケープが途切れた")),
                        Some(e) => e,
                    };
                    self.position += 1;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.parse_hex4()?;
                            if (0xd800..0xdc00).contains(&unit) {
                                // サロゲートペアの結合。
                                if !(self.consume_literal("\\u")) {
                                    return Err(self.error("上位サロゲートに続きがない"));
                                }
                                let low = self.parse_hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err(self.error("下位サロゲートが不正"));
                                }
                                let code =
                                    0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
                                match char::from_u32(code) {
                                    Some(c) => out.push(c),
                                    None => return Err(self.error("不正なコードポイント")),
                                }
                            } else if (0xdc00..0xe000).contains(&unit) {
                                return Err(self.error("孤立した下位サロゲート"));
                            } else {
                                match char::from_u32(unit) {
                                    Some(c) => out.push(c),
                                    None => return Err(self.error("不正なコードポイント")),
                                }
                            }
                        }
                        other => {
                            return Err(
                                self.error(&format!("不明なエスケープ \\{}", other as char))
                            )
                        }
                    }
                }
                0x00..=0x1f => return Err(self.error("文字列中の生の制御文字")),
                _ => {
                    // UTF-8 のマルチバイト列をそのまま写す。
                    let rest = &self.bytes[self.position..];
                    let text = match std::str::from_utf8(rest) {
                        Ok(t) => t,
                        Err(e) if e.valid_up_to() > 0 => {
                            std::str::from_utf8(&rest[..e.valid_up_to()]).expect("検証済み")
                        }
                        Err(_) => return Err(self.error("不正な UTF-8")),
                    };
                    let ch = text.chars().next().expect("少なくとも1文字ある");
                    out.push(ch);
                    self.position += ch.len_utf8();
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, ParseError> {
        if self.position + 4 > self.bytes.len() {
            return Err(self.error("\\u の16進4桁が足りない"));
        }
        let text = std::str::from_utf8(&self.bytes[self.position..self.position + 4])
            .map_err(|_| self.error("\\u の16進が不正"))?;
        let value =
            u32::from_str_radix(text, 16).map_err(|_| self.error("\\u の16進が不正"))?;
        self.position += 4;
        Ok(value)
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                }
                Some(b']') => {
                    self.position += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("',' か ']' を期待した")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value = self.parse_value(depth + 1)?;
            if map.insert(key, value).is_some() {
                return Err(self.error("重複キーは許されない"));
            }
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                }
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.error("',' か '}' を期待した")),
            }
        }
    }
}

/// 正規形(c1)への直列化。
pub fn to_canonical_bytes(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Integer(v) => out.extend_from_slice(v.to_string().as_bytes()),
        Value::Text(text) => write_canonical_string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            out.push(b'{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical_string(key, out);
                out.push(b':');
                write_canonical(item, out);
            }
            out.push(b'}');
        }
    }
}

fn write_canonical_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for ch in text.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{0000}'..='\u{001f}' => {
                out.extend_from_slice(format!("\\u{:04x}", ch as u32).as_bytes());
            }
            _ => {
                let mut buffer = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// 構造化オブジェクトの ID(SPEC §4.2)。
pub fn object_id(value: &Value) -> String {
    id_for_bytes(&to_canonical_bytes(value))
}

/// blob(生バイト列)の ID。
pub fn id_for_bytes(bytes: &[u8]) -> String {
    format!("s256:{}", crate::sha2::hex(&sha256(bytes)))
}

/// c1 値の中のオブジェクト参照(`s256:` + 16進64桁の文字列)を列挙する(SPEC §4.3)。
pub fn collect_references(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Text(text)
            if is_object_id(text) => {
                out.push(text.clone());
            }
        Value::Array(items) => {
            for item in items {
                collect_references(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_references(item, out);
            }
        }
        _ => {}
    }
}

pub fn is_object_id(text: &str) -> bool {
    text.len() == 69
        && text.starts_with("s256:")
        && text[5..].bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_text(input: &str) -> String {
        String::from_utf8(to_canonical_bytes(&parse(input).expect("parse"))).expect("utf8")
    }

    #[test]
    fn canonicalization_sorts_keys_and_strips_whitespace() {
        assert_eq!(
            canonical_text("{ \"b\" : 1 , \"a\" : [ 1 , 2 ] }"),
            "{\"a\":[1,2],\"b\":1}"
        );
    }

    #[test]
    fn keys_sort_by_utf8_bytes() {
        // "あ" (E3 81 82) は "z" (7A) より後。
        assert_eq!(canonical_text("{\"あ\":1,\"z\":2}"), "{\"z\":2,\"あ\":1}");
    }

    #[test]
    fn escapes_normalize_to_minimal_form() {
        // 短縮形・\uXXXX 大文字・不要な \/ はすべて正規形に畳まれる。
        assert_eq!(
            canonical_text("\"a\\n\\u000A\\t\\/\\u0041\\uD834\\uDD1E\""),
            "\"a\\u000a\\u000a\\u0009/A𝄞\""
        );
    }

    #[test]
    fn rejects_floats_duplicate_keys_and_leading_zeros() {
        assert!(parse("1.5").is_err());
        assert!(parse("1e3").is_err());
        assert!(parse("{\"a\":1,\"a\":2}").is_err());
        assert!(parse("01").is_err());
        assert!(parse("9223372036854775808").is_err()); // i64::MAX + 1
        assert!(parse("{\"a\":1}x").is_err());
        assert!(parse("\"\\uD834\"").is_err()); // 孤立サロゲート
    }

    #[test]
    fn integer_bounds_round_trip() {
        assert_eq!(canonical_text("-9223372036854775808"), "-9223372036854775808");
        assert_eq!(canonical_text("9223372036854775807"), "9223372036854775807");
        assert_eq!(canonical_text("-0"), "0");
    }

    #[test]
    fn object_id_is_stable_and_input_form_independent() {
        let a = object_id(&parse("{\"v\":1,\"kind\":\"node\",\"contents\":\"x\"}").expect("p"));
        let b = object_id(
            &parse("{ \"contents\" : \"x\" , \"kind\" : \"node\" , \"v\" : 1 }").expect("p"),
        );
        assert_eq!(a, b);
        assert!(is_object_id(&a));
    }

    #[test]
    fn collect_references_finds_ids_everywhere() {
        let id = id_for_bytes(b"hello");
        let doc = format!(
            "{{\"members\":[\"{id}\",\"not-an-id\"],\"meta\":{{\"prev\":\"{id}\"}}}}"
        );
        let mut refs = Vec::new();
        collect_references(&parse(&doc).expect("p"), &mut refs);
        assert_eq!(refs, vec![id.clone(), id]);
    }

    #[test]
    fn depth_limit_is_enforced() {
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert!(parse(&deep).is_err());
        let ok = "[".repeat(100) + &"]".repeat(100);
        assert!(parse(&ok).is_ok());
    }
}
