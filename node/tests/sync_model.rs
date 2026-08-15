//! レプリケーションのランダム化モデルテスト。実装そのもの(Store + sync の核)を
//! 決定論的な乱数で駆動し、任意の書き込み・同期・再起動の交錯の後に全DBノードが
//! 収束することを検証する。sim/ が仕様 §8 を机上で検証したのと同じ発想を、
//! 実コードの §7.3 に適用したもの。
//!
//! 不変条件(全 seed で成立しなければならない):
//! - I-a 収束: 全対の全方向同期を数周した後、全ストアの ref 集合(全名前空間)が一致する。
//! - I-b 閉包: どのストアでも、ref の対象から辿れる参照は「意図的に存在しない ID」を
//!   除きすべてローカルに存在する。
//! - I-c 健全: 全ストアで fsck エラーが 0、かつ全同期後は foreign の欠けも 0。

use std::collections::BTreeSet;
use std::path::PathBuf;
use uniqnode::c1;
use uniqnode::store::{Store, StoreConfig};
use uniqnode::sync::{sync_from_peer, LocalPeer};
use uniqnode_sim::Rng;

const NODE_COUNT: usize = 3;
const OPERATIONS: usize = 120;

struct ModelNode {
    store: Option<Store>,
    dir: PathBuf,
    /// 自分の名前空間に作った ref パス(tombstone の選択肢)。
    own_paths: Vec<String>,
    /// 直前に書いたオブジェクト(チェーンの prev 参照に使う)。
    chain_head: Option<String>,
}

fn open_store(dir: &PathBuf) -> Store {
    let mut config = StoreConfig::new(dir);
    // モデル中に封印も起きるように小さくする(回復経路のカバレッジ)。
    config.pack_seal_bytes = 512;
    Store::open(config).expect("open store")
}

fn model_dir(seed: u64, index: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "uniqnode-model-{}-s{seed}-n{index}",
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    dir
}

fn run_model(seed: u64) {
    let mut rng = Rng::new(seed);
    let mut nodes: Vec<ModelNode> = (0..NODE_COUNT)
        .map(|i| {
            let dir = model_dir(seed, i);
            ModelNode { store: Some(open_store(&dir)), dir, own_paths: Vec::new(), chain_head: None }
        })
        .collect();
    let mut dangling: BTreeSet<String> = BTreeSet::new();
    let mut counter: u64 = 0;

    for _ in 0..OPERATIONS {
        let choice = rng.range_inclusive(0, 99);
        let i = rng.range_inclusive(0, NODE_COUNT as u64 - 1) as usize;
        match choice {
            // 書き込み: チェーン状のオブジェクト + ref。
            0..=44 => {
                counter += 1;
                let node = &mut nodes[i];
                let store = node.store.as_mut().expect("open");
                let body = match &node.chain_head {
                    Some(prev) => format!("{{\"n\":{counter},\"prev\":\"{prev}\",\"v\":1}}"),
                    None => format!("{{\"n\":{counter},\"v\":1}}"),
                };
                let (id, _) = store.put_object(body.as_bytes()).expect("put");
                let path = format!("k/{counter}");
                store.set_ref(&path, Some(&id)).expect("set_ref");
                node.own_paths.push(path);
                node.chain_head = Some(id);
            }
            // tombstone。
            45..=54 => {
                let node = &mut nodes[i];
                if node.own_paths.is_empty() {
                    continue;
                }
                let pick =
                    rng.range_inclusive(0, node.own_paths.len() as u64 - 1) as usize;
                let path = node.own_paths[pick].clone();
                node.store.as_mut().expect("open").set_ref(&path, None).expect("tombstone");
            }
            // 意図的に存在しない参照(忘れられた歴史)を含む辺。
            55..=64 => {
                counter += 1;
                let fake = format!(
                    "s256:{:064x}",
                    (rng.next_u64() as u128) << 64 | rng.next_u64() as u128
                );
                dangling.insert(fake.clone());
                let node = &mut nodes[i];
                let store = node.store.as_mut().expect("open");
                let body =
                    format!("{{\"kind\":\"edge\",\"members\":[\"{fake}\"],\"n\":{counter},\"v\":1}}");
                let (id, _) = store.put_object(body.as_bytes()).expect("put");
                let path = format!("k/{counter}");
                store.set_ref(&path, Some(&id)).expect("set_ref");
                node.own_paths.push(path);
            }
            // 同期 i ← j。
            65..=89 => {
                let j = rng.range_inclusive(0, NODE_COUNT as u64 - 1) as usize;
                if i == j {
                    continue;
                }
                let source = nodes[j].store.take().expect("open");
                let mut destination = nodes[i].store.take().expect("open");
                sync_from_peer(&mut destination, &LocalPeer(&source)).expect("sync");
                nodes[j].store = Some(source);
                nodes[i].store = Some(destination);
            }
            // 再起動(drop → open で回復経路を通す)。
            _ => {
                let node = &mut nodes[i];
                node.store = None;
                node.store = Some(open_store(&node.dir));
            }
        }
    }

    // 収束: 全対全方向の同期を数周。
    for _ in 0..3 {
        for i in 0..NODE_COUNT {
            for j in 0..NODE_COUNT {
                if i == j {
                    continue;
                }
                let source = nodes[j].store.take().expect("open");
                let mut destination = nodes[i].store.take().expect("open");
                sync_from_peer(&mut destination, &LocalPeer(&source)).expect("sync");
                nodes[j].store = Some(source);
                nodes[i].store = Some(destination);
            }
        }
    }

    // I-a: ref 集合の一致。
    let snapshot = |store: &Store| -> Vec<(String, Option<String>, u64)> {
        store
            .list_refs()
            .map(|(name, state)| (name.clone(), state.target.clone(), state.seq))
            .collect()
    };
    let reference = snapshot(nodes[0].store.as_ref().expect("open"));
    for (index, node) in nodes.iter().enumerate().skip(1) {
        assert_eq!(
            snapshot(node.store.as_ref().expect("open")),
            reference,
            "seed {seed}: store {index} の ref 集合が store 0 と一致しない"
        );
    }

    // I-b: 閉包の完全性(意図的な dangling を除く)。I-c: fsck。
    for (index, node) in nodes.iter().enumerate() {
        let store = node.store.as_ref().expect("open");
        for (name, state) in store.list_refs() {
            let target = match &state.target {
                None => continue,
                Some(t) => t.clone(),
            };
            let mut queue = vec![target];
            let mut seen = BTreeSet::new();
            while let Some(id) = queue.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                if dangling.contains(&id) {
                    continue;
                }
                let bytes = store
                    .get_object(&id)
                    .expect("read")
                    .unwrap_or_else(|| {
                        panic!("seed {seed}: store {index} で {name} の閉包の {id} が欠けている")
                    });
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    if let Ok(value) = c1::parse(text) {
                        let mut references = Vec::new();
                        c1::collect_references(&value, &mut references);
                        queue.extend(references);
                    }
                }
            }
        }
        let report = store.fsck().expect("fsck");
        assert!(
            report.errors.is_empty(),
            "seed {seed}: store {index} fsck: {:?}",
            report.errors
        );
        assert_eq!(
            report.foreign_targets_absent, 0,
            "seed {seed}: store {index} に取り残された欠けがある"
        );
    }

    for node in nodes {
        drop(node.store);
        std::fs::remove_dir_all(&node.dir).expect("cleanup");
    }
}

#[test]
fn randomized_writes_syncs_and_restarts_converge_seed_1() {
    run_model(1);
}

#[test]
fn randomized_writes_syncs_and_restarts_converge_seed_2() {
    run_model(2);
}

#[test]
fn randomized_writes_syncs_and_restarts_converge_seed_3() {
    run_model(3);
}
