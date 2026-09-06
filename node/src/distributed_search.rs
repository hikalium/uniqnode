//! 分散検索(SPEC §7.1 の kind:search。DISTRIBUTED_SEARCH
//! (uuid:e577f6db-659e-4eb8-a152-3b7780e4a9d1))。
//!
//! 検索クエリを登録ピアへ 1 ホップで散布し、各DBノードのローカル top-k を順位ベースで
//! 融合する。順位だけを使うので、DBノード間で得点を較正する必要がない(これが RRF を
//! 採る理由であり、線に得点を載せない理由でもある)。
//!
//! この層が持つのは四つである。
//! - QUERY / ANSWER の形と、その署名・検証(SPEC §6.1: プロトコルメッセージは発行
//!   DBノードの署名を持つ)。要求者を名乗りだけで信じない。
//! - 応答側の双方向フィルタ(SPEC §7.1): 要求者の DBノードID を peers.json で引き、
//!   trust_level が 0 なら答えず、share.collections が挙げたコレクションだけを見る。
//! - 散布と決着(SPEC §7.2 の 3 値。判定そのものは crate::query::settlement が持つ)。
//! - 順位の融合(crate::embed::fuse_by_rank。融合の家は 1 つ)。
//!
//! 検索そのもの(方式の既定・劣化の判断・引用の組み立て)は node/src/api.rs の run_search
//! が持ち、この層は呼ぶだけである(検索の判断を二重に実装しない。should/0135)。

use crate::api::{Citation, SearchRequest, SearchResult, SearchResults};
use crate::c1;
use crate::clock::unix_now;
use crate::embed::fuse_by_rank;
use crate::json::Json;
use crate::query::{settlement, PeerEntry, PeerState, QueryOutcome};
use crate::search::CollectionScope;
use crate::store::Store;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// ピアからの QUERY を受ける口(応答側)。自分が発信者になる口(POST /v1/query、
/// POST /v1/search)と対にある。
pub const PEER_QUERY_PATH: &str = "/v1/peer/query";

/// QUERY の at がこの秒数より古い・先だと受け取らない。署名は再送を防がないので、
/// 窓で古い封筒を落とす。時計のずれを吸える程度に広く採る。
pub const QUERY_FRESHNESS_SECONDS: i64 = 300;

/// 散布の既定の予算(観測の打ち切り。POST /v1/query と同じ既定)。
pub const DEFAULT_BUDGET_MS: u64 = 2_000;

/// 沈黙したピアへ問い直す周期(沈黙は終端ではない。SPEC §7.2)。
const RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// 1 本の要求に掛ける期限の上限。予算がこれより長くても、1 回の往復はここで打ち切って
/// 問い直しに回す(期限のない待ちを作らない。should/0104)。
const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// ---- 線の上の形(QUERY と ANSWER) ----

/// 受け取った QUERY(署名を検証した後の中身)。
#[derive(Debug)]
pub struct IncomingQuery {
    pub query_id: String,
    /// 要求者の DBノードID(署名が通った公開鍵)。
    pub origin: String,
    pub request: SearchRequest,
}

/// QUERY を断る理由。status は REST に載せる符号で、区分は「誰が直せるか」で分ける
/// (MCP の誤りの区分と同じ考え方): 400 は封筒の組み立て、401 は名乗りと署名、
/// 403 は共有ポリシーの判断である。
#[derive(Debug)]
pub struct Rejection {
    pub status: u16,
    pub reason: String,
}

impl Rejection {
    fn new(status: u16, reason: impl Into<String>) -> Rejection {
        Rejection { status, reason: reason.into() }
    }
}

/// 署名対象 = sig を除いた正規形(ref レコード・証明書と同じ規約。SPEC §4.4/§6.4)。
fn signing_message(map: &BTreeMap<String, c1::Value>) -> Vec<u8> {
    let mut unsigned = map.clone();
    unsigned.remove("sig");
    c1::to_canonical_bytes(&c1::Value::Object(unsigned))
}

/// 署名を検証する。鍵は DBノードID そのもの(SPEC §6.1: DBノードID は Ed25519 公開鍵)。
fn signature_is_valid(map: &BTreeMap<String, c1::Value>, node_id: &str) -> bool {
    let (Some(c1::Value::Text(signature_hex)), Some(key_bytes)) =
        (map.get("sig"), crate::sha2::from_hex(node_id))
    else {
        return false;
    };
    let Some(signature_bytes) = crate::sha2::from_hex(signature_hex) else { return false };
    if key_bytes.len() != 32 || signature_bytes.len() != 64 {
        return false;
    }
    let mut public = [0u8; 32];
    public.copy_from_slice(&key_bytes);
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&signature_bytes);
    crate::ed25519::verify(&public, &signing_message(map), &signature)
}

/// 自分の鍵で sig を足す(書き手の家はここだけ)。
fn sign_into(store: &Store, mut map: BTreeMap<String, c1::Value>) -> Vec<u8> {
    let signature = store.sign_message(&signing_message(&map));
    map.insert("sig".to_string(), c1::Value::Text(crate::sha2::hex(&signature)));
    c1::to_canonical_bytes(&c1::Value::Object(map))
}

/// 散布する QUERY を組んで署名する。同じバイト列を全ピアへ送る(1 ホップなので宛先ごとに
/// 変える必要が無く、宛先を書かないことで「誰に配ったか」も線に載らない)。
///
/// scope は線に載せない。転送は 1 ホップだけ(SPEC §7.1 の MUST)で、受け手はスコープを
/// 使わないからであり、載せれば自分のピア一覧を配って回ることになるからである。
pub fn query_message(
    store: &Store,
    query_id: &str,
    request: &SearchRequest,
    budget_ms: u64,
) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert("v".to_string(), c1::Value::Integer(1));
    map.insert("kind".to_string(), c1::Value::Text("search".to_string()));
    map.insert("query_id".to_string(), c1::Value::Text(query_id.to_string()));
    map.insert("origin".to_string(), c1::Value::Text(store.node_id_hex().to_string()));
    map.insert("payload".to_string(), crate::api::search_request_value(request));
    map.insert("budget_ms".to_string(), c1::Value::Integer(budget_ms as i64));
    map.insert("at".to_string(), c1::Value::Integer(unix_now()));
    sign_into(store, map)
}

/// 受け取った QUERY を検証する(封筒 → 署名 → 鮮度 → 中身)。共有ポリシーの判定は
/// この後の answer_policy が行う。
pub fn verify_query(bytes: &[u8], now: i64) -> Result<IncomingQuery, Rejection> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Rejection::new(400, "QUERY が UTF-8 でない"))?;
    let value =
        c1::parse(text).map_err(|e| Rejection::new(400, format!("QUERY が JSON でない: {e}")))?;
    let c1::Value::Object(map) = &value else {
        return Err(Rejection::new(400, "QUERY がオブジェクトでない"));
    };
    match map.get("kind") {
        Some(c1::Value::Text(kind)) if kind == "search" => {}
        Some(c1::Value::Text(kind)) => {
            // kind:object と kind:refs は自己認証的なので GET で運ぶ(SPEC §7.1 の L2 の
            // 具体化)。この口は kind:search 専用である。
            return Err(Rejection::new(
                400,
                format!("kind {kind} はこの口では受けない(この口は search だけ)"),
            ));
        }
        _ => return Err(Rejection::new(400, "kind がない")),
    }
    let Some(c1::Value::Text(query_id)) = map.get("query_id") else {
        return Err(Rejection::new(400, "query_id がない"));
    };
    let Some(c1::Value::Text(origin)) = map.get("origin") else {
        return Err(Rejection::new(400, "origin がない"));
    };
    if !signature_is_valid(map, origin) {
        return Err(Rejection::new(401, "QUERY の署名が origin の鍵で検証できない"));
    }
    let at = match map.get("at") {
        Some(c1::Value::Integer(at)) => *at,
        _ => return Err(Rejection::new(400, "at がない")),
    };
    if (now - at).abs() > QUERY_FRESHNESS_SECONDS {
        return Err(Rejection::new(
            401,
            format!(
                "QUERY の at が {QUERY_FRESHNESS_SECONDS} 秒の窓の外(at {at}, こちらの今 \
                 {now})。署名は再送を防がないので、古い封筒は受けない"
            ),
        ));
    }
    let payload = map.get("payload").ok_or_else(|| Rejection::new(400, "payload がない"))?;
    let request = crate::api::parse_search_request(payload)
        .map_err(|reason| Rejection::new(400, format!("payload が検索要求として読めない: {reason}")))?;
    Ok(IncomingQuery { query_id: query_id.clone(), origin: origin.clone(), request })
}

/// 応答側の共有ポリシー(SPEC §7.1 の双方向フィルタの、応答側の半分)。要求者の
/// DBノードID を peers.json で引き、答えるなら見てよいコレクションの範囲を返す。
///
/// 名乗りだけの相手には答えない: 登録していない DBノードID からの問いも、node_id を
/// 書いていないエントリからの問いも、断る。手動のメンバーシップが信頼の根である
/// (SPEC §6.3)以上、根に無い相手へ自分の索引を開かない。
pub fn answer_policy(entries: &[PeerEntry], origin: &str) -> Result<CollectionScope, Rejection> {
    let Some(entry) = entries.iter().find(|entry| entry.node_id.as_deref() == Some(origin)) else {
        return Err(Rejection::new(
            403,
            format!(
                "DBノード {origin} は peers.json に無い(答えるには、そのピアの \
                 node_id を書いた項が要る)"
            ),
        ));
    };
    if entry.trust_level <= 0 {
        return Err(Rejection::new(
            403,
            format!("DBノード {origin} の trust_level は {} である", entry.trust_level),
        ));
    }
    Ok(entry.share.clone())
}

/// ANSWER を組んで署名する。
///
/// 得点は載せない。融合は順位だけを使うので要らず、載せれば c1(整数のみ)の正規形から
/// 外れて署名対象を別に定義することになるからである。順位は results の並びそのものである。
pub fn answer_message(store: &Store, query_id: &str, results: &SearchResults) -> Vec<u8> {
    let mut payload = BTreeMap::new();
    payload.insert("method".to_string(), c1::Value::Text(results.method.as_str().to_string()));
    if let Some(reason) = &results.degraded {
        payload.insert("degraded".to_string(), c1::Value::Text(reason.clone()));
    }
    if results.filtered_low_information > 0 {
        payload.insert(
            "filtered_low_information".to_string(),
            c1::Value::Integer(results.filtered_low_information as i64),
        );
    }
    let items: Vec<c1::Value> = results
        .results
        .iter()
        .map(|result| {
            let mut entry = BTreeMap::new();
            entry.insert("citation".to_string(), crate::api::citation_value(&result.citation));
            entry.insert("id".to_string(), c1::Value::Text(result.id.clone()));
            entry.insert("snippet".to_string(), c1::Value::Text(result.snippet.clone()));
            c1::Value::Object(entry)
        })
        .collect();
    payload.insert("results".to_string(), c1::Value::Array(items));

    let mut map = BTreeMap::new();
    map.insert("v".to_string(), c1::Value::Integer(1));
    map.insert("query_id".to_string(), c1::Value::Text(query_id.to_string()));
    map.insert("responder".to_string(), c1::Value::Text(store.node_id_hex().to_string()));
    map.insert("payload".to_string(), c1::Value::Object(payload));
    map.insert("at".to_string(), c1::Value::Integer(unix_now()));
    sign_into(store, map)
}

/// ピアが返した順位の 1 件。得点は運ばれないので、順位(この列の位置)だけが情報である。
#[derive(Clone, Debug)]
pub struct RemoteHit {
    pub id: String,
    pub snippet: String,
    pub citation: Citation,
}

/// ピア 1 台ぶんの答え。
pub struct PeerAnswer {
    /// 署名が通った応答者の DBノードID。
    pub responder: String,
    pub method: String,
    pub degraded: Option<String>,
    pub hits: Vec<RemoteHit>,
}

/// 受け取った ANSWER を検証して読む。query_id が違う応答と、期待する DBノードID と違う
/// 相手からの応答は受けない(双方向フィルタの、要求側の半分)。
pub fn verify_answer(
    bytes: &[u8],
    query_id: &str,
    expected_node_id: Option<&str>,
) -> Result<PeerAnswer, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "ANSWER が UTF-8 でない".to_string())?;
    // 署名の検証は c1 の正規形の上で行い、中身の読み取りは小数を読める JSON
    // (node/src/json.rs)ではなく c1 を通す。応答は得点を載せないので c1 で足りる。
    let value = c1::parse(text).map_err(|e| format!("ANSWER が JSON でない: {e}"))?;
    let c1::Value::Object(map) = &value else {
        return Err("ANSWER がオブジェクトでない".to_string());
    };
    let Some(c1::Value::Text(responder)) = map.get("responder") else {
        return Err("ANSWER に responder がない".to_string());
    };
    if !signature_is_valid(map, responder) {
        return Err("ANSWER の署名が responder の鍵で検証できない".to_string());
    }
    if let Some(expected) = expected_node_id {
        if expected != responder {
            return Err(format!(
                "答えたのは {responder} で、peers.json が待っているのは {expected} である"
            ));
        }
    }
    match map.get("query_id") {
        Some(c1::Value::Text(answered)) if answered == query_id => {}
        Some(c1::Value::Text(answered)) => {
            return Err(format!("別の問い {answered} への答えである(こちらの問いは {query_id})"))
        }
        _ => return Err("ANSWER に query_id がない".to_string()),
    }
    let Some(c1::Value::Object(payload)) = map.get("payload") else {
        return Err("ANSWER に payload がない".to_string());
    };
    let method = match payload.get("method") {
        Some(c1::Value::Text(method)) => method.clone(),
        _ => return Err("ANSWER の payload に method がない".to_string()),
    };
    let degraded = match payload.get("degraded") {
        Some(c1::Value::Text(reason)) => Some(reason.clone()),
        _ => None,
    };
    let Some(c1::Value::Array(items)) = payload.get("results") else {
        return Err("ANSWER の payload に results がない".to_string());
    };
    let mut hits = Vec::with_capacity(items.len());
    for item in items {
        let c1::Value::Object(entry) = item else {
            return Err("results の要素がオブジェクトでない".to_string());
        };
        let Some(c1::Value::Text(id)) = entry.get("id") else {
            return Err("results の要素に id がない".to_string());
        };
        if !c1::is_object_id(id) {
            return Err(format!("results の id {id} がオブジェクト ID の形でない"));
        }
        let snippet = match entry.get("snippet") {
            Some(c1::Value::Text(snippet)) => snippet.clone(),
            _ => return Err("results の要素に snippet がない".to_string()),
        };
        // 引用の読み手は REST と同じ citation_from_json である(読み方を二重に実装
        // しない。should/0135)。c1 の値をその読み手へ渡すために JSON へ通し直す。
        let citation_text = String::from_utf8(c1::to_canonical_bytes(
            entry.get("citation").ok_or("results の要素に citation がない")?,
        ))
        .map_err(|_| "citation が UTF-8 でない".to_string())?;
        let citation = crate::api::citation_from_json(&Json::parse(&citation_text)?)?;
        hits.push(RemoteHit { id: id.clone(), snippet, citation });
    }
    Ok(PeerAnswer { responder: responder.clone(), method, degraded, hits })
}

// ---- 散布(要求側) ----

/// 散布先 1 台。
#[derive(Clone, Debug)]
pub struct PeerTarget {
    pub address: String,
    /// peers.json が主張する相手の DBノードID(あれば、答えた相手を照合する)。
    pub node_id: Option<String>,
}

/// 散布の設定。
pub struct ScatterOptions {
    /// 観測の打ち切り(クエリの属性ではない。SPEC §7.2)。
    pub budget_ms: u64,
    /// この信頼度未満のピアへは問いを送らない(scope の trust_level 閾値。SPEC §7.1)。
    /// 問いの文そのものが情報なので、要求側にも絞る手段を持たせる。
    pub min_trust_level: i64,
    /// 明示の宛先(空なら peers.json の全ピア)。
    pub addresses: Vec<String>,
}

/// ピア 1 台ぶんの経過(応答の有無と、答えが持っていた注記)。
pub struct PeerReport {
    pub address: String,
    pub node_id: Option<String>,
    pub state: PeerState,
    /// 沈黙・拒否の理由、または相手の応答が持っていた劣化の理由。
    pub note: Option<String>,
    pub hit_count: usize,
}

/// 散布 1 回の結果。
pub struct Scattered {
    pub outcome: QueryOutcome,
    pub peers: Vec<PeerReport>,
    /// 答えた相手ごとの順位(融合の入力)。
    pub answers: Vec<(String, Vec<RemoteHit>)>,
}

/// ピアへ QUERY を届ける口(テストが差し替えられるように関数で持つ)。
pub type AskPeer =
    Arc<dyn Fn(&PeerTarget, &[u8], Duration) -> Result<PeerAnswer, String> + Send + Sync>;

/// HTTP でピアへ QUERY を届ける(既定の口)。query_id は応答の照合に使う。
pub fn http_ask_peer(query_id: &str) -> AskPeer {
    let query_id = query_id.to_string();
    Arc::new(move |target: &PeerTarget, message: &[u8], timeout: Duration| {
        let response = crate::http::post_json(&target.address, PEER_QUERY_PATH, message, timeout)?;
        if response.status != 200 {
            return Err(format!(
                "{} が {} を返した: {}",
                PEER_QUERY_PATH,
                response.status,
                crate::http::body_head(&response.body)
            ));
        }
        verify_answer(&response.body, &query_id, target.node_id.as_deref())
    })
}

/// 散布先を決める(既定のスコープは peers.json。trust_level で絞る)。明示のアドレスを
/// 与えたときは、peers.json に無いアドレスもそのまま宛先にする(相手の DBノードID は
/// 分からないので、答えた相手の照合はしない)。
pub fn targets_for(entries: &[PeerEntry], options: &ScatterOptions) -> Vec<PeerTarget> {
    if options.addresses.is_empty() {
        return entries
            .iter()
            .filter(|entry| entry.trust_level >= options.min_trust_level)
            .map(|entry| PeerTarget {
                address: entry.address.clone(),
                node_id: entry.node_id.clone(),
            })
            .collect();
    }
    options
        .addresses
        .iter()
        .map(|address| {
            let known = entries.iter().find(|entry| &entry.address == address);
            PeerTarget {
                address: address.clone(),
                node_id: known.and_then(|entry| entry.node_id.clone()),
            }
        })
        .collect()
}

/// 散布の途中経過(スレッド間で共有する)。
struct ScatterState {
    peers: Vec<PeerReport>,
    answers: Vec<(String, Vec<RemoteHit>)>,
    settled: bool,
}

/// 登録ピアへ 1 ホップで散布し、予算の中で決着させる。ローカルの順位は呼び手が
/// 別に持っており(run_search が返す)、融合は fuse がまとめて行う。
///
/// 決着は SPEC §7.2 の 3 値で、判定は crate::query::settlement が持つ(1 台目のクエリと
/// 同じ規則を二度実装しない。should/0135)。
pub fn scatter(
    message: &[u8],
    targets: &[PeerTarget],
    budget_ms: u64,
    local_has_hits: bool,
    ask: AskPeer,
) -> Scattered {
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    let shared = Arc::new((
        Mutex::new(ScatterState {
            peers: targets
                .iter()
                .map(|target| PeerReport {
                    address: target.address.clone(),
                    node_id: target.node_id.clone(),
                    state: PeerState::Pending,
                    note: None,
                    hit_count: 0,
                })
                .collect(),
            answers: Vec::new(),
            settled: false,
        }),
        Condvar::new(),
    ));

    let mut workers = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let shared = shared.clone();
        let ask = ask.clone();
        let target = target.clone();
        let message = message.to_vec();
        workers.push(std::thread::spawn(move || {
            let (state, awake) = &*shared;
            loop {
                if state.lock().expect("scatter state lock").settled {
                    return;
                }
                let timeout = PEER_REQUEST_TIMEOUT
                    .min(deadline.saturating_duration_since(Instant::now()))
                    .max(Duration::from_millis(1));
                let attempt = ask(&target, &message, timeout);
                let mut guard = state.lock().expect("scatter state lock");
                match attempt {
                    Ok(answer) => {
                        // 空の答えは「私の索引には該当がない」という肯定的言明であって、
                        // 沈黙ではない(SPEC §7.2)。
                        guard.peers[index].state = if answer.hits.is_empty() {
                            PeerState::Empty
                        } else {
                            PeerState::Answered
                        };
                        guard.peers[index].node_id = Some(answer.responder.clone());
                        guard.peers[index].note = answer.degraded.clone();
                        guard.peers[index].hit_count = answer.hits.len();
                        if !answer.hits.is_empty() {
                            guard.answers.push((target.address.clone(), answer.hits));
                        }
                        drop(guard);
                        awake.notify_all();
                        return;
                    }
                    Err(reason) => {
                        // 届かない・断られた・検証に落ちた、はどれも沈黙である
                        // (情報ゼロ)。理由は残して報告する(must/0022)。
                        guard.peers[index].state = PeerState::Silent;
                        guard.peers[index].note = Some(reason);
                        drop(guard);
                        awake.notify_all();
                    }
                }
                if Instant::now() + RETRY_INTERVAL >= deadline {
                    return;
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
        }));
    }

    // 決着まで待つ(予算で必ず終わる)。
    let (state, awake) = &*shared;
    let mut guard = state.lock().expect("scatter state lock");
    let outcome = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let expired = remaining.is_zero();
        if expired {
            for peer in guard.peers.iter_mut() {
                if peer.state == PeerState::Pending {
                    peer.state = PeerState::Silent;
                }
            }
        }
        let states: Vec<PeerState> = guard.peers.iter().map(|peer| peer.state).collect();
        let has_answer = local_has_hits || !guard.answers.is_empty();
        if let Some(outcome) = settlement(&states, has_answer, expired) {
            break outcome;
        }
        let (next, _) = awake.wait_timeout(guard, remaining).expect("condvar wait");
        guard = next;
    };
    guard.settled = true;
    drop(guard);
    awake.notify_all();
    // 走らせたスレッドは決着を見て戻る。回収してから結果を取り出す(応答を返した後も
    // 動き続けるスレッドを残さない)。
    for worker in workers {
        let _ = worker.join();
    }
    let state = Arc::try_unwrap(shared)
        .unwrap_or_else(|_| unreachable!("worker を回収した後は所有者が 1 つ"))
        .0
        .into_inner()
        .expect("scatter state lock");
    Scattered { outcome, peers: state.peers, answers: state.answers }
}

// ---- 融合 ----

/// 融合の結果 1 件。得点は順位から作った RRF で、どのDBノードから来たかを sources が持つ
/// (同じチャンクが複数のDBノードにあれば、内容ハッシュが同じなので 1 件にまとまり、
/// 順位が足し合わされる)。
pub struct FusedResult {
    pub result: SearchResult,
    pub sources: Vec<String>,
}

/// ローカルの順位とピアの順位を RRF で融合する。順位だけを使うので、DBノード間で得点を
/// 較正する必要がない(融合の家は crate::embed::fuse_by_rank。should/0135)。
///
/// 同一視の鍵はチャンクのオブジェクト ID である。content-addressed なので、同じ本文の
/// チャンクは、どのDBノードから来ても同じ ID を持つ。
/// 融合のあいだ覚えておく、チャンク 1 件の見え方。原本への道(source_url)は手元の件から
/// しか採らない: ピアの件はそのピアのストアにあるので、こちらの URL では取れない。
struct Seen {
    snippet: String,
    citation: Citation,
    source_url: Option<String>,
    /// 全文(要求が full のとき)。道と同じく手元の件からしか採らない: ピアの答えは
    /// 抜粋しか運ばない。
    text: Option<String>,
    sources: Vec<String>,
}

pub fn fuse(
    local: &[SearchResult],
    answers: &[(String, Vec<RemoteHit>)],
    top_k: usize,
) -> Vec<FusedResult> {
    let mut rankings: Vec<Vec<String>> = Vec::new();
    // 各チャンクの見え方(抜粋と引用)と、どこから来たか。最初に見た側のものを使う
    // (ローカルを先に入れるので、手元にあるチャンクは手元の引用で答える)。
    let mut seen: BTreeMap<String, Seen> = BTreeMap::new();
    let mut note = |id: &str,
                    snippet: &str,
                    citation: &Citation,
                    url: Option<&String>,
                    text: Option<&String>,
                    source: &str| {
        let entry = seen.entry(id.to_string()).or_insert_with(|| Seen {
            snippet: snippet.to_string(),
            citation: citation.clone(),
            source_url: url.cloned(),
            text: text.cloned(),
            sources: Vec::new(),
        });
        if !entry.sources.iter().any(|known| known == source) {
            entry.sources.push(source.to_string());
        }
    };
    rankings.push(
        local
            .iter()
            .map(|hit| {
                note(
                    &hit.id,
                    &hit.snippet,
                    &hit.citation,
                    hit.source_url.as_ref(),
                    hit.text.as_ref(),
                    "local",
                );
                hit.id.clone()
            })
            .collect(),
    );
    for (source, hits) in answers {
        rankings.push(
            hits.iter()
                .map(|hit| {
                    note(&hit.id, &hit.snippet, &hit.citation, None, None, source);
                    hit.id.clone()
                })
                .collect(),
        );
    }
    fuse_by_rank(&rankings, top_k)
        .into_iter()
        .filter_map(|fused| {
            let seen = seen.get(&fused.item)?;
            Some(FusedResult {
                result: SearchResult {
                    id: fused.item.clone(),
                    score: fused.score,
                    snippet: seen.snippet.clone(),
                    citation: seen.citation.clone(),
                    source_url: seen.source_url.clone(),
                    text: seen.text.clone(),
                },
                sources: seen.sources.clone(),
            })
        })
        .collect()
}

/// 分散検索 1 回の結果(散布の経過と、融合した順位)。
pub struct Distributed {
    pub outcome: QueryOutcome,
    pub peers: Vec<PeerReport>,
    pub results: Vec<FusedResult>,
}

/// 分散検索を 1 回行う(要求側の入口。POST /v1/search に peers を付けた要求がここへ
/// 来る)。ローカルの順位は呼び手が run_search で先に引いておく。自分もクエリの参加者
/// であり、自分の索引には自分で答えるからである。
///
/// 署名のために store のロックを取るのは QUERY を組む一瞬だけで、ピアの応答を待つ
/// あいだは持たない(ネットワーク待ちのあいだ API 全体を塞がない。sync と同じ規律)。
pub fn search_across_peers(
    store: &Mutex<Store>,
    entries: &[PeerEntry],
    request: &SearchRequest,
    local: &SearchResults,
    options: &ScatterOptions,
) -> Distributed {
    let query_id = crate::query::random_hex_id();
    let targets = targets_for(entries, options);
    let message = {
        let store = store.lock().expect("store lock");
        query_message(&store, &query_id, request, options.budget_ms)
    };
    let scattered = scatter(
        &message,
        &targets,
        options.budget_ms,
        !local.results.is_empty(),
        http_ask_peer(&query_id),
    );
    for peer in &scattered.peers {
        if let (PeerState::Silent, Some(note)) = (peer.state, &peer.note) {
            // 沈黙は情報ゼロだが、その理由は運用者が直せることが多い(届かない・断られた・
            // 署名が合わない)。応答にも載るが、応答を読まない側にも見えるところへ出す。
            crate::log_line!("uniqnode: 分散検索: ピア {} は沈黙した: {note}", peer.address);
        }
    }
    let results = fuse(&local.results, &scattered.answers, request.top_k);
    Distributed { outcome: scattered.outcome, peers: scattered.peers, results }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreConfig;

    fn temp_store(name: &str) -> (std::path::PathBuf, Store) {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-dsearch-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let store = Store::open(StoreConfig::new(&dir)).expect("open");
        (dir, store)
    }

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            collection: None,
            top_k: 10,
            method: None,
            include_low_information: false,
            full: false,
        }
    }

    fn citation(document: &str, position: usize) -> Citation {
        Citation {
            collection: "notes".to_string(),
            document: document.to_string(),
            position,
            page: None,
            breadcrumbs: vec!["章".to_string()],
            at: 1_700_000_000,
        }
    }

    fn hit(id: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            score: 1.0,
            snippet: format!("{id} の抜粋"),
            citation: citation("memo", 0),
            source_url: Some(format!("/v1/objects/{id}/rendition/source")),
            text: None,
        }
    }

    /// QUERY は署名され、改竄されれば通らず、鮮度の窓の外なら受けない。
    #[test]
    fn a_query_is_authenticated_by_its_signature_and_freshness() {
        let (dir, store) = temp_store("query-signature");
        let now = unix_now();
        let message = query_message(&store, "q1", &request("世代の整合"), 2_000);

        let accepted = verify_query(&message, now).expect("自分の署名は通る");
        assert_eq!(accepted.origin, store.node_id_hex());
        assert_eq!(accepted.query_id, "q1");
        assert_eq!(accepted.request.query, "世代の整合");

        // 本文の改竄(問いの差し替え)は署名を割る。
        let text = String::from_utf8(message.clone()).expect("utf-8");
        let tampered = text.replace("世代の整合", "別の問い金");
        assert_ne!(tampered, text, "差し替えが起きている");
        let rejected = verify_query(tampered.as_bytes(), now).expect_err("改竄は通らない");
        assert_eq!(rejected.status, 401, "{}", rejected.reason);

        // 鮮度の窓の外(署名は正しいが古い封筒)。
        let stale = verify_query(&message, now + QUERY_FRESHNESS_SECONDS + 1)
            .expect_err("古い封筒は受けない");
        assert_eq!(stale.status, 401, "{}", stale.reason);

        // 名乗りだけを差し替えても通らない(署名は origin の鍵で検証する)。
        let impostor = text.replace(store.node_id_hex(), &"aa".repeat(32));
        let rejected = verify_query(impostor.as_bytes(), now).expect_err("名乗りの詐称");
        assert_eq!(rejected.status, 401, "{}", rejected.reason);

        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 応答側の双方向フィルタ: 知らない DBノード・trust_level 0 には答えず、
    /// share.collections を書いた相手にはその範囲だけを見る。
    #[test]
    fn the_answer_policy_table_holds() {
        let known = "11".repeat(32);
        let untrusted = "22".repeat(32);
        let limited = "33".repeat(32);
        let entries = vec![
            PeerEntry {
                address: "10.0.0.2:7440".to_string(),
                node_id: Some(known.clone()),
                trust_level: 50,
                share: CollectionScope::All,
            },
            PeerEntry {
                address: "10.0.0.3:7440".to_string(),
                node_id: Some(untrusted.clone()),
                trust_level: 0,
                share: CollectionScope::All,
            },
            PeerEntry {
                address: "10.0.0.4:7440".to_string(),
                node_id: Some(limited.clone()),
                trust_level: 10,
                share: CollectionScope::Only(vec!["notes".to_string()]),
            },
            // node_id を書いていないエントリは、要求者の認証には使えない。
            PeerEntry {
                address: "10.0.0.5:7440".to_string(),
                node_id: None,
                trust_level: 90,
                share: CollectionScope::All,
            },
        ];

        assert_eq!(answer_policy(&entries, &known).expect("答える"), CollectionScope::All);
        assert_eq!(
            answer_policy(&entries, &limited).expect("答える"),
            CollectionScope::Only(vec!["notes".to_string()])
        );
        assert_eq!(answer_policy(&entries, &untrusted).expect_err("答えない").status, 403);
        assert_eq!(answer_policy(&entries, &"99".repeat(32)).expect_err("知らない相手").status, 403);
    }

    /// ANSWER は署名され、query_id と応答者の照合を通る。
    #[test]
    fn an_answer_carries_its_ranking_under_the_responder_signature() {
        let (dir, store) = temp_store("answer-signature");
        let results = SearchResults {
            method: crate::embed::SearchMethod::Bm25,
            degraded: Some("埋め込みサーバが設定されていない".to_string()),
            results: vec![hit(&format!("s256:{}", "ab".repeat(32)))],
            filtered_low_information: 2,
            reranked: false,
        };
        let message = answer_message(&store, "q7", &results);

        let answer = verify_answer(&message, "q7", Some(store.node_id_hex())).expect("通る");
        assert_eq!(answer.responder, store.node_id_hex());
        assert_eq!(answer.method, "bm25");
        assert_eq!(answer.degraded.as_deref(), Some("埋め込みサーバが設定されていない"));
        assert_eq!(answer.hits.len(), 1);
        assert_eq!(answer.hits[0].citation.document, "memo");
        assert_eq!(answer.hits[0].citation.at, 1_700_000_000);

        // 別の問いへの答えは受けない。
        assert!(verify_answer(&message, "q8", None).is_err(), "query_id の照合");
        // 待っている相手と違えば受けない。
        assert!(
            verify_answer(&message, "q7", Some(&"cd".repeat(32))).is_err(),
            "responder の照合"
        );
        // 抜粋の差し替えは署名を割る。
        let tampered = String::from_utf8(message).expect("utf-8").replace("の抜粋", "の改竄");
        assert!(verify_answer(tampered.as_bytes(), "q7", None).is_err(), "改竄");

        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 融合: 同じチャンクは内容ハッシュで 1 件にまとまって順位が足され、両方のDBノードが
    /// 出所として残る。片方にしか無いチャンクも順位に入る。
    #[test]
    fn fusion_merges_the_same_chunk_across_nodes() {
        let shared_id = format!("s256:{}", "11".repeat(32));
        let local_only = format!("s256:{}", "22".repeat(32));
        let remote_only = format!("s256:{}", "33".repeat(32));
        let local = vec![hit(&local_only), hit(&shared_id)];
        let remote = vec![
            RemoteHit {
                id: shared_id.clone(),
                snippet: "遠くの抜粋".to_string(),
                citation: citation("remote", 3),
            },
            RemoteHit {
                id: remote_only.clone(),
                snippet: "遠くだけの抜粋".to_string(),
                citation: citation("remote", 4),
            },
        ];
        let fused = fuse(&local, &[("10.0.0.2:7440".to_string(), remote)], 10);

        assert_eq!(fused.len(), 3, "3 件のチャンクが並ぶ");
        // 両方に出たチャンクが 1 位(2 本の順位が足し合わされる)。
        assert_eq!(fused[0].result.id, shared_id);
        assert_eq!(fused[0].sources, vec!["local".to_string(), "10.0.0.2:7440".to_string()]);
        // 手元にもあるチャンクは手元の引用で答える。
        assert_eq!(fused[0].result.citation.document, "memo");
        assert!(fused[0].result.score > fused[1].result.score, "順位の和が上位に来る");
        let remote_entry =
            fused.iter().find(|entry| entry.result.id == remote_only).expect("遠くだけの件");
        assert_eq!(remote_entry.sources, vec!["10.0.0.2:7440".to_string()]);
        assert_eq!(remote_entry.result.snippet, "遠くだけの抜粋");
    }

    /// 散布の決着表(SPEC §7.2 の 3 値)。答えた相手がいれば found、全員が肯定的に
    /// 「無い」と言えば scope_empty、沈黙が残ったまま予算が切れれば timed_out。
    #[test]
    fn scatter_settles_by_the_open_world_table() {
        fn target(address: &str) -> PeerTarget {
            PeerTarget { address: address.to_string(), node_id: None }
        }
        // アドレスが挙動を決めるモックのピア。
        let ask: AskPeer = Arc::new(|target: &PeerTarget, _: &[u8], _: Duration| {
            match target.address.as_str() {
                "empty" => Ok(PeerAnswer {
                    responder: "mock".to_string(),
                    method: "bm25".to_string(),
                    degraded: None,
                    hits: Vec::new(),
                }),
                "answer" => Ok(PeerAnswer {
                    responder: "mock".to_string(),
                    method: "bm25".to_string(),
                    degraded: None,
                    hits: vec![RemoteHit {
                        id: format!("s256:{}", "44".repeat(32)),
                        snippet: "遠くの抜粋".to_string(),
                        citation: Citation {
                            collection: "notes".to_string(),
                            document: "remote".to_string(),
                            position: 0,
                            page: None,
                            breadcrumbs: Vec::new(),
                            at: 0,
                        },
                    }],
                }),
                _ => Err("届かない".to_string()),
            }
        });

        struct Case {
            addresses: Vec<&'static str>,
            local_has_hits: bool,
            expected: QueryOutcome,
            expected_silent: usize,
        }
        let cases = [
            Case {
                addresses: vec!["empty", "answer"],
                local_has_hits: false,
                expected: QueryOutcome::Found,
                expected_silent: 0,
            },
            Case {
                addresses: vec!["empty", "empty"],
                local_has_hits: false,
                expected: QueryOutcome::ScopeEmpty,
                expected_silent: 0,
            },
            Case {
                addresses: vec!["empty", "silent"],
                local_has_hits: false,
                expected: QueryOutcome::TimedOut,
                expected_silent: 1,
            },
            // ローカルに当たりがあれば、沈黙が残っていても found である(こちらの知識に
            // ついての肯定的言明が既にある)。
            Case {
                addresses: vec!["silent"],
                local_has_hits: true,
                expected: QueryOutcome::Found,
                expected_silent: 1,
            },
            Case {
                addresses: vec![],
                local_has_hits: false,
                expected: QueryOutcome::ScopeEmpty,
                expected_silent: 0,
            },
        ];
        for (index, case) in cases.iter().enumerate() {
            let targets: Vec<PeerTarget> = case.addresses.iter().map(|a| target(a)).collect();
            let scattered =
                scatter(b"{}", &targets, 600, case.local_has_hits, ask.clone());
            assert_eq!(scattered.outcome, case.expected, "case {index}");
            let silent =
                scattered.peers.iter().filter(|peer| peer.state == PeerState::Silent).count();
            assert_eq!(silent, case.expected_silent, "case {index} の沈黙数");
        }
    }

    /// 散布先: 既定は peers.json で、trust_level の閾値で絞れる。明示のアドレスは
    /// そのまま宛先になる。
    #[test]
    fn targets_follow_the_scope_and_the_trust_threshold() {
        let entries = vec![
            PeerEntry {
                address: "trusted:7440".to_string(),
                node_id: Some("11".repeat(32)),
                trust_level: 80,
                share: CollectionScope::All,
            },
            PeerEntry {
                address: "casual:7440".to_string(),
                node_id: None,
                trust_level: 20,
                share: CollectionScope::All,
            },
        ];
        let all = targets_for(
            &entries,
            &ScatterOptions { budget_ms: 0, min_trust_level: 0, addresses: Vec::new() },
        );
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].node_id.as_deref(), Some("11".repeat(32).as_str()));

        let strict = targets_for(
            &entries,
            &ScatterOptions { budget_ms: 0, min_trust_level: 50, addresses: Vec::new() },
        );
        assert_eq!(strict.len(), 1, "閾値未満のピアには問いを送らない");
        assert_eq!(strict[0].address, "trusted:7440");

        let explicit = targets_for(
            &entries,
            &ScatterOptions {
                budget_ms: 0,
                min_trust_level: 0,
                addresses: vec!["trusted:7440".to_string(), "stranger:7440".to_string()],
            },
        );
        assert_eq!(explicit.len(), 2);
        assert_eq!(explicit[0].node_id.as_deref(), Some("11".repeat(32).as_str()), "既知の相手");
        assert_eq!(explicit[1].node_id, None, "peers.json に無い宛先は照合できない");
    }
}
