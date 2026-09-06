//! HTTP API の統合テスト。実プロセスの serve を立て、生の TCP で HTTP/1.1 を話す
//! (本番の呼び出し元 = curl や LLM エージェントが組み立てる形のリクエスト。should/0138)。

mod common;
use common::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::Command;

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

/// ロックを取れない serve は `listening on` を一度も言わずに終わる。標準出力のこの 1 行は
/// 「出たら要求を受け付ける」という起動スクリプトとの取り決めで、束縛してから開く順だと、
/// ロックを持つ別のプロセスがいるときにこの行を出した後で exit 1 し、行を待つ側が騙される
/// (2026-09-05 に systemd の据え付けで観測。docs/mop/SYSTEMD.md の二重起動の症状)。
#[test]
fn a_serve_that_cannot_take_the_store_lock_never_says_listening_on() {
    let holder = start_server("lock-holder");
    // 2 本目に渡すアドレスは、いま空いているポートを一度束縛して知る。:0 だと、2 本目が
    // 終わった後に「そこで何も受け付けていない」を確かめる相手が分からない。
    let spare = std::net::TcpListener::bind("127.0.0.1:0").expect("bind spare");
    let address = spare.local_addr().expect("local addr").to_string();
    drop(spare);

    let second = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["serve", holder.dir.to_str().expect("utf-8"), &address])
        .output()
        .expect("spawn second serve");
    let stdout = String::from_utf8_lossy(&second.stdout);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        !stdout.contains("listening on"),
        "ロックを取れないのに待ち受けると言った: stdout={stdout:?} stderr={stderr}"
    );
    assert_eq!(
        second.status.code(),
        Some(1),
        "ロックを取れない serve の終了コードは 1(unit は起こし直さない): stderr={stderr}"
    );
    assert!(
        stderr.contains("別プロセスが開いている"),
        "ロックを取れなかった理由を言っていない: {stderr}"
    );
    // 終わった後に、渡したアドレスで何も受け付けていない(束縛したまま残していない)。
    assert!(
        TcpStream::connect(&address).is_err(),
        "2 本目が終わった後も {address} で何かが受け付けている"
    );
    // ロックを持つ 1 本目は影響を受けずに答え続ける。
    let status = simple(&holder.address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200, "{}", body_text(&status));
}

/// 装備の誤り(誤った --rerank の URL)も `listening on` の前に落ちる。装備の表示と断りが
/// 待ち受けの表示より後にあると、行を待つ側が受け付けていない serve を生きていると読む。
#[test]
fn a_serve_with_a_bad_rerank_url_stops_before_it_says_listening_on() {
    let dir = unique_dir("bad-rerank");
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args([
            "serve",
            dir.to_str().expect("utf-8"),
            "127.0.0.1:0",
            "--rerank",
            "https://127.0.0.1:1/v1/rerank",
        ])
        .output()
        .expect("spawn serve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("listening on"),
        "装備できないのに待ち受けると言った: stdout={stdout:?} stderr={stderr}"
    );
    assert_eq!(output.status.code(), Some(2), "引数の誤りの終了コードは 2: stderr={stderr}");
    assert!(
        stderr.contains("uniqnode: rerank:") && stderr.contains("https は未対応"),
        "装備できなかった理由を言っていない: {stderr}"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// GET /v1/collections はコレクションごとの見えの文書数を名前順に返す。数えるのは
/// collections/<c>/<name> の形で target が null でない ref だけである(判断は api.rs の
/// handle_collections の 1 箇所。ref 名の読み方は search::document_ref_parts)。
///
/// 欠陥を戻すとどこで落ちるか(should/0137): tombstone の除外(target の検査)を消すと
/// 最後の段(notes が 1 に減る)が落ちる。collections/ 以外の ref を数えると annotations/
/// の段が落ちる。名前順を崩すと 2 コレクションの段の全文一致が落ちる。
#[test]
fn collections_are_listed_with_their_document_counts() {
    let server = start_server("api-collections");
    let get = || {
        let response = simple(&server.address, "GET", "/v1/collections", b"");
        assert_eq!(response.status, 200, "{}", body_text(&response));
        body_text(&response)
    };
    let put = |collection: &str, name: &str, body: &str| {
        let path = format!("/v1/collections/{collection}/documents/{name}");
        let response = simple(&server.address, "PUT", &path, body.as_bytes());
        assert_eq!(response.status, 200, "{}", body_text(&response));
    };

    // 空のストアは空の配列(不在の言明ではなく、見えに文書が無いという事実)。
    assert_eq!(get(), "{\"collections\":[]}");

    put("specs", "c.md", "# c\n\nthird document\n");
    put("notes", "a.md", "# a\n\nfirst document\n");
    put("notes", "b.md", "# b\n\nsecond document\n");
    let two = "{\"collections\":[{\"documents\":2,\"name\":\"notes\"},\
               {\"documents\":1,\"name\":\"specs\"}]}";
    assert_eq!(get(), two);

    // 同じ文書を入れ直しても ref は 1 本のままで、数は増えない(べき等)。
    put("notes", "a.md", "# a\n\nfirst document\n");
    assert_eq!(get(), two);
    // 改版しても文書は 1 件のまま(ref の張り替え)。
    put("notes", "a.md", "# a\n\nfirst document, revised\n");
    assert_eq!(get(), two);

    // collections/ の外の ref は文書ではない。
    let id = put_object(&server.address, b"{\"kind\":\"note\",\"v\":1}");
    put_ref(&server.address, "annotations/x", Some(&id));
    assert_eq!(get(), two);

    // tombstone は見えに無いので数えない。
    put_ref(&server.address, "collections/notes/a", None);
    assert_eq!(
        get(),
        "{\"collections\":[{\"documents\":1,\"name\":\"notes\"},{\"documents\":1,\"name\":\"specs\"}]}"
    );
}

/// PUT の query `?meta.<key>=<value>` は主の口でも同じに通る(判断は api.rs の put_document
/// と parse_meta_query の 1 箇所で、読み口だけの機能ではない)。写った meta は doc_rev で
/// 読め、name・media は取り込みが決めたまま。形が違う query は 400 で理由を言い、ストアには
/// 何も入らない。`?` だけの空の query は meta 無しと同じ。
/// should/0137: handle の parse_meta_query を `Ok(Vec::new())` に置き換えると meta の全文一致と
/// 400 の段が落ちる。
#[test]
fn the_meta_query_lands_in_the_doc_rev_through_the_main_door_too() {
    let server = start_server("api-put-meta");
    let put = |path: &str| simple(&server.address, "PUT", path, b"# m\n\nmeta document\n");

    let written = put("/v1/collections/notes/documents/m.md?meta.agent=agent-7&meta.task=t%201");
    assert_eq!(written.status, 200, "{}", body_text(&written));
    let doc_rev = json_text_field(&body_text(&written), "doc_rev").expect("doc_rev");
    let object = simple(&server.address, "GET", &format!("/v1/objects/{doc_rev}"), b"");
    let text = body_text(&object);
    assert!(
        text.contains("\"meta\":{\"agent\":\"agent-7\",\"media\":\"markdown\",\"name\":\"m\",\"task\":\"t 1\"}"),
        "{text}"
    );

    // 同じ本文の再 PUT は meta が違っても no-op(同一内容の判定は source と chunks 列で、
    // meta は見ない。URL からの取り込みの fetched_at と同じ扱い)。doc_rev も meta も最初の
    // まま。
    let again = put("/v1/collections/notes/documents/m.md?meta.agent=agent-8");
    assert_eq!(again.status, 200, "{}", body_text(&again));
    assert_eq!(json_text_field(&body_text(&again), "doc_rev").expect("doc_rev"), doc_rev);
    assert!(body_text(&again).contains("\"ref_updated\":false"), "{}", body_text(&again));
    // `?` だけの空の query は meta 無しと同じ。
    let empty = simple(
        &server.address,
        "PUT",
        "/v1/collections/notes/documents/n.md?",
        b"# n\n\nsecond document\n",
    );
    assert_eq!(empty.status, 200, "{}", body_text(&empty));
    let n_rev = json_text_field(&body_text(&empty), "doc_rev").expect("doc_rev");
    let n_text = body_text(&simple(&server.address, "GET", &format!("/v1/objects/{n_rev}"), b""));
    assert!(n_text.contains("\"meta\":{\"media\":\"markdown\",\"name\":\"n\"}"), "{n_text}");

    let refused = put("/v1/collections/notes/documents/o.md?meta.agent=a&meta.agent=b");
    assert_eq!(refused.status, 400, "{}", body_text(&refused));
    assert_eq!(body_text(&refused), "{\"error\":\"meta.agent が 2 度ある\"}");
    let collections = simple(&server.address, "GET", "/v1/collections", b"");
    assert_eq!(
        body_text(&collections),
        "{\"collections\":[{\"documents\":2,\"name\":\"notes\"}]}",
        "断った PUT は入らない"
    );
}
