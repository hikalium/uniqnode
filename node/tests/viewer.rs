//! RAG ビューワの統合テスト(VIEWER (uuid:4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847))。
//! 実プロセスの serve と viewer を起こし、ブラウザが組み立てるのと同じ生 HTTP/1.1 で
//! 当てる(should/0138)。資材はソースとは別ファイル(should/0112)。

mod common;
use common::*;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// 日本語文書(検索の統合テストと共用)。
const SEARCH_JA: &str = include_str!("assets/search_ja.md");

/// 起こしたビューワ(Drop で確実に落とす。serve と違って正常終了の口を持たないので
/// kill で終える)。
struct ViewerProcess {
    child: Child,
    address: String,
}

impl Drop for ViewerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// ビューワを :0 で起こし、標準出力の 1 行から実際の待ち受け先を読む(serve と同じ
/// 取り決め)。
fn start_viewer(data_dir: &str, serve_url: &str) -> ViewerProcess {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["viewer", data_dir, "127.0.0.1:0", "--serve-url", serve_url, "--no-log"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn viewer");
    let stdout = child.stdout.take().expect("stdout");
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).expect("listening line");
    let address = line
        .trim()
        .strip_prefix("listening on ")
        .expect("listening line")
        .to_string();
    ViewerProcess { child, address }
}

/// 完了条件: ブラウザが頁を受け取り、その頁が呼ぶ口だけで検索から全文まで辿れる。
#[test]
fn the_page_and_the_endpoints_it_uses_carry_a_search_from_query_to_full_text() {
    let server = start_server("viewer-serve");
    let path = "/v1/collections/notes/documents/search_ja.md";
    assert_eq!(simple(&server.address, "PUT", path, SEARCH_JA.as_bytes()).status, 200);
    let serve_url = format!("http://{}", server.address);
    let viewer = start_viewer(server.dir.to_str().expect("utf-8"), &serve_url);

    // (1) 頁そのもの。1 枚の HTML が text/html で返る。
    let page = simple(&viewer.address, "GET", "/", b"");
    assert_eq!(page.status, 200);
    let html = body_text(&page);
    assert!(html.starts_with("<!DOCTYPE html>"), "{}", &html[..80.min(html.len())]);
    assert!(html.contains("uniqnode viewer"), "題が入っている");
    // 頁が写しを引く道は、この頁の中に字面で入っている(ブラウザがこれを組み立てる)。
    assert!(html.contains("`/v1/objects/${id}/rendition`"), "写しの目録を引く道");
    assert!(html.contains("`/v1/objects/${id}/rendition/thumb`"), "サムネを引く道");

    // (2) 頁が起動時に読む二つ(素性とコレクションの一覧)。
    let status = body_text(&simple(&viewer.address, "GET", "/v1/status", b""));
    assert!(status.contains("\"node_id\""), "{status}");
    let refs = body_text(&simple(&viewer.address, "GET", "/v1/refs", b""));
    assert!(refs.contains("collections/notes/search_ja"), "{refs}");

    // (3) 検索。要求も応答も serve のものがそのまま通る。
    let body = "{\"query\":\"世代の整合\",\"top_k\":5}";
    let found = simple(&viewer.address, "POST", "/v1/search", body.as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    let text = body_text(&found);
    assert!(text.contains("\"document\":\"search_ja\""), "{text}");
    assert!(text.contains("\"score_semantics\""), "{text}");

    // (4) 全文。頁の「全文」ボタンが引くのと同じ口。
    let id = json_text_field(&text, "id").expect("チャンク ID");
    let chunk = body_text(&simple(&viewer.address, "GET", &format!("/v1/objects/{id}"), b""));
    assert!(chunk.contains("世代の整合はオブジェクト数と署名者ごとの最終列番号"), "{chunk}");
    let citation =
        body_text(&simple(&viewer.address, "GET", &format!("/v1/objects/{id}/citation"), b""));
    assert!(citation.contains("\"document\":\"search_ja\""), "{citation}");
}

/// 通すのは頁が使う口だけである(ビューワの口がストア API の素通しにならない)。
/// 書き込みの口も、逆引きも、同期も、ここからは届かない。
#[test]
fn the_viewer_is_not_an_open_relay_to_the_store_api() {
    let server = start_server("viewer-relay");
    let serve_url = format!("http://{}", server.address);
    let viewer = start_viewer(server.dir.to_str().expect("utf-8"), &serve_url);

    // 書き込みの口(直接 serve へ投げれば通るもの)は、ビューワからは 404。
    let write = simple(&viewer.address, "POST", "/v1/objects", b"{\"v\":1}");
    assert_eq!(write.status, 404, "{}", body_text(&write));
    let ingest = simple(
        &viewer.address,
        "PUT",
        "/v1/collections/notes/documents/x.md",
        b"# x\n",
    );
    assert_eq!(ingest.status, 404, "{}", body_text(&ingest));
    for path in ["/v1/sync", "/v1/query", "/v1/admin/shutdown"] {
        let response = simple(&viewer.address, "POST", path, b"{}");
        assert_eq!(response.status, 404, "{path}: {}", body_text(&response));
    }
    for path in ["/v1/pins", "/v1/peers", "/v1/health/events", "/v1/replication/signers"] {
        let response = simple(&viewer.address, "GET", path, b"");
        assert_eq!(response.status, 404, "{path}: {}", body_text(&response));
    }
    // 断り方は、何が通るのかを言う(読んだ者が次の手を打てる形。must/0022)。
    let refused = body_text(&simple(&viewer.address, "GET", "/v1/peers", b""));
    assert!(refused.contains("/v1/search"), "{refused}");

    // serve 側は生きている(断ったのはビューワであって、serve が壊れたのではない)。
    assert_eq!(simple(&server.address, "GET", "/v1/status", b"").status, 200);
}

/// 写しの口(目録と、許可表にある 4 つの別名)は転送し、それ以外の別名はビューワが断る。
///
/// 断ったのがビューワか serve かは本文で見分ける(ビューワの断り文には「ビューワの口では
/// ない」が入る)。この試験は写しが実際に作れるかには依らない。ここで見たいのは
/// 「頁が引く道だけが通ること」であって、poppler の有無や、その ID の写しが作れるか
/// どうかは serve 側の試験の持ち物である。
#[test]
fn the_viewer_forwards_the_rendition_paths_and_refuses_unknown_aliases() {
    let server = start_server("viewer-rendition");
    let serve_url = format!("http://{}", server.address);
    let viewer = start_viewer(server.dir.to_str().expect("utf-8"), &serve_url);
    let id = format!("s256:{}", "ab".repeat(32));
    let refusal = "ビューワの口ではない";

    // 通す道(目録と 4 つの別名)。serve まで届いていることを、断り文が無いことで見る。
    let mut forwarded = vec![format!("/v1/objects/{id}/rendition")];
    for alias in ["source", "thumb", "page", "pagepdf"] {
        forwarded.push(format!("/v1/objects/{id}/rendition/{alias}"));
    }
    for path in &forwarded {
        let response = simple(&viewer.address, "GET", path, b"");
        let text = body_text(&response);
        assert!(!text.contains(refusal), "{path} がビューワに止められた: {text}");
    }

    // 通さない道。許可表に無い別名・寸法指定・ページ番号らしき細工は、serve へ届く前に
    // ビューワが断る(口の広さがそのまま容量の広さになるのを防ぐ)。
    for tail in [
        "rendition/w1200",
        "rendition/1414",
        "rendition/thumb/1414",
        "rendition/",
        "renditions",
        "rendition/thumb/../citation",
    ] {
        let path = format!("/v1/objects/{id}/{tail}");
        let response = simple(&viewer.address, "GET", &path, b"");
        assert_eq!(response.status, 404, "{path}");
        assert!(body_text(&response).contains(refusal), "{path} を止めたのがビューワでない");
    }
    // ID の形が違えば、別名が正しくても通らない。
    let bad = simple(&viewer.address, "GET", "/v1/objects/s256:zz/rendition/thumb", b"");
    assert_eq!(bad.status, 404);
    assert!(body_text(&bad).contains(refusal));
}

/// 写しの口を持たない serve(旧版)に繋いだときも、頁と検索は従来どおり動く。頁は写しの
/// 目録が 404 なら何も足さないので(単体試験 a_serve_without_the_catalog_leaves_the_page_as_it_was)、
/// ここでは土台のほう — 目録が無くても頁と検索が壊れないこと — を見る。
#[test]
fn a_serve_without_renditions_still_answers_the_page_and_the_search() {
    let server = start_server("viewer-old-serve");
    let path = "/v1/collections/notes/documents/search_ja.md";
    assert_eq!(simple(&server.address, "PUT", path, SEARCH_JA.as_bytes()).status, 200);
    let serve_url = format!("http://{}", server.address);
    let viewer = start_viewer(server.dir.to_str().expect("utf-8"), &serve_url);

    let body = "{\"query\":\"世代の整合\",\"top_k\":5}";
    let found = simple(&viewer.address, "POST", "/v1/search", body.as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    let text = body_text(&found);
    let id = json_text_field(&text, "id").expect("チャンク ID");

    // markdown 由来のチャンクには紙面が無い。目録の口があってもなくても、頁はこの件に
    // 何も足さない(出せないものの席を作らない)。
    let catalog = simple(&viewer.address, "GET", &format!("/v1/objects/{id}/rendition"), b"");
    assert!(catalog.status == 404 || catalog.status == 200, "{}", body_text(&catalog));
    if catalog.status == 200 {
        let catalog = body_text(&catalog);
        assert!(!catalog.contains("\"media\":\"pdf\""), "markdown の件が PDF を名乗った: {catalog}");
    }
    // 頁と全文はどちらの serve でも同じように出る。
    assert_eq!(simple(&viewer.address, "GET", "/", b"").status, 200);
    let chunk = body_text(&simple(&viewer.address, "GET", &format!("/v1/objects/{id}"), b""));
    assert!(chunk.contains("世代の整合"), "{chunk}");
}

/// 転送先が死んでいても頁は出る。届かないことは、原因と起こし方を添えて言う。
#[test]
fn the_page_still_opens_when_the_serve_is_down() {
    let dir = unique_dir("viewer-down");
    std::fs::create_dir_all(&dir).expect("mkdir");
    // 何も待ち受けていない宛先。
    let viewer = start_viewer(dir.to_str().expect("utf-8"), "http://127.0.0.1:1");

    assert_eq!(simple(&viewer.address, "GET", "/", b"").status, 200, "頁は出る");
    let status = simple(&viewer.address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 502);
    let text = body_text(&status);
    assert!(text.contains("走っている serve に届かない"), "{text}");
    assert!(text.contains("uniqnode serve"), "起こし方を添える: {text}");

    drop(viewer);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
