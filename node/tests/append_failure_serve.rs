//! ストアの書き込みに失敗した後の扱い(APPEND_FAILURE (docs/plan/APPEND_FAILURE.md) の S1a)を、
//! 実プロセスの serve・mcp・CLI に通す試験。失敗は環境変数 UNIQNODE_APPEND_FAULT(debug ビルド
//! だけが読む。node/src/fault.rs)で注入する。ポートは一時のもの、ストアは一時ディレクトリだけを
//! 使う。ストアの層の試験は node/tests/append_failure.rs。

mod common;
use common::*;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

/// 次の pack への追記で 5 バイトだけ書いて ENOSPC(切り詰めは成功する。kind は no_space)。
const NOSPACE_ON_NEXT_PACK: &str = "torn:5@pack:1";
/// 次の pack への追記で 1 バイトも書かずに EIO(kind は io)。
const EIO_ON_NEXT_PACK: &str = "before@pack:1";

/// テスト資材の最小 PDF(3 ページ)。
const THREE_PAGE_PDF: &[u8] = include_bytes!("assets/three_pages.pdf");

fn status_text(address: &str) -> String {
    let response = simple(address, "GET", "/v1/status", b"");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    body_text(&response)
}

/// 書けない状態の 503 であること、本文が `error` に案内を含み `writes_disabled` に kind を
/// 持つことを確かめ、本文を返す。
fn assert_writes_disabled(response: &HttpResponse, kind: &str, context: &str) -> String {
    let body = body_text(response);
    assert_eq!(response.status, 503, "{context}: {body}");
    assert!(
        body.contains(&format!("\"error\":\"writes disabled ({kind}): ")),
        "{context}: error が書けない状態の文でない: {body}"
    );
    assert!(body.contains(&format!("\"kind\":\"{kind}\"")), "{context}: kind が無い: {body}");
    assert!(body.contains("\"writes_disabled\":{"), "{context}: writes_disabled の欄が無い: {body}");
    let guidance = match kind {
        "no_space" => "空きを作ってから serve を再起動する",
        _ => "ホストを再起動する",
    };
    assert!(body.contains(guidance), "{context}: 案内が無い: {body}");
    body
}

/// 書けない状態の serve(1 回目の put を失敗させた)を起こし、失敗した put の応答を返す。
fn disabled_server(name: &str, fault: &str) -> (Server, HttpResponse) {
    let server = start_server_with_env(name, &[("UNIQNODE_APPEND_FAULT", fault)]);
    let failed = simple(&server.address, "POST", "/v1/objects", b"\"the first write fails\"");
    (server, failed)
}

/// 失敗した要求そのものが 503 と案内を受け取り、以後の書き込み(admin/gc を含む)も 503 で
/// 断られ、読み出しと検索は続き、/v1/status が kind を言う。応答済みのものは読める。
#[test]
fn the_failing_request_and_every_later_write_get_503_while_reads_continue() {
    let server = start_server_with_env(
        "af-serve-nospace",
        &[("UNIQNODE_APPEND_FAULT", "torn:5@pack:2")],
    );
    let address = server.address.clone();
    let kept = put_object(&address, b"\"acknowledged before the failure\"");
    assert!(status_text(&address).contains("\"writes_disabled\":null"));

    let failed = simple(&address, "POST", "/v1/objects", b"\"the failing write\"");
    let body = assert_writes_disabled(&failed, "no_space", "失敗した要求そのもの");
    assert!(body.contains("\"op\":\"write\""), "{body}");

    // 以後の書き込みの入口は全部 503(既に在るものの put も、ref も、gc も、dry-run の gc も)。
    let later = simple(&address, "POST", "/v1/objects", b"\"a later write\"");
    assert_writes_disabled(&later, "no_space", "後の put");
    let existing = simple(&address, "POST", "/v1/objects", b"\"acknowledged before the failure\"");
    assert_writes_disabled(&existing, "no_space", "既に在るものの put");
    let reference = simple(
        &address,
        "PUT",
        "/v1/refs/notes/after",
        format!("{{\"target\":\"{kept}\"}}").as_bytes(),
    );
    assert_writes_disabled(&reference, "no_space", "ref");
    let document = simple(
        &address,
        "PUT",
        "/v1/collections/notes/documents/memo.md",
        b"# memo\n\nwritten while disabled\n",
    );
    assert_writes_disabled(&document, "no_space", "文書の PUT");
    let gc = simple(&address, "POST", "/v1/admin/gc", b"");
    assert_writes_disabled(&gc, "no_space", "admin/gc");
    let dry_run = simple(&address, "POST", "/v1/admin/gc", b"{\"dry_run\":true}");
    assert_writes_disabled(&dry_run, "no_space", "dry-run の admin/gc");

    // 読み出しと検索は続く。
    let read = simple(&address, "GET", &format!("/v1/objects/{kept}"), b"");
    assert_eq!(read.status, 200);
    assert_eq!(read.body, b"\"acknowledged before the failure\"");
    let search = simple(&address, "POST", "/v1/search", b"{\"query\":\"anything\"}");
    assert_eq!(search.status, 200, "{}", body_text(&search));

    let status = status_text(&address);
    assert!(status.contains("\"writes_disabled\":{"), "{status}");
    assert!(status.contains("\"kind\":\"no_space\""), "{status}");
    assert!(status.contains("\"op\":\"write\""), "{status}");
}

/// EIO の失敗は io で、案内はホストの再起動を言う(no_space との区別が本文に出る)。
#[test]
fn an_io_failure_is_reported_as_io_with_its_own_guidance() {
    let (server, failed) = disabled_server("af-serve-io", EIO_ON_NEXT_PACK);
    let body = assert_writes_disabled(&failed, "io", "EIO の put");
    assert!(!body.contains("no_space"), "{body}");
    assert!(status_text(&server.address).contains("\"kind\":\"io\""));
}

/// POST /v1/sync は、取り込むものが無くても(相手が空でも)書けない間は 503 で断る。
#[test]
fn sync_is_refused_with_503_even_when_there_is_nothing_to_ingest() {
    let (server, failed) = disabled_server("af-serve-sync", EIO_ON_NEXT_PACK);
    assert_writes_disabled(&failed, "io", "最初の put");
    let empty_peer = start_server("af-serve-sync-peer");
    let body = format!("{{\"peer\":\"{}\"}}", empty_peer.address);
    let sync = simple(&server.address, "POST", "/v1/sync", body.as_bytes());
    assert_writes_disabled(&sync, "io", "差分の無い sync");
}

fn query_body(target: &str, peer: &str, wait: bool) -> String {
    format!(
        "{{\"kind\":\"object\",\"target\":\"{target}\",\"budget_ms\":5000,\
         \"scope\":[\"{peer}\"],\"wait\":{wait}}}"
    )
}

/// 書けない間、手元に無いオブジェクトを取りに行く query は 503 で断り、手元に在るものを
/// 答える query は続ける(保存したと偽らない)。
#[test]
fn a_query_for_a_missing_object_is_refused_while_one_for_a_local_object_is_answered() {
    let server = start_server_with_env(
        "af-serve-query-refused",
        &[("UNIQNODE_APPEND_FAULT", "before@pack:2")],
    );
    let local = put_object(&server.address, b"\"held locally\"");
    let failed = simple(&server.address, "POST", "/v1/objects", b"\"fails\"");
    assert_writes_disabled(&failed, "io", "最初の失敗");
    let holder = start_server("af-serve-query-refused-holder");
    let remote = put_object(&holder.address, b"\"only the holder has this\"");

    let refused = simple(
        &server.address,
        "POST",
        "/v1/query",
        query_body(&remote, &holder.address, true).as_bytes(),
    );
    let body = assert_writes_disabled(&refused, "io", "手元に無いものの query");
    assert!(!body.contains("\"stored\":true"), "{body}");

    let answered = simple(
        &server.address,
        "POST",
        "/v1/query",
        query_body(&local, &holder.address, true).as_bytes(),
    );
    assert_eq!(answered.status, 200, "{}", body_text(&answered));
    assert!(body_text(&answered).contains("\"outcome\":\"found\""), "{}", body_text(&answered));
}

/// query 自身の保存が最初の失敗になった形: wait:true は 503 と案内を返す。
#[test]
fn a_query_whose_own_store_fails_answers_503_when_waiting() {
    let (server, holder, remote) = query_fixture("af-serve-query-wait");
    let response = simple(
        &server.address,
        "POST",
        "/v1/query",
        query_body(&remote, &holder.address, true).as_bytes(),
    );
    let body = assert_writes_disabled(&response, "io", "wait:true の query");
    assert!(!body.contains("\"stored\":true"), "{body}");
    assert!(status_text(&server.address).contains("\"kind\":\"io\""));
}

/// 同じ形の wait:false: 後の状態の取得(GET /v1/queries/{id})が 503 と案内を返す。
#[test]
fn a_query_whose_own_store_fails_reports_503_on_the_later_status_read() {
    let (server, holder, remote) = query_fixture("af-serve-query-nowait");
    let started = simple(
        &server.address,
        "POST",
        "/v1/query",
        query_body(&remote, &holder.address, false).as_bytes(),
    );
    // 即返りの時点で決着していれば、それも 503 である。
    if started.status == 503 {
        assert_writes_disabled(&started, "io", "wait:false の即返り");
        return;
    }
    assert_eq!(started.status, 200, "{}", body_text(&started));
    let query_id = json_text_field(&body_text(&started), "query_id").expect("query_id");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let observed = simple(&server.address, "GET", &format!("/v1/queries/{query_id}"), b"");
        if observed.status == 503 {
            let body = assert_writes_disabled(&observed, "io", "後の状態の取得");
            assert!(!body.contains("\"stored\":true"), "{body}");
            return;
        }
        assert_eq!(observed.status, 200, "{}", body_text(&observed));
        let text = body_text(&observed);
        assert!(text.contains("\"outcome\":\"running\""), "書けないのに決着した: {text}");
        assert!(std::time::Instant::now() < deadline, "決着しない: {text}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// 最初の pack 追記で EIO になる serve と、オブジェクトを 1 つ持つ相手。
fn query_fixture(name: &str) -> (Server, Server, String) {
    let server = start_server_with_env(name, &[("UNIQNODE_APPEND_FAULT", EIO_ON_NEXT_PACK)]);
    let holder = start_server(&format!("{name}-holder"));
    let remote = put_object(&holder.address, b"\"fetched and then refused\"");
    (server, holder, remote)
}

/// 書けない間も、まだ作っていない写しは作って返す(保存しない。オブジェクト数が増えず、
/// カタログの席は absent のまま)。
#[test]
fn a_rendition_is_returned_without_storing_while_writes_are_disabled() {
    require_poppler();
    let dir = unique_dir("af-serve-rendition");
    let mut first = start_server_at(dir.clone());
    first.remove_dir_on_drop = false;
    let path = "/v1/collections/specs/documents/three_pages.pdf";
    let put = simple(&first.address, "PUT", path, THREE_PAGE_PDF);
    assert_eq!(put.status, 200, "{}", body_text(&put));
    drop(first);

    let server = start_server_at_with_env(dir, &[("UNIQNODE_APPEND_FAULT", EIO_ON_NEXT_PACK)]);
    let failed = simple(&server.address, "POST", "/v1/objects", b"\"fails\"");
    assert_writes_disabled(&failed, "io", "最初の put");
    let search = simple(
        &server.address,
        "POST",
        "/v1/search",
        b"{\"query\":\"Page two\",\"top_k\":1,\"include_low_information\":true}",
    );
    assert_eq!(search.status, 200, "{}", body_text(&search));
    let chunk = json_text_field(&body_text(&search), "id").expect("チャンク ID");
    let objects_before = json_integer_field(&status_text(&server.address), "objects");

    let thumb = simple(&server.address, "GET", &format!("/v1/objects/{chunk}/rendition/thumb"), b"");
    assert_eq!(thumb.status, 200, "{}", String::from_utf8_lossy(&thumb.body));
    assert_eq!(thumb.content_type, "image/jpeg");
    assert_eq!(&thumb.body[..2], &[0xff, 0xd8], "JPEG の先頭ではない");
    assert_eq!(json_integer_field(&status_text(&server.address), "objects"), objects_before);
    let catalog = simple(&server.address, "GET", &format!("/v1/objects/{chunk}/rendition"), b"");
    let text = body_text(&catalog);
    assert!(
        text.contains("{\"alias\":\"thumb\"") && text.contains("\"state\":\"absent\""),
        "写しは保存されないので absent のまま: {text}"
    );
}

/// 転送する形の mcp の add_document は、ツールの失敗(isError)の文に書けない状態の理由と
/// 案内を載せる(serve の `error` をそのまま写す)。
#[test]
fn the_mcp_error_text_carries_the_reason_and_guidance() {
    let (server, failed) = disabled_server("af-serve-mcp", NOSPACE_ON_NEXT_PACK);
    assert_writes_disabled(&failed, "no_space", "最初の put");
    let serve_url = format!("http://{}", server.address);
    let mut child = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["mcp", server.dir.to_str().expect("utf-8"), "--serve-url", &serve_url])
        .args(["--writable", "notes"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let request = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\
                   \"add_document\",\"arguments\":{\"collection\":\"notes\",\"name\":\"memo\",\
                   \"text\":\"# memo\\n\\nwritten while disabled\"}}}\n";
    stdin.write_all(request.as_bytes()).expect("write request");
    stdin.flush().expect("flush");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    drop(stdin);
    let _ = child.wait();
    let mut rest = String::new();
    let _ = stdout.read_to_string(&mut rest);
    assert!(line.contains("\"isError\":true"), "{line}");
    assert!(line.contains("503"), "{line}");
    assert!(line.contains("writes disabled (no_space): "), "{line}");
    assert!(line.contains("空きを作ってから serve を再起動する"), "{line}");
}

fn uniqnode_with(arguments: &[&str], envs: &[(&str, &str)], stdin: &[u8]) -> (Option<i32>, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_uniqnode"));
    command.args(arguments).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn uniqnode");
    child.stdin.take().expect("stdin").write_all(stdin).expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// CLI の書き込み(put と、差分のある sync)は、書けない状態の理由と案内を言って終了コード 1 で
/// 終わる。
#[test]
fn the_cli_exits_1_with_the_reason_when_a_write_fails() {
    let dir = unique_dir("af-cli");
    let dir_text = dir.to_str().expect("utf-8");
    let (code, _, stderr) = uniqnode_with(
        &["put", dir_text],
        &[("UNIQNODE_APPEND_FAULT", NOSPACE_ON_NEXT_PACK)],
        b"\"cli write\"",
    );
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("writes disabled (no_space): "), "{stderr}");
    assert!(stderr.contains("空きを作ってから serve を再起動する"), "{stderr}");

    let peer = start_server("af-cli-peer");
    // 複製は ref の記録とその指すオブジェクトを運ぶ(ref の無いオブジェクトは差分にならない)。
    let synced = put_object(&peer.address, b"\"to be synced\"");
    put_ref(&peer.address, "notes/synced", Some(&synced));
    let (code, _, stderr) = uniqnode_with(
        &["sync", dir_text, &peer.address],
        &[("UNIQNODE_APPEND_FAULT", EIO_ON_NEXT_PACK)],
        b"",
    );
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("writes disabled (io): "), "{stderr}");
    assert!(stderr.contains("ホストを再起動する"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
