//! ノードローカル API(SPEC §10)。HTTP の上に store の操作を露出する。
//! 404 は「このDBノードは持っていない」というローカルな事実であって、ネットワークに
//! 対する不存在の言明ではない(SPEC §7.2/§10)。

use crate::c1;
use crate::http::{Request, Response};
use crate::json::Json;
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

/// 検索要求(POST /v1/search のボディと、MCP の search ツールの引数は同じ形である)。
/// 読み取りと検証の家は parse_search_request の一箇所で、REST も MCP もそこを通る
/// (should/0135)。
pub struct SearchRequest {
    pub query: String,
    /// 省略時は全コレクション。
    pub collection: Option<String>,
    pub top_k: usize,
    /// 省略時は None。装備に従う既定を決めるのは run_search である。
    pub method: Option<crate::embed::SearchMethod>,
}

/// 引用(取り込み層の引用規則。INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の
/// 「文書モデル」節): document は ref パスから collections/<コレクション名>/ を除いた
/// 残り、position は chunks 列の添字、breadcrumbs は見出しの入れ子パス、PDF はさらに
/// page、at はその版を見えに置いた ref レコードの時刻(取得日時。unix 秒)。
pub struct Citation {
    pub collection: String,
    pub document: String,
    pub position: usize,
    pub page: Option<u32>,
    pub breadcrumbs: Vec<String>,
    pub at: i64,
}

impl Citation {
    /// 索引済みチャンクから引用を組む(組み立ての家はここだけ。should/0135)。
    fn of(chunk: &crate::search::IndexedChunk) -> Citation {
        Citation {
            collection: chunk.collection.clone(),
            document: chunk.document.clone(),
            position: chunk.position,
            page: chunk.page,
            breadcrumbs: chunk.breadcrumbs.clone(),
            at: chunk.at,
        }
    }
}

/// 検索結果 1 件。id はチャンクのオブジェクト ID(全文は GET /v1/objects/{id} か、
/// MCP の fetch ツールで取る)。
pub struct SearchResult {
    pub id: String,
    pub score: f64,
    pub snippet: String,
    pub citation: Citation,
}

/// 検索 1 回の答え。method は実際に使った方式、degraded は要求した方式で答えられな
/// かったときだけ現れる理由である(黙って劣化しない。should/0128)。
pub struct SearchResults {
    pub method: crate::embed::SearchMethod,
    pub degraded: Option<String>,
    pub results: Vec<SearchResult>,
}

/// 検索要求の読み取りと検証。誤りの文言は呼び手がそのまま使う(REST は 400 の本文、
/// MCP は JSON-RPC の invalid params の message)。
///
/// method は "bm25"(語の一致)・"embedding"(意味の近さ)・"hybrid"(両者の RRF 融合)。
pub fn parse_search_request(value: &c1::Value) -> Result<SearchRequest, String> {
    let map = match value {
        c1::Value::Object(m) => m,
        _ => return Err("要求はオブジェクトであるべき".to_string()),
    };
    let query = match map.get("query") {
        Some(c1::Value::Text(t)) if !t.is_empty() => t.clone(),
        _ => return Err("query がない(空でない文字列)".to_string()),
    };
    let collection = match map.get("collection") {
        None | Some(c1::Value::Null) => None,
        Some(c1::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
        _ => return Err("collection は空でない文字列".to_string()),
    };
    let top_k = match map.get("top_k") {
        None | Some(c1::Value::Null) => 10usize,
        Some(c1::Value::Integer(n)) if (1..=1000).contains(n) => *n as usize,
        _ => return Err("top_k は 1..=1000 の整数".to_string()),
    };
    let method = match map.get("method") {
        None | Some(c1::Value::Null) => None,
        Some(c1::Value::Text(t)) => match crate::embed::SearchMethod::parse(t) {
            Some(method) => Some(method),
            None => {
                return Err("method は \"bm25\"・\"embedding\"・\"hybrid\" のどれか".to_string())
            }
        },
        _ => return Err("method は文字列".to_string()),
    };
    if crate::search::terms_of(&query).is_empty() {
        return Err("クエリに索引語が無い(英数字の語か、仮名・漢字などの文字が要る)".to_string());
    }
    Ok(SearchRequest { query, collection, top_k, method })
}

/// 検索索引を最新にして貸す。索引は導出データの遅延キャッシュで、referrers と同じく
/// 初回要求時に構築し、世代がずれたら次の要求で作り直す(検索と全文取得が共用する
/// 唯一の入口。should/0135)。store のロックは呼び手が持つ。
fn with_current_index<T>(
    context: &ApiContext,
    store: &Store,
    use_index: impl FnOnce(&crate::search::SearchIndex) -> T,
) -> Result<T, StoreError> {
    let mut cache = context.search.lock().expect("lock");
    if !cache.as_ref().is_some_and(|index| index.is_current(store)) {
        *cache = Some(crate::search::SearchIndex::build(store)?);
    }
    Ok(use_index(cache.as_ref().expect("直前に構築した")))
}

/// 検索の本体(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))。方式の既定・劣化の
/// 判断・引用の組み立てはここ 1 箇所にあり、REST の POST /v1/search と MCP の search
/// ツールはどちらもこれを呼ぶ(検索の判断を二重に実装しない。should/0135)。
///
/// 埋め込みが使えないときは BM25 だけに劣化して答え、返り値の method(実際に使った
/// 方式)と degraded(理由)がそれを語る。理由は標準エラーにも残すので、どちらの
/// 呼び手から来ても劣化は観測できる(黙って劣化しない。should/0128)。
pub fn run_search(
    context: &ApiContext,
    request: &SearchRequest,
) -> Result<SearchResults, StoreError> {
    // 既定は装備に従う。埋め込みを設定した節点で融合を既定にするのは、方式の指定を
    // 知らない呼び手(CLI・MCP)が黙って BM25 だけに取り残されないため。
    let requested = request.method.unwrap_or(match &context.embedding {
        Some(_) => crate::embed::SearchMethod::Hybrid,
        None => crate::embed::SearchMethod::Bm25,
    });
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
            &request.query,
        ),
    };

    let store = context.store.lock().expect("lock");
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
    let mut outcome = with_current_index(context, &store, |index| {
        let search = crate::embed::HybridSearch {
            lexical: index,
            vectors: vector_cache.as_ref().and_then(|guard| guard.as_ref()),
        };
        let ranked = search.ranked(
            requested,
            &request.query,
            &embedding,
            request.collection.as_deref(),
            request.top_k,
        );
        let results = ranked
            .hits
            .iter()
            .map(|hit| {
                let chunk = index.chunk(hit.position);
                SearchResult {
                    id: chunk.id.clone(),
                    score: hit.score,
                    snippet: chunk.snippet.clone(),
                    citation: Citation::of(chunk),
                }
            })
            .collect();
        SearchResults { method: ranked.method, degraded: ranked.degraded, results }
    })?;
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
    Ok(outcome)
}

/// 全文取得(MCP の fetch ツール)が返す 1 件。REST では GET /v1/objects/{id} が同じ
/// バイト列を返すが、そちらは生のオブジェクトだけで引用は組まない。
pub enum Fetched {
    /// チャンクの全文。citation は見え(collections/ 配下の現行 doc_rev)にあるときだけ
    /// 組める。旧版のチャンクは ID で取れても見えには無い。
    Chunk { text: String, citation: Option<Citation> },
    /// チャンクでない c1 オブジェクト(doc_rev・注釈など)。正規形のまま返す。
    Object { text: String },
    /// テキストでないバイト列(PDF の原文 blob など)。全文の代わりに大きさを言う。
    Binary { bytes: usize },
}

/// オブジェクト ID から全文を取る。ローカルに無ければ None(「このDBノードは持って
/// いない」というローカルな事実であって、不存在の言明ではない。SPEC §7.2/§10)。
/// 引用は検索索引が持つ(同じ索引を使うので、検索の出典と全文の出典は一致する)。
pub fn fetch_object(context: &ApiContext, id: &str) -> Result<Option<Fetched>, StoreError> {
    let store = context.store.lock().expect("lock");
    let Some(bytes) = store.get_object(id)? else { return Ok(None) };
    match classify_object(bytes) {
        Fetched::Chunk { text, .. } => {
            let citation = citation_in_view(context, &store, id)?;
            Ok(Some(Fetched::Chunk { text, citation }))
        }
        other => Ok(Some(other)),
    }
}

/// 見え(collections/ 配下の現行 doc_rev)にあるチャンクの引用。ID で取れても見えに無い
/// チャンク(旧版など)は None である。store のロックは呼び手が持つ。
fn citation_in_view(
    context: &ApiContext,
    store: &Store,
    id: &str,
) -> Result<Option<Citation>, StoreError> {
    with_current_index(context, store, |index| index.chunk_by_id(id).map(Citation::of))
}

/// POST /v1/search(SEARCH (uuid:19574e78-9bf5-4f87-a4c2-c4a10222c580))。ボディ:
/// {"query": "...", "collection": "...", "top_k": N, "method": "..."}(collection 省略時は
/// 全コレクション、top_k 省略時は 10)。判断は run_search が持ち、ここは HTTP の被せ物で
/// ある。
fn handle_search(context: &ApiContext, request: &Request) -> Response {
    let body_text = match std::str::from_utf8(&request.body) {
        Ok(t) => t,
        Err(_) => return error_response(400, "ボディが UTF-8 でない"),
    };
    let value = match c1::parse(body_text) {
        Ok(v) => v,
        Err(e) => return error_response(400, &format!("JSON が不正: {e}")),
    };
    let search_request = match parse_search_request(&value) {
        Ok(request) => request,
        Err(message) => return error_response(400, &message),
    };
    match run_search(context, &search_request) {
        Ok(results) => Response::json(200, search_response_body(&results)),
        Err(e) => store_error_response(e),
    }
}

/// JSON 文字列 1 個ぶんの直列化(引用符・エスケープ込み)。検索応答は score が小数で
/// c1(整数のみ)では表せないため手で組むが、文字列のエスケープは c1 の直列化を通して
/// 実装を増やさない(should/0135)。
fn json_text(text: &str) -> String {
    String::from_utf8(c1::to_canonical_bytes(&c1::Value::Text(text.to_string())))
        .expect("c1 直列化は UTF-8")
}

/// 検索応答の本文。results の各件は snippet(チャンク本文の先頭)・id(チャンク ID。
/// 全文の取得は既存の GET /v1/objects/{id})・score・citation(引用規則は Citation)。
/// citation の at は取得日時(その版を見えに置いた ref レコードの時刻。unix 秒)である。
///
/// method は実際に使った方式、score_semantics はその方式の得点の意味("bm25" の得点・
/// "cosine"・順位から作った "rrf")である。どれも同一応答内の順位付けにだけ意味があり、
/// 応答をまたいだ比較や絶対値の閾値には使えない。degraded は、要求した方式で答えられ
/// なかったときだけ現れる理由である。
pub fn search_response_body(outcome: &SearchResults) -> Vec<u8> {
    let mut results = Vec::new();
    for result in &outcome.results {
        let rendered = json_line(&citation_value(&result.citation));
        results.push(format!(
            "{{\"citation\":{rendered},\"id\":{},\"score\":{},\"snippet\":{}}}",
            json_text(&result.id),
            result.score,
            json_text(&result.snippet),
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

/// c1 の正規形を 1 行の文字列にする(c1 は JSON の部分集合なので、そのまま JSON の
/// 一部として埋め込める)。
fn json_line(value: &c1::Value) -> String {
    String::from_utf8(c1::to_canonical_bytes(value)).expect("c1 直列化は UTF-8")
}

// ---- REST の線の上の形(書き手と読み手を並べて置く) ----
//
// 検索の応答と引用は、serve が書き、二人が読む: REST の呼び手と、走っている serve へ
// 転送する形の MCP(node/src/mcp.rs)である。書き手と読み手を隣り合わせに置くのは、
// 形を変えるときに片方だけが直る事故を防ぐためである(should/0135。両者が噛み合うことは
// 単体試験 rest_search_bodies_survive_a_round_trip が確かめる)。

/// 引用の JSON。検索応答の citation と GET /v1/objects/{id}/citation はこの 1 箇所から
/// 出る。値はすべて整数・文字列・配列なので c1 の正規形で書ける(小数を持つのは score
/// だけである)。
pub fn citation_value(citation: &Citation) -> c1::Value {
    let mut map = BTreeMap::new();
    map.insert("at".to_string(), c1::Value::Integer(citation.at));
    map.insert(
        "breadcrumbs".to_string(),
        c1::Value::Array(
            citation.breadcrumbs.iter().map(|title| c1::Value::Text(title.clone())).collect(),
        ),
    );
    map.insert("collection".to_string(), c1::Value::Text(citation.collection.clone()));
    map.insert("document".to_string(), c1::Value::Text(citation.document.clone()));
    if let Some(page) = citation.page {
        map.insert("page".to_string(), c1::Value::Integer(page as i64));
    }
    map.insert("position".to_string(), c1::Value::Integer(citation.position as i64));
    c1::Value::Object(map)
}

/// 引用を JSON から組み直す(citation_value の裏返し)。欠けた項は黙って埋めず、何が
/// 足りないのかを言って失敗する(must/0022)。
pub fn citation_from_json(value: &Json) -> Result<Citation, String> {
    let text = |name: &str| -> Result<String, String> {
        value
            .field(name)
            .and_then(Json::text)
            .map(|found| found.to_string())
            .ok_or_else(|| format!("citation の {name} がない(文字列)"))
    };
    let integer = |name: &str| -> Result<i64, String> {
        value
            .field(name)
            .and_then(Json::integer)
            .ok_or_else(|| format!("citation の {name} がない(整数)"))
    };
    let breadcrumbs = match value.field("breadcrumbs").and_then(Json::array) {
        Some(items) => items
            .iter()
            .map(|item| {
                item.text()
                    .map(|title| title.to_string())
                    .ok_or_else(|| "citation の breadcrumbs は文字列の配列".to_string())
            })
            .collect::<Result<Vec<String>, String>>()?,
        None => return Err("citation の breadcrumbs がない(配列)".to_string()),
    };
    let position = integer("position")?;
    let page = match value.field("page") {
        None | Some(Json::Null) => None,
        Some(found) => Some(
            found
                .integer()
                .filter(|page| (0..=u32::MAX as i64).contains(page))
                .ok_or_else(|| "citation の page が非負整数でない".to_string())?
                as u32,
        ),
    };
    Ok(Citation {
        collection: text("collection")?,
        document: text("document")?,
        position: usize::try_from(position).map_err(|_| "citation の position が負".to_string())?,
        page,
        breadcrumbs,
        at: integer("at")?,
    })
}

/// 検索要求の本文(POST /v1/search のボディ)。読み取りは parse_search_request の
/// 一箇所なので、書き手もここ 1 箇所に置く。省略できる項は、省略時の既定を持つ側
/// (parse_search_request)に任せず、決まった値をそのまま書く。
pub fn search_request_body(request: &SearchRequest) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert("query".to_string(), c1::Value::Text(request.query.clone()));
    if let Some(collection) = &request.collection {
        map.insert("collection".to_string(), c1::Value::Text(collection.clone()));
    }
    map.insert("top_k".to_string(), c1::Value::Integer(request.top_k as i64));
    if let Some(method) = request.method {
        map.insert("method".to_string(), c1::Value::Text(method.as_str().to_string()));
    }
    c1::to_canonical_bytes(&c1::Value::Object(map))
}

/// 検索応答を読む(search_response_body の裏返し)。score が小数なので c1 では読めず、
/// 小数を読める最小の JSON(node/src/json.rs)を通す。
pub fn parse_search_response(body: &[u8]) -> Result<SearchResults, String> {
    let text = std::str::from_utf8(body).map_err(|_| "応答が UTF-8 でない".to_string())?;
    let value = Json::parse(text)?;
    let method = match value.field("method").and_then(Json::text) {
        Some(name) => crate::embed::SearchMethod::parse(name)
            .ok_or_else(|| format!("応答の method が知らない方式: {name}"))?,
        None => return Err("応答に method がない".to_string()),
    };
    let degraded = match value.field("degraded") {
        None | Some(Json::Null) => None,
        Some(found) => Some(
            found
                .text()
                .ok_or_else(|| "応答の degraded が文字列でない".to_string())?
                .to_string(),
        ),
    };
    let items = value.field("results").and_then(Json::array).ok_or("応答に results がない")?;
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .field("id")
            .and_then(Json::text)
            .ok_or_else(|| "results の要素に id がない".to_string())?
            .to_string();
        let score = item
            .field("score")
            .and_then(Json::number)
            .ok_or_else(|| "results の要素に score がない".to_string())?;
        let snippet = item
            .field("snippet")
            .and_then(Json::text)
            .ok_or_else(|| "results の要素に snippet がない".to_string())?
            .to_string();
        let citation = match item.field("citation") {
            Some(found) => citation_from_json(found)?,
            None => return Err("results の要素に citation がない".to_string()),
        };
        results.push(SearchResult { id, score, snippet, citation });
    }
    Ok(SearchResults { method, degraded, results })
}

/// オブジェクトのバイト列がどれなのかを見分ける(チャンクの本文・チャンクでない c1
/// オブジェクト・テキストでないバイト列)。引用は付けない。ストアから直に読んだときも、
/// REST の GET /v1/objects/{id} から読んだときも、この 1 箇所を通る(should/0135)。
pub fn classify_object(bytes: Vec<u8>) -> Fetched {
    let byte_count = bytes.len();
    let Ok(text) = String::from_utf8(bytes) else {
        return Fetched::Binary { bytes: byte_count };
    };
    let chunk_text = match c1::parse(&text) {
        Ok(c1::Value::Object(map)) => match (map.get("kind"), map.get("text")) {
            (Some(c1::Value::Text(kind)), Some(c1::Value::Text(body))) if kind == "chunk" => {
                Some(body.clone())
            }
            _ => None,
        },
        _ => None,
    };
    match chunk_text {
        Some(chunk_text) => Fetched::Chunk { text: chunk_text, citation: None },
        None => Fetched::Object { text },
    }
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
        // 出典(引用規則は Citation)。全文そのものは GET /v1/objects/{id} が返すので、
        // ここは引用だけを返す。走っている serve へ転送する形の MCP(node/src/mcp.rs)が
        // fetch の出典を組むために呼ぶ。見えに無いチャンクと、チャンクでない ID は
        // citation:null(持っていない ID も同じ。逆引きと同じく「自分の見えの範囲」の
        // 導出データであり、不在の言明ではない)。
        if let Some(id) = rest.strip_suffix("/citation") {
            if method != "GET" {
                return error_response(405, "GET のみ");
            }
            if !c1::is_object_id(id) {
                return error_response(400, "オブジェクトIDの形式が不正");
            }
            let store = store.lock().expect("lock");
            return match citation_in_view(context, &store, id) {
                Ok(Some(citation)) => {
                    Response::json(200, json_object(vec![("citation", citation_value(&citation))]))
                }
                Ok(None) => Response::json(200, json_object(vec![("citation", c1::Value::Null)])),
                Err(e) => store_error_response(e),
            };
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// REST の線の上の形は、書き手と読み手が噛み合う。走っている serve へ転送する形の
    /// MCP は、この読み手だけを頼りに出典を組み直すので、片方だけが直ると出典が消える。
    /// 期待値は書き手の出力から導かず、線の上の文字列をリテラルで置く(should/0137)。
    #[test]
    fn rest_search_bodies_survive_a_round_trip() {
        let outcome = SearchResults {
            method: crate::embed::SearchMethod::Hybrid,
            degraded: Some("埋め込みサーバに届かない".to_string()),
            results: vec![
                SearchResult {
                    id: format!("s256:{}", "1".repeat(64)),
                    score: 7.6674,
                    // 本文の改行は c1 の直列化がエスケープに畳む(読み手が戻す)。
                    snippet: "転置索引は\n導出データ".to_string(),
                    citation: Citation {
                        collection: "notes".to_string(),
                        document: "search_ja".to_string(),
                        position: 1,
                        page: None,
                        breadcrumbs: vec!["分散設計".to_string(), "世代の整合".to_string()],
                        at: 1_786_904_557,
                    },
                },
                SearchResult {
                    id: format!("s256:{}", "2".repeat(64)),
                    score: 0.5,
                    snippet: "Page two".to_string(),
                    citation: Citation {
                        collection: "specs".to_string(),
                        document: "three_pages".to_string(),
                        position: 0,
                        page: Some(2),
                        breadcrumbs: Vec::new(),
                        at: 1_786_904_600,
                    },
                },
            ],
        };
        let body = search_response_body(&outcome);
        let text = String::from_utf8(body.clone()).expect("utf-8");
        // 出典はコレクション名も持つ(転送する形の MCP は「<コレクション>/<文書>」と
        // 描くので、これが無いと出典が組めない)。
        assert!(
            text.contains(
                "\"citation\":{\"at\":1786904557,\"breadcrumbs\":[\"分散設計\",\"世代の整合\"],\
                 \"collection\":\"notes\",\"document\":\"search_ja\",\"position\":1}"
            ),
            "{text}"
        );
        assert!(text.contains("\"page\":2"), "PDF のチャンクはページを持つ: {text}");

        let read = parse_search_response(&body).expect("読み直せるべき");
        assert_eq!(read.method.as_str(), "hybrid");
        assert_eq!(read.degraded.as_deref(), Some("埋め込みサーバに届かない"));
        assert_eq!(read.results.len(), 2);
        assert_eq!(read.results[0].id, format!("s256:{}", "1".repeat(64)));
        assert!((read.results[0].score - 7.6674).abs() < 1e-12, "{}", read.results[0].score);
        assert_eq!(read.results[0].snippet, "転置索引は\n導出データ");
        assert_eq!(read.results[0].citation.collection, "notes");
        assert_eq!(read.results[0].citation.document, "search_ja");
        assert_eq!(read.results[0].citation.position, 1);
        assert_eq!(read.results[0].citation.page, None);
        assert_eq!(read.results[0].citation.breadcrumbs, vec!["分散設計", "世代の整合"]);
        assert_eq!(read.results[0].citation.at, 1_786_904_557);
        assert_eq!(read.results[1].citation.page, Some(2));
        assert!(read.results[1].citation.breadcrumbs.is_empty());

        // 欠けた形は黙って通さない(must/0022)。
        let missing =
            match parse_search_response(b"{\"method\":\"bm25\",\"results\":[{\"id\":\"x\"}]}") {
                Ok(_) => panic!("score も snippet も無い応答を通してはならない"),
                Err(message) => message,
            };
        assert!(missing.contains("score"), "{missing}");
        let no_citation = match parse_search_response(
            b"{\"method\":\"bm25\",\"results\":[{\"citation\":{\"at\":1},\"id\":\"x\",\
              \"score\":1.0,\"snippet\":\"y\"}]}",
        ) {
            Ok(_) => panic!("citation の欠けた応答を通してはならない"),
            Err(message) => message,
        };
        assert!(no_citation.contains("citation の"), "{no_citation}");
    }

    /// 検索要求も、書いた本文がそのまま parse_search_request を通る(転送する形の MCP は
    /// 自分で検証してから同じ本文を serve へ送る)。
    #[test]
    fn a_search_request_body_reads_back_as_the_same_request() {
        let request = SearchRequest {
            query: "世代の整合".to_string(),
            collection: Some("notes".to_string()),
            top_k: 3,
            method: Some(crate::embed::SearchMethod::Bm25),
        };
        let body = search_request_body(&request);
        assert_eq!(
            String::from_utf8(body.clone()).expect("utf-8"),
            "{\"collection\":\"notes\",\"method\":\"bm25\",\"query\":\"世代の整合\",\"top_k\":3}"
        );
        let value = c1::parse(std::str::from_utf8(&body).expect("utf-8")).expect("c1");
        let read = parse_search_request(&value).expect("読み直せるべき");
        assert_eq!(read.query, "世代の整合");
        assert_eq!(read.collection.as_deref(), Some("notes"));
        assert_eq!(read.top_k, 3);
        assert_eq!(read.method.map(|method| method.as_str()), Some("bm25"));

        // 省略できる項は省いたまま書く(既定は読み手が持つ)。
        let bare = SearchRequest {
            query: "x".to_string(),
            collection: None,
            top_k: 10,
            method: None,
        };
        assert_eq!(
            String::from_utf8(search_request_body(&bare)).expect("utf-8"),
            "{\"query\":\"x\",\"top_k\":10}"
        );
    }

    /// バイト列の見分けは 1 箇所(classify_object)で、ストアから読んでも REST から
    /// 読んでも同じ答えになる。
    #[test]
    fn object_bytes_are_classified_as_chunk_object_or_binary() {
        let chunk = b"{\"kind\":\"chunk\",\"text\":\"\xe6\x9c\xac\xe6\x96\x87\"}".to_vec();
        match classify_object(chunk) {
            Fetched::Chunk { text, citation } => {
                assert_eq!(text, "本文");
                // 引用は索引が持つ。バイト列だけからは組めない。
                assert!(citation.is_none());
            }
            _ => panic!("チャンクのはず"),
        }
        match classify_object(b"{\"kind\":\"doc_rev\",\"v\":1}".to_vec()) {
            Fetched::Object { text } => assert_eq!(text, "{\"kind\":\"doc_rev\",\"v\":1}"),
            _ => panic!("チャンクでない c1 オブジェクトのはず"),
        }
        match classify_object(vec![0xff, 0xfe, 0x00]) {
            Fetched::Binary { bytes } => assert_eq!(bytes, 3),
            _ => panic!("テキストでないバイト列のはず"),
        }
    }
}
