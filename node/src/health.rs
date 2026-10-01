//! 健全性エンジン(SPEC §8)。sim/ で検証した規則を実装に落としたもの。
//!
//! 構造は sim と同じく「観測を組み立てる → 純関数で判断する → 実行する」。判断部
//! (assess)は入出力が値だけの純関数で、sim のシナリオと同じ規則を時刻を偽装して
//! 単体テストできる。周期処理(tick)は伝播交換(レコードのみの同期)・生存確認・
//! 修復と降格の実行・遷移イベントの記録を行う。
//!
//! sim で確認した既定値の対応: exchange_period 5 / T_hb 15 / T_prop 20 / T_heal 100 /
//! 降格猶予 20(単位は sim が tick、実装は秒)。

use crate::store::Store;
use crate::sync::{self, HttpPeer, SyncReport};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct HealthParams {
    pub exchange_period: Duration,
    /// 生存信号タイムアウト。これより古い接触しかない相手は生存とみなさない(SPEC §8.2)。
    pub t_hb: Duration,
    pub t_prop: Duration,
    pub t_heal: Duration,
    pub demotion_grace: Duration,
}

impl Default for HealthParams {
    fn default() -> Self {
        HealthParams {
            exchange_period: Duration::from_secs(5),
            t_hb: Duration::from_secs(15),
            t_prop: Duration::from_secs(20),
            t_heal: Duration::from_secs(100),
            demotion_grace: Duration::from_secs(20),
        }
    }
}

/// rendezvous ハッシュ(SPEC §8.2)。値が小さいほど優先。
pub fn rendezvous_rank(root: &str, node_id: &str) -> u64 {
    let mut input = Vec::with_capacity(root.len() + node_id.len());
    input.extend_from_slice(root.as_bytes());
    input.extend_from_slice(node_id.as_bytes());
    let digest = crate::sha2::sha256(&input);
    u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootStateKind {
    Satisfied,
    Degraded,
    Alert,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Capacity,
    Unknown,
}

/// 判断への入力(1つの pin された root について観測できたこと)。
#[derive(Clone, Debug)]
pub struct RootView {
    pub root: String,
    pub required: u32,
    /// 生存割引済み(自分、または T_hb 以内に接触できた相手)の保持表明者。
    pub fresh_holders: Vec<String>,
    /// 修復候補(生存していて容量に見込みのある node_id。保持者は含めない)。
    pub candidates: Vec<String>,
    pub self_id: String,
    pub self_holds: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootAssessment {
    pub state: RootStateKind,
    pub reason: Option<AlertKind>,
    /// 自分が rendezvous で修復担当に選ばれた。
    pub repair_selected: bool,
    /// 過剰レプリカの降格条件(SPEC §8.3 の4条件)を自分が満たした。
    pub demote_selected: bool,
}

/// 判断の純関数(sim の decide_for_pinned / decide_demotion と同じ規則)。
/// deficit_for = 不足が続いている時間(不足していなければ None)。
/// since_tier_change = 自分の直近の昇格・降格からの経過(なければ None = 十分昔)。
pub fn assess(
    view: &RootView,
    deficit_for: Option<Duration>,
    since_tier_change: Option<Duration>,
    params: &HealthParams,
) -> RootAssessment {
    let count = view.fresh_holders.len();
    let required = view.required as usize;

    if count >= required {
        // 充足。過剰なら降格を検討する(SPEC §8.3)。
        let mut demote_selected = false;
        if view.self_holds && count > required {
            let worst = view
                .fresh_holders
                .iter()
                .max_by_key(|holder| rendezvous_rank(&view.root, holder));
            let grace_passed = match since_tier_change {
                None => true,
                Some(elapsed) => elapsed >= params.demotion_grace,
            };
            demote_selected = worst.map(|w| *w == view.self_id).unwrap_or(false) && grace_passed;
        }
        return RootAssessment {
            state: RootStateKind::Satisfied,
            reason: None,
            repair_selected: false,
            demote_selected,
        };
    }

    // 不足。T_prop の猶予内は伝播待ちとして扱う(SPEC §8.2)。
    let deficit_for = match deficit_for {
        None => Duration::ZERO,
        Some(d) => d,
    };
    if deficit_for < params.t_prop {
        return RootAssessment {
            state: RootStateKind::Satisfied,
            reason: None,
            repair_selected: false,
            demote_selected: false,
        };
    }
    let deficit = required - count;
    let (state, reason) = if view.candidates.len() < deficit {
        // 容量は待っても解決しない → 即時 ALERT(SPEC §8.4)。
        (RootStateKind::Alert, Some(AlertKind::Capacity))
    } else if deficit_for >= params.t_heal {
        (RootStateKind::Alert, Some(AlertKind::Unknown))
    } else {
        (RootStateKind::Degraded, None)
    };

    // 修復担当の自己選出: 候補を rank 昇順に並べ、不足数以内に自分が入るときだけ動く。
    let mut ranked = view.candidates.clone();
    ranked.sort_by_key(|node| rendezvous_rank(&view.root, node));
    let repair_selected = !view.self_holds
        && ranked
            .iter()
            .position(|node| *node == view.self_id)
            .map(|position| position < deficit)
            .unwrap_or(false);

    RootAssessment { state, reason, repair_selected, demote_selected: false }
}

// ---- エンジン(周期処理と状態) ----

#[derive(Clone, Debug)]
struct PeerContact {
    node_id: Option<String>,
    last_seen: Option<Instant>,
    /// None = 容量無制限(または未取得)。unlimited が真のときのみ無制限と確定。
    free_bytes: Option<u64>,
    unlimited: bool,
}

#[derive(Clone, Debug)]
struct RootRuntime {
    state: RootStateKind,
    reason: Option<AlertKind>,
    deficit_since: Option<Instant>,
    last_tier_change: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct HealthEvent {
    pub elapsed_ms: u64,
    pub root: String,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RootStatus {
    pub root: String,
    pub required: u32,
    pub observed: usize,
    pub state: String,
    pub reason: Option<String>,
}

pub struct HealthEngine {
    store: Arc<Mutex<Store>>,
    data_dir: PathBuf,
    pub params: HealthParams,
    contacts: Mutex<BTreeMap<String, PeerContact>>,
    roots: Mutex<BTreeMap<String, RootRuntime>>,
    events: Mutex<Vec<HealthEvent>>,
    /// ストアが書けない状態に入ったことを記録したか(遷移で 1 度だけ記録する。should/0129)。
    writes_disabled_recorded: Mutex<bool>,
    started: Instant,
}

/// ストアが書けない状態に入った遷移の記録の root と state(HealthEvent の欄を借りる)。
pub const WRITES_DISABLED_EVENT_ROOT: &str = "store";
pub const WRITES_DISABLED_EVENT_STATE: &str = "writes_disabled";

fn state_name(state: RootStateKind) -> &'static str {
    match state {
        RootStateKind::Satisfied => "satisfied",
        RootStateKind::Degraded => "degraded",
        RootStateKind::Alert => "alert",
    }
}

fn reason_name(reason: Option<AlertKind>) -> Option<String> {
    reason.map(|r| {
        match r {
            AlertKind::Capacity => "capacity",
            AlertKind::Unknown => "unknown",
        }
        .to_string()
    })
}

impl HealthEngine {
    pub fn new(store: Arc<Mutex<Store>>, data_dir: PathBuf, params: HealthParams) -> HealthEngine {
        HealthEngine {
            store,
            data_dir,
            params,
            contacts: Mutex::new(BTreeMap::new()),
            roots: Mutex::new(BTreeMap::new()),
            events: Mutex::new(Vec::new()),
            writes_disabled_recorded: Mutex::new(false),
            started: Instant::now(),
        }
    }

    /// ストアが書けない状態か。入った遷移を 1 度だけ記録する(周期ごとには記録しない。
    /// 飛ばしたことは、この 1 行と /v1/status の writes_disabled が示す)。
    fn writes_disabled(&self) -> bool {
        let kind = self
            .store
            .lock()
            .expect("store lock")
            .writes_disabled()
            .map(|failure| failure.kind.name().to_string());
        let Some(kind) = kind else {
            return false;
        };
        let mut recorded = self.writes_disabled_recorded.lock().expect("recorded lock");
        if !*recorded {
            *recorded = true;
            crate::log_line!(
                "uniqnode: health: ストアが書けない状態({kind})なので、伝播交換・修復・降格の\
                 書き込みを飛ばす"
            );
            self.push_event(HealthEvent {
                elapsed_ms: self.started.elapsed().as_millis() as u64,
                root: WRITES_DISABLED_EVENT_ROOT.to_string(),
                state: WRITES_DISABLED_EVENT_STATE.to_string(),
                reason: Some(kind),
            });
        }
        true
    }

    fn push_event(&self, event: HealthEvent) {
        let mut events = self.events.lock().expect("events lock");
        events.push(event);
        if events.len() > 1000 {
            events.remove(0);
        }
    }

    /// 周期実行のループ(serve が専用スレッドで回す)。
    pub fn run(self: Arc<Self>) {
        loop {
            self.tick();
            // 制御ループの周期(should/0104 の許容: 条件を再確認する cadence)。
            std::thread::sleep(self.params.exchange_period);
        }
    }

    pub fn events_snapshot(&self) -> Vec<HealthEvent> {
        self.events.lock().expect("events lock").clone()
    }

    pub fn roots_snapshot(&self) -> Vec<RootStatus> {
        let store = self.store.lock().expect("store lock");
        let runtime = self.roots.lock().expect("roots lock");
        store
            .effective_pins()
            .into_iter()
            .map(|(root, required)| {
                let observed = self.fresh_holders_of(&store, &root).len();
                let (state, reason) = runtime
                    .get(&root)
                    .map(|r| (state_name(r.state).to_string(), reason_name(r.reason)))
                    .unwrap_or(("satisfied".to_string(), None));
                RootStatus { root, required, observed, state, reason }
            })
            .collect()
    }

    fn fresh_holders_of(&self, store: &Store, root: &str) -> Vec<String> {
        let self_id = store.node_id_hex().to_string();
        let contacts = self.contacts.lock().expect("contacts lock");
        let now = Instant::now();
        store
            .attest_holders(root)
            .into_iter()
            .filter(|holder| {
                if *holder == self_id {
                    return true;
                }
                contacts.values().any(|contact| {
                    contact.node_id.as_deref() == Some(holder.as_str())
                        && contact
                            .last_seen
                            .map(|seen| now.duration_since(seen) <= self.params.t_hb)
                            .unwrap_or(false)
                })
            })
            .collect()
    }

    /// 1周期: 生存確認と伝播交換 → 判断 → 修復・降格の実行 → 遷移の記録。
    pub fn tick(&self) {
        // 書けない状態に入ったことは、ピアやピンの有無に依らず、入った後の最初の周期で記録する。
        // 周期の途中で入った場合は、下の書き込みの前の確かめが同じ記録を残す。
        self.writes_disabled();
        let peer_entries = crate::query::read_peer_entries(&self.data_dir);
        let peer_addresses: Vec<String> =
            peer_entries.iter().map(|entry| entry.address.clone()).collect();

        // 生存確認(status)と署名レコードの伝播交換。期限は短く(沈黙の確定を速く)。
        // node_id を主張するエントリ(明示の設定か証明書)は、実際の node_id が主張と
        // 一致するときだけ受け入れる。
        for entry_config in &peer_entries {
            let address = &entry_config.address;
            let peer = HttpPeer::with_timeout(address.clone(), Duration::from_secs(2));
            let status = peer.fetch_status();
            let mut accepted = false;
            {
                let mut contacts = self.contacts.lock().expect("contacts lock");
                let entry = contacts.entry(address.clone()).or_insert(PeerContact {
                    node_id: None,
                    last_seen: None,
                    free_bytes: None,
                    unlimited: false,
                });
                match &status {
                    Ok(view) => {
                        let identity_matches = match &entry_config.node_id {
                            None => true,
                            Some(expected) if *expected == view.node_id => true,
                            Some(expected) => {
                                crate::log_line!(
                                    "uniqnode: ピア {address} の node_id が設定と一致しない\
                                     (設定 {expected}, 実際 {})",
                                    view.node_id
                                );
                                false
                            }
                        };
                        if identity_matches {
                            entry.node_id = Some(view.node_id.clone());
                            entry.last_seen = Some(Instant::now());
                            entry.free_bytes = view.free_bytes;
                            entry.unlimited = view.free_bytes.is_none();
                            accepted = true;
                        }
                    }
                    Err(_) => {
                        // 接触失敗 = 沈黙。last_seen を進めないだけで、消しはしない。
                    }
                }
            }
            // 書けない間は伝播交換(取り込み = 書き込み)を黙って飛ばす。
            if accepted && !self.writes_disabled() {
                let mut report = SyncReport::default();
                if let Err(e) = sync::sync_records(&self.store, &peer, &mut report) {
                    crate::log_line!("uniqnode: 伝播交換({address}): {e}");
                }
            }
        }

        // 判断と実行。
        let pins = self.store.lock().expect("store lock").effective_pins();
        for (root, required) in pins {
            let assessment = self.assess_root(&root, required);
            self.record_transition(&root, &assessment);
            // 修復と降格は保持表明を書くので、書けない間は飛ばす(判断と遷移の記録は続ける)。
            if self.writes_disabled() {
                continue;
            }
            if assessment.repair_selected {
                self.execute_repair(&root, &peer_addresses);
            }
            if assessment.demote_selected {
                let mut store = self.store.lock().expect("store lock");
                match store.set_attest(&root, false) {
                    Ok(_) => {
                        self.roots
                            .lock()
                            .expect("roots lock")
                            .entry(root.clone())
                            .and_modify(|r| r.last_tier_change = Some(Instant::now()));
                    }
                    Err(e) => crate::log_line!("uniqnode: 降格に失敗({root}): {e}"),
                }
            }
        }
    }

    fn assess_root(&self, root: &str, required: u32) -> RootAssessment {
        let now = Instant::now();
        let (view, count) = {
            let store = self.store.lock().expect("store lock");
            let self_id = store.node_id_hex().to_string();
            let fresh_holders = self.fresh_holders_of(&store, root);
            let self_holds = fresh_holders.contains(&self_id)
                && store.own_attested_roots().contains(&root.to_string());
            // 修復に必要な見込みサイズ: 自分が閉包を持つならその実測、なければ 1(楽観)。
            let estimated = store.closure_bytes(root).unwrap_or(0).max(1);
            let contacts = self.contacts.lock().expect("contacts lock");
            let mut candidates: Vec<String> = contacts
                .values()
                .filter(|contact| {
                    let fresh = contact
                        .last_seen
                        .map(|seen| now.duration_since(seen) <= self.params.t_hb)
                        .unwrap_or(false);
                    let has_room =
                        contact.unlimited || contact.free_bytes.unwrap_or(0) >= estimated;
                    let is_holder = contact
                        .node_id
                        .as_ref()
                        .map(|id| fresh_holders.contains(id))
                        .unwrap_or(false);
                    fresh && has_room && !is_holder
                })
                .filter_map(|contact| contact.node_id.clone())
                .collect();
            let self_has_room = match store.free_bytes() {
                None => true,
                Some(free) => free >= estimated || store.has_object(root),
            };
            if !self_holds && self_has_room {
                candidates.push(self_id.clone());
            }
            candidates.sort();
            candidates.dedup();
            let count = fresh_holders.len();
            (
                RootView {
                    root: root.to_string(),
                    required,
                    fresh_holders,
                    candidates,
                    self_id,
                    self_holds,
                },
                count,
            )
        };

        // 不足時間の記録(判断の外側で管理する状態)。
        let (deficit_for, since_tier_change) = {
            let mut roots = self.roots.lock().expect("roots lock");
            let runtime = roots.entry(root.to_string()).or_insert(RootRuntime {
                state: RootStateKind::Satisfied,
                reason: None,
                deficit_since: None,
                last_tier_change: None,
            });
            if count >= required as usize {
                runtime.deficit_since = None;
            } else if runtime.deficit_since.is_none() {
                runtime.deficit_since = Some(now);
            }
            (
                runtime.deficit_since.map(|since| now.duration_since(since)),
                runtime.last_tier_change.map(|change| now.duration_since(change)),
            )
        };

        assess(&view, deficit_for, since_tier_change, &self.params)
    }

    /// 遷移でのみイベントを記録する(should/0129)。
    fn record_transition(&self, root: &str, assessment: &RootAssessment) {
        let mut roots = self.roots.lock().expect("roots lock");
        let runtime = roots.get_mut(root).expect("assess_root が作成済み");
        if runtime.state != assessment.state || runtime.reason != assessment.reason {
            runtime.state = assessment.state;
            runtime.reason = assessment.reason;
            self.push_event(HealthEvent {
                elapsed_ms: self.started.elapsed().as_millis() as u64,
                root: root.to_string(),
                state: state_name(assessment.state).to_string(),
                reason: reason_name(assessment.reason),
            });
        }
    }

    /// 修復: 保持者から閉包を取り寄せ、保持表明を発行する。
    fn execute_repair(&self, root: &str, peer_addresses: &[String]) {
        // 保持者のアドレスを探す(node_id → address)。
        let holder_addresses: Vec<String> = {
            let store = self.store.lock().expect("store lock");
            let holders = store.attest_holders(root);
            let contacts = self.contacts.lock().expect("contacts lock");
            peer_addresses
                .iter()
                .filter(|address| {
                    contacts
                        .get(*address)
                        .and_then(|c| c.node_id.as_ref())
                        .map(|id| holders.contains(id))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };
        // すでに閉包を全部持っている場合は昇格だけでよい(sim §8.2 の昇格優先)。
        let mut complete = {
            let store = self.store.lock().expect("store lock");
            store.has_object(root)
        };
        if !complete {
            for address in &holder_addresses {
                let peer =
                    HttpPeer::with_timeout(address.clone(), Duration::from_secs(10));
                let mut report = SyncReport::default();
                match sync::fetch_closures(&self.store, &peer, vec![root.to_string()], &mut report)
                {
                    Ok(()) => {
                        if self.store.lock().expect("store lock").has_object(root) {
                            complete = true;
                            break;
                        }
                    }
                    Err(e) => crate::log_line!("uniqnode: 修復の取り寄せ({address}): {e}"),
                }
            }
        }
        if complete {
            let mut store = self.store.lock().expect("store lock");
            match store.set_attest(root, true) {
                Ok(_) => {
                    self.roots
                        .lock()
                        .expect("roots lock")
                        .entry(root.to_string())
                        .and_modify(|r| r.last_tier_change = Some(Instant::now()));
                }
                Err(e) => crate::log_line!("uniqnode: 保持表明に失敗({root}): {e}"),
            }
        }
        // 取り寄せ元が見つからない場合は何もしない。時間経過で ALERT(unknown) が拾う。
    }
}

/// data_dir/node.json から容量と健全性パラメータを読む。
/// `{"capacity_bytes": N, "health": {"exchange_period_ms": …, "t_hb_ms": …,
///   "t_prop_ms": …, "t_heal_ms": …, "demotion_grace_ms": …}}`(すべて任意)。
pub fn read_node_config(data_dir: &std::path::Path) -> (Option<u64>, HealthParams) {
    let mut params = HealthParams::default();
    let path = data_dir.join("node.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return (None, params),
    };
    let value = match crate::c1::parse(&text) {
        Ok(v) => v,
        Err(e) => {
            crate::log_line!("uniqnode: node.json が読めない(既定値で続行): {e}");
            return (None, params);
        }
    };
    let map = match &value {
        crate::c1::Value::Object(m) => m,
        _ => return (None, params),
    };
    let capacity = match map.get("capacity_bytes") {
        Some(crate::c1::Value::Integer(n)) if *n >= 0 => Some(*n as u64),
        _ => None,
    };
    if let Some(crate::c1::Value::Object(health)) = map.get("health") {
        let ms = |key: &str, default: Duration| -> Duration {
            match health.get(key) {
                Some(crate::c1::Value::Integer(n)) if *n > 0 => {
                    Duration::from_millis(*n as u64)
                }
                _ => default,
            }
        };
        params.exchange_period = ms("exchange_period_ms", params.exchange_period);
        params.t_hb = ms("t_hb_ms", params.t_hb);
        params.t_prop = ms("t_prop_ms", params.t_prop);
        params.t_heal = ms("t_heal_ms", params.t_heal);
        params.demotion_grace = ms("demotion_grace_ms", params.demotion_grace);
    }
    (capacity, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> HealthParams {
        HealthParams {
            exchange_period: Duration::from_secs(5),
            t_hb: Duration::from_secs(15),
            t_prop: Duration::from_secs(20),
            t_heal: Duration::from_secs(100),
            demotion_grace: Duration::from_secs(20),
        }
    }

    fn view(required: u32, holders: &[&str], candidates: &[&str], self_id: &str) -> RootView {
        RootView {
            root: "s256:test-root".to_string(),
            required,
            fresh_holders: holders.iter().map(|s| s.to_string()).collect(),
            candidates: candidates.iter().map(|s| s.to_string()).collect(),
            self_id: self_id.to_string(),
            self_holds: holders.contains(&self_id),
        }
    }

    /// sim の「DBノード脱落 → T_prop 猶予 → 修復選出」に対応する規則。
    #[test]
    fn deficit_waits_t_prop_then_selects_by_rendezvous() {
        let p = params();
        let v = view(2, &["holder-a"], &["node-b", "node-c"], "node-b");

        // 猶予内は Satisfied のまま、修復もしない。
        let early = assess(&v, Some(Duration::from_secs(5)), None, &p);
        assert_eq!(early.state, RootStateKind::Satisfied);
        assert!(!early.repair_selected);

        // T_prop 経過で Degraded。rank 上位1台(不足1)だけが選出される。
        let late = assess(&v, Some(Duration::from_secs(25)), None, &p);
        assert_eq!(late.state, RootStateKind::Degraded);
        let rank_b = rendezvous_rank(&v.root, "node-b");
        let rank_c = rendezvous_rank(&v.root, "node-c");
        assert_eq!(late.repair_selected, rank_b < rank_c, "rendezvous 順位どおり");

        // もう一方の視点では選出が反転する(同時修復の雪崩がない)。
        let v_c = view(2, &["holder-a"], &["node-b", "node-c"], "node-c");
        let late_c = assess(&v_c, Some(Duration::from_secs(25)), None, &p);
        assert_eq!(late_c.repair_selected, rank_c < rank_b);
    }

    /// sim の「容量の構造的不足 → T_heal を待たず ALERT(capacity)」。
    #[test]
    fn capacity_shortage_alerts_immediately_after_t_prop() {
        let p = params();
        let v = view(2, &["holder-a"], &[], "observer");
        let result = assess(&v, Some(Duration::from_secs(21)), None, &p);
        assert_eq!(result.state, RootStateKind::Alert);
        assert_eq!(result.reason, Some(AlertKind::Capacity));

        // T_heal 未満でも出る(即時性)。
        assert!(Duration::from_secs(21) < p.t_heal);
    }

    /// sim の「取得元喪失 → T_heal 経過で ALERT(unknown)」。
    #[test]
    fn long_deficit_with_candidates_alerts_unknown_after_t_heal() {
        let p = params();
        let v = view(1, &[], &["node-b"], "observer");
        let before = assess(&v, Some(Duration::from_secs(50)), None, &p);
        assert_eq!(before.state, RootStateKind::Degraded);
        let after = assess(&v, Some(Duration::from_secs(101)), None, &p);
        assert_eq!(after.state, RootStateKind::Alert);
        assert_eq!(after.reason, Some(AlertKind::Unknown));
    }

    /// sim の降格規則(SPEC §8.3 の4条件): 過剰 + rank 最下位 + 猶予経過のときだけ。
    #[test]
    fn demotion_requires_all_four_conditions() {
        let p = params();
        let holders = ["node-a", "node-b", "node-c"];
        let worst = holders
            .iter()
            .max_by_key(|h| rendezvous_rank("s256:test-root", h))
            .expect("worst");
        let not_worst = holders.iter().find(|h| *h != worst).expect("other");

        // rank 最下位の保持者だけが降格を選ぶ。
        let v_worst = view(2, &holders, &[], worst);
        assert!(assess(&v_worst, None, None, &p).demote_selected);
        let v_other = view(2, &holders, &[], not_worst);
        assert!(!assess(&v_other, None, None, &p).demote_selected);

        // 過剰でなければ降格しない(count-1 >= min の保護と同値)。
        let two = [holders[0], holders[1]];
        let v_exact = view(2, &two, &[], two[0]);
        assert!(!assess(&v_exact, None, None, &p).demote_selected);

        // 猶予内は降格しない(振動防止)。
        let recent = assess(&v_worst, None, Some(Duration::from_secs(3)), &p);
        assert!(!recent.demote_selected);
    }

    /// 保持表明数が戻れば Satisfied に戻る(遷移で閉じる)。
    #[test]
    fn recovery_returns_to_satisfied() {
        let p = params();
        let v = view(2, &["a", "b"], &[], "observer");
        let result = assess(&v, None, None, &p);
        assert_eq!(result.state, RootStateKind::Satisfied);
        assert!(!result.repair_selected && !result.demote_selected);
    }

    /// ストアが書けない状態に入ったら、周期は書き込みを飛ばし、入ったことを 1 度だけ記録する
    /// (周期ごとに積まない)。
    #[test]
    fn a_disabled_store_is_recorded_once_and_the_tick_writes_nothing() {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-health-unit-{}-writes-disabled", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::open(crate::store::StoreConfig::new(&dir)).expect("open");
        store.inject_fault("before@pack:1").expect("inject");
        assert!(store.put_object(b"fails").is_err());
        let seq = store.last_seq();
        let store = Arc::new(Mutex::new(store));
        let engine = HealthEngine::new(Arc::clone(&store), dir.clone(), params());
        engine.tick();
        engine.tick();
        let recorded: Vec<HealthEvent> = engine
            .events_snapshot()
            .into_iter()
            .filter(|event| event.root == WRITES_DISABLED_EVENT_ROOT)
            .collect();
        assert_eq!(recorded.len(), 1, "遷移の記録は 1 度だけ");
        assert_eq!(recorded[0].state, WRITES_DISABLED_EVENT_STATE);
        assert_eq!(recorded[0].reason.as_deref(), Some("io"));
        assert_eq!(store.lock().expect("lock").last_seq(), seq, "周期は何も書かない");
        drop(engine);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
