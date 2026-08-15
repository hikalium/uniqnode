//! 最小の HTTP/1.1 サーバ(std::net + 接続ごとのスレッド)。LAN 内の少数クライアント
//! という前提(SPEC §10)なので、非同期ランタイムは使わない。keep-alive、
//! Content-Length ボディ、`Expect: 100-continue` に対応する。chunked 受信は未対応
//! (501 を返す。必要になった時点で足す)。

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
                            eprintln!("uniqnode: http connection error: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!("uniqnode: accept error: {e}");
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
