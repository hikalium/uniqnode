//! BM25 検索の統合テスト(RAG (uuid:35a888f7-5ac6-4333-831c-dd7753b82315) の
//! BM25 検索の項の完了条件の確認)。実プロセスの serve に生 HTTP/1.1 で当てる
//! (should/0138)。資材はソースとは別ファイル(should/0112)。

mod common;
use common::*;

/// 日本語文書(見出し 2 段。世代の整合の節が識別しやすい語を持つ)。
const SEARCH_JA: &str = include_str!("assets/search_ja.md");
/// 日本語文書の改版(世代の整合の節を削った版。見えの検証用)。
const SEARCH_JA_V2: &str = include_str!("assets/search_ja_v2.md");
/// 英語文書(識別子 token_estimate を持つ節と、token と estimate を別語で含む
/// おとりの節)。
const SEARCH_EN: &str = include_str!("assets/search_en.md");
/// テスト資材の最小 PDF(3 ページ。ingest の統合テストと共用)。
const THREE_PAGE_PDF: &[u8] = include_bytes!("assets/three_pages.pdf");

fn put_document(address: &str, collection: &str, name: &str, body: &[u8]) {
    let path = format!("/v1/collections/{collection}/documents/{name}");
    let response = simple(address, "PUT", &path, body);
    assert_eq!(response.status, 200, "{}", body_text(&response));
}

fn search(address: &str, body: &str) -> HttpResponse {
    simple(address, "POST", "/v1/search", body.as_bytes())
}

/// 空振りの応答(results 空)の全文。空振りの検証は全文一致で行う。
const EMPTY_RESULTS: &str = "{\"results\":[],\"score_semantics\":\"bm25\"}";

/// 完了条件の三つ組: (1) 日本語クエリ・(2) 英語クエリ・(3) 識別子の完全一致で該当
/// チャンクが top-k に入り、(5) citation の document・position・breadcrumbs が正しい。
/// fetch 側は既存の GET /v1/objects/{id} で足りる。
#[test]
fn search_finds_japanese_english_and_identifier_chunks_with_citations() {
    let server = start_server("search-api");
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    put_document(&server.address, "notes", "search_en.md", SEARCH_EN.as_bytes());

    // (1) 日本語クエリ。該当チャンクだけが返り、引用が組める。
    let response = search(&server.address, "{\"query\":\"世代の整合\"}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body.matches("\"id\":").count(), 1, "該当チャンクだけが返るべき: {body}");
    assert!(body.contains("\"score_semantics\":\"bm25\""), "{body}");
    assert!(body.contains("\"score\":"), "{body}");
    assert!(body.contains("\"document\":\"search_ja\""), "{body}");
    assert!(body.contains("\"position\":1"), "{body}");
    assert!(body.contains("\"breadcrumbs\":[\"分散設計\",\"世代の整合\"]"), "{body}");
    assert!(body.contains("\"snippet\":\"転置索引は導出データであり"), "{body}");
    // fetch: 返った ID から既存のオブジェクト取得で全文に届く。
    let id = json_text_field(&body, "id").expect("チャンク ID");
    let chunk = body_text(&simple(&server.address, "GET", &format!("/v1/objects/{id}"), b""));
    assert!(chunk.contains("世代の整合はオブジェクト数と署名者ごとの最終列番号"), "{chunk}");

    // (2) 英語クエリ。
    let response =
        search(&server.address, "{\"query\":\"saturates repeated term frequency\"}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body.matches("\"id\":").count(), 1, "該当チャンクだけが返るべき: {body}");
    assert!(body.contains("\"document\":\"search_en\""), "{body}");
    assert!(body.contains("\"position\":0"), "{body}");
    assert!(body.contains("\"breadcrumbs\":[\"Retrieval notes\",\"Scoring\"]"), "{body}");

    // (3) 識別子の完全一致。token と estimate を別語で含むおとりの節(position 1)は
    // 一致しない(識別子は 1 語まるごと索引されるため)。
    let response = search(&server.address, "{\"query\":\"token_estimate\"}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body.matches("\"id\":").count(), 1, "識別子の完全一致だけが返るべき: {body}");
    assert!(body.contains("\"document\":\"search_en\""), "{body}");
    assert!(body.contains("\"position\":2"), "{body}");
    assert!(
        body.contains("\"breadcrumbs\":[\"Retrieval notes\",\"Chunker internals\"]"),
        "{body}"
    );
    assert!(body.contains("token_estimate returns"), "{body}");
}

/// (4) 見えの検証(原理 5): 文書を改版したら旧版のチャンクは検索から消え、オブジェクト
/// の増えない ref だけの変化(tombstone)でも消える(世代整合に ref の変化も効く)。
#[test]
fn a_revision_and_a_tombstone_remove_old_chunks_from_search() {
    let server = start_server("search-visibility");
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    let body = body_text(&search(&server.address, "{\"query\":\"世代の整合\"}"));
    let old_id = json_text_field(&body, "id").expect("旧版のチャンク ID");

    // 改版: 世代の整合の節を削った v2 を同じ名前で取り込む。
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA_V2.as_bytes());
    let response = search(&server.address, "{\"query\":\"世代の整合\"}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body, EMPTY_RESULTS, "旧版のチャンク {old_id} が見えに残っている");

    // ref の張り替えだけの変化: 検索が索引を作った後に tombstone しても、次の検索は
    // 作り直された索引で答える。
    put_document(&server.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    let body = body_text(&search(&server.address, "{\"query\":\"token_estimate\"}"));
    assert_eq!(body.matches("\"id\":").count(), 1, "{body}");
    put_ref(&server.address, "collections/notes/search_en", None);
    let body = body_text(&search(&server.address, "{\"query\":\"token_estimate\"}"));
    assert_eq!(body, EMPTY_RESULTS, "tombstone 後もチャンクが見えに残っている");
}

/// PDF のチャンクは citation に物理ページ番号を持つ。
#[test]
fn pdf_search_citations_carry_the_page_number() {
    require_pdftotext();
    let server = start_server("search-pdf");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    let response = search(&server.address, "{\"query\":\"Page two\",\"top_k\":1}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body.matches("\"id\":").count(), 1, "top_k=1 で 1 件だけ返るべき: {body}");
    assert!(body.contains("\"document\":\"three_pages\""), "{body}");
    assert!(body.contains("\"page\":2"), "{body}");
    assert!(body.contains("\"position\":1"), "{body}");
    assert!(body.contains("\"breadcrumbs\":[]"), "PDF は breadcrumbs を持たない: {body}");
}

/// collection の絞り込み: 指定すればそのコレクションだけ、省略すれば全コレクション。
#[test]
fn a_collection_filter_narrows_the_search_scope() {
    let server = start_server("search-collection");
    put_document(&server.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    put_document(&server.address, "wiki", "search_en.md", SEARCH_EN.as_bytes());
    let body = body_text(&search(&server.address, "{\"query\":\"token_estimate\"}"));
    assert_eq!(body.matches("\"id\":").count(), 2, "省略時は全コレクション: {body}");
    let body = body_text(&search(
        &server.address,
        "{\"query\":\"token_estimate\",\"collection\":\"notes\"}",
    ));
    assert_eq!(body.matches("\"id\":").count(), 1, "指定時はそのコレクションだけ: {body}");
    let body = body_text(&search(
        &server.address,
        "{\"query\":\"token_estimate\",\"collection\":\"absent\"}",
    ));
    assert_eq!(body, EMPTY_RESULTS, "存在しないコレクションは空振り: {body}");
}

/// 形式外の要求は理由を言って拒み(must/0022)、空のストアへの検索は空の成功。
#[test]
fn search_rejects_malformed_requests_explicitly() {
    let server = start_server("search-bad-requests");
    let cases: &[(&str, &str)] = &[
        ("{}", "query"),
        ("{\"query\":\"\"}", "query"),
        ("{\"query\":\"。。。\"}", "索引語"),
        ("{\"query\":\"x\",\"top_k\":0}", "top_k"),
        ("{\"query\":\"x\",\"top_k\":\"ten\"}", "top_k"),
        ("{\"query\":\"x\",\"collection\":7}", "collection"),
        ("not json", "JSON"),
    ];
    for (request_body, needle) in cases {
        let response = search(&server.address, request_body);
        let body = body_text(&response);
        assert_eq!(response.status, 400, "{request_body} への応答: {body}");
        assert!(body.contains(needle), "{request_body} への応答が理由を言わない: {body}");
    }
    // 空のストアへの検索は誤りではなく、空の結果が返る。
    let response = search(&server.address, "{\"query\":\"anything\"}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_text(&response), EMPTY_RESULTS);
}
