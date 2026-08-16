//! BM25 検索の統合テスト(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の
//! API・引用・見えの確認)。実プロセスの serve に生 HTTP/1.1 で当てる
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

/// 空振りの応答(results 空)の全文。空振りの検証は全文一致で行う。method は実際に
/// 使った方式で、埋め込みを装備しない serve では bm25 になる。
const EMPTY_RESULTS: &str = "{\"method\":\"bm25\",\"results\":[],\"score_semantics\":\"bm25\"}";

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

/// 意味検索の効き目を生産経路で確かめる(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580)):
/// 言い換えのクエリ(索引語を一つも共有しない問い)が、POST /v1/search 越しに正解の節へ
/// 届く。資材は評価ハーネスの語彙の隔たりの場を使い回す(対の由来を一箇所に保つ。
/// should/0135)。
///
/// 経路は運用と同じ順序である: CLI で取り込み、CLI でベクトルを作り(serve は止めた
/// まま。ストアは二重に開けない)、そのあと serve を --embed 付きで起こす。
#[test]
fn a_paraphrased_query_reaches_its_section_through_the_search_api() {
    require_embedding_server();
    let dir = unique_dir("search-embedding");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets");
    // 評価ハーネスと同じ 4 文書を入れる。1 文書だけでは競合が薄く、語の一致が空回り
    // していることが順位に出ない。
    for document in ["eval_ja.md", "eval_en.md", "eval_api.md", "eval_gap.md"] {
        let path = assets.join(document);
        run_cli(&["ingest", dir.to_str().expect("utf-8"), "notes", path.to_str().expect("utf-8")]);
    }
    let embedded = run_cli(&["embed", dir.to_str().expect("utf-8")]);
    assert!(embedded.contains("embedded: 23"), "23 チャンクぶん作るはず: {embedded}");
    // 二度目は何も計算しない(鍵はチャンクのオブジェクト ID なので、同じ内容は再利用)。
    let again = run_cli(&["embed", dir.to_str().expect("utf-8")]);
    assert!(again.contains("cached: 23") && again.contains("embedded: 0"), "{again}");

    let server = start_server_at_with_args(
        dir.clone(),
        &["--embed", uniqnode::embed::DEFAULT_EMBEDDING_URL],
    );
    // 正解は eval_gap の 0 番(電源断からの復帰)。問いと索引語を一つも共有しないので、
    // BM25 では返らない(評価ハーネスの bm25_cannot_reach_any_vocabulary_gap_pair が
    // 原理として確かめている対である)。本文の一節で名指しする。
    let answer = "無停電電源が尽きて";
    let query = "{\"query\":\"急に電気が消えたときの立ち上げ\",\"top_k\":3";
    let body = body_text(&search(&server.address, &format!("{query},\"method\":\"bm25\"}}")));
    assert!(body.contains("\"method\":\"bm25\""), "{body}");
    assert!(
        !body.contains(answer),
        "BM25 で正解が返るなら、この対はもう語彙の隔たりの対ではない: {body}"
    );

    // 埋め込み単独では正解が 1 位に来る(意味検索が効いている)。
    let body = body_text(&search(&server.address, &format!("{query},\"method\":\"embedding\"}}")));
    assert!(!body.contains("degraded"), "劣化していないはず: {body}");
    assert!(body.contains("\"score_semantics\":\"cosine\""), "{body}");
    let first = body.split("\"citation\"").nth(1).expect("1 件目");
    assert!(
        first.contains("\"document\":\"eval_gap\"") && first.contains("\"position\":0"),
        "言い換えの問いで正解が 1 位に来るべき: {body}"
    );
    assert!(first.contains("\"breadcrumbs\":[\"運用の覚え書き\",\"電源断からの復帰\"]"), "{body}");

    // 既定(埋め込みを装備した serve では融合)でも、上位 3 件に入る。BM25 の順位も
    // 混ぜるので順位は下がりうるが、届かないことはない。
    let body = body_text(&search(&server.address, &format!("{query}}}")));
    assert!(body.contains("\"method\":\"hybrid\""), "{body}");
    assert!(body.contains("\"score_semantics\":\"rrf\""), "融合の得点は BM25 とは別物: {body}");
    assert!(!body.contains("degraded"), "劣化していないはず: {body}");
    assert!(body.contains(answer), "融合でも正解が上位 3 件に入るべき: {body}");
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// CLI を 1 回動かして標準出力を返す(取り込みとベクトル作りは serve 停止中のストア用の
/// 経路であり、テストも同じ道を通る。should/0138)。
fn run_cli(arguments: &[&str]) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .output()
        .expect("uniqnode の起動");
    assert!(
        output.status.success(),
        "uniqnode {arguments:?} が失敗した: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

/// 劣化の経路(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580)): 埋め込みを装備した
/// serve でも、埋め込みが使えないときは BM25 だけで答え続ける。落ちないことと、黙って
/// 劣化しないこと(応答が実際に使った方式と理由を言うこと)の両方を確かめる(should/0128)。
///
/// このテストは埋め込みサーバを要求しない。届かない相手(誰も待ち受けていないポート)を
/// 指し、ベクトルは試験が自分でキャッシュに書くので、劣化の判断だけを切り出して測れる。
#[test]
fn search_degrades_to_bm25_when_the_embedding_server_cannot_be_reached() {
    let dir = unique_dir("search-degraded");
    let server = start_server_at_with_args(dir.clone(), &["--embed", "http://127.0.0.1:1"]);
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());

    // (1) ベクトルが 1 件も無い段階: 既定の方式(融合)を求めても BM25 で答え、理由が
    // 「ベクトルが無い」であることを言う。
    let body = body_text(&search(&server.address, "{\"query\":\"世代の整合\"}"));
    assert!(body.contains("\"method\":\"bm25\""), "劣化後の方式が読めるべき: {body}");
    assert!(body.contains("\"score_semantics\":\"bm25\""), "{body}");
    assert!(body.contains("ベクトルが 1 件も無い"), "劣化の理由が読めるべき: {body}");
    assert_eq!(body.matches("\"id\":").count(), 1, "劣化しても BM25 の結果は返るべき: {body}");
    let chunk_id = json_text_field(&body, "id").expect("チャンク ID");

    // (2) ベクトルはあるが埋め込みサーバに届かない段階: クエリを埋め込めないので、
    // やはり BM25 で答え、理由は接続の失敗になる。ベクトルはストアの外の導出データ
    // なので、serve を止めずに書ける(世代ではなくキャッシュの見かけで作り直される)。
    let mut cache = uniqnode::embed::VectorCache::open(
        uniqnode::embed::VectorCache::path_for(&dir, "bge-m3"),
        "bge-m3",
        uniqnode::embed::DEFAULT_EMBEDDING_DIMENSION,
    )
    .expect("open cache");
    let mut vector = vec![0.0f32; uniqnode::embed::DEFAULT_EMBEDDING_DIMENSION];
    vector[0] = 1.0;
    cache.extend(vec![(chunk_id, vector)]).expect("write cache");

    let body = body_text(&search(&server.address, "{\"query\":\"世代の整合\"}"));
    assert!(body.contains("\"method\":\"bm25\""), "{body}");
    assert!(body.contains("127.0.0.1:1"), "どのサーバに届かなかったかを言うべき: {body}");
    assert!(body.contains("接続できない"), "劣化の理由が読めるべき: {body}");
    assert_eq!(body.matches("\"id\":").count(), 1, "劣化しても BM25 の結果は返るべき: {body}");

    // 方式を明示して BM25 を求めたときは、そもそも埋め込みを試さないので理由も出ない。
    let body = body_text(&search(
        &server.address,
        "{\"query\":\"世代の整合\",\"method\":\"bm25\"}",
    ));
    assert!(body.contains("\"method\":\"bm25\""), "{body}");
    assert!(!body.contains("degraded"), "劣化していないのに理由を出さない: {body}");
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
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
        ("{\"query\":\"x\",\"method\":\"magic\"}", "method"),
        ("{\"query\":\"x\",\"method\":7}", "method"),
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
