//! グラフ層(作業グラフ)。SPEC §4.3 の kind:node と kind:edge の上に、可変な名前空間を
//! ref 層で載せる。設計と、なぜこの形かは docs/design/GRAPH.md にある。
//!
//! 中核の問題は 1 つだけである: オブジェクトは不変(I1)で、作業の項目は状態が変わる。
//! 素朴に「節点 = kind:node」にすると、状態を変えるたびに ID が変わり、その節点を指して
//! いた辺が古い版を指したままになる。だから可変な状態は ref 層だけに置き(I2)、1 つの
//! 節点を 3 つに分ける。
//!
//! 1. 恒等ノード: 中身は名前だけで、二度と変わらない。その ID が節点の永久の身元である。
//! 2. 状態ノード: 属性の実体。更新のたびに新しい ID で、previous が前版を指す。
//! 3. ref `graph/<グラフ>/nodes/<節点>`: 現行の状態ノードを指す唯一の可変状態。
//!
//! 辺は恒等ノードの ID を members に持つ(I7)。だから節点をいくら更新しても辺は張り替え
//! なくてよい。辺 1 本が ref 1 本なので gc の根に入り(根は ref・pin・保持表明の和集合。
//! docs/design/GC.md)、削除は tombstone で表せる。
//!
//! この層は意味を知らない。state の語彙も priority の意味も、どの型が向きを持つかも
//! 呼び手のものである。知ると、呼び手の書式が uniqnode の仕様の一部になる。

use crate::c1::{self, Value};
use crate::store::{Result, Store, StoreError};
use std::collections::BTreeMap;

/// ref のパスの頭。この下だけがグラフ層の名前空間である(collections/ 配下ではないので、
/// 検索索引の世代も見えも動かさない。node/src/search.rs の Generation)。
pub const REF_PREFIX: &str = "graph";
/// 名前(グラフ名・節点名・辺の型)に許す長さ。
pub const NAME_MAX_CHARS: usize = 64;
/// attrs の c1 正規形に許す大きさ。作業の項目 1 つ分であり、本文の置き場ではない
/// (計画文書の本文は文書層に置き、節点は anchor で指す)。
pub const ATTRS_MAX_BYTES: usize = 8 * 1024;

/// 名前に許す字種: ASCII の英数字と `_` `-` `.`。`/` と空白を含まないので ref のパスに
/// そのまま置け、`.` と `..` だけの名前は道に見えるので断る。判断はこの 1 箇所にある
/// (should/0135)。
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= NAME_MAX_CHARS
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// 名前を断る理由の文面。応答も試験もここから作る(must/0023)。
pub fn invalid_name_refusal(what: &str, name: &str) -> String {
    format!(
        "{what} {name:?} の形が違う(ASCII の英数字と _ - . の 1..={NAME_MAX_CHARS} 字。\
         / と空白は使えない)"
    )
}

// ---- オブジェクトの形 ----

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = BTreeMap::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

/// 恒等ノード。中身は名前だけで、二度と変わらない(この ID が節点の永久の身元)。
pub fn identity_value(graph: &str, node: &str) -> Value {
    object(vec![
        ("v", Value::Integer(1)),
        ("kind", text("node")),
        (
            "contents",
            object(vec![("graph", text(graph)), ("id", text(node))]),
        ),
    ])
}

/// 状態ノード。attrs が実体で、previous が前版を指す(初版はキーなし)。
fn state_value(identity: &str, attrs: &Value, previous: Option<&str>) -> Value {
    let mut contents = vec![("identity", text(identity)), ("attrs", attrs.clone())];
    if let Some(previous) = previous {
        contents.push(("previous", text(previous)));
    }
    object(vec![
        ("v", Value::Integer(1)),
        ("kind", text("node")),
        ("contents", object(contents)),
    ])
}

/// 辺の型ノード。型は第一級のオブジェクトである(I7)。
pub fn edge_type_value(edge_type: &str) -> Value {
    object(vec![
        ("v", Value::Integer(1)),
        ("kind", text("node")),
        ("contents", object(vec![("edge_type", text(edge_type))])),
    ])
}

/// 辺。members は恒等ノードの ID で、順序が向きである(I7)。どの型が向きを持つかを
/// この層は知らない: 向きを持たない型を使うなら、呼び手が members の順を正規化して渡す。
fn edge_value(type_id: &str, from_identity: &str, to_identity: &str) -> Value {
    object(vec![
        ("v", Value::Integer(1)),
        ("kind", text("edge")),
        ("type", text(type_id)),
        (
            "members",
            Value::Array(vec![text(from_identity), text(to_identity)]),
        ),
    ])
}

// ---- ref の名前 ----

pub fn node_ref_path(graph: &str, node: &str) -> String {
    format!("{REF_PREFIX}/{graph}/nodes/{node}")
}

pub fn edge_ref_path(graph: &str, edge_type: &str, from: &str, to: &str) -> String {
    format!("{REF_PREFIX}/{graph}/edges/{edge_type}/{from}/{to}")
}

/// 完全名(`<署名者>/<パス>`)を節点の ref として読む。読み方はこの 1 箇所にある
/// (search::document_ref_parts と同じ役目。should/0135)。
pub fn node_ref_parts(name: &str) -> Option<(&str, &str)> {
    let (_signer, path) = name.split_once('/')?;
    let rest = path.strip_prefix(REF_PREFIX)?.strip_prefix('/')?;
    let (graph, rest) = rest.split_once('/')?;
    let node = rest.strip_prefix("nodes/")?;
    (!graph.is_empty() && !node.is_empty() && !node.contains('/')).then_some((graph, node))
}

/// 完全名を辺の ref として読む。返すのは (グラフ, 型, from, to)。
pub fn edge_ref_parts(name: &str) -> Option<(&str, &str, &str, &str)> {
    let (_signer, path) = name.split_once('/')?;
    let rest = path.strip_prefix(REF_PREFIX)?.strip_prefix('/')?;
    let (graph, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("edges/")?;
    let (edge_type, rest) = rest.split_once('/')?;
    let (from, to) = rest.split_once('/')?;
    (!graph.is_empty() && !edge_type.is_empty() && !from.is_empty() && !to.is_empty() && !to.contains('/'))
        .then_some((graph, edge_type, from, to))
}

// ---- 見え ----

/// 節点 1 つの現在。
#[derive(Clone, Debug)]
pub struct NodeView {
    pub name: String,
    /// 恒等ノードの ID(辺が指しているのはこれ)。
    pub identity: String,
    /// 現行の状態ノードの ID。
    pub state: String,
    pub attrs: Value,
    pub seq: u64,
    /// この版を見えに置いた ref レコードの時刻(unix 秒)。着地の時刻はこれである。
    pub at: i64,
}

/// 辺 1 本の現在。
#[derive(Clone, Debug)]
pub struct EdgeView {
    pub edge_type: String,
    pub from: String,
    pub to: String,
    /// 辺オブジェクトの ID。
    pub id: String,
    pub seq: u64,
    pub at: i64,
}

/// 版 1 つ(history)。
#[derive(Clone, Debug)]
pub struct Version {
    pub state: String,
    pub attrs: Value,
    /// その版を見えに置いた ref レコードの時刻。reflog に見当たらなければ None
    /// (忘却された古いレコード。SPEC §4.4)。
    pub at: Option<i64>,
}

/// 書き込みの結果。updated が false なら何も書いていない(同じ attrs の再 PUT)。
#[derive(Clone, Debug)]
pub struct PutOutcome {
    pub identity: String,
    pub state: String,
    pub updated: bool,
    pub seq: Option<u64>,
    pub new_objects: usize,
}

// ---- 読み ----

/// 節点の ref のうち、tombstone でないものを (グラフ, 節点) で引ける形に集める。同じ名前を
/// 別の署名者が持っていたら (at, seq, 署名者) の大きい方を採る(複製した先で読むときの
/// ための決定的な選び方。should/0125)。
fn live_refs<'a, T, F>(store: &'a Store, parse: F) -> BTreeMap<T, (&'a str, &'a crate::store::RefState)>
where
    T: Ord,
    F: Fn(&'a str) -> Option<T>,
{
    let mut out: BTreeMap<T, (&'a str, &'a crate::store::RefState)> = BTreeMap::new();
    for (name, state) in store.list_refs() {
        let Some(target) = state.target.as_deref() else { continue };
        let Some(key) = parse(name.as_str()) else { continue };
        let entry = (target, state);
        match out.get(&key) {
            Some((_, existing)) if (existing.at, existing.seq) >= (state.at, state.seq) => {}
            _ => {
                out.insert(key, entry);
            }
        }
    }
    out
}

/// 状態ノードのオブジェクトから (identity, attrs, previous) を読む。形が違えば None
/// (グラフ層の外から put_object された同名の何かを、節点として読まない)。
fn read_state(bytes: &[u8]) -> Option<(String, Value, Option<String>)> {
    let value = c1::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let Value::Object(map) = &value else { return None };
    if map.get("kind") != Some(&Value::Text("node".to_string())) {
        return None;
    }
    let Some(Value::Object(contents)) = map.get("contents") else { return None };
    let Some(Value::Text(identity)) = contents.get("identity") else { return None };
    let attrs = contents.get("attrs")?.clone();
    let previous = match contents.get("previous") {
        Some(Value::Text(previous)) => Some(previous.clone()),
        _ => None,
    };
    Some((identity.clone(), attrs, previous))
}

/// 辺のオブジェクトから型の ID を読む(members は ref の名前から分かるので読み直さない)。
fn is_edge(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    let Ok(Value::Object(map)) = c1::parse(text) else { return false };
    map.get("kind") == Some(&Value::Text("edge".to_string()))
}

/// グラフの名前の一覧(節点か辺を 1 つでも持つグラフ)。
pub fn list_graphs(store: &Store) -> Vec<String> {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (name, state) in store.list_refs() {
        if state.target.is_none() {
            continue;
        }
        if let Some((graph, _)) = node_ref_parts(name.as_str()) {
            names.insert(graph.to_string());
        } else if let Some((graph, _, _, _)) = edge_ref_parts(name.as_str()) {
            names.insert(graph.to_string());
        }
    }
    names.into_iter().collect()
}

/// グラフの節点を名前順に返す。読めない(ストアに無い・形が崩れた)ものは飛ばさずに
/// 断る: 節点の一覧は描画の入力であり、黙って欠けた一覧を渡すと、消えた項目と
/// 読めなかった項目が見分けられない(must/0022)。
pub fn list_nodes(store: &Store, graph: &str) -> Result<Vec<NodeView>> {
    let refs = live_refs(store, |name| {
        node_ref_parts(name).and_then(|(g, n)| (g == graph).then(|| n.to_string()))
    });
    let mut out = Vec::with_capacity(refs.len());
    for (node, (target, ref_state)) in refs {
        let Some(bytes) = store.get_object(target)? else {
            return Err(StoreError::Invalid(format!(
                "節点 {node} の状態 {target} をこのDBノードは持っていない"
            )));
        };
        let Some((identity, attrs, _)) = read_state(&bytes) else {
            return Err(StoreError::Invalid(format!(
                "節点 {node} の状態 {target} が状態ノードの形ではない"
            )));
        };
        out.push(NodeView {
            name: node,
            identity,
            state: target.to_string(),
            attrs,
            seq: ref_state.seq,
            at: ref_state.at,
        });
    }
    Ok(out)
}

/// グラフの辺を (型, from, to) の順に返す。
pub fn list_edges(store: &Store, graph: &str) -> Result<Vec<EdgeView>> {
    let refs = live_refs(store, |name| {
        edge_ref_parts(name).and_then(|(g, t, f, to)| {
            (g == graph).then(|| (t.to_string(), f.to_string(), to.to_string()))
        })
    });
    let mut out = Vec::with_capacity(refs.len());
    for ((edge_type, from, to), (target, ref_state)) in refs {
        out.push(EdgeView {
            edge_type,
            from,
            to,
            id: target.to_string(),
            seq: ref_state.seq,
            at: ref_state.at,
        });
    }
    Ok(out)
}

/// 節点 1 つの現在。無ければ None(tombstone も無いのと同じ扱いで、404 の材料になる)。
pub fn get_node(store: &Store, graph: &str, node: &str) -> Result<Option<NodeView>> {
    Ok(list_nodes(store, graph)?
        .into_iter()
        .find(|view| view.name == node))
}

/// 版の鎖(現在から古い方へ)。previous が指す先をこのDBノードが持っていなければそこで
/// 止め、止まったことを言えるように、読めた分だけを返す(世界が忘れた歴史。SPEC §6.5)。
pub fn history(store: &Store, graph: &str, node: &str) -> Result<Option<Vec<Version>>> {
    let Some(current) = get_node(store, graph, node)? else {
        return Ok(None);
    };
    // その名前の ref レコードを 1 度だけ集めて、版 → 時刻の表にする。reflog 全体の走査に
    // なるが、history は描画の道ではなく、たまに引く道である(docs/design/GRAPH.md の「費用」)。
    let mut at_of: BTreeMap<String, i64> = BTreeMap::new();
    let full_name_suffix = format!("/{}", node_ref_path(graph, node));
    for (signer, _) in store.signers() {
        for payload in store.export_ref_records(&signer, 0)? {
            let Ok(text) = std::str::from_utf8(&payload) else { continue };
            let Ok(Value::Object(record)) = c1::parse(text) else { continue };
            let Some(Value::Text(name)) = record.get("name") else { continue };
            if !name.ends_with(&full_name_suffix) {
                continue;
            }
            let (Some(Value::Text(target)), Some(Value::Integer(at))) =
                (record.get("target"), record.get("at"))
            else {
                continue;
            };
            // 同じ版を 2 度置いた(巻き戻し)ときは最初の時刻を採る。
            at_of.entry(target.clone()).or_insert(*at);
        }
    }
    let mut versions = Vec::new();
    let mut next = Some(current.state.clone());
    while let Some(id) = next {
        let Some(bytes) = store.get_object(&id)? else { break };
        let Some((_, attrs, previous)) = read_state(&bytes) else { break };
        versions.push(Version { state: id.clone(), attrs, at: at_of.get(&id).copied() });
        next = previous;
    }
    Ok(Some(versions))
}

/// 節点に繋がる辺。direction は "in"・"out"・"both"。
pub fn neighbors(
    store: &Store,
    graph: &str,
    node: &str,
    direction: Direction,
    edge_type: Option<&str>,
) -> Result<Vec<EdgeView>> {
    Ok(list_edges(store, graph)?
        .into_iter()
        .filter(|edge| edge_type.is_none_or(|t| edge.edge_type == t))
        .filter(|edge| match direction {
            Direction::Out => edge.from == node,
            Direction::In => edge.to == node,
            Direction::Both => edge.from == node || edge.to == node,
        })
        .collect())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    In,
    Out,
    Both,
}

impl Direction {
    pub fn parse(text: &str) -> Option<Direction> {
        match text {
            "in" => Some(Direction::In),
            "out" => Some(Direction::Out),
            "both" => Some(Direction::Both),
            _ => None,
        }
    }
}

// ---- 書き ----

/// 節点の作成と更新。同じ attrs の再 PUT は何も書かない(updated が false)。書く順は
/// オブジェクト(恒等 → 状態)→ ref である(set_ref は存在しない target を拒む。SPEC §5.3)。
pub fn put_node(store: &mut Store, graph: &str, node: &str, attrs: &Value) -> Result<PutOutcome> {
    let canonical = c1::to_canonical_bytes(attrs);
    if canonical.len() > ATTRS_MAX_BYTES {
        return Err(StoreError::Invalid(format!(
            "attrs が大きすぎる({} バイト。上限 {ATTRS_MAX_BYTES})",
            canonical.len()
        )));
    }
    let current = get_node(store, graph, node)?;
    if let Some(current) = &current {
        if &current.attrs == attrs {
            // 同じ言明を 2 度書かない。ref も触らないので seq も進まない(I1 の帰結)。
            return Ok(PutOutcome {
                identity: current.identity.clone(),
                state: current.state.clone(),
                updated: false,
                seq: None,
                new_objects: 0,
            });
        }
    }
    let mut new_objects = 0usize;
    let (identity, is_new) =
        store.put_object(&c1::to_canonical_bytes(&identity_value(graph, node)))?;
    new_objects += usize::from(is_new);
    let previous = current.as_ref().map(|view| view.state.clone());
    let state_bytes =
        c1::to_canonical_bytes(&state_value(&identity, attrs, previous.as_deref()));
    let (state, is_new) = store.put_object(&state_bytes)?;
    new_objects += usize::from(is_new);
    let seq = store.set_ref(&node_ref_path(graph, node), Some(&state))?;
    Ok(PutOutcome { identity, state, updated: true, seq: Some(seq), new_objects })
}

/// 節点の tombstone。繋がったままの辺があれば断り、どれかを言う(黙って辺を宙に浮かせ
/// ない。must/0022)。返り値は「在ったものを消した」かどうか。
pub fn delete_node(store: &mut Store, graph: &str, node: &str) -> Result<bool> {
    if get_node(store, graph, node)?.is_none() {
        return Ok(false);
    }
    let attached = neighbors(store, graph, node, Direction::Both, None)?;
    if !attached.is_empty() {
        let names: Vec<String> = attached
            .iter()
            .map(|edge| format!("{}/{}/{}", edge.edge_type, edge.from, edge.to))
            .collect();
        return Err(StoreError::Invalid(format!(
            "節点 {node} には辺が {} 本ある(先に消す: {})",
            names.len(),
            names.join(", ")
        )));
    }
    store.set_ref(&node_ref_path(graph, node), None)?;
    Ok(true)
}

/// 辺の作成。両端が節点として在ることを求める(在らぬ先への辺を作れると、描画の入力に
/// 名前だけの端が現れる)。同じ辺の再作成は ref を張り直すだけで、オブジェクトは増えない。
pub fn put_edge(
    store: &mut Store,
    graph: &str,
    edge_type: &str,
    from: &str,
    to: &str,
) -> Result<PutOutcome> {
    for end in [from, to] {
        if get_node(store, graph, end)?.is_none() {
            return Err(StoreError::Invalid(format!(
                "節点 {end} がグラフ {graph} に無い(辺の両端は先に作る)"
            )));
        }
    }
    let path = edge_ref_path(graph, edge_type, from, to);
    let existing = store.get_ref(&store.own_ref_name(&path)).and_then(|s| s.target.clone());
    let mut new_objects = 0usize;
    let (type_id, is_new) =
        store.put_object(&c1::to_canonical_bytes(&edge_type_value(edge_type)))?;
    new_objects += usize::from(is_new);
    let (from_id, is_new) =
        store.put_object(&c1::to_canonical_bytes(&identity_value(graph, from)))?;
    new_objects += usize::from(is_new);
    let (to_id, is_new) = store.put_object(&c1::to_canonical_bytes(&identity_value(graph, to)))?;
    new_objects += usize::from(is_new);
    let (edge, is_new) =
        store.put_object(&c1::to_canonical_bytes(&edge_value(&type_id, &from_id, &to_id)))?;
    new_objects += usize::from(is_new);
    if existing.as_deref() == Some(edge.as_str()) {
        // 既に同じ辺が同じ名前で在る。ref を触らない(seq を進めない)。
        return Ok(PutOutcome {
            identity: type_id,
            state: edge,
            updated: false,
            seq: None,
            new_objects,
        });
    }
    let seq = store.set_ref(&path, Some(&edge))?;
    Ok(PutOutcome { identity: type_id, state: edge, updated: true, seq: Some(seq), new_objects })
}

/// 辺の tombstone。返り値は「在ったものを消した」かどうか。
pub fn delete_edge(
    store: &mut Store,
    graph: &str,
    edge_type: &str,
    from: &str,
    to: &str,
) -> Result<bool> {
    let present = list_edges(store, graph)?
        .into_iter()
        .any(|edge| edge.edge_type == edge_type && edge.from == from && edge.to == to);
    if !present {
        return Ok(false);
    }
    store.set_ref(&edge_ref_path(graph, edge_type, from, to), None)?;
    Ok(true)
}

/// 辺が生きているかどうかを、辺オブジェクトの形からも確かめる(グラフ層の外から張られた
/// 同名の ref を辺として読まないための検め。list_edges は ref の名前だけで組むので、
/// 形の検めが要る場面ではこちらを呼ぶ)。
pub fn edge_object_is_well_formed(store: &Store, id: &str) -> Result<bool> {
    Ok(match store.get_object(id)? {
        Some(bytes) => is_edge(&bytes),
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_one_word_without_slashes_or_spaces() {
        assert!(is_valid_name("n_verbatim_measure"));
        assert!(is_valid_name("lamalium-plan"));
        assert!(is_valid_name("blocks"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("n a"));
        assert!(!is_valid_name("a/b"));
        assert!(!is_valid_name("."), "道に見える名前は断る");
        assert!(!is_valid_name(".."));
        assert!(!is_valid_name(&"a".repeat(NAME_MAX_CHARS + 1)));
        // 断りの文面は 1 箇所から出る(must/0023)。
        assert!(invalid_name_refusal("節点名", "n a").contains("n a"));
    }

    #[test]
    fn ref_names_round_trip() {
        let signer = "a".repeat(64);
        let node = format!("{signer}/{}", node_ref_path("plan", "n_a"));
        assert_eq!(node_ref_parts(&node), Some(("plan", "n_a")));
        assert_eq!(edge_ref_parts(&node), None);

        let edge = format!("{signer}/{}", edge_ref_path("plan", "blocks", "n_a", "n_b"));
        assert_eq!(edge_ref_parts(&edge), Some(("plan", "blocks", "n_a", "n_b")));
        assert_eq!(node_ref_parts(&edge), None);

        // グラフ層の外の ref は読まない(collections/ の文書を節点と読み違えない)。
        assert_eq!(node_ref_parts(&format!("{signer}/collections/notes/memo")), None);
        assert_eq!(node_ref_parts(&format!("{signer}/graphs/plan/nodes/n_a")), None);
        assert_eq!(node_ref_parts(&format!("{signer}/graph/plan/nodes/a/b")), None);
    }

    #[test]
    fn the_identity_object_never_changes_and_the_state_object_does() {
        let identity = c1::object_id(&identity_value("plan", "n_a"));
        assert_eq!(identity, c1::object_id(&identity_value("plan", "n_a")));
        assert_ne!(identity, c1::object_id(&identity_value("plan", "n_b")));
        assert_ne!(identity, c1::object_id(&identity_value("other", "n_a")));

        let planned = c1::parse(r#"{"state":"planned"}"#).expect("attrs");
        let landed = c1::parse(r#"{"state":"landed"}"#).expect("attrs");
        let first = state_value(&identity, &planned, None);
        let second = state_value(&identity, &landed, Some(&c1::object_id(&first)));
        assert_ne!(c1::object_id(&first), c1::object_id(&second));
        // 前版を指す鎖があるので、同じ attrs でも版が違えば別の ID になる。
        let third = state_value(&identity, &planned, Some(&c1::object_id(&second)));
        assert_ne!(c1::object_id(&third), c1::object_id(&first));
    }

    #[test]
    fn an_edge_points_at_identities_not_at_versions() {
        let from = c1::object_id(&identity_value("plan", "n_a"));
        let to = c1::object_id(&identity_value("plan", "n_b"));
        let type_id = c1::object_id(&edge_type_value("blocks"));
        let edge = edge_value(&type_id, &from, &to);
        let Value::Object(map) = &edge else { panic!("オブジェクト") };
        assert_eq!(map.get("kind"), Some(&Value::Text("edge".to_string())));
        assert_eq!(
            map.get("members"),
            Some(&Value::Array(vec![text(&from), text(&to)])),
            "members の順が向きである"
        );
        // 向きを入れ替えれば別の辺(この層は正規化しない)。
        assert_ne!(
            c1::object_id(&edge),
            c1::object_id(&edge_value(&type_id, &to, &from))
        );
    }
}
