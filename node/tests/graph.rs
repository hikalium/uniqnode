//! グラフ層(docs/design/GRAPH.md)の試験。実プロセスの serve を起こし、生 HTTP/1.1 で
//! 当てる(should/0138)。各試験の冒頭に、何を消すとどの assert が落ちるかを記す
//! (should/0137)。

mod common;

use common::{body_text, json_text_field, simple, start_server, Server};

fn put_node(server: &Server, graph: &str, node: &str, attrs: &str) -> common::HttpResponse {
    simple(
        &server.address,
        "PUT",
        &format!("/v1/graphs/{graph}/nodes/{node}"),
        attrs.as_bytes(),
    )
}

fn get(server: &Server, path: &str) -> common::HttpResponse {
    simple(&server.address, "GET", path, b"")
}

/// 節点は作れて読める。恒等ノードは版が変わっても同じで、状態ノードは変わる。この行を
/// 消すと、可変な名前空間(ref)と不変な身元(恒等ノード)の分けが壊れても気づかない。
#[test]
fn a_node_keeps_its_identity_across_versions() {
    let server = start_server("graph-identity");
    let first = put_node(&server, "plan", "n_a", r#"{"state":"planned","title":"A"}"#);
    assert_eq!(first.status, 200, "{}", body_text(&first));
    let first = body_text(&first);
    let identity = json_text_field(&first, "identity").expect("identity");
    let first_state = json_text_field(&first, "state").expect("state");

    let second = body_text(&put_node(
        &server,
        "plan",
        "n_a",
        r#"{"state":"landed","title":"A"}"#,
    ));
    assert_eq!(
        json_text_field(&second, "identity").as_deref(),
        Some(identity.as_str()),
        "身元は版をまたいで同じ: {second}"
    );
    assert_ne!(
        json_text_field(&second, "state"),
        Some(first_state.clone()),
        "状態ノードは版ごとに変わる: {second}"
    );

    let read = body_text(&get(&server, "/v1/graphs/plan/nodes/n_a"));
    assert!(read.contains("\"state\":\"landed\""), "{read}");
    assert!(read.contains(&identity), "現在も同じ身元を名乗る: {read}");
}

/// 同じ attrs の再 PUT は何も書かない。この行を消すと、棚卸しが 1 巡打ち直すたびに
/// 版と seq が増える(冪等が壊れる)。
#[test]
fn writing_the_same_attributes_again_changes_nothing() {
    let server = start_server("graph-idempotent");
    let first = body_text(&put_node(&server, "plan", "n_a", r#"{"state":"planned"}"#));
    assert!(first.contains("\"updated\":true"), "{first}");
    let again = body_text(&put_node(&server, "plan", "n_a", r#"{"state":"planned"}"#));
    assert!(again.contains("\"updated\":false"), "{again}");
    assert!(again.contains("\"seq\":null"), "ref も触らない: {again}");
    assert!(again.contains("\"new_objects\":0"), "オブジェクトも増えない: {again}");
    assert_eq!(
        json_text_field(&first, "state"),
        json_text_field(&again, "state"),
        "同じ版のまま"
    );
}

/// 辺は両端が在るときだけ張れ、節点の更新で張り替えなくてよい。この行を消すと、名前だけの
/// 端を持つ辺が描画の入力に現れる。
#[test]
fn an_edge_needs_both_ends_and_survives_node_updates() {
    let server = start_server("graph-edge");
    put_node(&server, "plan", "n_a", r#"{"state":"planned"}"#);
    let dangling = simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");
    assert_eq!(dangling.status, 400, "{}", body_text(&dangling));
    assert!(
        body_text(&dangling).contains("節点 n_b がグラフ plan に無い"),
        "何が足りないかを言う: {}",
        body_text(&dangling)
    );

    put_node(&server, "plan", "n_b", r#"{"state":"planned"}"#);
    let edge = body_text(&simple(
        &server.address,
        "PUT",
        "/v1/graphs/plan/edges/blocks/n_a/n_b",
        b"",
    ));
    let edge_id = json_text_field(&edge, "id").expect("id");
    assert!(edge.contains("\"updated\":true"), "{edge}");
    let again = body_text(&simple(
        &server.address,
        "PUT",
        "/v1/graphs/plan/edges/blocks/n_a/n_b",
        b"",
    ));
    assert!(again.contains("\"updated\":false"), "同じ辺の再作成は no-op: {again}");

    // 端の状態を変えても、辺の ID は変わらない(members は恒等ノードを指している)。
    put_node(&server, "plan", "n_a", r#"{"state":"landed"}"#);
    let edges = body_text(&get(&server, "/v1/graphs/plan/edges"));
    assert!(edges.contains(&edge_id), "辺は張り替え不要: {edges}");
}

/// 「X は何を待つか」が引ける。この行を消すと、向き付きの隣接が壊れても気づかない。
#[test]
fn neighbors_answer_what_waits_for_what() {
    let server = start_server("graph-neighbors");
    for node in ["n_a", "n_b", "n_c"] {
        put_node(&server, "plan", node, r#"{"state":"planned"}"#);
    }
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/ruling/n_a/n_c", b"");

    let out = body_text(&get(&server, "/v1/graphs/plan/nodes/n_a/neighbors?direction=out"));
    assert!(out.contains("n_b") && out.contains("n_c"), "{out}");
    let incoming = body_text(&get(&server, "/v1/graphs/plan/nodes/n_b/neighbors?direction=in"));
    assert!(incoming.contains("n_a"), "{incoming}");
    let typed = body_text(&get(
        &server,
        "/v1/graphs/plan/nodes/n_a/neighbors?direction=out&type=ruling",
    ));
    assert!(typed.contains("n_c") && !typed.contains("n_b"), "型で絞れる: {typed}");
    let bad = get(&server, "/v1/graphs/plan/nodes/n_a/neighbors?direction=sideways");
    assert_eq!(bad.status, 400, "{}", body_text(&bad));
}

/// 履歴は残り、着地の時刻が読める。この行を消すと、着地済みへの遷移が前の版を捨てても
/// 気づかない。
#[test]
fn history_keeps_every_version_with_its_time() {
    let server = start_server("graph-history");
    put_node(&server, "plan", "n_a", r#"{"state":"planned"}"#);
    put_node(&server, "plan", "n_a", r#"{"state":"active"}"#);
    put_node(&server, "plan", "n_a", r#"{"state":"landed"}"#);
    let history = body_text(&get(&server, "/v1/graphs/plan/nodes/n_a/history"));
    // 現在から古い方へ 3 版。
    let landed = history.find("landed").expect("landed");
    let active = history.find("active").expect("active");
    let planned = history.find("planned").expect("planned");
    assert!(landed < active && active < planned, "新しい順: {history}");
    assert!(!history.contains("\"at\":null"), "各版に時刻が付く: {history}");
}

/// 消すのは tombstone で、辺が付いたままの節点は消せない。この行を消すと、辺が宙に浮く。
#[test]
fn deleting_refuses_to_leave_an_edge_hanging() {
    let server = start_server("graph-delete");
    put_node(&server, "plan", "n_a", r#"{"state":"planned"}"#);
    put_node(&server, "plan", "n_b", r#"{"state":"planned"}"#);
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");

    let refused = simple(&server.address, "DELETE", "/v1/graphs/plan/nodes/n_a", b"");
    assert_eq!(refused.status, 400, "{}", body_text(&refused));
    assert!(
        body_text(&refused).contains("blocks/n_a/n_b"),
        "どの辺かを言う: {}",
        body_text(&refused)
    );

    let dropped = simple(&server.address, "DELETE", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");
    assert_eq!(dropped.status, 200, "{}", body_text(&dropped));
    let gone = simple(&server.address, "DELETE", "/v1/graphs/plan/nodes/n_a", b"");
    assert_eq!(gone.status, 200, "{}", body_text(&gone));
    let missing = simple(&server.address, "DELETE", "/v1/graphs/plan/nodes/n_a", b"");
    assert_eq!(missing.status, 404, "2 度目は 404: {}", body_text(&missing));
    let snapshot = body_text(&get(&server, "/v1/graphs/plan"));
    assert!(!snapshot.contains("n_a"), "見えから消える: {snapshot}");
    assert!(snapshot.contains("n_b"), "残りは残る: {snapshot}");
}

/// 全件は 1 本で返り、節点は名前順・辺は (型, from, to) 順である(描画の入力の決定性。
/// should/0125)。この行を消すと、同じグラフから毎回違う HTML が出る。
#[test]
fn the_whole_graph_comes_back_in_one_request_in_a_fixed_order() {
    let server = start_server("graph-snapshot");
    for node in ["n_c", "n_a", "n_b"] {
        put_node(&server, "plan", node, r#"{"state":"planned"}"#);
    }
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_c/n_a", b"");
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");
    let snapshot = body_text(&get(&server, "/v1/graphs/plan"));
    let a = snapshot.find("\"name\":\"n_a\"").expect("n_a");
    let b = snapshot.find("\"name\":\"n_b\"").expect("n_b");
    let c = snapshot.find("\"name\":\"n_c\"").expect("n_c");
    assert!(a < b && b < c, "節点は名前順: {snapshot}");
    let first = snapshot.find("\"from\":\"n_a\"").expect("n_a の辺");
    let second = snapshot.find("\"from\":\"n_c\"").expect("n_c の辺");
    assert!(first < second, "辺は (型, from, to) 順: {snapshot}");
    assert_eq!(body_text(&get(&server, "/v1/graphs")), "{\"graphs\":[\"plan\"]}");
}

/// グラフ層の書き込みは検索の見えを動かさない。この行を消すと、1 巡の書き込みが本番規模の
/// 索引の作り直しを起こす道が戻る(2026-09-08 の実測で 1 回 10.5 秒)。
#[test]
fn graph_writes_do_not_disturb_the_search_index() {
    let server = start_server("graph-index");
    let before = body_text(&simple(
        &server.address,
        "POST",
        "/v1/search",
        br#"{"query":"anything","top_k":3}"#,
    ));
    for node in ["n_a", "n_b"] {
        put_node(&server, "plan", node, r#"{"state":"planned"}"#);
    }
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");
    let after = body_text(&simple(
        &server.address,
        "POST",
        "/v1/search",
        br#"{"query":"anything","top_k":3}"#,
    ));
    assert_eq!(before, after, "見えは動かない");
    assert!(after.contains("\"results\":[]"), "グラフは索引に入らない: {after}");
}

/// 形の違う要求は、何が通るのかを言って断る(must/0022)。この行を消すと、打ち間違いが
/// 黙って別のものを作る道が開く。
#[test]
fn malformed_requests_are_refused_with_what_would_pass() {
    let server = start_server("graph-refusals");
    let bad_name = put_node(&server, "plan", "n%20a", "{}");
    assert_eq!(bad_name.status, 400, "{}", body_text(&bad_name));
    assert!(body_text(&bad_name).contains("節点名"), "{}", body_text(&bad_name));

    let bad_body = put_node(&server, "plan", "n_a", "[1,2]");
    assert_eq!(bad_body.status, 400, "{}", body_text(&bad_body));
    assert!(body_text(&bad_body).contains("attrs"), "{}", body_text(&bad_body));

    put_node(&server, "plan", "n_a", "{}");
    let wrong_method = simple(&server.address, "POST", "/v1/graphs/plan/nodes/n_a", b"{}");
    assert_eq!(wrong_method.status, 405, "{}", body_text(&wrong_method));

    let unknown = get(&server, "/v1/graphs/plan/nodes/n_a/whatever");
    assert_eq!(unknown.status, 404, "{}", body_text(&unknown));
    assert!(body_text(&unknown).contains("history"), "何が通るかを言う: {}", body_text(&unknown));

    let short_edge = simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a", b"");
    assert_eq!(short_edge.status, 404, "{}", body_text(&short_edge));

    let missing = get(&server, "/v1/graphs/plan/nodes/n_zzz");
    assert_eq!(missing.status, 404, "{}", body_text(&missing));
}

/// query で渡す名前(`?type=`・`?from=`・`?to=`)も、道の名前と同じ形の検査を通る。
/// 通さないと「該当 0 件」を返してしまい、呼び手には「そんな辺は無い」と「その名は名前
/// ではない」の区別が付かない(must/0022)。形の正しい名で 0 件なのは 200 のままである。
/// should/0137: refuse_bad_query_name の呼び出しを消すと、400 を見る 4 つの assert が
/// (200 になって)落ちる。
#[test]
fn a_name_in_the_query_is_checked_like_a_name_in_the_path() {
    let server = start_server("graph-query-names");
    for node in ["n_a", "n_b"] {
        put_node(&server, "plan", node, r#"{"state":"planned"}"#);
    }
    simple(&server.address, "PUT", "/v1/graphs/plan/edges/blocks/n_a/n_b", b"");

    for (path, what) in [
        ("/v1/graphs/plan/nodes/n_a/neighbors?type=a/b", "辺の型"),
        ("/v1/graphs/plan/edges?type=a/b", "辺の型"),
        ("/v1/graphs/plan/edges?from=n%20a", "節点名"),
        ("/v1/graphs/plan/edges?to=n%20a", "節点名"),
    ] {
        let refused = get(&server, path);
        assert_eq!(refused.status, 400, "{path}: {}", body_text(&refused));
        let text = body_text(&refused);
        assert!(text.contains(what), "{path}: 何の名前かを言う: {text}");
        assert!(text.contains("ASCII の英数字"), "{path}: 何が通るかを言う: {text}");
    }

    // 形の正しい名で 1 本も当たらないのは誤りではない(型は自由に生えるもので、一覧は無い)。
    let empty = get(&server, "/v1/graphs/plan/edges?type=no_such_type");
    assert_eq!(empty.status, 200, "{}", body_text(&empty));
    assert!(body_text(&empty).contains("\"edges\":[]"), "{}", body_text(&empty));
    let empty = get(&server, "/v1/graphs/plan/nodes/n_a/neighbors?type=no_such_type");
    assert_eq!(empty.status, 200, "{}", body_text(&empty));
    assert!(body_text(&empty).contains("\"edges\":[]"), "{}", body_text(&empty));
}
