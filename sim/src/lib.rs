//! uniqnode のレプリカ・健全性モデル(SPEC.md §8)の離散イベントシミュレータ。
//!
//! 1 tick = 1 単位時間。各 tick で「メッセージ配送 → 各DBノードの判断(読み取り) →
//! 適用(書き込み) → evict → レコード交換の送信 → 計測」を行う。判断は tick 開始時点の
//! 状態だけを見て行われるため、複数DBノードの同時判断による競合(同時修復・同時降格)が
//! 現実と同じ形で発生する。
//!
//! 決定論: 反復はすべて順序付きコンテナ(BTreeMap/BTreeSet/Vec)上で行い、
//! 乱数は種を与えた自前の生成器だけを使う。

use std::collections::{BTreeMap, BTreeSet};

pub type Tick = u64;
pub type NodeId = u32;
pub type ObjectId = u64;

fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// rendezvous ハッシュによる修復・降格の優先順位。値が小さいほど優先(SPEC §8.2)。
pub fn replica_rank(object: ObjectId, node: NodeId) -> u64 {
    splitmix64(splitmix64(object) ^ splitmix64(node as u64 ^ 0xA5A5_5A5A_0000_0001))
}

/// xorshift64* による決定論的乱数生成器。
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let state = splitmix64(seed) | 1;
        Rng(state)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn range_inclusive(&mut self, low: u64, high: u64) -> u64 {
        if high <= low {
            return low;
        }
        low + self.next_u64() % (high - low + 1)
    }
}

#[derive(Clone, Debug)]
pub struct Params {
    /// レコード交換の送信間隔(DBノードごとに位相をずらして送る)。
    pub exchange_period: Tick,
    /// レコード交換の配送遅延の範囲。
    pub exchange_delay_min: Tick,
    pub exchange_delay_max: Tick,
    /// これより古い heartbeat の保持者は生存とみなさない。
    pub heartbeat_timeout: Tick,
    /// 伝播時間スレッショルド。不足がこれを超えて持続したら修復開始(SPEC §8.2)。
    pub t_prop: Tick,
    /// 自己治癒スレッショルド。これを超えたら ALERT(unknown)(SPEC §8.4)。
    pub t_heal: Tick,
    /// 取得(転送)にかかる時間。
    pub fetch_time: Tick,
    /// 昇格・降格の直後に再び層を変えない猶予(SPEC §8.3 条件4)。
    pub demotion_grace: Tick,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            exchange_period: 5,
            exchange_delay_min: 1,
            exchange_delay_max: 3,
            heartbeat_timeout: 15,
            t_prop: 20,
            t_heal: 100,
            fetch_time: 3,
            demotion_grace: 20,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ObjectSpec {
    pub id: ObjectId,
    pub size: u64,
    pub min_replicas: u32,
    /// このオブジェクトが参照する他オブジェクト(evict の参照カウント保護に使う)。
    pub references: Vec<ObjectId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthState {
    Satisfied,
    Degraded,
    Alert,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertReason {
    Capacity,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AttestationView {
    held: bool,
    seq: u64,
}

#[derive(Clone, Debug)]
pub enum Event {
    HealthTransition {
        tick: Tick,
        observer: NodeId,
        object: ObjectId,
        state: HealthState,
        reason: Option<AlertReason>,
    },
    FetchStarted {
        tick: Tick,
        node: NodeId,
        object: ObjectId,
    },
    FetchAborted {
        tick: Tick,
        node: NodeId,
        object: ObjectId,
    },
    Promoted {
        tick: Tick,
        node: NodeId,
        object: ObjectId,
    },
    Demoted {
        tick: Tick,
        node: NodeId,
        object: ObjectId,
    },
    Evicted {
        tick: Tick,
        node: NodeId,
        object: ObjectId,
    },
}

#[derive(Clone, Debug)]
struct AttestationRecord {
    object: ObjectId,
    held: bool,
    seq: u64,
}

#[derive(Clone, Debug)]
struct SnapshotMessage {
    to: NodeId,
    from: NodeId,
    sender_partition: u32,
    sent_at: Tick,
    free_capacity: u64,
    attestations: Vec<AttestationRecord>,
}

#[derive(Clone, Debug)]
pub struct NodeSim {
    pub capacity: u64,
    pub alive: bool,
    pub partition: u32,
    /// 確約層。保持表明を発行済みのオブジェクト。自動 evict されない(SPEC §5.4)。
    pub committed: BTreeSet<ObjectId>,
    /// 機会層。先頭が最近使用(LRU)。
    pub cache: Vec<ObjectId>,
    own_attestation: BTreeMap<ObjectId, AttestationView>,
    view_attestation: BTreeMap<(ObjectId, NodeId), AttestationView>,
    view_heartbeat: BTreeMap<NodeId, Tick>,
    view_free: BTreeMap<NodeId, u64>,
    deficit_since: BTreeMap<ObjectId, Tick>,
    health: BTreeMap<ObjectId, HealthState>,
    fetch_done_at: BTreeMap<ObjectId, Tick>,
    last_tier_change: BTreeMap<ObjectId, Tick>,
}

impl NodeSim {
    fn new(capacity: u64) -> Self {
        NodeSim {
            capacity,
            alive: true,
            partition: 0,
            committed: BTreeSet::new(),
            cache: Vec::new(),
            own_attestation: BTreeMap::new(),
            view_attestation: BTreeMap::new(),
            view_heartbeat: BTreeMap::new(),
            view_free: BTreeMap::new(),
            deficit_since: BTreeMap::new(),
            health: BTreeMap::new(),
            fetch_done_at: BTreeMap::new(),
            last_tier_change: BTreeMap::new(),
        }
    }
}

/// 判断フェーズが出力し、適用フェーズが実行する行動。
#[derive(Clone, Debug)]
enum PlannedAction {
    SetDeficitSince {
        node: usize,
        object: ObjectId,
    },
    ClearDeficit {
        node: usize,
        object: ObjectId,
    },
    Transition {
        node: usize,
        object: ObjectId,
        state: HealthState,
        reason: Option<AlertReason>,
    },
    PromoteFromCache {
        node: usize,
        object: ObjectId,
    },
    StartFetch {
        node: usize,
        object: ObjectId,
    },
    CompleteFetch {
        node: usize,
        object: ObjectId,
    },
    Demote {
        node: usize,
        object: ObjectId,
    },
}

pub struct Sim {
    pub params: Params,
    pub now: Tick,
    pub nodes: Vec<NodeSim>,
    pub objects: BTreeMap<ObjectId, ObjectSpec>,
    pub pinned: BTreeSet<ObjectId>,
    pub events: Vec<Event>,
    messages: BTreeMap<Tick, Vec<SnapshotMessage>>,
    rng: Rng,
    /// pin されたオブジェクトごとの、同時に取得中だったDBノード数の最大値(雪崩の観測)。
    pub max_concurrent_fetch: BTreeMap<ObjectId, usize>,
    /// 一度 min_replicas に到達した後に、生存DBノード上の実複製数が min を割っていた tick 数。
    pub min_breach_ticks: BTreeMap<ObjectId, u64>,
    /// min_breach_ticks のうち、全DBノードが生存していた tick 数。
    /// 故障が存在しないのに複製数が割れている = 降格の競合など自傷によるもの。
    pub min_breach_ticks_all_alive: BTreeMap<ObjectId, u64>,
    /// breach の連続区間(開始 tick, 最終 tick)。修復遅延の分析に使う。
    pub breach_episodes: BTreeMap<ObjectId, Vec<BreachEpisode>>,
    reached_min: BTreeSet<ObjectId>,
}

#[derive(Clone, Copy, Debug)]
pub struct BreachEpisode {
    pub start: Tick,
    pub end: Tick,
}

impl Sim {
    pub fn new(params: Params, capacities: &[u64], seed: u64) -> Self {
        Sim {
            params,
            now: 0,
            nodes: capacities.iter().map(|c| NodeSim::new(*c)).collect(),
            objects: BTreeMap::new(),
            pinned: BTreeSet::new(),
            events: Vec::new(),
            messages: BTreeMap::new(),
            rng: Rng::new(seed),
            max_concurrent_fetch: BTreeMap::new(),
            min_breach_ticks: BTreeMap::new(),
            min_breach_ticks_all_alive: BTreeMap::new(),
            breach_episodes: BTreeMap::new(),
            reached_min: BTreeSet::new(),
        }
    }

    pub fn add_object(
        &mut self,
        spec: ObjectSpec,
        committed_on: &[NodeId],
        cache_on: &[NodeId],
        pinned: bool,
    ) {
        let object = spec.id;
        self.objects.insert(object, spec);
        if pinned {
            self.pinned.insert(object);
        }
        for node in committed_on {
            let n = &mut self.nodes[*node as usize];
            n.committed.insert(object);
            n.own_attestation.insert(object, AttestationView { held: true, seq: 1 });
        }
        for node in cache_on {
            self.nodes[*node as usize].cache.insert(0, object);
        }
    }

    pub fn kill(&mut self, node: NodeId) {
        self.nodes[node as usize].alive = false;
    }

    pub fn revive(&mut self, node: NodeId) {
        self.nodes[node as usize].alive = true;
    }

    pub fn set_partition(&mut self, node: NodeId, group: u32) {
        self.nodes[node as usize].partition = group;
    }

    pub fn merge_partitions(&mut self) {
        for n in &mut self.nodes {
            n.partition = 0;
        }
    }

    fn object_size(&self, object: ObjectId) -> u64 {
        self.objects[&object].size
    }

    fn committed_used(&self, node: usize) -> u64 {
        self.nodes[node].committed.iter().map(|o| self.object_size(*o)).sum()
    }

    fn cache_used(&self, node: usize) -> u64 {
        self.nodes[node].cache.iter().map(|o| self.object_size(*o)).sum()
    }

    fn free_committed_capacity(&self, node: usize) -> u64 {
        self.nodes[node].capacity.saturating_sub(self.committed_used(node))
    }

    /// 観測者 `observer` から見て保持者 `holder` が生存しているか。
    /// 自分自身は常に生存。他者は heartbeat の鮮度で判定する。
    fn holder_is_fresh(&self, observer: usize, holder: NodeId) -> bool {
        if holder as usize == observer {
            return self.nodes[observer].alive;
        }
        match self.nodes[observer].view_heartbeat.get(&holder) {
            Some(seen) => self.now.saturating_sub(*seen) <= self.params.heartbeat_timeout,
            None => false,
        }
    }

    /// 観測者から見た、生存中の保持者の一覧(自分自身の確約層は直接知識として数える)。
    fn observed_holders(&self, observer: usize, object: ObjectId) -> Vec<NodeId> {
        let mut holders = BTreeSet::new();
        if self.nodes[observer].committed.contains(&object) {
            holders.insert(observer as NodeId);
        }
        for ((o, holder), att) in &self.nodes[observer].view_attestation {
            if *o == object && att.held && self.holder_is_fresh(observer, *holder) {
                // 自分の状態は view ではなく直接知識を使う。
                if *holder as usize != observer {
                    holders.insert(*holder);
                }
            }
        }
        holders.into_iter().collect()
    }

    /// 観測者から見た修復候補: 生存していて、保持しておらず、確約層の空きが足りるDBノード。
    /// rendezvous rank 昇順で返す。
    fn observed_candidates(&self, observer: usize, object: ObjectId, size: u64) -> Vec<NodeId> {
        let holders: BTreeSet<NodeId> = self.observed_holders(observer, object).into_iter().collect();
        let mut candidates = Vec::new();
        for id in 0..self.nodes.len() as NodeId {
            if holders.contains(&id) {
                continue;
            }
            if !self.holder_is_fresh(observer, id) {
                continue;
            }
            let free = if id as usize == observer {
                self.free_committed_capacity(observer)
            } else {
                match self.nodes[observer].view_free.get(&id) {
                    Some(f) => *f,
                    None => continue,
                }
            };
            if free >= size {
                candidates.push(id);
            }
        }
        candidates.sort_by_key(|id| replica_rank(object, *id));
        candidates
    }

    /// 実複製数(神視点)。生存DBノードの確約層のみ数える。
    pub fn oracle_committed_count(&self, object: ObjectId) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.alive && n.committed.contains(&object))
            .count()
    }

    /// 神視点: どこか(死んだDBノードや機会層を含む)に複製が残っているか。
    pub fn oracle_any_copy(&self, object: ObjectId) -> bool {
        self.nodes
            .iter()
            .any(|n| n.committed.contains(&object) || n.cache.contains(&object))
    }

    /// 観測者 `node` から到達可能(生存・同一分断)なDBノードに複製が存在するか。
    fn fetch_source_exists(&self, node: usize, object: ObjectId) -> bool {
        let my_partition = self.nodes[node].partition;
        self.nodes.iter().enumerate().any(|(i, n)| {
            i != node
                && n.alive
                && n.partition == my_partition
                && (n.committed.contains(&object) || n.cache.contains(&object))
        })
    }

    pub fn health_of(&self, observer: NodeId, object: ObjectId) -> HealthState {
        *self.nodes[observer as usize]
            .health
            .get(&object)
            .unwrap_or(&HealthState::Satisfied)
    }

    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step();
        }
    }

    pub fn step(&mut self) {
        self.now += 1;
        self.deliver_messages();
        let actions = self.decide();
        self.apply(&actions);
        self.evict_all();
        self.send_record_exchange();
        self.measure();
    }

    fn deliver_messages(&mut self) {
        let batch = match self.messages.remove(&self.now) {
            Some(b) => b,
            None => return,
        };
        for message in batch {
            let recipient = &mut self.nodes[message.to as usize];
            if !recipient.alive || recipient.partition != message.sender_partition {
                continue;
            }
            let seen = recipient.view_heartbeat.entry(message.from).or_insert(0);
            if message.sent_at > *seen {
                *seen = message.sent_at;
            }
            recipient.view_free.insert(message.from, message.free_capacity);
            for record in &message.attestations {
                let key = (record.object, message.from);
                let entry = recipient
                    .view_attestation
                    .entry(key)
                    .or_insert(AttestationView { held: false, seq: 0 });
                if record.seq > entry.seq {
                    *entry = AttestationView { held: record.held, seq: record.seq };
                }
            }
        }
    }

    /// 判断フェーズ。tick 開始時点の状態だけを読み、行動を列挙する。
    fn decide(&self) -> Vec<PlannedAction> {
        let mut actions = Vec::new();
        for node in 0..self.nodes.len() {
            if !self.nodes[node].alive {
                continue;
            }
            // 取得完了の刈り取り。
            for (object, done_at) in &self.nodes[node].fetch_done_at {
                if *done_at <= self.now {
                    actions.push(PlannedAction::CompleteFetch { node, object: *object });
                }
            }
            for object in &self.pinned {
                let spec = &self.objects[object];
                if spec.min_replicas == 0 {
                    continue;
                }
                self.decide_for_pinned(node, spec, &mut actions);
            }
        }
        actions
    }

    fn decide_for_pinned(&self, node: usize, spec: &ObjectSpec, actions: &mut Vec<PlannedAction>) {
        let object = spec.id;
        let min = spec.min_replicas as usize;
        let holders = self.observed_holders(node, object);
        let count = holders.len();
        let state_now = self.health_of(node as NodeId, object);

        if count >= min {
            if self.nodes[node].deficit_since.contains_key(&object) {
                actions.push(PlannedAction::ClearDeficit { node, object });
            }
            if state_now != HealthState::Satisfied {
                actions.push(PlannedAction::Transition {
                    node,
                    object,
                    state: HealthState::Satisfied,
                    reason: None,
                });
            }
            self.decide_demotion(node, spec, &holders, actions);
            return;
        }

        // 不足している。
        let since = match self.nodes[node].deficit_since.get(&object) {
            Some(s) => *s,
            None => {
                actions.push(PlannedAction::SetDeficitSince { node, object });
                return;
            }
        };
        let duration = self.now.saturating_sub(since);
        if duration < self.params.t_prop {
            return;
        }
        let deficit = min - count;
        let candidates = self.observed_candidates(node, object, spec.size);

        // 健全性状態(SPEC §8.4)。capacity は待っても解決しないため即時 ALERT。
        let target = if candidates.len() < deficit {
            (HealthState::Alert, Some(AlertReason::Capacity))
        } else if duration >= self.params.t_heal {
            (HealthState::Alert, Some(AlertReason::Unknown))
        } else {
            (HealthState::Degraded, None)
        };
        if state_now != target.0 {
            actions.push(PlannedAction::Transition {
                node,
                object,
                state: target.0,
                reason: target.1,
            });
        }

        // 修復(SPEC §8.2): 候補中 rank 上位 deficit 台に入るときだけ行動する。
        let my_id = node as NodeId;
        let my_position = candidates.iter().position(|id| *id == my_id);
        let selected = match my_position {
            Some(p) => p < deficit,
            None => false,
        };
        if !selected {
            return;
        }
        if self.nodes[node].committed.contains(&object)
            || self.nodes[node].fetch_done_at.contains_key(&object)
        {
            return;
        }
        if self.nodes[node].cache.contains(&object) {
            actions.push(PlannedAction::PromoteFromCache { node, object });
        } else if self.fetch_source_exists(node, object) {
            actions.push(PlannedAction::StartFetch { node, object });
        }
        // 取得元が存在しない場合は何もできない。ALERT(unknown) が時間経過で拾う。
    }

    /// 降格の判断(SPEC §8.3)。4条件すべてをローカル観測で満たすときだけ降格する。
    fn decide_demotion(
        &self,
        node: usize,
        spec: &ObjectSpec,
        holders: &[NodeId],
        actions: &mut Vec<PlannedAction>,
    ) {
        let object = spec.id;
        let min = spec.min_replicas as usize;
        let my_id = node as NodeId;
        if !self.nodes[node].committed.contains(&object) {
            return;
        }
        // 条件1と3(count > min は count - 1 >= min と同値)。
        if holders.len() <= min {
            return;
        }
        // 条件2: 自分が観測上の保持者の中で rank 最下位。
        let worst = holders
            .iter()
            .max_by_key(|id| replica_rank(object, **id))
            .copied();
        if worst != Some(my_id) {
            return;
        }
        // 条件4: 直近の層変更から猶予が経過している。
        if let Some(changed) = self.nodes[node].last_tier_change.get(&object) {
            if self.now.saturating_sub(*changed) < self.params.demotion_grace {
                return;
            }
        }
        actions.push(PlannedAction::Demote { node, object });
    }

    fn apply(&mut self, actions: &[PlannedAction]) {
        for action in actions {
            match action {
                PlannedAction::SetDeficitSince { node, object } => {
                    self.nodes[*node].deficit_since.insert(*object, self.now);
                }
                PlannedAction::ClearDeficit { node, object } => {
                    self.nodes[*node].deficit_since.remove(object);
                }
                PlannedAction::Transition { node, object, state, reason } => {
                    self.nodes[*node].health.insert(*object, *state);
                    self.events.push(Event::HealthTransition {
                        tick: self.now,
                        observer: *node as NodeId,
                        object: *object,
                        state: *state,
                        reason: *reason,
                    });
                }
                PlannedAction::PromoteFromCache { node, object } => {
                    self.promote(*node, *object);
                }
                PlannedAction::StartFetch { node, object } => {
                    self.nodes[*node]
                        .fetch_done_at
                        .insert(*object, self.now + self.params.fetch_time);
                    self.events.push(Event::FetchStarted {
                        tick: self.now,
                        node: *node as NodeId,
                        object: *object,
                    });
                }
                PlannedAction::CompleteFetch { node, object } => {
                    self.nodes[*node].fetch_done_at.remove(object);
                    let size = self.object_size(*object);
                    if self.free_committed_capacity(*node) >= size {
                        self.commit(*node, *object);
                    } else {
                        self.events.push(Event::FetchAborted {
                            tick: self.now,
                            node: *node as NodeId,
                            object: *object,
                        });
                    }
                }
                PlannedAction::Demote { node, object } => {
                    self.nodes[*node].committed.remove(object);
                    self.nodes[*node].cache.insert(0, *object);
                    self.nodes[*node].last_tier_change.insert(*object, self.now);
                    let seq = self.next_attestation_seq(*node, *object);
                    self.nodes[*node]
                        .own_attestation
                        .insert(*object, AttestationView { held: false, seq });
                    self.events.push(Event::Demoted {
                        tick: self.now,
                        node: *node as NodeId,
                        object: *object,
                    });
                }
            }
        }
    }

    fn next_attestation_seq(&self, node: usize, object: ObjectId) -> u64 {
        match self.nodes[node].own_attestation.get(&object) {
            Some(a) => a.seq + 1,
            None => 1,
        }
    }

    fn promote(&mut self, node: usize, object: ObjectId) {
        let size = self.object_size(object);
        if self.free_committed_capacity(node) < size {
            return;
        }
        self.nodes[node].cache.retain(|o| *o != object);
        self.commit(node, object);
    }

    fn commit(&mut self, node: usize, object: ObjectId) {
        self.nodes[node].committed.insert(object);
        self.nodes[node].last_tier_change.insert(object, self.now);
        let seq = self.next_attestation_seq(node, object);
        self.nodes[node]
            .own_attestation
            .insert(object, AttestationView { held: true, seq });
        self.events.push(Event::Promoted {
            tick: self.now,
            node: node as NodeId,
            object,
        });
    }

    /// 機会層の evict(SPEC §5.4)。確約層には触れない。
    /// LRU 末尾から、ローカルの確約層オブジェクトが参照していないものを優先して追い出す。
    fn evict_all(&mut self) {
        for node in 0..self.nodes.len() {
            loop {
                let used = self.committed_used(node) + self.cache_used(node);
                if used <= self.nodes[node].capacity {
                    break;
                }
                let referenced = self.locally_referenced(node);
                let cache = &self.nodes[node].cache;
                if cache.is_empty() {
                    break;
                }
                let victim_position = cache
                    .iter()
                    .rposition(|o| !referenced.contains(o))
                    .unwrap_or(cache.len() - 1);
                let victim = self.nodes[node].cache.remove(victim_position);
                self.events.push(Event::Evicted {
                    tick: self.now,
                    node: node as NodeId,
                    object: victim,
                });
            }
        }
    }

    fn locally_referenced(&self, node: usize) -> BTreeSet<ObjectId> {
        let mut referenced = BTreeSet::new();
        for object in &self.nodes[node].committed {
            if let Some(spec) = self.objects.get(object) {
                for r in &spec.references {
                    referenced.insert(*r);
                }
            }
        }
        referenced
    }

    fn send_record_exchange(&mut self) {
        let period = self.params.exchange_period;
        let mut outgoing = Vec::new();
        for (index, node) in self.nodes.iter().enumerate() {
            if !node.alive {
                continue;
            }
            if !(self.now + index as u64).is_multiple_of(period) {
                continue;
            }
            let attestations: Vec<AttestationRecord> = node
                .own_attestation
                .iter()
                .map(|(object, att)| AttestationRecord {
                    object: *object,
                    held: att.held,
                    seq: att.seq,
                })
                .collect();
            let free = self.free_committed_capacity(index);
            for (peer_index, peer) in self.nodes.iter().enumerate() {
                if peer_index == index || !peer.alive || peer.partition != node.partition {
                    continue;
                }
                outgoing.push(SnapshotMessage {
                    to: peer_index as NodeId,
                    from: index as NodeId,
                    sender_partition: node.partition,
                    sent_at: self.now,
                    free_capacity: free,
                    attestations: attestations.clone(),
                });
            }
        }
        for message in outgoing {
            let delay = self
                .rng
                .range_inclusive(self.params.exchange_delay_min, self.params.exchange_delay_max);
            self.messages
                .entry(self.now + delay)
                .or_default()
                .push(message);
        }
    }

    fn measure(&mut self) {
        for object in self.pinned.clone() {
            let spec = &self.objects[&object];
            let min = spec.min_replicas as usize;
            if min == 0 {
                continue;
            }
            let fetching = self
                .nodes
                .iter()
                .filter(|n| n.alive && n.fetch_done_at.contains_key(&object))
                .count();
            let max = self.max_concurrent_fetch.entry(object).or_insert(0);
            if fetching > *max {
                *max = fetching;
            }
            let count = self.oracle_committed_count(object);
            if count >= min {
                self.reached_min.insert(object);
            } else if self.reached_min.contains(&object) {
                *self.min_breach_ticks.entry(object).or_insert(0) += 1;
                if self.nodes.iter().all(|n| n.alive) {
                    *self.min_breach_ticks_all_alive.entry(object).or_insert(0) += 1;
                }
                let episodes = self.breach_episodes.entry(object).or_default();
                match episodes.last_mut() {
                    Some(last) if last.end + 1 == self.now => last.end = self.now,
                    _ => episodes.push(BreachEpisode { start: self.now, end: self.now }),
                }
            }
        }
    }

    pub fn alert_events(&self) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|e| matches!(e, Event::HealthTransition { state: HealthState::Alert, .. }))
            .collect()
    }

    pub fn transitions_of(&self, observer: NodeId, object: ObjectId) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|e| match e {
                Event::HealthTransition { observer: o, object: obj, .. } => {
                    *o == observer && *obj == object
                }
                _ => false,
            })
            .collect()
    }

    pub fn count_events<F: Fn(&Event) -> bool>(&self, predicate: F) -> usize {
        self.events.iter().filter(|e| predicate(e)).count()
    }
}
