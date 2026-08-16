//! L2 分散クエリの統合テスト(SPEC §7.1, §7.2、§11 L2)。実プロセス3台。
//! 受け入れ基準: 1台だけが持つ知識が budget 内に返る / 持ち主が全員停止なら timed_out
//! (不存在とは報告されない) / 空応答と沈黙が区別されて報告される。
//! 空応答と沈黙の区別は、決着を待ってから読む
//! silence_of_the_only_holder_times_out_without_nonexistence_claims が担う。
//! object の回答が来たら即 found で決着する規則があるため、found で決着した報告では
//! 「持たないピアがまだ答えていない」ことがあり、その状態を決めつけてはならない。

mod common;
use common::*;

fn query_body(kind: &str, target: &str, budget_ms: u64, scope: &[&str]) -> String {
    let scope_json: Vec<String> = scope.iter().map(|a| format!("\"{a}\"")).collect();
    format!(
        "{{\"kind\":\"{kind}\",\"target\":\"{target}\",\"budget_ms\":{budget_ms},\"scope\":[{}]}}",
        scope_json.join(",")
    )
}

fn peer_state_of<'a>(report: &'a str, address: &str) -> &'a str {
    // peers 配列の {"address":"…","state":"…"} から該当項目の state を抜く。
    let entry_start = report.find(&format!("\"address\":\"{address}\"")).expect("peer entry");
    let rest = &report[entry_start..];
    let state_start = rest.find("\"state\":\"").expect("state field") + 9;
    let state_rest = &rest[state_start..];
    &state_rest[..state_rest.find('"').expect("close quote")]
}

/// 受け入れ基準1: 1台だけが持つ知識が budget 内に返り、取得したオブジェクトは
/// ローカルに残る。持たないピアの状態は決着の時点によって empty(自分は持たないという
/// 肯定的言明)にも pending(まだ何も言っていない)にもなり、どちらも正しいので
/// 決めつけない。
#[test]
fn knowledge_held_by_one_peer_is_found_within_budget() {
    let a = start_server("q-origin");
    let b = start_server("q-empty");
    let c = start_server("q-holder");
    let id = put_object(&c.address, b"\"only c has this\"");

    let response = simple(
        &a.address,
        "POST",
        "/v1/query",
        query_body("object", &id, 5_000, &[&b.address, &c.address]).as_bytes(),
    );
    assert_eq!(response.status, 200);
    let report = body_text(&response);
    assert!(report.contains("\"outcome\":\"found\""), "{report}");
    assert_eq!(peer_state_of(&report, &c.address), "answered", "{report}");
    // 持たないピアは、決着までに答えていれば empty、間に合わなければ pending。
    // どちらも正しく、どちらであるかは決着の速さで決まるので、集合で受ける。
    let holder_less_state = peer_state_of(&report, &b.address);
    assert!(
        holder_less_state == "empty" || holder_less_state == "pending",
        "持たないピアの状態は empty か pending であるべきだが {holder_less_state} だった: {report}"
    );

    // 取得済みオブジェクトが a から読める(機会的な複製)。
    let fetched = simple(&a.address, "GET", &format!("/v1/objects/{id}"), b"");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body, b"\"only c has this\"");
}

/// 受け入れ基準2: 持ち主が全員停止なら timed_out。「存在しない」という語彙は現れず、
/// 停止したピアは silent、生きていて持たないピアは empty と区別される。
#[test]
fn silence_of_the_only_holder_times_out_without_nonexistence_claims() {
    let a = start_server("q2-origin");
    let b = start_server("q2-empty");
    let holder = start_server("q2-holder");
    let id = put_object(&holder.address, b"\"will go dark\"");
    let holder_address = holder.address.clone();
    drop(holder); // 持ち主を停止する

    let response = simple(
        &a.address,
        "POST",
        "/v1/query",
        query_body("object", &id, 900, &[&b.address, &holder_address]).as_bytes(),
    );
    assert_eq!(response.status, 200);
    let report = body_text(&response);
    assert!(report.contains("\"outcome\":\"timed_out\""), "{report}");
    assert_eq!(peer_state_of(&report, &b.address), "empty", "{report}");
    assert_eq!(peer_state_of(&report, &holder_address), "silent", "{report}");
    assert!(!report.contains("not_found"), "不存在の語彙を使わない: {report}");
    assert!(!report.contains("exist"), "不存在の語彙を使わない: {report}");
}

/// スコープ全員が肯定的に「持っていない」と言明したら、予算を待たず scope_empty で決着する。
#[test]
fn all_empty_scope_settles_before_the_budget() {
    let a = start_server("q3-origin");
    let b = start_server("q3-empty");
    let missing = format!("s256:{}", "7".repeat(64));

    let started = std::time::Instant::now();
    let response = simple(
        &a.address,
        "POST",
        "/v1/query",
        query_body("object", &missing, 30_000, &[&b.address]).as_bytes(),
    );
    let elapsed = started.elapsed();
    let report = body_text(&response);
    assert!(report.contains("\"outcome\":\"scope_empty\""), "{report}");
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "全員の言明が揃えば予算(30s)を待たない: {elapsed:?}"
    );
}

/// ref クエリ: 所有者の ref が見つかり、signer と seq が報告される。
#[test]
fn ref_query_reports_the_owner_view() {
    let a = start_server("q4-origin");
    let b = start_server("q4-owner");
    let c = start_server("q4-empty");
    let id = put_object(&b.address, b"\"ref target\"");
    put_ref(&b.address, "known/thing", Some(&id));
    let refs = body_text(&simple(&b.address, "GET", "/v1/refs", b""));
    let full_name = json_text_field(&refs, "name").expect("full name");

    let response = simple(
        &a.address,
        "POST",
        "/v1/query",
        query_body("ref", &full_name, 5_000, &[&b.address, &c.address]).as_bytes(),
    );
    let report = body_text(&response);
    assert!(report.contains("\"outcome\":\"found\""), "{report}");
    assert!(report.contains(&id), "{report}");
    assert_eq!(peer_state_of(&report, &c.address), "empty", "{report}");
}

/// ハンドル: wait:false で即返り、GET /v1/queries/{id} の観測が決着まで単調に進む。
#[test]
fn query_handle_progresses_monotonically() {
    let a = start_server("q5-origin");
    let holder = start_server("q5-holder");
    let id = put_object(&holder.address, b"\"handle target\"");

    let started = simple(
        &a.address,
        "POST",
        "/v1/query",
        format!(
            "{{\"kind\":\"object\",\"target\":\"{id}\",\"budget_ms\":5000,\"scope\":[\"{}\"],\"wait\":false}}",
            holder.address
        )
        .as_bytes(),
    );
    let first = body_text(&started);
    let query_id = json_text_field(&first, "query_id").expect("query_id");

    // 決着まで観測(条件ポーリング、期限付き)。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_answer_count = 0usize;
    loop {
        let snapshot =
            body_text(&simple(&a.address, "GET", &format!("/v1/queries/{query_id}"), b""));
        let answers = snapshot.matches("\"source\"").count();
        assert!(answers >= last_answer_count, "回答集合は単調増加: {snapshot}");
        last_answer_count = answers;
        if snapshot.contains("\"outcome\":\"found\"") {
            break;
        }
        assert!(
            !snapshot.contains("timed_out") && !snapshot.contains("scope_empty"),
            "この構成では found 以外で決着しない: {snapshot}"
        );
        assert!(std::time::Instant::now() < deadline, "決着しない");
        std::thread::yield_now();
    }

    // 未知のハンドルはローカルな 404。
    let unknown = simple(&a.address, "GET", "/v1/queries/deadbeef", b"");
    assert_eq!(unknown.status, 404);
}

/// peers.json が既定スコープになる(scope 省略時)。ファイル編集に再起動は要らない。
#[test]
fn peers_file_provides_the_default_scope() {
    let a = start_server("q6-origin");
    let holder = start_server("q6-holder");
    let id = put_object(&holder.address, b"\"via default scope\"");
    std::fs::write(
        a.dir.join("peers.json"),
        format!("{{\"peers\":[{{\"address\":\"{}\"}}]}}", holder.address),
    )
    .expect("write peers.json");

    let listed = body_text(&simple(&a.address, "GET", "/v1/peers", b""));
    assert!(listed.contains(&holder.address), "{listed}");

    let response = simple(
        &a.address,
        "POST",
        "/v1/query",
        format!("{{\"kind\":\"object\",\"target\":\"{id}\",\"budget_ms\":5000}}").as_bytes(),
    );
    let report = body_text(&response);
    assert!(report.contains("\"outcome\":\"found\""), "{report}");
}
