//! 分散クエリ(SPEC §7.1, §7.2)。登録ピアへの1ホップ scatter-gather。
//!
//! 開世界セマンティクスの実装点:
//! - ピアの応答は「そのピアの知識についての肯定的言明」。404 は「私は持っていない」という
//!   言明(empty)であり、到達不能・無応答は情報ゼロ(silent)として区別する。
//! - 「存在しない」という結果は存在しない。決着は found / scope_empty(スコープ内の全員が
//!   肯定的に「持っていない」と言明) / timed_out(予算切れ。沈黙者が残っている)の3値。
//! - budget はクエリの属性ではなく観測の打ち切りであり、クエリハンドル
//!   (`GET /v1/queries/{id}`)は回答の単調増加集合を返す。
//! - object の回答は content-addressing で検証してからローカルに保存する。ref の回答は
//!   署名付きレコードではないため報告のみ(取り込みは sync 経由のみ)。

use crate::c1;
use crate::clock::unix_now;
use crate::store::Store;
use crate::sync::{HttpPeer, PeerSource, RefRecordView};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// 沈黙ピアへの問い直しの周期。沈黙は終端ではない(開世界)ため、予算内は再試行する。
const RETRY_INTERVAL: Duration = Duration::from_millis(250);
/// 保持するクエリハンドルの上限(超えた分は古いものから忘れる)。
const REGISTRY_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryKind {
    Object,
    Ref,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerState {
    Pending,
    Silent,
    Empty,
    Answered,
}

impl PeerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PeerState::Pending => "pending",
            PeerState::Silent => "silent",
            PeerState::Empty => "empty",
            PeerState::Answered => "answered",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryOutcome {
    Running,
    Found,
    ScopeEmpty,
    TimedOut,
    /// 取ってきたオブジェクトをストアに置けない(書けない状態。APPEND_FAILURE の方針 2)。
    /// 置けなければ答えの本文を渡せないので、保存したと偽らずに断る。理由は
    /// QueryState::write_failure。
    WritesDisabled,
}

impl QueryOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueryOutcome::Running => "running",
            QueryOutcome::Found => "found",
            QueryOutcome::ScopeEmpty => "scope_empty",
            QueryOutcome::TimedOut => "timed_out",
            QueryOutcome::WritesDisabled => "writes_disabled",
        }
    }
}

/// 決着の判定(SPEC §7.2 の 3 値。判定の家はここだけである。should/0135)。
/// スコープ内の全員が肯定的言明(empty / answered)で出揃えば予算前に決着してよく、
/// 予算が切れたら沈黙を残したまま決着する。「存在しない」を表す決着値は無い。
///
/// まだ決着していなければ None。kind:object の「回答が 1 つ得られたら即 found」は
/// この関数の外にある(そこだけは kind ごとの規則である)。
pub fn settlement(states: &[PeerState], has_answer: bool, expired: bool) -> Option<QueryOutcome> {
    let all_positive =
        states.iter().all(|state| matches!(state, PeerState::Empty | PeerState::Answered));
    if !all_positive && !expired {
        return None;
    }
    Some(match (has_answer, all_positive) {
        (true, _) => QueryOutcome::Found,
        (false, true) => QueryOutcome::ScopeEmpty,
        (false, false) => QueryOutcome::TimedOut,
    })
}

#[derive(Clone, Debug)]
pub enum QueryAnswer {
    /// オブジェクトは検証の上ローカルに保存済み。source はどこから来たか。
    Object { source: String },
    Ref { source: String, view: RefRecordView },
}

#[derive(Clone, Debug)]
pub struct PeerProgress {
    pub address: String,
    pub state: PeerState,
}

#[derive(Clone, Debug)]
pub struct QueryState {
    pub id: String,
    pub kind: QueryKind,
    pub target: String,
    pub budget_ms: u64,
    pub outcome: QueryOutcome,
    pub answers: Vec<QueryAnswer>,
    pub peers: Vec<PeerProgress>,
    /// outcome が WritesDisabled のときの理由(HTTP は 503 と案内にする)。
    pub write_failure: Option<crate::store::WriteFailure>,
}

pub struct QueryShared {
    state: Mutex<QueryState>,
    settled: Condvar,
    deadline: Instant,
}

impl QueryShared {
    pub fn snapshot(&self) -> QueryState {
        self.state.lock().expect("query state lock").clone()
    }

    /// 決着(outcome != Running)まで待つ。finalizer が期限で必ず決着させるので有界。
    pub fn wait_settled(&self) -> QueryState {
        let mut guard = self.state.lock().expect("query state lock");
        while guard.outcome == QueryOutcome::Running {
            let remaining = self
                .deadline
                .saturating_duration_since(Instant::now())
                .checked_add(Duration::from_millis(200))
                .expect("duration add");
            let (next, _) = self
                .settled
                .wait_timeout(guard, remaining)
                .expect("condvar wait");
            guard = next;
        }
        guard.clone()
    }
}

pub type PeerFactory = Arc<dyn Fn(&str) -> Box<dyn PeerSource + Send + Sync> + Send + Sync>;

pub struct QueryEngine {
    store: Arc<Mutex<Store>>,
    data_dir: PathBuf,
    registry: Mutex<VecDeque<(String, Arc<QueryShared>)>>,
    peer_factory: PeerFactory,
}

/// ピアの既定の信頼度(peers.json が trust_level を書かないとき)。0 は「答えない」で
/// あり、既定はその上の中間に置く。数値そのものの意味は運用者が決める(順序だけが
/// プロトコルの意味を持つ)。
pub const DEFAULT_TRUST_LEVEL: i64 = 50;

/// 受け入れ済みのピア。手動エントリ(証明書なし)は信頼の根としてそのまま、
/// 証明書付きエントリは groups.json の統一検証規則(SPEC §6.4)を通ったものだけ。
#[derive(Clone, Debug)]
pub struct PeerEntry {
    pub address: String,
    /// このエントリが主張する相手の DBノードID(明示の node_id か、証明書の node_id)。
    /// 出ていく側では実際の接触で照合され(健全性エンジン)、入ってくる側では要求者の
    /// 認証に使う(DISTRIBUTED_SEARCH (uuid:e577f6db-659e-4eb8-a152-3b7780e4a9d1))。
    /// 書かなければ相手を名指しで認証できないので、そのピアからの kind:search には
    /// 答えられない。
    pub node_id: Option<String>,
    /// 信頼度(SPEC §6.3 の trust_level)。0 は「このピアには答えない」で、それ以外の
    /// 数値の意味は運用者が決める。要求側は scope の min_trust_level 未満のピアへ問いを
    /// 送らず、応答側は 0 のピアへ答えない(双方向フィルタ。SPEC §7.1)。
    pub trust_level: i64,
    /// このピアへ出してよいコレクション(SPEC §6.3 の share)。書かなければ全部。
    pub share: crate::search::CollectionScope,
}

/// data_dir/peers.json から受け入れ済みピアを読む。
/// `{"peers":[{"address":"host:port"}, {"address":"…","node_id":"…","trust_level":50,
/// "share":{"collections":["notes"]}}, {"address":"…","certificate":{…}}, …]}`
/// ファイルがなければ空。毎回読み直すので、編集に再起動は要らない(should/0118)。
pub fn read_peer_entries(data_dir: &Path) -> Vec<PeerEntry> {
    let path = data_dir.join("peers.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value = match c1::parse(&text) {
        Ok(v) => v,
        Err(e) => {
            crate::log_line!("uniqnode: peers.json が読めない(無視して空扱い): {e}");
            return Vec::new();
        }
    };
    let groups = crate::groups::read_groups(data_dir);
    let now = unix_now();
    let mut out = Vec::new();
    if let c1::Value::Object(map) = &value {
        if let Some(c1::Value::Array(items)) = map.get("peers") {
            for item in items {
                let c1::Value::Object(peer) = item else { continue };
                let Some(c1::Value::Text(address)) = peer.get("address") else { continue };
                let declared_node_id = match peer.get("node_id") {
                    Some(c1::Value::Text(id)) => Some(id.clone()),
                    _ => None,
                };
                let trust_level = match peer.get("trust_level") {
                    Some(c1::Value::Integer(level)) => *level,
                    None => DEFAULT_TRUST_LEVEL,
                    Some(_) => {
                        crate::log_line!(
                            "uniqnode: ピア {address} の trust_level が整数でない(既定 \
                             {DEFAULT_TRUST_LEVEL} として扱う)"
                        );
                        DEFAULT_TRUST_LEVEL
                    }
                };
                let share = read_share(address, peer.get("share"));
                let certified_node_id = match peer.get("certificate") {
                    None => None,
                    Some(certificate) => {
                        match crate::groups::verify_membership(certificate, &groups, now) {
                            Ok(_) => match certificate {
                                c1::Value::Object(c) => match c.get("node_id") {
                                    Some(c1::Value::Text(id)) => Some(id.clone()),
                                    _ => None,
                                },
                                _ => None,
                            },
                            Err(reason) => {
                                crate::log_line!(
                                    "uniqnode: ピア {address} の証明書を受け入れない: {reason}"
                                );
                                // 証明書付きのエントリは、証明書が通らなければスコープに
                                // 入らない(SPEC §6.3)。
                                continue;
                            }
                        }
                    }
                };
                // 明示の node_id と証明書の主張が食い違うエントリは、どちらを信じるかを
                // こちらで決めずに断る(黙って片方を採らない。must/0022)。
                if let (Some(declared), Some(certified)) = (&declared_node_id, &certified_node_id) {
                    if declared != certified {
                        crate::log_line!(
                            "uniqnode: ピア {address} の node_id が証明書と食い違う\
                             (設定 {declared}, 証明書 {certified})。このエントリは使わない"
                        );
                        continue;
                    }
                }
                out.push(PeerEntry {
                    address: address.clone(),
                    node_id: declared_node_id.or(certified_node_id),
                    trust_level,
                    share,
                });
            }
        }
    }
    out
}

/// ピアエントリの share を読む。`{"collections":["notes", …]}` だけを読み、書かれて
/// いなければ全コレクション。形が違えば、黙って全部を共有せずに何も共有しない側へ倒す
/// (共有ポリシーの読み違いは、意図しない開示になるため。must/0022)。
fn read_share(address: &str, share: Option<&c1::Value>) -> crate::search::CollectionScope {
    let Some(share) = share else { return crate::search::CollectionScope::All };
    let collections = match share {
        c1::Value::Object(map) => map.get("collections"),
        _ => None,
    };
    match collections {
        Some(c1::Value::Array(items)) => {
            let mut names = Vec::new();
            for item in items {
                match item {
                    c1::Value::Text(name) => names.push(name.clone()),
                    _ => {
                        crate::log_line!(
                            "uniqnode: ピア {address} の share.collections に文字列でない\
                             要素がある。このエントリへは何も共有しない"
                        );
                        return crate::search::CollectionScope::Only(Vec::new());
                    }
                }
            }
            crate::search::CollectionScope::Only(names)
        }
        _ => {
            crate::log_line!(
                "uniqnode: ピア {address} の share が \
                 {{\"collections\":[…]}} の形でない。このエントリへは何も共有しない"
            );
            crate::search::CollectionScope::Only(Vec::new())
        }
    }
}

/// 既定スコープ(受け入れ済みピアのアドレス)。
pub fn read_peer_addresses(data_dir: &Path) -> Vec<String> {
    read_peer_entries(data_dir).into_iter().map(|entry| entry.address).collect()
}

/// 識別子(クエリハンドルの ID、分散検索の query_id)。セキュリティ境界ではないが、
/// 重複すると別のクエリの答えと混ざるので乱数から採る。
pub fn random_hex_id() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    match std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)) {
        Ok(()) => {}
        Err(e) => {
            // ID はセキュリティ境界ではない(ハンドルの識別子)ので、失敗は時刻で代用する。
            crate::log_line!("uniqnode: /dev/urandom が読めない({e})。時刻で代用する");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            bytes[..16].copy_from_slice(&now.to_le_bytes());
        }
    }
    crate::sha2::hex(&bytes)
}

impl QueryEngine {
    pub fn new(store: Arc<Mutex<Store>>, data_dir: PathBuf) -> QueryEngine {
        QueryEngine {
            store,
            data_dir,
            registry: Mutex::new(VecDeque::new()),
            peer_factory: Arc::new(|address| {
                Box::new(HttpPeer::with_timeout(address, Duration::from_secs(5)))
            }),
        }
    }

    /// テストがモックのピアを注入するための差し替え口。
    pub fn with_peer_factory(mut self, factory: PeerFactory) -> QueryEngine {
        self.peer_factory = factory;
        self
    }

    pub fn lookup(&self, id: &str) -> Option<Arc<QueryShared>> {
        self.registry
            .lock()
            .expect("registry lock")
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, shared)| shared.clone())
    }

    pub fn default_scope(&self) -> Vec<String> {
        read_peer_addresses(&self.data_dir)
    }

    /// 受け入れ済みのピア(信頼度と共有ポリシー込み)。分散検索は、散布先を選ぶのにも、
    /// 入ってきた QUERY の要求者を引くのにも、この一覧を使う。毎回読み直すので、
    /// peers.json の編集に再起動は要らない(should/0118)。
    pub fn peer_entries(&self) -> Vec<PeerEntry> {
        read_peer_entries(&self.data_dir)
    }

    /// クエリを開始する。返ったハンドルは即座に観測でき、期限までに必ず決着する。
    pub fn start(
        &self,
        kind: QueryKind,
        target: &str,
        budget_ms: u64,
        scope: Vec<String>,
    ) -> Arc<QueryShared> {
        let deadline = Instant::now() + Duration::from_millis(budget_ms);
        let mut state = QueryState {
            id: random_hex_id(),
            kind,
            target: target.to_string(),
            budget_ms,
            outcome: QueryOutcome::Running,
            answers: Vec::new(),
            peers: scope
                .iter()
                .map(|address| PeerProgress { address: address.clone(), state: PeerState::Pending })
                .collect(),
            write_failure: None,
        };

        // 自分自身も参加者(ローカルの知識は即答)。
        let local_object_hit = {
            let store = self.store.lock().expect("store lock");
            match kind {
                QueryKind::Object => {
                    if store.has_object(target) {
                        state.answers.push(QueryAnswer::Object { source: "local".into() });
                        true
                    } else if let Some(failure) = store.writes_disabled() {
                        // 手元に無いものを取りに行っても置けないので、散布せずに断る。
                        state.outcome = QueryOutcome::WritesDisabled;
                        state.write_failure = Some(failure.clone());
                        true
                    } else {
                        false
                    }
                }
                QueryKind::Ref => {
                    if let Some(found) = store.get_ref(target) {
                        state.answers.push(QueryAnswer::Ref {
                            source: "local".into(),
                            view: RefRecordView {
                                name: target.to_string(),
                                target: found.target.clone(),
                                seq: found.seq,
                                at: found.at,
                                signer: target.split('/').next().unwrap_or("").to_string(),
                            },
                        });
                    }
                    false
                }
            }
        };

        let shared = Arc::new(QueryShared {
            state: Mutex::new(state),
            settled: Condvar::new(),
            deadline,
        });
        {
            let mut registry = self.registry.lock().expect("registry lock");
            registry.push_back((shared.snapshot().id, shared.clone()));
            while registry.len() > REGISTRY_LIMIT {
                registry.pop_front();
            }
        }

        // object がローカルで見つかったら(か、書けなくて断るなら)散布は不要。
        if local_object_hit {
            let mut guard = shared.state.lock().expect("query state lock");
            if guard.outcome == QueryOutcome::Running {
                guard.outcome = QueryOutcome::Found;
            }
            drop(guard);
            shared.settled.notify_all();
            return shared;
        }

        for peer_index in 0..shared.snapshot().peers.len() {
            let shared = shared.clone();
            let store = self.store.clone();
            let factory = self.peer_factory.clone();
            std::thread::spawn(move || {
                peer_worker(shared, store, factory, peer_index);
            });
        }
        {
            let shared = shared.clone();
            std::thread::spawn(move || finalizer(shared));
        }
        shared
    }
}

fn peer_worker(
    shared: Arc<QueryShared>,
    store: Arc<Mutex<Store>>,
    factory: PeerFactory,
    peer_index: usize,
) {
    let (kind, target, address) = {
        let guard = shared.state.lock().expect("query state lock");
        (guard.kind, guard.target.clone(), guard.peers[peer_index].address.clone())
    };
    let peer = factory(&address);
    loop {
        if shared.snapshot().outcome != QueryOutcome::Running {
            return;
        }
        let attempt: Result<Option<QueryAnswer>, ()> = match kind {
            QueryKind::Object => match peer.fetch_object(&target) {
                Ok(Some(bytes)) => {
                    // content-addressing の検証(SPEC §7.1)。不一致は沈黙と同じ扱いで捨てる。
                    if c1::id_for_bytes(&bytes) == target {
                        let put = store.lock().expect("store lock").put_object(&bytes);
                        match put {
                            Ok(_) => Ok(Some(QueryAnswer::Object { source: address.clone() })),
                            Err(crate::store::StoreError::WritesDisabled(failure)) => {
                                // 置けない(取得中に別の要求が書けない状態を立てたか、この
                                // 保存が最初の失敗だった)。再試行をやめ、待つ側へ理由を渡す。
                                let mut guard = shared.state.lock().expect("query state lock");
                                if guard.outcome == QueryOutcome::Running {
                                    guard.outcome = QueryOutcome::WritesDisabled;
                                    guard.write_failure = Some(failure);
                                }
                                drop(guard);
                                shared.settled.notify_all();
                                return;
                            }
                            Err(e) => {
                                crate::log_line!("uniqnode: query の保存に失敗: {e}");
                                Err(())
                            }
                        }
                    } else {
                        crate::log_line!("uniqnode: {address} が {target} と異なる内容を返した(破棄)");
                        Err(())
                    }
                }
                Ok(None) => Ok(None),
                Err(_) => Err(()),
            },
            QueryKind::Ref => match peer.fetch_ref(&target) {
                Ok(Some(view)) => {
                    Ok(Some(QueryAnswer::Ref { source: address.clone(), view }))
                }
                Ok(None) => Ok(None),
                Err(_) => Err(()),
            },
        };
        let mut guard = shared.state.lock().expect("query state lock");
        match attempt {
            Ok(Some(answer)) => {
                guard.peers[peer_index].state = PeerState::Answered;
                guard.answers.push(answer);
                shared.settled.notify_all();
                return;
            }
            Ok(None) => {
                guard.peers[peer_index].state = PeerState::Empty;
                shared.settled.notify_all();
                return;
            }
            Err(()) => {
                guard.peers[peer_index].state = PeerState::Silent;
                shared.settled.notify_all();
            }
        }
        drop(guard);
        // 次の問い直しの周期まで待つ(予算が残っていなければ打ち切り)。
        if Instant::now() + RETRY_INTERVAL >= shared.deadline {
            return;
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
}

/// 決着まで待って、決着の規則(settlement)を適用する。
/// - object で回答が得られたら即 found(kind:object にだけある規則。同じ内容がどのピアに
///   あっても content-addressing で同一なので、1 つ得れば足りる)。
/// - それ以外は settlement の 3 値に従う。期限が来たら沈黙(pending 含む)を silent に
///   確定してから判定する。
fn finalizer(shared: Arc<QueryShared>) {
    let mut guard = shared.state.lock().expect("query state lock");
    loop {
        if guard.outcome != QueryOutcome::Running {
            break;
        }
        if guard.kind == QueryKind::Object && !guard.answers.is_empty() {
            guard.outcome = QueryOutcome::Found;
            break;
        }
        let remaining = shared.deadline.saturating_duration_since(Instant::now());
        let expired = remaining.is_zero();
        if expired {
            for peer in guard.peers.iter_mut() {
                if peer.state == PeerState::Pending {
                    peer.state = PeerState::Silent;
                }
            }
        }
        let states: Vec<PeerState> = guard.peers.iter().map(|peer| peer.state).collect();
        if let Some(outcome) = settlement(&states, !guard.answers.is_empty(), expired) {
            guard.outcome = outcome;
            break;
        }
        let (next, _) = shared
            .settled
            .wait_timeout(guard, remaining)
            .expect("condvar wait");
        guard = next;
    }
    shared.settled.notify_all();
}

// ---- 直列化(API 応答) ----

pub fn state_to_json(state: &QueryState) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert("query_id".to_string(), c1::Value::Text(state.id.clone()));
    map.insert(
        "kind".to_string(),
        c1::Value::Text(
            match state.kind {
                QueryKind::Object => "object",
                QueryKind::Ref => "ref",
            }
            .to_string(),
        ),
    );
    map.insert("target".to_string(), c1::Value::Text(state.target.clone()));
    map.insert("budget_ms".to_string(), c1::Value::Integer(state.budget_ms as i64));
    map.insert("outcome".to_string(), c1::Value::Text(state.outcome.as_str().to_string()));
    let answers: Vec<c1::Value> = state
        .answers
        .iter()
        .map(|answer| {
            let mut entry = BTreeMap::new();
            match answer {
                QueryAnswer::Object { source } => {
                    entry.insert("source".to_string(), c1::Value::Text(source.clone()));
                    entry.insert("stored".to_string(), c1::Value::Bool(true));
                }
                QueryAnswer::Ref { source, view } => {
                    entry.insert("source".to_string(), c1::Value::Text(source.clone()));
                    entry.insert("name".to_string(), c1::Value::Text(view.name.clone()));
                    entry.insert(
                        "target".to_string(),
                        match &view.target {
                            Some(t) => c1::Value::Text(t.clone()),
                            None => c1::Value::Null,
                        },
                    );
                    entry.insert("seq".to_string(), c1::Value::Integer(view.seq as i64));
                    entry.insert("signer".to_string(), c1::Value::Text(view.signer.clone()));
                }
            }
            c1::Value::Object(entry)
        })
        .collect();
    map.insert("answers".to_string(), c1::Value::Array(answers));
    let peers: Vec<c1::Value> = state
        .peers
        .iter()
        .map(|peer| {
            let mut entry = BTreeMap::new();
            entry.insert("address".to_string(), c1::Value::Text(peer.address.clone()));
            entry.insert("state".to_string(), c1::Value::Text(peer.state.as_str().to_string()));
            c1::Value::Object(entry)
        })
        .collect();
    map.insert("peers".to_string(), c1::Value::Array(peers));
    c1::to_canonical_bytes(&c1::Value::Object(map))
}

/// クエリの状態を HTTP の応答にする(POST /v1/query と GET /v1/queries/{id} の 1 箇所)。
/// 書けなくて決着したクエリは、ストアの誤りの変換(503 と案内)を通す。
pub fn state_response(state: &QueryState) -> crate::http::Response {
    match &state.write_failure {
        Some(failure) => crate::api::store_error_response(
            crate::store::StoreError::WritesDisabled(failure.clone()),
        ),
        None => crate::http::Response::json(200, state_to_json(state)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreConfig;
    use crate::sync::SyncError;

    fn temp_engine(name: &str) -> (QueryEngine, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-query-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let store = Arc::new(Mutex::new(Store::open(StoreConfig::new(&dir)).expect("open")));
        (QueryEngine::new(store, dir.clone()), dir)
    }

    /// 挙動を番号で指定できるモックピア。
    /// アドレスが "empty" → 常に無し、"answer:<bytes-hex>" → その内容、"silent" → 常に失敗。
    struct MockPeer {
        behavior: String,
    }

    impl PeerSource for MockPeer {
        fn signers(&self) -> Result<Vec<(String, u64)>, SyncError> {
            Ok(Vec::new())
        }
        fn refs_since(&self, _: &str, _: u64) -> Result<Vec<Vec<u8>>, SyncError> {
            Ok(Vec::new())
        }
        fn fetch_object(&self, _id: &str) -> Result<Option<Vec<u8>>, SyncError> {
            if self.behavior == "empty" {
                Ok(None)
            } else if let Some(hex) = self.behavior.strip_prefix("answer:") {
                Ok(Some(crate::sha2::from_hex(hex).expect("hex")))
            } else {
                Err(SyncError::Peer("silent".into()))
            }
        }
        fn fetch_ref(&self, name: &str) -> Result<Option<RefRecordView>, SyncError> {
            if self.behavior == "empty" {
                Ok(None)
            } else if self.behavior == "ref" {
                Ok(Some(RefRecordView {
                    name: name.to_string(),
                    target: None,
                    seq: 7,
                    at: 0,
                    signer: "mock".into(),
                }))
            } else {
                Err(SyncError::Peer("silent".into()))
            }
        }
    }

    fn mock_factory() -> PeerFactory {
        Arc::new(|address| Box::new(MockPeer { behavior: address.to_string() }))
    }

    /// 決着の列挙表: ピアの挙動の組ごとに outcome が仕様どおりになる。
    #[test]
    fn outcome_table_matches_the_open_world_semantics() {
        let bytes = b"the object";
        let id = c1::id_for_bytes(bytes);
        let answer = format!("answer:{}", crate::sha2::hex(bytes));

        // (スコープ, 期待outcome, 期待沈黙数)
        struct Case {
            scope: Vec<String>,
            expected: QueryOutcome,
            expected_silent: usize,
        }
        let cases = [
            Case {
                scope: vec!["empty".into(), answer.clone()],
                expected: QueryOutcome::Found,
                expected_silent: 0,
            },
            Case {
                scope: vec!["empty".into(), "empty".into()],
                expected: QueryOutcome::ScopeEmpty,
                expected_silent: 0,
            },
            Case {
                scope: vec!["empty".into(), "silent".into()],
                expected: QueryOutcome::TimedOut,
                expected_silent: 1,
            },
            Case {
                scope: vec!["silent".into(), answer.clone()],
                expected: QueryOutcome::Found,
                expected_silent: 1,
            },
            Case { scope: vec![], expected: QueryOutcome::ScopeEmpty, expected_silent: 0 },
        ];
        for (index, case) in cases.iter().enumerate() {
            let (engine, dir) = temp_engine(&format!("outcome-{index}"));
            let engine = engine.with_peer_factory(mock_factory());
            let shared = engine.start(QueryKind::Object, &id, 600, case.scope.clone());
            let state = shared.wait_settled();
            assert_eq!(state.outcome, case.expected, "case {index}");
            let silent =
                state.peers.iter().filter(|p| p.state == PeerState::Silent).count();
            assert_eq!(silent, case.expected_silent, "case {index} の沈黙数");
            if case.expected == QueryOutcome::Found {
                // 回答オブジェクトは検証の上ローカルに保存される。
                assert!(engine.store.lock().expect("lock").has_object(&id), "case {index}");
            }
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
    }

    /// ローカルに既にあるオブジェクトは散布なしで即 found。
    #[test]
    fn local_hit_settles_without_scatter() {
        let (engine, dir) = temp_engine("local-hit");
        let engine = engine.with_peer_factory(mock_factory());
        let id = {
            let mut store = engine.store.lock().expect("lock");
            store.put_object(b"already here").expect("put").0
        };
        // silent なピアがいても待たない。
        let shared = engine.start(QueryKind::Object, &id, 5_000, vec!["silent".into()]);
        let state = shared.wait_settled();
        assert_eq!(state.outcome, QueryOutcome::Found);
        assert!(matches!(&state.answers[0], QueryAnswer::Object { source } if source == "local"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 偽の内容を返すピアは沈黙と同じ扱いになり、保存もされない。
    #[test]
    fn mislabeled_answer_counts_as_silence() {
        let (engine, dir) = temp_engine("mislabeled");
        let engine = engine.with_peer_factory(mock_factory());
        let wrong = format!("answer:{}", crate::sha2::hex(b"impostor bytes"));
        let real_id = c1::id_for_bytes(b"the real thing");
        let shared = engine.start(QueryKind::Object, &real_id, 600, vec![wrong]);
        let state = shared.wait_settled();
        assert_eq!(state.outcome, QueryOutcome::TimedOut);
        assert!(!engine.store.lock().expect("lock").has_object(&real_id));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// ハンドルの観測は単調: outcome は Running から終端へ一度だけ動き、回答数は減らない。
    #[test]
    fn handle_snapshots_are_monotonic() {
        let (engine, dir) = temp_engine("monotonic");
        let engine = engine.with_peer_factory(mock_factory());
        let bytes = b"eventually found";
        let id = c1::id_for_bytes(bytes);
        let answer = format!("answer:{}", crate::sha2::hex(bytes));
        let shared = engine.start(
            QueryKind::Object,
            &id,
            2_000,
            vec!["silent".into(), "empty".into(), answer],
        );
        let mut last_answers = 0usize;
        let mut settled_seen = false;
        loop {
            let state = shared.snapshot();
            assert!(state.answers.len() >= last_answers, "回答集合は単調増加");
            last_answers = state.answers.len();
            if state.outcome != QueryOutcome::Running {
                if settled_seen {
                    // 決着後にもう一周して不変を確認した。
                    break;
                }
                settled_seen = true;
                assert_eq!(state.outcome, QueryOutcome::Found);
            }
            std::thread::yield_now();
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn peers_file_round_trip() {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-peers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(read_peer_addresses(&dir).is_empty(), "無ければ空");
        std::fs::write(
            dir.join("peers.json"),
            b"{\"peers\":[{\"address\":\"10.0.0.2:7440\"},{\"address\":\"10.0.0.3:7440\"}]}",
        )
        .expect("write");
        assert_eq!(
            read_peer_addresses(&dir),
            vec!["10.0.0.2:7440".to_string(), "10.0.0.3:7440".to_string()]
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
