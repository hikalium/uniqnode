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

pub fn handle(context: &ApiContext, request: &Request) -> Response {
    let store = &*context.store;
    let path = request.path.as_str();
    let method = request.method.as_str();
    match (method, path) {
        ("GET", "/healthz") => Response::text(200, "ok\n"),
        ("POST", "/v1/query") => handle_query(context, request),
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

    if let Some(id) = path.strip_prefix("/v1/objects/") {
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

    error_response(404, "no such endpoint")
}
