//! HTTP API の統合テスト。実プロセスの serve を立て、生の TCP で HTTP/1.1 を話す
//! (本番の呼び出し元 = curl や LLM エージェントが組み立てる形のリクエスト。should/0138)。

mod common;
use common::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

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
    let id = json_text_field(&created_text, "id").expect("id in response");

    let duplicated = simple(address, "POST", "/v1/objects", object_body);
    assert_eq!(duplicated.status, 200);
    assert!(body_text(&duplicated).contains("\"new\":false"));

    // 取得はバイト列がそのまま返る。
    let fetched = simple(address, "GET", &format!("/v1/objects/{id}"), b"");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body, object_body);

    // ref の設定と解決。
    put_ref(address, "notes/hello", Some(&id));
    let refs_list = body_text(&simple(address, "GET", "/v1/refs", b""));
    let full_name = json_text_field(&refs_list, "name").expect("full name");
    let resolved = simple(address, "GET", &format!("/v1/refs/{full_name}"), b"");
    assert_eq!(resolved.status, 200);
    assert!(body_text(&resolved).contains(&id));

    // tombstone。
    put_ref(address, "notes/hello", None);
    let resolved_after = simple(address, "GET", &format!("/v1/refs/{full_name}"), b"");
    assert!(body_text(&resolved_after).contains("\"target\":null"));

    // status に反映されている。
    let status = body_text(&simple(address, "GET", "/v1/status", b""));
    assert_eq!(json_integer_field(&status, "objects"), Some(1), "{status}");
    assert_eq!(json_integer_field(&status, "last_seq"), Some(2), "{status}");
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
    let leaf_id = put_object(address, b"\"leaf\"");
    let edge_body = format!("{{\"v\":1,\"kind\":\"edge\",\"members\":[\"{leaf_id}\"]}}");
    let edge_id = put_object(address, edge_body.as_bytes());

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
