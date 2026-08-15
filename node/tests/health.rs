//! L3 レプリカ・健全性の統合テスト(SPEC §8、§11 L3)。実プロセス3台。
//! 受け入れ基準: min_replicas=2 の root の保持者を1台落とす → 自動修復で充足に戻る /
//! 容量の構造的不足 → ALERT(capacity)。時間は test 用に短縮した設定
//! (data_dir/node.json)を使い、待ちはすべて条件ポーリング。

mod common;
use common::*;
use std::path::Path;

/// テスト用の健全性設定(gossip 300ms / T_hb 1.5s / T_prop 1s / T_heal 60s / 猶予 60s)。
/// T_heal と猶予を長くして、テスト中に unknown ALERT や降格の雑音が出ないようにする。
fn write_node_config(dir: &Path, capacity_bytes: Option<u64>) {
    std::fs::create_dir_all(dir).expect("mkdir");
    let capacity = match capacity_bytes {
        Some(c) => format!("\"capacity_bytes\":{c},"),
        None => String::new(),
    };
    std::fs::write(
        dir.join("node.json"),
        format!(
            "{{{capacity}\"health\":{{\"gossip_period_ms\":300,\"t_hb_ms\":1500,\
             \"t_prop_ms\":1000,\"t_heal_ms\":60000,\"demotion_grace_ms\":60000}}}}"
        ),
    )
    .expect("write node.json");
}

fn write_peers(dir: &Path, addresses: &[&str]) {
    let peers: Vec<String> =
        addresses.iter().map(|a| format!("{{\"address\":\"{a}\"}}")).collect();
    std::fs::write(dir.join("peers.json"), format!("{{\"peers\":[{}]}}", peers.join(",")))
        .expect("write peers.json");
}

fn node_id_of(address: &str) -> String {
    let status = body_text(&simple(address, "GET", "/v1/status", b""));
    json_text_field(&status, "node_id").expect("node_id")
}

fn holders_of(address: &str) -> Vec<String> {
    let pins = body_text(&simple(address, "GET", "/v1/pins", b""));
    // "holders":["…","…"] を素朴に抜き出す。
    let rest = match pins.split("\"holders\":[").nth(1) {
        None => return Vec::new(),
        Some(r) => r,
    };
    let list = &rest[..rest.find(']').expect("close bracket")];
    list.split(',')
        .filter_map(|piece| piece.trim().trim_matches('"').to_string().into())
        .filter(|s: &String| !s.is_empty())
        .collect()
}

fn observed_of(address: &str, root: &str) -> Option<i64> {
    // c1 はキーをソートするので observed は root より前に並ぶ。テストの root は
    // 1つだけなので、status 全体から拾えばよい。
    let status = body_text(&simple(address, "GET", "/v1/status", b""));
    if !status.contains(&format!("\"root\":\"{root}\"")) {
        return None;
    }
    json_integer_field(&status, "observed")
}

/// 受け入れ基準1: 保持者を1台落とすと、生存割引 → T_prop → rendezvous 修復の経路で
/// 別の候補が閉包を取り寄せて保持表明し、充足に戻る。警報は遷移でのみ記録される。
#[test]
fn holder_loss_is_repaired_automatically() {
    let dir_a = unique_dir("h1-a");
    let dir_b = unique_dir("h1-b");
    let dir_c = unique_dir("h1-c");
    for dir in [&dir_a, &dir_b, &dir_c] {
        write_node_config(dir, None);
    }
    let a = start_server_at(dir_a);
    let b = start_server_at(dir_b);
    let c = start_server_at(dir_c);
    write_peers(&a.dir, &[&b.address, &c.address]);
    write_peers(&b.dir, &[&a.address, &c.address]);
    write_peers(&c.dir, &[&a.address, &b.address]);

    // A に葉と根(辺)を置いて pin する。A が最初の保持者になる。
    let leaf = put_object(&a.address, b"\"replicated leaf\"");
    let root_body = format!("{{\"v\":1,\"kind\":\"edge\",\"members\":[\"{leaf}\"]}}");
    let root = put_object(&a.address, root_body.as_bytes());
    let pinned = simple(
        &a.address,
        "POST",
        "/v1/pins",
        format!("{{\"root\":\"{root}\",\"min_replicas\":2}}").as_bytes(),
    );
    assert_eq!(pinned.status, 200, "{}", body_text(&pinned));

    // 第2の保持者が rendezvous で自己選出し、閉包を取り寄せて表明するまで待つ。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if holders_of(&a.address).len() >= 2 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "第2保持者が現れない");
        std::thread::yield_now();
    }

    // A 以外の保持者を特定して落とす。
    let a_id = node_id_of(&a.address);
    let b_id = node_id_of(&b.address);
    let holders = holders_of(&a.address);
    assert!(holders.contains(&a_id), "pin の発行者が最初の保持者: {holders:?}");
    let victim_is_b = holders.contains(&b_id);
    let (victim, survivor) = if victim_is_b { (b, c) } else { (c, b) };
    let victim_id = node_id_of(&victim.address);
    drop(victim); // 保持者の1台が停止する

    // 生存割引(T_hb)→ 不足 T_prop 経過 → 残る候補が修復、で充足に戻る。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let observed = observed_of(&a.address, &root).unwrap_or(0);
        let holders = holders_of(&a.address);
        let survivor_id = node_id_of(&survivor.address);
        if observed >= 2 && holders.contains(&survivor_id) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "修復が完了しない: observed={observed} holders={holders:?} victim={victim_id}"
        );
        std::thread::yield_now();
    }

    // 修復された複製は実体も持っている(表明だけではない)。
    let fetched = simple(&survivor.address, "GET", &format!("/v1/objects/{leaf}"), b"");
    assert_eq!(fetched.status, 200, "閉包の葉まで取り寄せられている");

    // 警報は遷移でのみ: 同じ状態の連続イベントがない。ALERT はこの筋書きでは出ない。
    let events = body_text(&simple(&a.address, "GET", "/v1/health/events", b""));
    assert!(!events.contains("\"state\":\"alert\""), "{events}");
    let states: Vec<&str> = events
        .split("\"state\":\"")
        .skip(1)
        .map(|rest| &rest[..rest.find('"').expect("close")])
        .collect();
    for pair in states.windows(2) {
        assert!(pair[0] != pair[1], "同じ状態が連続で記録されている: {states:?}");
    }
}

/// 受け入れ基準2: スコープ内に容量の見込みがある候補がいなければ、T_heal を待たずに
/// ALERT(capacity) が(遷移として一度だけ)記録される。
#[test]
fn capacity_shortage_raises_a_capacity_alert() {
    let dir_a = unique_dir("h2-a");
    let dir_b = unique_dir("h2-b");
    let dir_c = unique_dir("h2-c");
    write_node_config(&dir_a, None);
    // B と C は容量 1 バイト: どの閉包も引き受けられない。
    write_node_config(&dir_b, Some(1));
    write_node_config(&dir_c, Some(1));
    let a = start_server_at(dir_a);
    let b = start_server_at(dir_b);
    let c = start_server_at(dir_c);
    write_peers(&a.dir, &[&b.address, &c.address]);
    write_peers(&b.dir, &[&a.address, &c.address]);
    write_peers(&c.dir, &[&a.address, &b.address]);

    let root = put_object(&a.address, b"\"too big for the scope\"");
    let pinned = simple(
        &a.address,
        "POST",
        "/v1/pins",
        format!("{{\"root\":\"{root}\",\"min_replicas\":2}}").as_bytes(),
    );
    assert_eq!(pinned.status, 200, "{}", body_text(&pinned));

    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(20);
    loop {
        let events = body_text(&simple(&a.address, "GET", "/v1/health/events", b""));
        if events.contains("\"reason\":\"capacity\"") {
            // T_heal(60s)よりはるかに早い(即時性)。
            assert!(started.elapsed() < std::time::Duration::from_secs(20));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "capacity ALERT が出ない: {events}");
        std::thread::yield_now();
    }

    // 遷移でのみ: 状態が変わらない限り2件目の alert は積まれない。
    // 数 gossip 周期分、イベント数が安定することを確認する。
    let count_alerts = |text: &str| text.matches("\"reason\":\"capacity\"").count();
    let first = count_alerts(&body_text(&simple(&a.address, "GET", "/v1/health/events", b"")));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let second = count_alerts(&body_text(&simple(&a.address, "GET", "/v1/health/events", b"")));
    assert_eq!(first, 1, "capacity ALERT は一度だけ");
    assert_eq!(second, 1, "時間が経っても再発火しない(遷移でのみ)");
}
