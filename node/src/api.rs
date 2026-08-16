//! ノードローカル API(SPEC §10)。HTTP の上に store の操作を露出する。
//! 404 は「このDBノードは持っていない」というローカルな事実であって、ネットワークに
//! 対する不存在の言明ではない(SPEC §7.2/§10)。

use crate::c1;
use crate::http::{Request, Response};
use crate::query::{QueryEngine, QueryKind};
use crate::store::{Store, StoreError};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// ハンドラが使うサーバ状態。クエリはネットワーク待ちを伴うため、store のロックとは
/// 独立に engine が内部で短くロックする(クエリ中に API 全体を塞がない)。
pub struct ApiContext {
    pub store: Arc<Mutex<Store>>,
    pub engine: Arc<QueryEngine>,
    /// serve でのみ Some(健全性エンジン。SPEC §8)。
    pub health: Option<Arc<crate::health::HealthEngine>>,
    /// 逆引き索引の遅延キャッシュ(GET /v1/objects/{id}/referrers)。起動時には作らず
    /// (Store::open に全件パースを足さない。INGEST の「逆引き」節)、初回要求時に
    /// 構築して、世代(object_count)がずれたら次の要求で作り直す。
    pub referrers: Mutex<Option<crate::store::ReferrerIndex>>,
    /// 検索索引の遅延キャッシュ(POST /v1/search)。referrers と同じ遅延構築だが、
    /// 世代はオブジェクト数に加えて署名者ごとの最終 seq も見る(ref の張り替えだけの
    /// 変化でも旧版のチャンクを見えから外すため。node/src/search.rs)。
    pub search: Mutex<Option<crate::search::SearchIndex>>,
    /// 埋め込みの装備(serve の --embed が与えられたときだけ Some。node/src/embed.rs)。
    /// 無ければ検索は BM25 だけで答える。
    pub embedding: Option<crate::embed::EmbeddingService>,
}

fn json_object(entries: Vec<(&str, c1::Value)>) -> Vec<u8> {
    let mut map = BTreeMap::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    c1::to_canonical_bytes(&c1::Value::Object(map))
}

fn error_response(status: u16, message: &str) -> Response {
    Response::json(
        status,
        json_object(vec![("error", c1::Value::Text(message.to_string()))]),
    )
}

fn store_error_response(error: StoreError) -> Response {
    match error {
        StoreError::Invalid(m) => error_response(400, &m),
        StoreError::Io(e) => error_response(500, &format!("io: {e}")),
        StoreError::Corruption(m) => error_response(500, &format!("corruption: {m}")),
    }
}

/// serve が PDF 抽出に使う pdftotext(PATH 依存。CLI と違い明示指定の引数は持たない)。
/// 版の取得(pdftotext -v)はプロセスで一度だけ行い、以後は使い回す。見つからない失敗は
/// 覚えず次の要求で引き直す(serve の実行中に導入されれば以後の要求は通る)。
fn pdf_extractor() -> Result<&'static crate::ingest::PdfExtractor, String> {
    static EXTRACTOR: std::sync::OnceLock<crate::ingest::PdfExtractor> = std::sync::OnceLock::new();
    if let Some(extractor) = EXTRACTOR.get() {
        return Ok(extractor);
    }
    let located = crate::ingest::PdfExtractor::locate(None)?;
    Ok(EXTRACTOR.get_or_init(|| located))
}

pub fn handle(context: &ApiContext, request: &Request) -> Response {
    let store = &*context.store;
    let path = request.path.as_str();
    let method = request.method.as_str();
    match (method, path) {
        ("GET", "/healthz") => Response::text(200, "ok\n"),
        ("POST", "/v1/query") => handle_query(context, request),
        ("POST", "/v1/search") => handle_search(context, request),
        ("GET", "/v1/peers") => {
            let peers: Vec<c1::Value> = context
                .engine
                .default_scope()
                .into_iter()
                .map(|address| {
                    let mut map = BTreeMap::new();
                    map.insert("address".to_string(), c1::Value::Text(address));
                    c1::Value::Object(map)
                })
                .collect();
            Response::json(200, json_object(vec![("peers", c1::Value::Array(peers))]))
        }
        ("POST", "/v1/admin/shutdown") => {
            // 正常終了。ストアは全書き込みを fsync 済みなので flush は不要。
            let mut response = Response::text(200, "shutting down\n");
            response.shutdown_after = true;
            response
        }
        ("GET", "/v1/status") => {
            let mut fields = {
                let store = store.lock().expect("lock");
                vec![
                    ("v", c1::Value::Integer(1)),
                    ("node_id", c1::Value::Text(store.node_id_hex().to_string())),
                    ("objects", c1::Value::Integer(store.object_count() as i64)),
                    ("last_seq", c1::Value::Integer(store.last_seq() as i64)),
                    ("used_bytes", c1::Value::Integer(store.used_bytes() as i64)),
                    (
                        "capacity_bytes",
                        match store.capacity_bytes() {
                            Some(c) => c1::Value::Integer(c as i64),
                            None => c1::Value::Null,
                        },
                    ),
                    (
                        "free_bytes",
                        match store.free_bytes() {
                            Some(f) => c1::Value::Integer(f as i64),
                            None => c1::Value::Null,
                        },
                    ),
                ]
            };
            if let Some(health) = &context.health {
                let roots: Vec<c1::Value> = health
                    .roots_snapshot()
                    .into_iter()
                    .map(|root| {
                        let mut map = BTreeMap::new();
                        map.insert("root".to_string(), c1::Value::Text(root.root));
                        map.insert(
                            "required".to_string(),
                            c1::Value::Integer(root.required as i64),
                        );
                        map.insert(
                            "observed".to_string(),
                            c1::Value::Integer(root.observed as i64),
                        );
                        map.insert("state".to_string(), c1::Value::Text(root.state));
                        map.insert(
                            "reason".to_string(),
                            match root.reason {
                                Some(r) => c1::Value::Text(r),
                                None => c1::Value::Null,
                            },
                        );
                        c1::Value::Object(map)
                    })
                    .collect();
                fields.push(("health", c1::Value::Array(roots)));
            }
            Response::json(200, json_object(fields))
        }
        ("GET", "/v1/health/events") => {
            let events: Vec<c1::Value> = match &context.health {
                None => Vec::new(),
                Some(health) => health
                    .events_snapshot()
                    .into_iter()
                    .map(|event| {
                        let mut map = BTreeMap::new();
                        map.insert(
                            "elapsed_ms".to_string(),
                            c1::Value::Integer(event.elapsed_ms as i64),
                        );
                        map.insert("root".to_string(), c1::Value::Text(event.root));
                        map.insert("state".to_string(), c1::Value::Text(event.state));
                        map.insert(
                            "reason".to_string(),
                            match event.reason {
                                Some(r) => c1::Value::Text(r),
                                None => c1::Value::Null,
                            },
                        );
                        c1::Value::Object(map)
                    })
                    .collect(),
            };
            Response::json(200, json_object(vec![("events", c1::Value::Array(events))]))
        }
        ("POST", "/v1/pins") => {
            let body_text = match std::str::from_utf8(&request.body) {
                Ok(t) => t,
                Err(_) => return error_response(400, "ボディが UTF-8 でない"),
            };
            let value = match c1::parse(body_text) {
                Ok(v) => v,
                Err(e) => return error_response(400, &format!("JSON が不正: {e}")),
            };
            let (root, min_replicas) = match &value {
                c1::Value::Object(map) => {
                    let root = match map.get("root") {
                        Some(c1::Value::Text(t)) => t.clone(),
                        _ => return error_response(400, "root がない"),
                    };
                    let min = match map.get("min_replicas") {
                        Some(c1::Value::Integer(n)) if (0..=1024).contains(n) => *n as u32,
                        _ => return error_response(400, "min_replicas は 0..=1024 の整数"),
                    };
                    (root, min)
                }
                _ => return error_response(400, "ボディはオブジェクトであるべき"),
            };
            let mut store = store.lock().expect("lock");
            match store.set_pin(&root, min_replicas) {
                Ok(seq) => {
                    // pin の発行者は root を保持しているので、最初の保持者として表明する
                    // (これが修復の取り寄せ元の種になる)。
                    if min_replicas > 0 && !store.own_attested_roots().contains(&root) {
                        if let Err(e) = store.set_attest(&root, true) {
                            return store_error_response(e);
                        }
                    }
                    Response::json(200, json_object(vec![("seq", c1::Value::Integer(seq as i64))]))
                }
                Err(e) => store_error_response(e),
            }
        }
        ("GET", "/v1/pins") => {
            let store = store.lock().expect("lock");
            let pins: Vec<c1::Value> = store
                .effective_pins()
                .into_iter()
                .map(|(root, min)| {
                    let holders: Vec<c1::Value> = store
                        .attest_holders(&root)
                        .into_iter()
                        .map(c1::Value::Text)
                        .collect();
                    let mut map = BTreeMap::new();
                    map.insert("root".to_string(), c1::Value::Text(root));
                    map.insert("min_replicas".to_string(), c1::Value::Integer(min as i64));
                    map.insert("holders".to_string(), c1::Value::Array(holders));
                    c1::Value::Object(map)
                })
                .collect();
            Response::json(200, json_object(vec![("pins", c1::Value::Array(pins))]))
        }
        ("POST", "/v1/sync") => crate::sync::handle_sync_request(store, request),
        ("GET", "/v1/replication/signers") => {
            let store = store.lock().expect("lock");
            let signers: Vec<c1::Value> = store
                .signers()
                .into_iter()
                .map(|(signer, last_seq)| {
                    let mut map = BTreeMap::new();
                    map.insert("signer".to_string(), c1::Value::Text(signer));
                    map.insert("last_seq".to_string(), c1::Value::Integer(last_seq as i64));
                    c1::Value::Object(map)
                })
                .collect();
            Response::json(200, json_object(vec![("signers", c1::Value::Array(signers))]))
        }
        ("POST", "/v1/objects") => {
            let mut store = store.lock().expect("lock");
            match store.put_object(&request.body) {
                Ok((id, new)) => Response::json(
                    if new { 201 } else { 200 },
                    json_object(vec![
                        ("id", c1::Value::Text(id)),
                        ("new", c1::Value::Bool(new)),
                    ]),
                ),
                Err(e) => store_error_response(e),
            }
        }
        _ => handle_with_path_argument(context, request),
    }
}

/// POST /v1/query。ボディ: {kind, target, budget_ms?, scope?, wait?}。
/// budget は観測の打ち切りであり、wait:false ならハンドルを即返す(SPEC §7.2)。
fn handle_query(context: &ApiContext, request: &Request) -> Response {
    let body_text = match std::str::from_utf8(&request.body) {
        Ok(t) => t,
        Err(_) => return error_response(400, "ボディが UTF-8 でない"),
    };
    let value = match c1::parse(body_text) {
        Ok(v) => v,
        Err(e) => return error_response(400, &format!("JSON が不正: {e}")),
    };
    let map = match &value {
        c1::Value::Object(m) => m,
        _ => return error_response(400, "ボディはオブジェクトであるべき"),
    };
    let kind = match map.get("kind") {
        Some(c1::Value::Text(t)) if t == "object" => QueryKind::Object,
        Some(c1::Value::Text(t)) if t == "ref" => QueryKind::Ref,
        _ => return error_response(400, "kind は \"object\" か \"ref\""),
    };
    let target = match map.get("target") {
        Some(c1::Value::Text(t)) => t.clone(),
        _ => return error_response(400, "target がない"),
    };
    match kind {
        QueryKind::Object if !c1::is_object_id(&target) => {
            return error_response(400, "オブジェクトIDの形式が不正")
        }
        QueryKind::Ref if !target.contains('/') => {
            return error_response(400, "ref は完全名(<node_id>/<path>)で指定する")
        }
        _ => {}
    }
    let budget_ms = match map.get("budget_ms") {
        None => 2_000,
        Some(c1::Value::Integer(n)) if (0..=60_000).contains(n) => *n as u64,
        Some(_) => return error_response(400, "budget_ms は 0..=60000 の整数"),
    };
    let scope = match map.get("scope") {
        None => context.engine.default_scope(),
        Some(c1::Value::Array(items)) => {
            let mut scope = Vec::new();
            for item in items {
                match item {
                    c1::Value::Text(address) => scope.push(address.clone()),
                    _ => return error_response(400, "scope はアドレス文字列の配列"),
                }
            }
            scope
        }
        Some(_) => return error_response(400, "scope はアドレス文字列の配列"),
    };
    let wait = match map.get("wait") {
        None => true,
        Some(c1::Value::Bool(b)) => *b,
        Some(_) => return error_response(400, "wait は真偽値"),
    };
    let shared = context.engine.start(kind, &target, budget_ms, scope);
    let state = if wait { shared.wait_settled() } else { shared.snapshot() };
    Response::json(200, crate::query::state_to_json(&state))
}

/// POST /v1/search(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))。ボディ:
/// {"query": "...", "collection": "...", "top_k": N, "method": "..."}(collection 省略時は
/// 全コレクション、top_k 省略時は 10)。索引は導出データの遅延キャッシュで、referrers と
/// 同じく初回要求時に構築し、世代がずれたら次の要求で作り直す。
///
/// method は "bm25"(語の一致)・"embedding"(意味の近さ)・"hybrid"(両者の RRF 融合)。
/// 省略時の既定は装備に従う: 埋め込みが設定されていれば hybrid、なければ bm25 である。
/// 埋め込みが使えないときは BM25 だけに劣化して答え、応答の method(実際に使った方式)と
/// degraded(理由)がそれを語る(黙って劣化しない。should/0128)。
fn handle_search(context: &ApiContext, request: &Request) -> Response {
    let body_text = match std::str::from_utf8(&request.body) {
        Ok(t) => t,
        Err(_) => return error_response(400, "ボディが UTF-8 でない"),
    };
    let value = match c1::parse(body_text) {
        Ok(v) => v,
        Err(e) => return error_response(400, &format!("JSON が不正: {e}")),
    };
    let map = match &value {
        c1::Value::Object(m) => m,
        _ => return error_response(400, "ボディはオブジェクトであるべき"),
    };
    let query = match map.get("query") {
        Some(c1::Value::Text(t)) if !t.is_empty() => t.clone(),
        _ => return error_response(400, "query がない(空でない文字列)"),
    };
    let collection = match map.get("collection") {
        None => None,
        Some(c1::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
        _ => return error_response(400, "collection は空でない文字列"),
    };
    let top_k = match map.get("top_k") {
        None => 10usize,
        Some(c1::Value::Integer(n)) if (1..=1000).contains(n) => *n as usize,
        _ => return error_response(400, "top_k は 1..=1000 の整数"),
    };
    let requested = match map.get("method") {
        // 既定は装備に従う。埋め込みを設定した節点で融合を既定にするのは、方式の指定を
        // 知らない呼び手(既存の CLI・MCP)が黙って BM25 だけに取り残されないため。
        None => match &context.embedding {
            Some(_) => crate::embed::SearchMethod::Hybrid,
            None => crate::embed::SearchMethod::Bm25,
        },
        Some(c1::Value::Text(t)) => match crate::embed::SearchMethod::parse(t) {
            Some(method) => method,
            None => {
                return error_response(400, "method は \"bm25\"・\"embedding\"・\"hybrid\" のどれか")
            }
        },
        _ => return error_response(400, "method は文字列"),
    };
    if crate::search::terms_of(&query).is_empty() {
        return error_response(
            400,
            "クエリに索引語が無い(英数字の語か、仮名・漢字などの文字が要る)",
        );
    }
    // クエリの埋め込みは、どのロックも取る前に済ませる。埋め込みサーバとの往復であり、
    // その待ちのあいだ store のロックを持つと、1 本の検索で API 全体が塞がる
    // (node/src/sync.rs のロックの規律と同じ理由)。届かなければ理由を持ち帰り、
    // 下の順位付けが BM25 だけに劣化して答える。
    let embedding = match requested {
        crate::embed::SearchMethod::Bm25 => {
            crate::embed::QueryEmbedding::Unavailable("bm25 を要求された".to_string())
        }
        _ => crate::embed::QueryEmbedding::of(
            context.embedding.as_ref().map(|service| &service.embedder),
            &query,
        ),
    };

    let store = context.store.lock().expect("lock");
    let mut cache = context.search.lock().expect("lock");
    if !cache.as_ref().is_some_and(|index| index.is_current(&store)) {
        match crate::search::SearchIndex::build(&store) {
            Ok(index) => *cache = Some(index),
            Err(e) => return store_error_response(e),
        }
    }
    let index = cache.as_ref().expect("直前に構築した");

    // ベクトルの索引も BM25 の索引と同じ遅延キャッシュで持つ。読むのはキャッシュ
    // ファイルだけなので、検索要求が模型の計算を待つことはない(コーパスの埋め込みは
    // CLI の uniqnode embed の仕事)。
    let mut vector_cache = None;
    let mut load_failure = None;
    if requested != crate::embed::SearchMethod::Bm25 {
        if let Some(service) = &context.embedding {
            let mut guard = service.index.lock().expect("lock");
            if !guard.as_ref().is_some_and(|vectors| service.index_is_current(vectors, &store)) {
                match service.load_index(&store) {
                    Ok(vectors) => *guard = Some(vectors),
                    Err(e) => {
                        *guard = None;
                        load_failure = Some(format!("{e}"));
                    }
                }
            }
            vector_cache = Some(guard);
        }
    }
    let search = crate::embed::HybridSearch {
        lexical: index,
        vectors: vector_cache.as_ref().and_then(|guard| guard.as_ref()),
    };
    let mut outcome =
        search.ranked(requested, &query, &embedding, collection.as_deref(), top_k);
    // キャッシュを読めなかったのが根の理由なら、そちらを載せる(「索引がない」だけでは
    // 何を直せばよいか読めない)。
    if load_failure.is_some() {
        outcome.degraded = load_failure;
    }
    if let Some(reason) = &outcome.degraded {
        eprintln!(
            "uniqnode: search: {} を求められて {} で答えた: {reason}",
            requested.as_str(),
            outcome.method.as_str()
        );
    }
    Response::json(200, search_response_body(index, &outcome))
}

/// JSON 文字列 1 個ぶんの直列化(引用符・エスケープ込み)。検索応答は score が小数で
/// c1(整数のみ)では表せないため手で組むが、文字列のエスケープは c1 の直列化を通して
/// 実装を増やさない(should/0135)。
fn json_text(text: &str) -> String {
    String::from_utf8(c1::to_canonical_bytes(&c1::Value::Text(text.to_string())))
        .expect("c1 直列化は UTF-8")
}

/// 検索応答の本文。results の各件は snippet(チャンク本文の先頭)・id(チャンク ID。
/// 全文の取得は既存の GET /v1/objects/{id})・score・citation(INGEST の「文書モデル」節
/// の引用規則: document は ref パスから collections/<コレクション名>/ を除いた残り、
/// position は chunks 列の添字、breadcrumbs、PDF なら page)。
///
/// method は実際に使った方式、score_semantics はその方式の得点の意味("bm25" の得点・
/// "cosine"・順位から作った "rrf")である。どれも同一応答内の順位付けにだけ意味があり、
/// 応答をまたいだ比較や絶対値の閾値には使えない。degraded は、要求した方式で答えられ
/// なかったときだけ現れる理由である。
fn search_response_body(
    index: &crate::search::SearchIndex,
    outcome: &crate::embed::RankedSearch,
) -> Vec<u8> {
    let mut results = Vec::new();
    for hit in &outcome.hits {
        let chunk = index.chunk(hit.position);
        let breadcrumbs: Vec<String> =
            chunk.breadcrumbs.iter().map(|title| json_text(title)).collect();
        let mut citation = format!(
            "{{\"breadcrumbs\":[{}],\"document\":{}",
            breadcrumbs.join(","),
            json_text(&chunk.document)
        );
        if let Some(page) = chunk.page {
            citation.push_str(&format!(",\"page\":{page}"));
        }
        citation.push_str(&format!(",\"position\":{}}}", chunk.position));
        results.push(format!(
            "{{\"citation\":{citation},\"id\":{},\"score\":{},\"snippet\":{}}}",
            json_text(&chunk.id),
            hit.score,
            json_text(&chunk.snippet),
        ));
    }
    let degraded = match &outcome.degraded {
        Some(reason) => format!("\"degraded\":{},", json_text(reason)),
        None => String::new(),
    };
    format!(
        "{{{degraded}\"method\":\"{}\",\"results\":[{}],\"score_semantics\":\"{}\"}}",
        outcome.method.as_str(),
        results.join(","),
        outcome.method.score_semantics(),
    )
    .into_bytes()
}

fn handle_with_path_argument(context: &ApiContext, request: &Request) -> Response {
    let store = &*context.store;
    let path = request.path.as_str();
    let method = request.method.as_str();

    if let Some(id) = path.strip_prefix("/v1/queries/") {
        if method != "GET" {
            return error_response(405, "GET のみ");
        }
        return match context.engine.lookup(id) {
            Some(shared) => Response::json(200, crate::query::state_to_json(&shared.snapshot())),
            None => error_response(404, "unknown query handle"),
        };
    }

    if let Some(query) = path.strip_prefix("/v1/replication/refs?") {
        if method != "GET" {
            return error_response(405, "GET のみ");
        }
        let mut signer = None;
        let mut since = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("signer", value)) => signer = Some(value.to_string()),
                Some(("since", value)) => since = value.parse::<u64>().ok(),
                _ => {}
            }
        }
        let (signer, since) = match (signer, since) {
            (Some(s), Some(n)) => (s, n),
            _ => return error_response(400, "signer と since が必要"),
        };
        let store = store.lock().expect("lock");
        return match store.export_ref_records(&signer, since) {
            Ok(payloads) => {
                let mut records = Vec::new();
                for payload in payloads {
                    let text = match std::str::from_utf8(&payload) {
                        Ok(t) => t,
                        Err(_) => return error_response(500, "レコードが UTF-8 でない"),
                    };
                    match c1::parse(text) {
                        Ok(value) => records.push(value),
                        Err(e) => {
                            return error_response(500, &format!("レコードが c1 でない: {e}"))
                        }
                    }
                }
                Response::json(200, json_object(vec![("records", c1::Value::Array(records))]))
            }
            Err(e) => store_error_response(e),
        };
    }

    if let Some(rest) = path.strip_prefix("/v1/objects/") {
        // 逆引き(INGEST の「逆引き」節): この ID を参照している既知オブジェクトの一覧。
        if let Some(id) = rest.strip_suffix("/referrers") {
            if method != "GET" {
                return error_response(405, "GET のみ");
            }
            if !c1::is_object_id(id) {
                return error_response(400, "オブジェクトIDの形式が不正");
            }
            let store = store.lock().expect("lock");
            let mut cache = context.referrers.lock().expect("lock");
            // 遅延構築。オブジェクトは追記専用なので、構築時点の object_count が現在値と
            // 一致する限り索引は最新。書き込みが挟まれば次の要求で作り直す。
            if !cache.as_ref().is_some_and(|index| index.is_current(&store)) {
                match crate::store::ReferrerIndex::build(&store) {
                    Ok(index) => *cache = Some(index),
                    Err(e) => return store_error_response(e),
                }
            }
            let index = cache.as_ref().expect("直前に構築した");
            // ローカルに無い ID も referrers 空の 200(逆引きは「自分の知る範囲」の
            // 導出データであり、空は不在の言明ではない)。
            let referrers: Vec<c1::Value> =
                index.referrers_of(id).iter().cloned().map(c1::Value::Text).collect();
            return Response::json(
                200,
                json_object(vec![("referrers", c1::Value::Array(referrers))]),
            );
        }
        let id = rest;
        if method != "GET" {
            return error_response(405, "GET のみ");
        }
        if !c1::is_object_id(id) {
            return error_response(400, "オブジェクトIDの形式が不正");
        }
        let store = store.lock().expect("lock");
        return match store.get_object(id) {
            Ok(Some(bytes)) => Response::bytes(200, bytes),
            Ok(None) => error_response(404, "not held locally"),
            Err(e) => store_error_response(e),
        };
    }

    if let Some(id) = path.strip_prefix("/v1/closure/") {
        if method != "GET" {
            return error_response(405, "GET のみ");
        }
        if !c1::is_object_id(id) {
            return error_response(400, "オブジェクトIDの形式が不正");
        }
        let store = store.lock().expect("lock");
        return match store.reachable_closure(id) {
            Ok(members) => Response::json(
                200,
                json_object(vec![(
                    "members",
                    c1::Value::Array(members.into_iter().map(c1::Value::Text).collect()),
                )]),
            ),
            Err(e) => store_error_response(e),
        };
    }

    if path == "/v1/refs" {
        if method != "GET" {
            return error_response(405, "GET のみ");
        }
        let store = store.lock().expect("lock");
        let refs: Vec<c1::Value> = store
            .list_refs()
            .map(|(name, state)| {
                let mut map = BTreeMap::new();
                map.insert("name".to_string(), c1::Value::Text(name.clone()));
                map.insert(
                    "target".to_string(),
                    match &state.target {
                        Some(t) => c1::Value::Text(t.clone()),
                        None => c1::Value::Null,
                    },
                );
                map.insert("seq".to_string(), c1::Value::Integer(state.seq as i64));
                map.insert("at".to_string(), c1::Value::Integer(state.at));
                c1::Value::Object(map)
            })
            .collect();
        return Response::json(200, json_object(vec![("refs", c1::Value::Array(refs))]));
    }

    if let Some(rest) = path.strip_prefix("/v1/refs/") {
        match method {
            // 完全名(<node_id>/<path>)での解決。
            "GET" => {
                let store = store.lock().expect("lock");
                return match store.get_ref(rest) {
                    Some(state) => Response::json(
                        200,
                        json_object(vec![
                            ("name", c1::Value::Text(rest.to_string())),
                            (
                                "target",
                                match &state.target {
                                    Some(t) => c1::Value::Text(t.clone()),
                                    None => c1::Value::Null,
                                },
                            ),
                            ("seq", c1::Value::Integer(state.seq as i64)),
                            ("at", c1::Value::Integer(state.at)),
                        ]),
                    ),
                    None => error_response(404, "not held locally"),
                };
            }
            // 自名前空間のパスへの書き込み。ボディ: {"target": "s256:…" | null}
            "PUT" => {
                let body_text = match std::str::from_utf8(&request.body) {
                    Ok(t) => t,
                    Err(_) => return error_response(400, "ボディが UTF-8 でない"),
                };
                let value = match c1::parse(body_text) {
                    Ok(v) => v,
                    Err(e) => return error_response(400, &format!("JSON が不正: {e}")),
                };
                let target = match &value {
                    c1::Value::Object(map) => match map.get("target") {
                        Some(c1::Value::Text(t)) => Some(t.clone()),
                        Some(c1::Value::Null) => None,
                        _ => return error_response(400, "target がない(文字列か null)"),
                    },
                    _ => return error_response(400, "ボディはオブジェクトであるべき"),
                };
                let mut store = store.lock().expect("lock");
                return match store.set_ref(rest, target.as_deref()) {
                    Ok(seq) => Response::json(
                        200,
                        json_object(vec![
                            ("name", c1::Value::Text(store.own_ref_name(rest))),
                            ("seq", c1::Value::Integer(seq as i64)),
                        ]),
                    ),
                    Err(e) => store_error_response(e),
                };
            }
            _ => return error_response(405, "GET か PUT のみ"),
        }
    }

    // 文書の取り込み(INGEST の「CLI と API」節)。本文は生バイト列、種別は
    // {name} の拡張子で判定する。
    if let Some(rest) = path.strip_prefix("/v1/collections/") {
        if method != "PUT" {
            return error_response(405, "PUT のみ");
        }
        let Some((collection, name)) = rest.split_once("/documents/") else {
            return error_response(404, "/v1/collections/{c}/documents/{name} の形");
        };
        if collection.is_empty() || name.is_empty() {
            return error_response(400, "コレクション名と文書名が要る");
        }
        let Some((stem, extension)) = name.rsplit_once('.') else {
            return error_response(400, "文書名に拡張子が要る(.md/.markdown/.txt/.pdf)");
        };
        let Some(media) = crate::ingest::media_for_extension(extension) else {
            return error_response(400, "対象外の拡張子(.md/.markdown/.txt/.pdf のみ)");
        };
        let extracted;
        let mut extractor_label = None;
        let text: &str = if media == "pdf" {
            // blob は PDF バイナリそのもの、チャンクは pdftotext の抽出テキストから作る。
            let extractor = match pdf_extractor() {
                Ok(extractor) => extractor,
                // 委譲先が無いのはこの過程の一時的な状態であって要求の誤りではない。
                Err(message) => return error_response(503, &message),
            };
            extractor_label = Some(extractor.extractor.as_str());
            match extractor.extract(&request.body) {
                Ok(text) => extracted = text,
                Err(e) => return store_error_response(e),
            }
            &extracted
        } else {
            match std::str::from_utf8(&request.body) {
                Ok(text) => text,
                Err(_) => return error_response(400, "ボディが UTF-8 でない"),
            }
        };
        let chunks = crate::ingest::chunk_for_media(media, text);
        let input = crate::ingest::DocumentInput {
            collection,
            name: stem,
            source: &request.body,
            media,
            chunks: &chunks,
            extractor: extractor_label,
        };
        let mut store = store.lock().expect("lock");
        return match crate::ingest::ingest_document(&mut store, &input) {
            Ok(outcome) => Response::json(
                200,
                json_object(vec![
                    ("doc_rev", c1::Value::Text(outcome.doc_rev_id)),
                    ("new_objects", c1::Value::Integer(outcome.new_objects as i64)),
                    ("ref_updated", c1::Value::Bool(outcome.ref_updated)),
                ]),
            ),
            Err(e) => store_error_response(e),
        };
    }

    error_response(404, "no such endpoint")
}
