//! 最小の HTTP/1.1(std::net)。LAN 内の少数クライアントという前提(SPEC §10)なので、
//! 非同期ランタイムは使わない。
//!
//! サーバは接続ごとのスレッドで、keep-alive、Content-Length ボディ、
//! `Expect: 100-continue` に対応する。chunked の受信は未対応(501 を返す)。
//!
//! クライアントは1リクエスト1接続で、レプリケーション(node/src/sync.rs の HttpPeer)と
//! 埋め込みサーバ(node/src/embed.rs)が共用する。相手の応答の読み方という同じ判断を
//! 二箇所に置かないための一箇所である(should/0135)。サーバと違って応答の chunked は
//! 読める: 相手は uniqnode とは限らず、llama-server のような別実装が相手になる。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct Request {
    pub method: String,
    /// パスは生のまま(パーセントデコードしない)。本 API の ID・ref パスは
    /// 予約文字を含まない字種に限られる。
    pub path: String,
    /// ヘッダ名は小文字化済み。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// 応答を書き切った後にプロセスを正常終了する(POST /v1/admin/shutdown 用。
    /// 正常終了はカバレッジのプロファイル書き出しも保証する)。
    pub shutdown_after: bool,
}

impl Response {
    pub fn json(status: u16, body: Vec<u8>) -> Response {
        Response { status, content_type: "application/json", body, shutdown_after: false }
    }
    pub fn text(status: u16, text: &str) -> Response {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            body: text.into(),
            shutdown_after: false,
        }
    }
    pub fn bytes(status: u16, body: Vec<u8>) -> Response {
        Response {
            status,
            content_type: "application/octet-stream",
            body,
            shutdown_after: false,
        }
    }
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Response",
    }
}

pub type Handler = dyn Fn(&Request) -> Response + Send + Sync;

/// listener を受け取り、接続ごとにスレッドを立てて捌き続ける(返らない)。
pub fn serve(listener: TcpListener, handler: Arc<Handler>) -> ! {
    loop {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let handler = handler.clone();
                std::thread::spawn(move || {
                    // 接続単位のエラーはその接続を閉じるだけでよい。
                    if let Err(e) = handle_connection(stream, handler) {
                        // タイムアウト・切断は平常運転なのでログにしない。
                        let benign = matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::BrokenPipe
                        );
                        if !benign {
                            crate::log_line!("uniqnode: http connection error: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                crate::log_line!("uniqnode: accept error: {e}");
            }
        }
    }
}

fn handle_connection(stream: TcpStream, handler: Arc<Handler>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    loop {
        let request = match read_request(&mut reader, &mut writer)? {
            None => return Ok(()), // クライアントが正常に切った
            Some(ReadOutcome::Bad(status, message)) => {
                write_response(&mut writer, &Response::text(status, &message), true)?;
                return Ok(());
            }
            Some(ReadOutcome::Ok(r)) => r,
        };
        let close = matches!(request.header("connection"), Some(v) if v.eq_ignore_ascii_case("close"));
        let response = handler(&request);
        let write_result = write_response(&mut writer, &response, close);
        if response.shutdown_after {
            // 応答の書き込みに失敗しても(クライアントが先に切っても)終了は実行する。
            std::process::exit(0);
        }
        write_result?;
        if close {
            return Ok(());
        }
    }
}

enum ReadOutcome {
    Ok(Request),
    Bad(u16, String),
}

fn read_request(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
) -> std::io::Result<Option<ReadOutcome>> {
    // リクエストライン+ヘッダを読む。
    let mut head = String::new();
    let mut lines = Vec::new();
    loop {
        head.clear();
        let n = reader.read_line(&mut head)?;
        if n == 0 {
            return Ok(if lines.is_empty() { None } else {
                Some(ReadOutcome::Bad(400, "リクエストが途中で切れた".into()))
            });
        }
        let line = head.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            if lines.is_empty() {
                continue; // リクエスト間の空行は読み飛ばす
            }
            break;
        }
        lines.push(line);
        if lines.iter().map(|l| l.len()).sum::<usize>() > MAX_HEADER_BYTES {
            return Ok(Some(ReadOutcome::Bad(400, "ヘッダが大きすぎる".into())));
        }
    }
    let mut parts = lines[0].split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let version = parts.next().unwrap_or("");
    if method.is_empty() || path.is_empty() || !version.starts_with("HTTP/1.") {
        return Ok(Some(ReadOutcome::Bad(400, "リクエストラインが不正".into())));
    }
    let mut headers = Vec::new();
    for line in &lines[1..] {
        match line.split_once(':') {
            Some((name, value)) => {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
            None => return Ok(Some(ReadOutcome::Bad(400, "ヘッダ行が不正".into()))),
        }
    }
    let request_head = Request { method, path, headers, body: Vec::new() };

    if request_head.header("transfer-encoding").is_some() {
        return Ok(Some(ReadOutcome::Bad(501, "chunked 転送は未対応".into())));
    }
    let content_length = match request_head.header("content-length") {
        None => 0usize,
        Some(v) => match v.parse::<usize>() {
            Ok(n) => n,
            Err(_) => return Ok(Some(ReadOutcome::Bad(400, "Content-Length が不正".into()))),
        },
    };
    if content_length > MAX_BODY_BYTES {
        return Ok(Some(ReadOutcome::Bad(413, "ボディが大きすぎる".into())));
    }
    if matches!(request_head.header("expect"), Some(v) if v.eq_ignore_ascii_case("100-continue")) {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        writer.flush()?;
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(ReadOutcome::Ok(Request { body, ..request_head })))
}

fn write_response(writer: &mut TcpStream, response: &Response, close: bool) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
        response.status,
        status_reason(response.status),
        response.content_type,
        response.body.len(),
        if close { "close" } else { "keep-alive" },
    );
    writer.write_all(head.as_bytes())?;
    writer.write_all(&response.body)?;
    writer.flush()
}

// ---- クライアント(1リクエスト1接続) ----

/// クライアントが受け入れる応答ボディの上限。相手の Content-Length やチャンク長を
/// そのまま信じて確保しないための歯止め。
const MAX_CLIENT_BODY_BYTES: usize = MAX_BODY_BYTES;

/// クライアントが読んだ応答。ヘッダは読み捨てる(呼び手が使うのは状態符号と本文だけ)。
#[derive(Debug)]
pub struct ClientResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// 応答の先頭だけを診断に載せる(全文をエラーメッセージに流し込まない)。相手が誤りを
/// 返したときの言い方は、埋め込みクライアントも MCP の転送する形も同じである
/// (should/0135)。
pub fn body_head(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    match text.char_indices().nth(200) {
        Some((offset, _)) => format!("{}…", &text[..offset]),
        None => text.to_string(),
    }
}

/// URL を (接続先の host:port, パス) に割る。scheme 省略と http:// を受け、https:// は
/// 黙って平文で繋がず断る(TLS を実装していない。must/0022)。ポートを省略した相手は
/// 80 番とみなす。
pub fn split_http_url(url: &str) -> Result<(String, String), String> {
    if url.starts_with("https://") {
        return Err(format!("{url}: https は未対応(TLS を実装していない)"));
    }
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (authority, path) = match rest.find('/') {
        Some(cut) => (&rest[..cut], &rest[cut..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(format!("{url}: 接続先がない"));
    }
    let address = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Ok((address, path.to_string()))
}

/// GET を1本送って応答を読む。
pub fn get(
    address: &str,
    path: &str,
    timeout: std::time::Duration,
) -> Result<ClientResponse, String> {
    request(address, "GET", path, None, timeout)
}

/// JSON ボディを POST して応答を読む。
pub fn post_json(
    address: &str,
    path: &str,
    body: &[u8],
    timeout: std::time::Duration,
) -> Result<ClientResponse, String> {
    request(address, "POST", path, Some(("application/json", body)), timeout)
}

/// 1リクエスト1接続で送って応答を読む。接続・読み・書きのすべてに同じ期限を掛ける
/// (期限のない待ちを作らない。should/0104)。ボディを持つ要求は Content-Type と
/// Content-Length を付ける。
pub fn request(
    address: &str,
    method: &str,
    path: &str,
    body: Option<(&str, &[u8])>,
    timeout: std::time::Duration,
) -> Result<ClientResponse, String> {
    use std::net::ToSocketAddrs;
    let resolved = address
        .to_socket_addrs()
        .map_err(|e| format!("{address} を解決できない: {e}"))?
        .next()
        .ok_or_else(|| format!("{address} を解決できない"))?;
    let stream = TcpStream::connect_timeout(&resolved, timeout)
        .map_err(|e| format!("{address} に接続できない: {e}"))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| format!("timeout 設定: {e}"))?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| format!("timeout 設定: {e}"))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    // Connection: close なので、応答は1本だけ読めばよい(相手が長さを言わない場合の
    // 終端も接続の切断で決まる)。
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    if let Some((content_type, bytes)) = body {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            bytes.len()
        ));
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes()).map_err(|e| format!("送信: {e}"))?;
    if let Some((_, bytes)) = body {
        writer.write_all(bytes).map_err(|e| format!("送信: {e}"))?;
    }
    writer.flush().map_err(|e| format!("送信: {e}"))?;
    let mut reader = BufReader::new(stream);
    read_client_response(&mut reader)
}

/// 応答の状態行・ヘッダ・本文を読む。本文の長さの決め方は3通りで、相手が言った形に
/// 従う: chunked(Transfer-Encoding)、Content-Length、どちらも無ければ接続の切断まで。
fn read_client_response(reader: &mut impl BufRead) -> Result<ClientResponse, String> {
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| format!("応答読み取り: {e}"))?;
    if status_line.is_empty() {
        return Err("応答が空(相手が何も返さずに切った)".to_string());
    }
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("応答ラインが不正: {status_line:?}"))?;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|e| format!("ヘッダ読み取り: {e}"))?;
        if read == 0 {
            return Err("ヘッダの途中で切れた".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let name = name.trim();
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value.parse().map_err(|_| format!("Content-Length が不正: {value:?}"))?,
                );
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    // chunked が付いていたら Content-Length より優先する(RFC 9112 の規定順)。
    let body = if chunked {
        read_chunked_body(reader)?
    } else if let Some(length) = content_length {
        if length > MAX_CLIENT_BODY_BYTES {
            return Err(format!("応答ボディが大きすぎる({length} バイト)"));
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).map_err(|e| format!("ボディ読み取り: {e}"))?;
        body
    } else {
        let mut body = Vec::new();
        reader
            .take(MAX_CLIENT_BODY_BYTES as u64)
            .read_to_end(&mut body)
            .map_err(|e| format!("ボディ読み取り: {e}"))?;
        body
    };
    Ok(ClientResponse { status, body })
}

/// chunked の本文を繋ぐ。長さ行(16進。`;` 以降の拡張は捨てる)、その長さのバイト列、
/// CRLF の繰り返しで、長さ 0 の行が終端。終端のあとのトレーラは空行まで読み飛ばす。
fn read_chunked_body(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|e| format!("チャンク長の読み取り: {e}"))?;
        if read == 0 {
            return Err("チャンクの途中で切れた".to_string());
        }
        let head = line.trim_end_matches(['\r', '\n']);
        let size_text = head.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("チャンク長が16進でない: {size_text:?}"))?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                let read = reader
                    .read_line(&mut trailer)
                    .map_err(|e| format!("トレーラの読み取り: {e}"))?;
                if read == 0 || trailer.trim_end_matches(['\r', '\n']).is_empty() {
                    return Ok(body);
                }
            }
        }
        if body.len() + size > MAX_CLIENT_BODY_BYTES {
            return Err(format!("応答ボディが大きすぎる({} バイト超)", body.len() + size));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..]).map_err(|e| format!("チャンクの読み取り: {e}"))?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(|e| format!("チャンク末尾の読み取り: {e}"))?;
        if &crlf != b"\r\n" {
            return Err("チャンクの末尾が CRLF でない".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 決められたバイト列を1本だけ返す試験用のサーバ。要求は読み切って呼び手に渡す
    /// (production の呼び手が実際に組み立てた要求を検査するため。should/0138)。
    fn canned_response(response: &'static [u8]) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            // 要求のヘッダ(と Content-Length ぶんのボディ)を読む。
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request = Vec::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("request line");
                request.extend_from_slice(line.as_bytes());
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if let Some((name, value)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().expect("length");
                    }
                }
                if trimmed.is_empty() {
                    break;
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).expect("request body");
            request.extend_from_slice(&body);
            stream.write_all(response).expect("write response");
            stream.flush().expect("flush");
            request
        });
        (address, handle)
    }

    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    /// Content-Length の応答を読む。POST の要求は、呼び手が渡した本文と Content-Type を
    /// そのまま載せる(埋め込みサーバはこれを見て JSON として読む)。
    #[test]
    fn a_post_carries_the_json_body_and_reads_a_content_length_response() {
        let (address, handle) = canned_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}",
        );
        let response =
            post_json(&address, "/v1/embeddings", b"{\"input\":[\"x\"]}", TIMEOUT).expect("post");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"a\":1}");
        let request = String::from_utf8(handle.join().expect("join")).expect("utf-8");
        assert!(request.starts_with("POST /v1/embeddings HTTP/1.1\r\n"), "{request:?}");
        assert!(request.contains("Content-Type: application/json\r\n"), "{request:?}");
        assert!(request.contains("Content-Length: 15\r\n"), "{request:?}");
        assert!(request.ends_with("\r\n\r\n{\"input\":[\"x\"]}"), "{request:?}");
    }

    /// chunked の応答を繋ぐ。llama-server の実測は Content-Length だが(1.4MB の応答でも
    /// 分割しなかった)、HTTP/1.1 のサーバは長さを先に言わずに返してよいので、読めない
    /// まま黙って切れる経路を残さない。
    #[test]
    fn a_chunked_response_is_reassembled_including_extensions_and_trailers() {
        let (address, handle) = canned_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
              4\r\n{\"a\"\r\n3;x=1\r\n:1}\r\n0\r\nX-Trailer: t\r\n\r\n",
        );
        let response = get(&address, "/health", TIMEOUT).expect("get");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"a\":1}", "チャンクが順に繋がるべき");
        let request = String::from_utf8(handle.join().expect("join")).expect("utf-8");
        assert!(request.starts_with("GET /health HTTP/1.1\r\n"), "{request:?}");
    }

    /// 長さを言わない応答(Connection: close で終端する形)も読める。
    #[test]
    fn a_response_without_a_length_ends_at_the_close() {
        let (address, handle) = canned_response(b"HTTP/1.1 500 Internal Server Error\r\n\r\nboom");
        let response = get(&address, "/x", TIMEOUT).expect("get");
        assert_eq!(response.status, 500);
        assert_eq!(response.body, b"boom");
        handle.join().expect("join");
    }

    /// 壊れた応答は黙って空として通さない(must/0022)。
    #[test]
    fn a_malformed_response_is_reported_not_swallowed() {
        let (address, handle) = canned_response(b"garbage\r\n\r\n");
        let error = get(&address, "/x", TIMEOUT).expect_err("状態行が不正なら失敗すべき");
        assert!(error.contains("応答ライン"), "{error}");
        handle.join().expect("join");

        let (address, handle) =
            canned_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n");
        let error = get(&address, "/x", TIMEOUT).expect_err("チャンク長が不正なら失敗すべき");
        assert!(error.contains("チャンク長"), "{error}");
        handle.join().expect("join");
    }

    /// 相手のいないアドレスは、期限内に接続できないことを言って失敗する。
    #[test]
    fn an_unreachable_peer_fails_with_the_address_in_the_message() {
        // ポート 1 は特権ポートで、この試験環境では誰も待ち受けていない。
        let error = get("127.0.0.1:1", "/x", std::time::Duration::from_millis(500))
            .expect_err("繋がらないはず");
        assert!(error.contains("127.0.0.1:1"), "{error}");
        assert!(error.contains("接続できない"), "{error}");
    }

    /// URL の割り方(期待値はリテラル。should/0137)。
    #[test]
    fn urls_split_into_authority_and_path() {
        assert_eq!(
            split_http_url("http://127.0.0.1:8083/v1/embeddings").expect("split"),
            ("127.0.0.1:8083".to_string(), "/v1/embeddings".to_string())
        );
        // scheme は省略できる。
        assert_eq!(
            split_http_url("127.0.0.1:8083/v1/embeddings").expect("split"),
            ("127.0.0.1:8083".to_string(), "/v1/embeddings".to_string())
        );
        // パスの省略は / 、ポートの省略は 80。
        assert_eq!(
            split_http_url("http://example.test").expect("split"),
            ("example.test:80".to_string(), "/".to_string())
        );
        // https は平文にすり替えずに断る。
        assert!(split_http_url("https://example.test/v1").is_err());
        assert!(split_http_url("http://").is_err());
    }
}
