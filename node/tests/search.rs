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
/// 目次の節(点線でページ番号をつないだ紙面)と本文の節を持つ日本語文書。低情報
/// チャンクの後処理を確かめるための資材である。
const SEARCH_LOWINFO: &str = include_str!("assets/search_lowinfo.md");

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
///
/// 資材の 1 ページは「Page two of three」の 4 語しかないので、低情報の判定
/// (crate::search::is_low_information)に落ちる。実データで柱だけの紙面を落とすための
/// 判定であり、この資材はそれと見分けがつかない。ここで見たいのは引用の形なので、
/// include_low_information で戻して測る(索引には入ったままである)。
#[test]
fn pdf_search_citations_carry_the_page_number() {
    require_pdftotext();
    let server = start_server("search-pdf");
    put_document(&server.address, "specs", "three_pages.pdf", THREE_PAGE_PDF);
    let response = search(
        &server.address,
        "{\"query\":\"Page two\",\"top_k\":1,\"include_low_information\":true}",
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body.matches("\"id\":").count(), 1, "top_k=1 で 1 件だけ返るべき: {body}");
    assert!(body.contains("\"document\":\"three_pages\""), "{body}");
    assert!(body.contains("\"page\":2"), "{body}");
    assert!(body.contains("\"position\":1"), "{body}");
    assert!(body.contains("\"breadcrumbs\":[]"), "PDF は breadcrumbs を持たない: {body}");
}

/// 検索の応答は、件ごとに原本(文書そのもの)への道を言う。道はチャンクの周りを読むために
/// 要るもので、読み手(ビューワ・MCP・素の API)が写しの目録を引き直さずに済む。
#[test]
fn every_result_carries_the_way_to_the_whole_document() {
    let server = start_server("search-source-url");
    let page = "<!DOCTYPE html><html><head><title>紙面</title></head><body>\
                <h2>節</h2><p>抜粋に出る本文。ここには紙面の主題が書いてあり、\
                読み手は抜粋の周りを読むために原本へ行く。</p>\
                <p>二つ目の段落。取り込みは札を落として本文だけを索引に載せる。</p>\
                </body></html>";
    put_document(&server.address, "site", "page.html", page.as_bytes());
    let response = search(&server.address, "{\"query\":\"抜粋に出る本文\",\"top_k\":1}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    let chunk_id = json_text_field(&body, "id").unwrap_or_else(|| panic!("チャンク ID: {body}"));
    let expected = format!("/v1/objects/{chunk_id}/rendition/source");
    assert!(body.contains(&format!("\"source_url\":\"{expected}\"")), "{body}");

    // 言われた道は実際に開き、返るのはチャンクではなく文書そのものである。
    let whole = simple(&server.address, "GET", &expected, b"");
    assert_eq!(whole.status, 200, "{}", body_text(&whole));
    assert_eq!(whole.content_type, "text/html; charset=utf-8");
    assert_eq!(whole.body, page.as_bytes(), "紙面まるごとが返る");
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

    // 方式を省いた要求の既定は、装備と top_k で決まる(SEARCH の「方式の既定」。
    // 決め方の家は uniqnode::embed::default_method)。3 件しか求めない要求は意味の
    // 近さだけで答える。
    let body = body_text(&search(&server.address, &format!("{query}}}")));
    assert!(body.contains("\"method\":\"embedding\""), "top_k 3 の既定は埋め込み: {body}");
    assert!(body.contains("\"score_semantics\":\"cosine\""), "{body}");
    assert!(!body.contains("degraded"), "劣化していないはず: {body}");
    assert!(body.contains(answer), "既定でも正解が上位 3 件に入るべき: {body}");

    // 10 件求める要求の既定は融合になる。BM25 の順位も混ぜるので順位は下がりうるが、
    // 届かないことはない。
    let body = body_text(&search(
        &server.address,
        "{\"query\":\"急に電気が消えたときの立ち上げ\",\"top_k\":10}",
    ));
    assert!(body.contains("\"method\":\"hybrid\""), "top_k 10 の既定は融合: {body}");
    assert!(body.contains("\"score_semantics\":\"rrf\""), "融合の得点は BM25 とは別物: {body}");
    assert!(body.contains(answer), "融合でも正解が上位 10 件に入るべき: {body}");
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 融合が片肺だったことを言う(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580)):
/// hybrid を求められても、クエリの語がどのチャンクにも無ければ BM25 の順位は空で、
/// 返る列は埋め込み単独とまったく同じものになる。融合が効いているように見えたまま
/// 片肺で答えないことを確かめる(should/0128)。
///
/// 実データ(仕様書 PDF 25 本)では、純日本語の問い 14 本すべてでこれが起きていた
/// (実測 2026-08-17。20260817-real-corpus-search-quality
/// (uuid:faeda9ac-5e9e-4091-8122-2fba9f80c8db))。ここでは
/// 同じ形を固定資材で作る: 日本語だけの文書に英語で問えば、語の集合は文字種の段階で
/// 交わらない。ベクトルは serve が書き込みの後に裏で埋めるものを待って使う(順位の質
/// ではなく「片肺だったと言うこと」だけを測る)。
#[test]
fn hybrid_says_when_bm25_matched_nothing_and_the_fusion_was_one_sided() {
    require_embedding_server();
    let dir = unique_dir("search-one-sided");
    let server = start_server_at_with_args(
        dir.clone(),
        &["--embed", uniqnode::embed::DEFAULT_EMBEDDING_URL],
    );
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    wait_for_log_lines(&server, VECTORS_FILLED, 1);

    // 日本語だけの文書に英語で問う。BM25 単独では 1 件も返らない。
    let query = "how long are request logs kept";
    let body = body_text(&search(
        &server.address,
        &format!("{{\"query\":\"{query}\",\"top_k\":10,\"method\":\"bm25\"}}"),
    ));
    assert_eq!(body, EMPTY_RESULTS, "この問いは BM25 では空振りのはず: {body}");

    // 融合を求めると答えは返るが、それは埋め込み単独の順位である。応答がそう言う。
    let fused = body_text(&search(
        &server.address,
        &format!("{{\"query\":\"{query}\",\"top_k\":10,\"method\":\"hybrid\"}}"),
    ));
    assert!(fused.contains("\"method\":\"hybrid\""), "{fused}");
    assert!(
        fused.contains("BM25 が 1 語も一致せず、順位は埋め込み単独と同じである"),
        "片肺の融合をそう言うべき: {fused}"
    );
    // 実際に同じ列であることを、埋め込み単独の応答と突き合わせて確かめる(言葉だけの
    // 表明にしない)。
    let alone = body_text(&search(
        &server.address,
        &format!("{{\"query\":\"{query}\",\"top_k\":10,\"method\":\"embedding\"}}"),
    ));
    let ids = |body: &str| -> Vec<String> {
        body.split("\"id\":\"")
            .skip(1)
            .filter_map(|part| part.split('"').next())
            .map(String::from)
            .collect()
    };
    assert!(!ids(&alone).is_empty(), "埋め込み単独では返るはず: {alone}");
    assert_eq!(ids(&fused), ids(&alone), "片肺の融合は埋め込み単独と同じ列のはず");

    // 語が一致する問いでは、この表明は出ない(いつでも出る文言ではない)。ベクトルは
    // 全チャンクぶん埋まっているので、被覆の欠落も無く、劣化は何も言わない。
    let matched = body_text(&search(
        &server.address,
        "{\"query\":\"世代の整合\",\"top_k\":10,\"method\":\"hybrid\"}",
    ));
    assert!(!matched.contains("BM25 が 1 語も一致せず"), "語が一致すれば片肺ではない: {matched}");
    assert!(!matched.contains("degraded"), "全チャンクぶん埋まっていれば劣化は無い: {matched}");
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 低情報チャンク(目次の紙面・柱だけ・ページ番号だけ)は既定で応答から落ちるが、
/// 索引からは消えない(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))。落とした
/// 件数を応答が言うこと、include_low_information で戻せること、GET /v1/objects では
/// いつでも引けることを確かめる。
#[test]
fn low_information_chunks_leave_the_results_but_stay_in_the_store() {
    let server = start_server("search-lowinfo");
    put_document(&server.address, "notes", "search_lowinfo.md", SEARCH_LOWINFO.as_bytes());
    // 「索引の構築」は目次の節と本文の節の両方に現れる句である。
    let query = "{\"query\":\"索引の構築\",\"top_k\":10";

    // 既定では目次の節が落ち、本文の節だけが返る。落とした件数は応答が言う。
    let body = body_text(&search(&server.address, &format!("{query}}}")));
    assert_eq!(body.matches("\"id\":").count(), 1, "本文の節だけが返るべき: {body}");
    assert!(body.contains("\"position\":1"), "残るのは本文の節(位置 1): {body}");
    assert!(body.contains("\"filtered_low_information\":1"), "落とした件数を言うべき: {body}");

    // include_low_information を立てれば、同じ問いで目次の節も戻る。
    let kept = body_text(&search(
        &server.address,
        &format!("{query},\"include_low_information\":true}}"),
    ));
    assert_eq!(kept.matches("\"id\":").count(), 2, "目次の節も返るべき: {kept}");
    assert!(!kept.contains("filtered_low_information"), "落としていない: {kept}");
    assert!(kept.contains("\"position\":0"), "目次の節(位置 0)が戻るべき: {kept}");

    // 索引から消したのではないので、チャンクの全文はいつでも引ける。
    let toc = kept.split("\"citation\"").nth(1).expect("1 件目");
    let id = json_text_field(toc, "id").expect("目次のチャンク ID");
    let chunk = body_text(&simple(&server.address, "GET", &format!("/v1/objects/{id}"), b""));
    assert!(chunk.contains("索引の構築"), "落としたチャンクも全文は引けるべき: {chunk}");
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

    // (3) 裏の補完も届かない相手に当たって失敗し、理由をログに残す(黙らない。must/0022)。
    // どのサーバに届かなかったかを言う。
    wait_for_log_lines(&server, FILL_REFUSED, 1);
    let refusal = serve_log(&server);
    let line = refusal.lines().find(|line| line.contains(FILL_REFUSED)).expect("補完の失敗の行");
    assert!(line.contains("127.0.0.1:1"), "どのサーバに届かなかったかを言うべき: {line}");
    assert!(line.contains("BM25 に劣化"), "検索がどうなるかを言うべき: {line}");
    // 同じ理由が続くあいだは繰り返さない: もう 1 文書書いて補完がもう 1 回失敗しても、
    // 行は 1 本のまま。補完が走ったことは温めの記録がもう 1 本増えたことで知り(起動直後の
    // 温めと最初の PUT のどちらが先かで本数が変わるので、増分で数える)、失敗は接続拒否
    // なので温めの直後に即座に決まる。短い猶予の後に数える(負の確認なので、記録を待つ
    // 形にはできない)。
    //
    // 欠陥を戻すとどこで落ちるか(should/0137): api.rs の fill_missing_vectors の
    // last_refusal の比較を消すと、2 本目の PUT の後に同じ行がもう 1 本積まれて最後の
    // 段で落ちる(戻して確かめた)。補完を呼ばなくすると、行が 0 本のままで
    // wait_for_log_lines が落ちる。
    let builds_before = index_builds_in_log(&server);
    put_document(&server.address, "notes", "search_en.md", SEARCH_EN.as_bytes());
    wait_for_index_builds(&server, builds_before + 1);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        log_lines_with(&server, FILL_REFUSED),
        1,
        "同じ理由の失敗を繰り返し書いている: {}",
        serve_log(&server)
    );
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 無いベクトルは serve が裏で埋める(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580) の
/// 「コーパスの埋め込みと serve の分担」): --embed 付きの serve にベクトル無しで文書を
/// 書くと、要求が 1 つも来なくても補完の記録が残り、次の融合検索は被覆の欠落を言わない
/// (キャッシュファイルが伸びたことを見かけで知って索引を読み直す)。
///
/// 欠陥を戻すとどこで落ちるか(should/0137): api.rs の start_index_warmer から
/// start_vector_filler の呼び出しを消す(補完が一切起きない)と、記録が 0 本のままで
/// wait_for_log_lines が落ちる。書き込みの後の nudge_vector_filler だけを消した場合は、
/// 起動直後の温めを待ってから書くので、その温めに続く補完は空のストアを見ており、
/// 書いたぶんを埋めるものが無くなって同じ所で落ちる(起動直後の補完の走査がまだ終わって
/// いない隙に PUT が滑り込むと通ってしまうが、走査は温めの記録の直後の数ミリ秒であり、
/// 消した欠陥を毎回は隠せない)。埋めてもファイルに追記しない(cache.extend を飛ばす)と、
/// 記録はあっても次の検索が「ベクトルは 0/n」と言って最後の段で落ちる。index_is_current
/// がキャッシュの見かけを見なくなると、同じく古い索引のままで落ちる。
#[test]
fn the_server_fills_missing_vectors_after_a_write_without_a_request() {
    require_embedding_server();
    let dir = unique_dir("search-fill-write");
    let server = start_server_at_with_args(
        dir.clone(),
        &["--embed", uniqnode::embed::DEFAULT_EMBEDDING_URL],
    );
    // 起動直後の温めを見届けてから書く(書き込みの合図の側が埋めることを測るため)。
    wait_for_index_builds(&server, 1);
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    // 検索を打たずに待つ。書き込みの合図 → 温め → 補完の順に裏で進む。
    wait_for_log_lines(&server, VECTORS_FILLED, 1);
    let logged = serve_log(&server);
    let line = logged.lines().find(|line| line.contains(VECTORS_FILLED)).expect("補完の記録");
    // 形は「embedded <n> missing vectors in <ms> ms (<cached>/<distinct> cached)」。
    let rest = line.split(VECTORS_FILLED).nth(1).expect("行の残り");
    let (count, rest) = rest.split_once(" missing vectors in ").expect("<n> missing vectors in");
    let (millis, rest) = rest.split_once(" ms (").expect("<ms> ms (");
    let (fraction, tail) = rest.split_once(" cached)").expect("<cached>/<distinct> cached)");
    let count: usize = count.parse().expect("件数が数");
    assert!(count > 0, "埋めた件数が 0: {line}");
    assert!(millis.parse::<u64>().is_ok(), "ミリ秒が数でない: {line}");
    // 新しいストアなので、相異なるチャンクは全部が無かった: 埋めた後は全部が有る。
    assert_eq!(fraction, format!("{count}/{count}"), "{line}");
    assert_eq!(tail, "", "{line}");

    // 次の融合検索は温めた索引と伸びたキャッシュで答え、劣化を何も言わない。
    let body = body_text(&search(
        &server.address,
        "{\"query\":\"世代の整合\",\"top_k\":10,\"method\":\"hybrid\"}",
    ));
    assert!(body.contains("\"method\":\"hybrid\""), "{body}");
    assert!(!body.contains("degraded"), "埋めた後に劣化を言っている: {body}");
    assert!(body.contains("\"document\":\"search_ja\""), "{body}");
    // 意味検索だけでも同じ文書に届く(ベクトルが実際に索引に載っている)。
    let alone = body_text(&search(
        &server.address,
        "{\"query\":\"世代の整合\",\"top_k\":3,\"method\":\"embedding\"}",
    ));
    assert!(alone.contains("\"document\":\"search_ja\""), "{alone}");
    assert!(!alone.contains("degraded"), "{alone}");
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

// ---- 全文つきの応答(full)と索引の温め ----

/// 抜粋(200 文字)より長い 1 チャンクの文書。改行も引用符も含めない(応答から
/// json_text_field で text を素朴に切り出すため)。
fn long_document() -> String {
    let sentence = "索引の温めは要求を待たずに裏のスレッドで行い、書き込みの後にも作り直す。";
    let mut body = String::from("# 温め\n\n");
    while body.chars().count() < 340 {
        body.push_str(sentence);
    }
    body.push('\n');
    body
}

/// full: true の応答は各件にチャンクの全文(text)を載せ、full の無い応答には text の鍵が
/// 無い。full のとき top_k は 10 まで。
///
/// 欠陥を戻すとどこで落ちるか(should/0137): result_json が text を載せなくなれば
/// 「text の鍵がある」で落ち、run_search が本文を引かなくなっても同じ。full を見ずに常に
/// 載せると「full 無しに text が無い」で落ちる。parse_search_request の上限を外すと
/// top_k=11 の 400 で落ちる。
#[test]
fn full_results_carry_the_whole_chunk_text() {
    let server = start_server("search-full");
    let document = long_document();
    put_document(&server.address, "notes", "warm.md", document.as_bytes());

    let response = search(&server.address, "{\"query\":\"索引の温め\",\"full\":true,\"top_k\":3}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert_eq!(body.matches("\"id\":").count(), 1, "{body}");
    let snippet = json_text_field(&body, "snippet").expect("snippet");
    let text = json_text_field(&body, "text").expect("full の応答に text の鍵が無い");
    assert_eq!(snippet.chars().count(), 200, "抜粋は先頭 200 文字: {snippet}");
    assert!(text.chars().count() > 300, "全文が抜粋と同じ長さしか無い: {text}");
    assert!(text.starts_with(&snippet), "全文の先頭が抜粋と一致しない: {text}");
    assert!(text.ends_with("作り直す。"), "全文が途中で切れている: {text}");
    // 鍵は辞書順: snippet → source_url → text。
    let snippet_at = body.find("\"snippet\":").expect("snippet");
    let source_at = body.find("\"source_url\":").expect("source_url");
    let text_at = body.find("\"text\":").expect("text");
    assert!(snippet_at < source_at && source_at < text_at, "鍵の並びが辞書順でない: {body}");

    // full の無い要求は従来の形のまま(text の鍵が無い)。
    let response = search(&server.address, "{\"query\":\"索引の温め\",\"top_k\":3}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("\"snippet\":"), "{body}");
    assert!(!body.contains("\"text\":"), "full 無しの応答に text が載っている: {body}");

    // full のとき top_k は 10 まで。境目の両側を見る。
    let response = search(&server.address, "{\"query\":\"索引の温め\",\"full\":true,\"top_k\":10}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let response = search(&server.address, "{\"query\":\"索引の温め\",\"full\":true,\"top_k\":11}");
    assert_eq!(response.status, 400, "{}", body_text(&response));
    assert!(
        body_text(&response).contains("top_k は full のとき 1..=10"),
        "{}",
        body_text(&response)
    );
    let response = search(&server.address, "{\"query\":\"索引の温め\",\"full\":\"yes\"}");
    assert_eq!(response.status, 400, "{}", body_text(&response));
    assert!(body_text(&response).contains("full"), "{}", body_text(&response));
}

/// 索引を作り直した記録(api.rs の with_current_index が残す 1 行)。
const INDEX_BUILT: &str = "uniqnode: search index built in ";
/// 無いベクトルを裏で埋めた記録(api.rs の fill_missing_vectors が残す 1 行)。
const VECTORS_FILLED: &str = "uniqnode: embed: embedded ";
/// 埋められなかった記録(同じ関数。同じ理由が続くあいだは 1 本だけ)。
const FILL_REFUSED: &str = "uniqnode: embed: 無いベクトルを埋められない: ";

/// serve のログの全文(起動直後はまだ無いことがある: ログを開くのは束縛より前だが、
/// ファイルは最初の 1 行で生まれる)。
fn serve_log(server: &Server) -> String {
    let path = uniqnode::log::default_path(&server.dir, uniqnode::log::SERVE_ROLE);
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// serve のログにある needle の数。
fn log_lines_with(server: &Server, needle: &str) -> usize {
    serve_log(server).matches(needle).count()
}

/// serve のログにある作り直しの記録の数。
fn index_builds_in_log(server: &Server) -> usize {
    log_lines_with(server, INDEX_BUILT)
}

/// needle の記録が少なくとも want 本になるまで待つ。待つ条件は記録そのものであり、
/// 上限は温めの静穏(INDEX_WARM_QUIET)と小さなストアの構築・埋め込みを十分に越える
/// 安全網である(期限が来たら黙って進まず落ちる。should/0104)。
fn wait_for_log_lines(server: &Server, needle: &str, want: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let seen = log_lines_with(server, needle);
        if seen >= want {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{needle:?} の記録が {want} 本にならない({seen} 本のまま): {}",
            serve_log(server)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// 作り直しの記録が少なくとも want 本になるまで待つ。
fn wait_for_index_builds(server: &Server, want: usize) {
    wait_for_log_lines(server, INDEX_BUILT, want);
}

/// 起動直後、要求が 1 つも来なくても索引は温まる(作り直しの記録が残る)。
///
/// 欠陥を戻すとどこで落ちるか(should/0137): main.rs の start_index_warmer の呼び出しを
/// 消すと、要求が無いので記録は 0 本のままで期限に落ちる。記録の行の形を崩すと形の検査で
/// 落ちる。
#[test]
fn the_index_is_warmed_without_a_request() {
    let server = start_server("search-warm-startup");
    wait_for_index_builds(&server, 1);
    let path = uniqnode::log::default_path(&server.dir, uniqnode::log::SERVE_ROLE);
    let logged = std::fs::read_to_string(&path).expect("read log");
    let line = logged.lines().find(|line| line.contains(INDEX_BUILT)).expect("記録の行");
    let rest = line.split(INDEX_BUILT).nth(1).expect("行の残り");
    let (millis, chunks) = rest.split_once(" ms (").expect("<ms> ms (<chunks> chunks) の形");
    assert!(millis.parse::<u64>().is_ok(), "ミリ秒が数でない: {line}");
    assert_eq!(chunks, "0 chunks)", "空のストアの索引は 0 チャンク: {line}");
}

/// 書き込みの後、次の検索を待たずに索引は作り直され、その検索は温めた索引で答える
/// (作り直しの記録がもう 1 本増えず、新しい文書が返る)。
///
/// 欠陥を戻すとどこで落ちるか(should/0137): put_document の nudge_index_warmer を消すと、
/// 検索を打たないので記録は 1 本のままで期限に落ちる。温めが索引を差し替えずに捨てる
/// (キャッシュに入れない)と、検索が自分で作り直して記録が 3 本になり、最後の段で落ちる。
#[test]
fn a_write_warms_the_index_before_the_next_search() {
    let server = start_server("search-warm-write");
    wait_for_index_builds(&server, 1);
    put_document(&server.address, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    wait_for_index_builds(&server, 2);
    let path = uniqnode::log::default_path(&server.dir, uniqnode::log::SERVE_ROLE);
    let logged = std::fs::read_to_string(&path).expect("read log");
    let second = logged.lines().filter(|line| line.contains(INDEX_BUILT)).nth(1).expect("2 本目");
    assert!(!second.contains("(0 chunks)"), "書き込み後の索引が空: {second}");

    let response = search(&server.address, "{\"query\":\"世代の整合\"}");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("\"document\":\"search_ja\""), "{body}");
    assert_eq!(index_builds_in_log(&server), 2, "検索が温めた索引を使わず作り直した: {logged}");
}
