//! 統合テスト共通ヘルパ: serve プロセスの起動と、curl が組み立てる形の
//! 生 HTTP/1.1 クライアント(should/0138)。
// 統合テストクレートごとに個別コンパイルされ、どのクレートも全ヘルパは使わないため。
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub struct Server {
    pub child: Child,
    pub address: String,
    pub dir: PathBuf,
    /// Drop 時にディレクトリを消すか(再起動テストでは残す)。
    pub remove_dir_on_drop: bool,
}

impl Drop for Server {
    fn drop(&mut self) {
        // まず正常終了を頼む(カバレッジのプロファイル書き出しは正常終了でのみ起きる)。
        // 応答を読み切ってから閉じる(書いた直後に閉じるとサーバ側の応答書き込みと
        // 競合する)。期限内に終わらなければ kill に切り替える。
        let asked = TcpStream::connect(&self.address).ok().and_then(|mut stream| {
            stream
                .write_all(
                    b"POST /v1/admin/shutdown HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .ok()?;
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            Some(())
        });
        if asked.is_some() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::yield_now(),
                    Err(_) => break,
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.remove_dir_on_drop {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

pub fn unique_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("uniqnode-test-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    dir
}

pub fn start_server_at(dir: PathBuf) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["serve", dir.to_str().expect("utf-8"), "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn serve");
    let stdout = child.stdout.take().expect("stdout");
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).expect("read listening line");
    let address = line
        .trim()
        .strip_prefix("listening on ")
        .expect("listening line")
        .to_string();
    Server { child, address, dir, remove_dir_on_drop: true }
}

pub fn start_server(name: &str) -> Server {
    start_server_at(unique_dir(name))
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub fn read_response(reader: &mut BufReader<TcpStream>) -> HttpResponse {
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("status line");
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("header line");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().expect("content length");
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).expect("body");
    HttpResponse { status, body }
}

pub fn simple(address: &str, method: &str, path: &str, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(address).expect("connect");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    let mut reader = BufReader::new(stream);
    read_response(&mut reader)
}

pub fn body_text(response: &HttpResponse) -> String {
    String::from_utf8(response.body.clone()).expect("utf-8 body")
}

/// 応答 JSON から "key":"value" の value を取り出す(テスト用の素朴な抽出)。
pub fn json_text_field(text: &str, key: &str) -> Option<String> {
    text.split(&format!("\"{key}\":\""))
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|s| s.to_string())
}

/// 応答 JSON から "key":<integer> を取り出す。
pub fn json_integer_field(text: &str, key: &str) -> Option<i64> {
    let rest = text.split(&format!("\"{key}\":")).nth(1)?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

pub fn put_object(address: &str, bytes: &[u8]) -> String {
    let response = simple(address, "POST", "/v1/objects", bytes);
    assert!(
        response.status == 200 || response.status == 201,
        "put failed: {} {}",
        response.status,
        body_text(&response)
    );
    json_text_field(&body_text(&response), "id").expect("id in response")
}

pub fn put_ref(address: &str, path: &str, target: Option<&str>) {
    let body = match target {
        Some(t) => format!("{{\"target\":\"{t}\"}}"),
        None => "{\"target\":null}".to_string(),
    };
    let response = simple(address, "PUT", &format!("/v1/refs/{path}"), body.as_bytes());
    assert_eq!(response.status, 200, "{}", body_text(&response));
}
