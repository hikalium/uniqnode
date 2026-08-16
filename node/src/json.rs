//! 応答を読むための最小の JSON。c1::parse は使えない: c1(SPEC §4.1)は整数しか持たない
//! 正規形で、埋め込みのベクトルや検索の得点(小数)を読めないからである。
//!
//! 読み手は二つある。埋め込みサーバの応答(node/src/embed.rs)と、走っている serve の
//! REST 応答を読む MCP の転送する形(node/src/mcp.rs、node/src/api.rs の
//! parse_search_response)である。小数を含む JSON を読むという同じ判断を二箇所に置かない
//! ための一箇所である(should/0135)。
//!
//! 求めた形だけを解釈し、それ以外は失敗させる(must/0020)。書き手の側はどちらの相手にも
//! c1 の正規形を使うので、この読み手が受ける形は「c1 が書けるもの + 小数」に収まる。

/// 読み取った JSON の値。値を持つのは文字列と数だけで、真偽値と null は形として区別する
/// (どちらの応答も、真偽値の中身を必要としない)。
#[derive(Debug)]
pub enum Json {
    Null,
    Bool,
    Number(f64),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// 入れ子の深さの上限(相手の応答で自分のスタックを壊さないための歯止め)。
const JSON_MAX_DEPTH: usize = 32;

struct JsonReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut reader = JsonReader { bytes: text.as_bytes(), at: 0 };
        let value = reader.value(0)?;
        reader.skip_whitespace();
        if reader.at != reader.bytes.len() {
            return Err(format!("JSON の末尾に余りがある(位置 {})", reader.at));
        }
        Ok(value)
    }

    /// オブジェクトの項(オブジェクトでなければ None)。同じ名前が二度現れる応答は
    /// 想定しないので、最初の一つを返す。
    pub fn field(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => {
                fields.iter().find(|(key, _)| key == name).map(|(_, value)| value)
            }
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Json::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn number(&self) -> Option<f64> {
        match self {
            Json::Number(number) => Some(*number),
            _ => None,
        }
    }

    /// 整数として読む(小数を含む値は整数ではないので None)。
    pub fn integer(&self) -> Option<i64> {
        match self {
            Json::Number(number) if number.fract() == 0.0 => Some(*number as i64),
            _ => None,
        }
    }

    pub fn array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }
}

impl JsonReader<'_> {
    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.bytes.get(self.at) {
            if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Result<u8, String> {
        self.bytes.get(self.at).copied().ok_or_else(|| "JSON が途中で終わった".to_string())
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek()? != byte {
            return Err(format!("位置 {} に {:?} を期待した", self.at, char::from(byte)));
        }
        self.at += 1;
        Ok(())
    }

    fn literal(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(format!("位置 {} が {word} でない", self.at))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > JSON_MAX_DEPTH {
            return Err(format!("入れ子が深すぎる(上限 {JSON_MAX_DEPTH})"));
        }
        self.skip_whitespace();
        match self.peek()? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                self.skip_whitespace();
                if self.peek()? == b'}' {
                    self.at += 1;
                    return Ok(Json::Object(fields));
                }
                loop {
                    self.skip_whitespace();
                    let name = self.string()?;
                    self.skip_whitespace();
                    self.expect(b':')?;
                    let value = self.value(depth + 1)?;
                    fields.push((name, value));
                    self.skip_whitespace();
                    match self.peek()? {
                        b',' => self.at += 1,
                        b'}' => {
                            self.at += 1;
                            return Ok(Json::Object(fields));
                        }
                        other => {
                            return Err(format!(
                                "位置 {} に , か }} を期待した(実際は {:?})",
                                self.at,
                                char::from(other)
                            ))
                        }
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.skip_whitespace();
                if self.peek()? == b']' {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.skip_whitespace();
                    match self.peek()? {
                        b',' => self.at += 1,
                        b']' => {
                            self.at += 1;
                            return Ok(Json::Array(items));
                        }
                        other => {
                            return Err(format!(
                                "位置 {} に , か ] を期待した(実際は {:?})",
                                self.at,
                                char::from(other)
                            ))
                        }
                    }
                }
            }
            b'"' => Ok(Json::Text(self.string()?)),
            b't' => {
                self.literal("true")?;
                Ok(Json::Bool)
            }
            b'f' => {
                self.literal("false")?;
                Ok(Json::Bool)
            }
            b'n' => {
                self.literal("null")?;
                Ok(Json::Null)
            }
            _ => self.number(),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self.peek()?;
            self.at += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = self.peek()?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        // \u エスケープ。c1 の正規形は制御文字をこの形に畳むので、
                        // serve の応答(本文の改行など)を読むために要る。
                        b'u' => {
                            let code = self.hex4()?;
                            match char::from_u32(code) {
                                Some(character) => out.push(character),
                                None => {
                                    return Err(format!(
                                        "位置 {} の \\u{code:04x} が文字にならない",
                                        self.at
                                    ))
                                }
                            }
                        }
                        other => {
                            return Err(format!("未対応のエスケープ \\{}", char::from(other)))
                        }
                    }
                }
                _ => {
                    // UTF-8 の続きバイトはそのまま繋ぐ(入力は &str なので境界は正しい)。
                    let start = self.at - 1;
                    while self.bytes.get(self.at).is_some_and(|b| (b & 0xC0) == 0x80) {
                        self.at += 1;
                    }
                    match std::str::from_utf8(&self.bytes[start..self.at]) {
                        Ok(fragment) => out.push_str(fragment),
                        Err(_) => return Err(format!("位置 {start} の文字が UTF-8 でない")),
                    }
                }
            }
        }
    }

    /// \u エスケープの 16 進 4 桁。代理対(サロゲート)は c1 の書き手が出さない形なので
    /// 受けない。読めない形を推測で通さず、言って断る(must/0022)。
    fn hex4(&mut self) -> Result<u32, String> {
        let start = self.at;
        let mut code = 0u32;
        for _ in 0..4 {
            let byte = self.peek()?;
            let digit = char::from(byte)
                .to_digit(16)
                .ok_or_else(|| format!("位置 {} の \\u が 16 進 4 桁でない", start))?;
            code = code * 16 + digit;
            self.at += 1;
        }
        Ok(code)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while let Some(byte) = self.bytes.get(self.at) {
            if byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E') {
                self.at += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| format!("位置 {start} の数が UTF-8 でない"))?;
        text.parse::<f64>()
            .map(Json::Number)
            .map_err(|_| format!("位置 {start} の {text:?} が数でない"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小の JSON 読み取りが、相手の応答に現れる形(指数表記・負数・入れ子・
    /// エスケープ)を読む。壊れた入力は黙って部分解釈しない(must/0020)。
    #[test]
    fn the_json_reader_accepts_the_shape_the_servers_return() {
        let text = "{\"a\":[-1.5e-3,0,2],\"b\":{\"c\":\"x\\ny\"},\"d\":true,\"e\":null}";
        let Json::Object(fields) = Json::parse(text).expect("parse") else {
            panic!("オブジェクトのはず");
        };
        assert_eq!(fields.len(), 4);
        let Json::Array(numbers) = &fields[0].1 else { panic!("配列のはず") };
        assert_eq!(numbers.len(), 3);
        let Json::Number(first) = numbers[0] else { panic!("数のはず") };
        assert!((first - (-0.0015)).abs() < 1e-12, "{first}");
        // 壊れた入力(閉じない・余りがある・16 進でない \u)は失敗する。
        assert!(Json::parse("{\"a\":1").is_err());
        assert!(Json::parse("{} {}").is_err());
        assert!(Json::parse("{\"a\":\"\\uZZZZ\"}").is_err());
        assert!(Json::parse("").is_err());
    }

    /// 文字列は値を持ち、c1 の正規形が畳んだ制御文字(\u000a など)も戻る。REST の
    /// 応答から出典と本文を組み直すために要る(node/src/api.rs の
    /// parse_search_response)。
    #[test]
    fn strings_carry_their_value_including_folded_control_characters() {
        let value = Json::parse("{\"snippet\":\"\\u000a一行目\\u000a\",\"at\":17,\"x\":1.5}")
            .expect("parse");
        assert_eq!(value.field("snippet").and_then(Json::text), Some("\n一行目\n"));
        assert_eq!(value.field("at").and_then(Json::integer), Some(17));
        // 小数は整数として読めない(黙って切り捨てない)。
        assert_eq!(value.field("x").and_then(Json::integer), None);
        assert_eq!(value.field("x").and_then(Json::number), Some(1.5));
        assert!(value.field("absent").is_none(), "無い項は None");
    }
}
