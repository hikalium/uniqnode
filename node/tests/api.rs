//! HTTP API の統合テスト。実プロセスの serve を立て、生の TCP で HTTP/1.1 を話す
//! (本番の呼び出し元 = curl や LLM エージェントが組み立てる形のリクエストで検査する。
//! should/0138)。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

struct Server {
    child: Child,
    address: String,
    dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start_server(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("uniqnode-api-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
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
    Server { child, address, dir }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn read_response(reader: &mut BufReader<TcpStream>) -> HttpResponse {
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

fn request(address: &str, raw: &str, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(address).expect("connect");
    stream.write_all(raw.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    let mut reader = BufReader::new(stream);
    read_response(&mut reader)
}

fn simple(address: &str, method: &str, path: &str, body: &[u8]) -> HttpResponse {
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    request(address, &head, body)
}

fn body_text(response: &HttpResponse) -> String {
    String::from_utf8(response.body.clone()).expect("utf-8 body")
}

#[test]
fn object_and_ref_round_trip_over_http() {
    let server = start_server("roundtrip");
    let address = server.address.as_str();

    let health = simple(address, "GET", "/healthz", b"");
    assert_eq!(health.status, 200);

    // オブジェクト投入(新規 201 → 再投入 200 new:false のべき等)。
    let object_body = br#"{"v":1,"kind":"node","contents":"hello uniqnode"}"#;
    let created = simple(address, "POST", "/v1/objects", object_body);
    assert_eq!(created.status, 201);
    let created_text = body_text(&created);
    assert!(created_text.contains("\"new\":true"), "{created_text}");
    let id = created_text
        .split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("id in response")
        .to_string();

    let duplicated = simple(address, "POST", "/v1/objects", object_body);
    assert_eq!(duplicated.status, 200);
    assert!(body_text(&duplicated).contains("\"new\":false"));

    // 取得はバイト列がそのまま返る。
    let fetched = simple(address, "GET", &format!("/v1/objects/{id}"), b"");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body, object_body);

    // ref の設定と解決。
    let put_ref = simple(
        address,
        "PUT",
        "/v1/refs/notes/hello",
        format!("{{\"target\":\"{id}\"}}").as_bytes(),
    );
    assert_eq!(put_ref.status, 200, "{}", body_text(&put_ref));
    let put_ref_text = body_text(&put_ref);
    let full_name = put_ref_text
        .split("\"name\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("full name")
        .to_string();

    let resolved = simple(address, "GET", &format!("/v1/refs/{full_name}"), b"");
    assert_eq!(resolved.status, 200);
    assert!(body_text(&resolved).contains(&id));

    // tombstone。
    let tombstone = simple(address, "PUT", "/v1/refs/notes/hello", b"{\"target\":null}");
    assert_eq!(tombstone.status, 200);
    let resolved_after = simple(address, "GET", &format!("/v1/refs/{full_name}"), b"");
    assert!(body_text(&resolved_after).contains("\"target\":null"));

    // status に反映されている。
    let status = simple(address, "GET", "/v1/status", b"");
    let status_text = body_text(&status);
    assert!(status_text.contains("\"objects\":1"), "{status_text}");
    assert!(status_text.contains("\"last_seq\":2"), "{status_text}");
}

#[test]
fn not_held_locally_is_404_with_the_open_world_wording() {
    let server = start_server("notfound");
    let missing = format!("s256:{}", "0".repeat(64));
    let response = simple(&server.address, "GET", &format!("/v1/objects/{missing}"), b"");
    assert_eq!(response.status, 404);
    // 「持っていない」というローカルな事実であり、「存在しない」とは言わない(SPEC §10)。
    assert!(body_text(&response).contains("not held locally"));

    let bad_id = simple(&server.address, "GET", "/v1/objects/zzz", b"");
    assert_eq!(bad_id.status, 400);
}

#[test]
fn closure_endpoint_walks_references() {
    let server = start_server("closure");
    let address = server.address.as_str();
    let leaf = simple(address, "POST", "/v1/objects", b"\"leaf\"");
    let leaf_id = body_text(&leaf)
        .split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("id")
        .to_string();
    let edge_body = format!("{{\"v\":1,\"kind\":\"edge\",\"members\":[\"{leaf_id}\"]}}");
    let edge = simple(address, "POST", "/v1/objects", edge_body.as_bytes());
    let edge_id = body_text(&edge)
        .split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("id")
        .to_string();

    let closure = simple(address, "GET", &format!("/v1/closure/{edge_id}"), b"");
    assert_eq!(closure.status, 200);
    let closure_text = body_text(&closure);
    assert!(closure_text.contains(&leaf_id));
    assert!(closure_text.contains(&edge_id));
}

#[test]
fn keep_alive_serves_multiple_requests_on_one_connection() {
    let server = start_server("keepalive");
    let stream = TcpStream::connect(&server.address).expect("connect");
    let mut writer = stream.try_clone().expect("clone");
    let mut reader = BufReader::new(stream);

    writer
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("first request");
    let first = read_response(&mut reader);
    assert_eq!(first.status, 200);

    writer
        .write_all(b"GET /v1/status HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .expect("second request");
    let second = read_response(&mut reader);
    assert_eq!(second.status, 200);
    assert!(body_text(&second).contains("node_id"));
}

#[test]
fn expect_100_continue_is_honored() {
    // curl が大きめの POST で送る Expect: 100-continue に応答できる。
    let server = start_server("expect100");
    let body = b"expect test";
    let head = format!(
        "POST /v1/objects HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut stream = TcpStream::connect(&server.address).expect("connect");
    stream.write_all(head.as_bytes()).expect("head");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    // 100 Continue の行を読んでからボディを送る。
    let mut line = String::new();
    reader.read_line(&mut line).expect("100 line");
    assert!(line.contains("100"), "{line}");
    loop {
        let mut l = String::new();
        reader.read_line(&mut l).expect("headers of 100");
        if l.trim_end_matches(['\r', '\n']).is_empty() {
            break;
        }
    }
    stream.write_all(body).expect("body");
    let response = read_response(&mut reader);
    assert_eq!(response.status, 201);
}
