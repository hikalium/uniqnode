//! ページの写し(node/src/rendition.rs)の統合テスト。カタログと実体の 2 つの口を、
//! 実プロセスの serve に生 HTTP/1.1 で当てて確かめる(should/0138)。資材はソースとは
//! 別ファイル(should/0112)。
//!
//! 外部コマンド(pdftotext と poppler 一式)が要るテストは、無い環境で黙って飛ばさず、
//! 導入手順を示して失敗する(common::require_poppler。docs/design/TESTING.md)。

mod common;
use common::*;

/// テスト資材の最小 PDF(3 ページ。ingest・search の統合テストと共用)。
const THREE_PAGE_PDF: &[u8] = include_bytes!("assets/three_pages.pdf");
/// 日本語の markdown(PDF でない原本の側)。
const SEARCH_JA: &str = include_str!("assets/search_ja.md");

fn put_document(address: &str, collection: &str, name: &str, body: &[u8]) {
    let path = format!("/v1/collections/{collection}/documents/{name}");
    let response = simple(address, "PUT", &path, body);
    assert_eq!(response.status, 200, "{}", body_text(&response));
}

/// 検索で 1 件だけ引いて、そのチャンク ID を返す。写しの鍵は取り込み済みチャンクの ID で
/// あり、呼び手は「どの PDF の何ページか」を知らなくてよい(それがこの口の要点である)。
/// PDF の紙面は語数が少なく低情報と判定されるので、探すときは戻してもらう。
fn chunk_id(address: &str, query: &str) -> String {
    let body = format!(
        "{{\"query\":\"{query}\",\"top_k\":1,\"include_low_information\":true}}"
    );
    let response = simple(address, "POST", "/v1/search", body.as_bytes());
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let text = body_text(&response);
    json_text_field(&text, "id").unwrap_or_else(|| panic!("{query} が引けない: {text}"))
}

fn catalog(address: &str, chunk: &str) -> HttpResponse {
    simple(address, "GET", &format!("/v1/objects/{chunk}/rendition"), b"")
}

fn rendition(address: &str, chunk: &str, alias: &str) -> HttpResponse {
    simple(address, "GET", &format!("/v1/objects/{chunk}/rendition/{alias}"), b"")
}

/// カタログの席 1 つを切り出す(応答の配列から、その別名のオブジェクト 1 個ぶん)。
fn view_of(body: &str, alias: &str) -> String {
    let start = body
        .find(&format!("{{\"alias\":\"{alias}\""))
        .unwrap_or_else(|| panic!("{alias} の席が無い: {body}"));
    let end = start
        + body[start..].find('}').unwrap_or_else(|| panic!("席が閉じていない: {body}"));
    body[start..=end].to_string()
}

/// ストアのオブジェクト数(写しが増えたか・増えなかったかを数える)。
fn objects(address: &str) -> i64 {
    let text = body_text(&simple(address, "GET", "/v1/status", b""));
    json_integer_field(&text, "objects").unwrap_or_else(|| panic!("objects が無い: {text}"))
}

/// PDF 由来のチャンクは 4 つの席を持ち、原本だけが既に在る。カタログは何も作らない。
#[test]
fn a_pdf_chunk_offers_source_thumb_page_and_pagepdf() {
    require_poppler();
    let server = start_server("rendition-catalog");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    let chunk = chunk_id(&server.address, "Page two");

    let before = objects(&server.address);
    let response = catalog(&server.address, &chunk);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response.content_type, "application/json");
    let body = body_text(&response);
    // 鍵(どの原本の何ページか)と出典が載る。出典の形は検索応答と同じ 1 箇所から出る。
    assert!(body.contains("\"media\":\"pdf\""), "{body}");
    assert!(body.contains("\"page\":2"), "{body}");
    assert!(body.contains("\"source\":\"s256:"), "{body}");
    assert!(body.contains("\"collection\":\"specs\""), "{body}");
    assert!(body.contains("\"document\":\"three_pages\""), "{body}");

    // 席は 4 つ。期待値は線の上の文字列をリテラルで置く(should/0137)。
    assert_eq!(body.matches("\"alias\":").count(), 4, "席は 4 つのはず: {body}");
    assert_eq!(
        view_of(&body, "source"),
        format!(
            "{{\"alias\":\"source\",\"content_type\":\"application/pdf\",\
             \"state\":\"stored\",\"url\":\"/v1/objects/{chunk}/rendition/source\"}}"
        )
    );
    assert_eq!(
        view_of(&body, "thumb"),
        format!(
            "{{\"alias\":\"thumb\",\"content_type\":\"image/jpeg\",\
             \"state\":\"absent\",\"url\":\"/v1/objects/{chunk}/rendition/thumb\"}}"
        )
    );
    assert_eq!(
        view_of(&body, "page"),
        format!(
            "{{\"alias\":\"page\",\"content_type\":\"image/png\",\
             \"state\":\"absent\",\"url\":\"/v1/objects/{chunk}/rendition/page\"}}"
        )
    );
    // 単ページ PDF は再描画であって字形の写しではない、という但し書きが席に付く。
    assert_eq!(
        view_of(&body, "pagepdf"),
        format!(
            "{{\"alias\":\"pagepdf\",\"content_type\":\"application/pdf\",\
             \"note\":\"再描画であり、字形の写しは完全ではない。\
             正典の本文は取り込み済みチャンクである\",\
             \"state\":\"absent\",\"url\":\"/v1/objects/{chunk}/rendition/pagepdf\"}}"
        )
    );
    assert_eq!(objects(&server.address), before, "カタログは何も作らない");
}

/// 原本の口は、取り込んだバイト列そのものを PDF として返す。オブジェクトは増えない
/// (恒等レシピは何も描かない)。
#[test]
fn the_source_view_returns_the_original_bytes_without_growing_the_store() {
    require_pdftotext();
    let server = start_server("rendition-source");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    let chunk = chunk_id(&server.address, "Page two");

    let before = objects(&server.address);
    let response = rendition(&server.address, &chunk, "source");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response.content_type, "application/pdf");
    assert_eq!(response.body, THREE_PAGE_PDF, "原本のバイト列そのものを返すべき");
    assert_eq!(objects(&server.address), before, "原本を配ってもオブジェクトは増えない");
}

/// 写しは一度だけ作られ、二度目は同じバイト列が作り直さずに返る。作ったあとはカタログの
/// state が absent から stored に変わり、頼んでいない席は absent のままである。
#[test]
fn a_thumbnail_is_generated_once_and_then_reused() {
    require_poppler();
    let server = start_server("rendition-thumb");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    let chunk = chunk_id(&server.address, "Page two");

    let before = objects(&server.address);
    let first = rendition(&server.address, &chunk, "thumb");
    assert_eq!(first.status, 200, "{}", body_text(&first));
    assert_eq!(first.content_type, "image/jpeg");
    assert_eq!(&first.body[..2], &[0xff, 0xd8], "JPEG の先頭ではない");
    assert_eq!(objects(&server.address), before + 1, "写しが 1 個増える");

    let second = rendition(&server.address, &chunk, "thumb");
    assert_eq!(second.status, 200, "{}", body_text(&second));
    assert_eq!(second.body, first.body, "2 度目は同じ写しが返るべき");
    assert_eq!(objects(&server.address), before + 1, "2 度目は作り直さない(冪等)");

    let body = body_text(&catalog(&server.address, &chunk));
    assert_eq!(
        view_of(&body, "thumb"),
        format!(
            "{{\"alias\":\"thumb\",\"content_type\":\"image/jpeg\",\
             \"state\":\"stored\",\"url\":\"/v1/objects/{chunk}/rendition/thumb\"}}"
        ),
        "作ったあとは stored になるべき"
    );
    assert!(
        view_of(&body, "page").contains("\"state\":\"absent\""),
        "頼んでいない席は absent のまま: {body}"
    );
}

/// PDF でないチャンクには原本の席しか作らない(出せないものの席を作らない)。ページの
/// 写しを頼めば、理由を言って断る。原本そのものは配れる。
#[test]
fn a_markdown_chunk_offers_only_its_source() {
    let server = start_server("rendition-markdown");
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    let chunk = chunk_id(&server.address, "世代の整合");

    let body = body_text(&catalog(&server.address, &chunk));
    assert!(body.contains("\"media\":\"markdown\""), "{body}");
    assert!(body.contains("\"page\":null"), "PDF でないチャンクは紙面を持たない: {body}");
    assert_eq!(body.matches("\"alias\":").count(), 1, "席は原本だけのはず: {body}");
    // 原本が PDF でないなら、そうは名乗らない(型は中身で決まる)。
    assert_eq!(
        view_of(&body, "source"),
        format!(
            "{{\"alias\":\"source\",\"content_type\":\"text/plain; charset=utf-8\",\
             \"state\":\"stored\",\"url\":\"/v1/objects/{chunk}/rendition/source\"}}"
        )
    );

    let refused = rendition(&server.address, &chunk, "page");
    assert_eq!(refused.status, 400, "{}", body_text(&refused));
    let text = body_text(&refused);
    assert!(text.contains("PDF の紙面に結びついていない"), "{text}");
    assert!(text.contains("media=markdown"), "何が理由かを言うべき: {text}");

    let source = rendition(&server.address, &chunk, "source");
    assert_eq!(source.status, 200, "{}", body_text(&source));
    assert_eq!(source.content_type, "text/plain; charset=utf-8");
    assert_eq!(source.body, SEARCH_JA.as_bytes(), "原文そのものを返すべき");
}

/// 知らない別名は 400(受け付ける別名を示す)、見えに無いチャンク ID は 404、ID の形で
/// ないものは 400、GET 以外は 405。
#[test]
fn unknown_aliases_and_chunks_outside_the_view_are_refused() {
    let server = start_server("rendition-refusals");
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    let chunk = chunk_id(&server.address, "世代の整合");

    let unknown = rendition(&server.address, &chunk, "w1200");
    assert_eq!(unknown.status, 400, "{}", body_text(&unknown));
    let text = body_text(&unknown);
    assert!(text.contains("許可表にない"), "{text}");
    assert!(text.contains("source, thumb, page, pagepdf"), "受け付ける別名を示す: {text}");

    // 見えに無いチャンク(このDBノードが持っていない ID)。不在の言明ではなく、
    // 「このDBノードは持っていない」というローカルな事実である(SPEC §7.2/§10)。
    let absent = format!("s256:{}", "0".repeat(64));
    assert_eq!(catalog(&server.address, &absent).status, 404);
    assert_eq!(rendition(&server.address, &absent, "thumb").status, 404);

    // ID の形でないものは要求の誤り。
    assert_eq!(catalog(&server.address, "not-an-id").status, 400);
    assert_eq!(rendition(&server.address, "not-an-id", "thumb").status, 400);

    // 読むだけの口なので GET しか受けない。
    let posted = simple(
        &server.address,
        "POST",
        &format!("/v1/objects/{chunk}/rendition"),
        b"",
    );
    assert_eq!(posted.status, 405, "{}", body_text(&posted));
}

/// 写しを作っているあいだ、ストアの錠を持たない(node/src/embed.rs・node/src/sync.rs と
/// 同じ規律。外部との往復のあいだ錠を握らない)。生成は poppler との往復で、この機械の
/// 実測で 0.1〜0.4 秒かかる。その間 API 全体が止まるかどうかを、実際に別の要求を投げて
/// 観測する(should/0116: 設定ではなく観測された効果で確かめる)。
///
/// 待ち時間はこの機械で実測した生成の費用から決めるので、機械の速さに依らない。
#[test]
fn the_api_stays_open_while_a_rendition_is_being_made() {
    require_poppler();
    let server = start_server("rendition-lock");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    let third = chunk_id(&server.address, "Page three");
    let first = chunk_id(&server.address, "Page one");

    // 1 枚作って費用を測る(以下の待ちはこの値から決める)。
    let started = std::time::Instant::now();
    let warm = rendition(&server.address, &third, "page");
    assert_eq!(warm.status, 200, "{}", body_text(&warm));
    let cost = started.elapsed();

    // もう 1 枚(別の紙面なので別の鍵)を別のスレッドに作らせ、その最中に別の要求を投げる。
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handle = {
        let address = server.address.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let response = rendition(&address, &first, "page");
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(response.status, 200, "{}", body_text(&response));
        })
    };
    std::thread::sleep(cost / 4);
    let started = std::time::Instant::now();
    let status = simple(&server.address, "GET", "/v1/status", b"");
    let waited = started.elapsed();
    let still_rendering = !done.load(std::sync::atomic::Ordering::SeqCst);
    handle.join().expect("写しのスレッド");

    assert_eq!(status.status, 200, "{}", body_text(&status));
    assert!(
        waited < cost / 2,
        "写しを作っているあいだ API が止まっている(生成 {cost:?} に対し、状態の\
         問い合わせが {waited:?} 待った)"
    );
    // すぐ返ったのが「生成がもう終わっていたから」なら、この観測は何も言っていない。
    // 黙って緑にしない(検証したのか検証を諦めたのかが結果から区別できなくなる)。
    assert!(still_rendering, "写しの生成({cost:?})が先に終わり、錠の観測になっていない");
}

/// 写しをストアへ足しても検索の索引は作り直されない(世代が見るのは collections/ 配下の
/// 束縛だけである。node/src/search.rs の Generation)。これを守れないと、ページ画像を
/// 1 枚作るたびに次の検索が全再構築を引き、この機能自体が成り立たない。
///
/// 測り方: (1) 索引が最新のときの検索(warm)、(2) 文書を 1 つ足した直後の検索
/// (rebuild。束縛が変わるので再構築を引く)、(3) ページ画像を作った直後の検索。
/// (3) が (1) と (2) の真ん中より warm 側にあることを見る。絶対値ではなくこの機械で
/// 実測した 2 つの基準の間で言うので、機械の速さに依らない。
#[test]
fn generating_a_page_image_does_not_rebuild_the_search_index() {
    require_poppler();
    let server = start_server("rendition-generation");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    // 再構築の費用が測れる大きさの索引にする(150 文書で実測 20 ミリ秒ほど。最新の
    // 索引を使う検索は 1 ミリ秒未満なので、両者は桁で違う)。
    for number in 0..150 {
        put_document(
            &server.address,
            "notes",
            &format!("search_ja_{number}.md"),
            SEARCH_JA.as_bytes(),
        );
    }
    let chunk = chunk_id(&server.address, "Page two");

    let search = || -> std::time::Duration {
        let started = std::time::Instant::now();
        let response = simple(
            &server.address,
            "POST",
            "/v1/search",
            b"{\"query\":\"\xe4\xb8\x96\xe4\xbb\xa3\xe3\x81\xae\xe6\x95\xb4\xe5\x90\x88\"}",
        );
        let elapsed = started.elapsed();
        assert_eq!(response.status, 200, "{}", body_text(&response));
        elapsed
    };

    // (1) 索引が最新のときの検索。揺らぎを避けるため最小値を採る。
    search();
    let warm = (0..5).map(|_| search()).min().expect("5 回測る");
    // (2) 文書を 1 つ足すと束縛が変わるので、次の検索は索引を作り直す。
    put_document(&server.address, "notes", "search_ja_extra.md", SEARCH_JA.as_bytes());
    let rebuild = search();
    assert!(
        rebuild > warm * 3,
        "再構築の費用がこの機械で測れていない(warm {warm:?} / rebuild {rebuild:?})。\
         資材を増やさないとこの試験は何も言えない"
    );

    // (3) ページ画像を作る。ストアにはオブジェクトも ref も増える。
    let before = objects(&server.address);
    let image = rendition(&server.address, &chunk, "page");
    assert_eq!(image.status, 200, "{}", body_text(&image));
    assert_eq!(image.content_type, "image/png");
    assert_eq!(&image.body[..4], b"\x89PNG");
    assert_eq!(objects(&server.address), before + 1);

    let after = search();
    assert!(
        after < warm + (rebuild - warm) / 2,
        "ページ画像を足したら検索が索引を作り直している(warm {warm:?} / \
         rebuild {rebuild:?} / 写しのあと {after:?})"
    );
}
