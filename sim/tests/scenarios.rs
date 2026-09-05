//! SPEC.md §8(レプリカと健全性)のシナリオ検証。結果と限界は docs/analysis/20260815-replica-model-simulation.md (uuid:31e38823-b783-4dfe-bc7c-3cd268f5e7b4) にある。

use uniqnode_sim::*;

fn object(id: ObjectId, size: u64, min_replicas: u32) -> ObjectSpec {
    ObjectSpec { id, size, min_replicas, references: Vec::new() }
}

/// DBノード脱落 → T_prop 後に不足検知 → rendezvous 修復で収束し、ALERT は出ない。
#[test]
fn node_loss_is_repaired_without_alert() {
    let mut sim = Sim::new(Params::default(), &[10, 10, 10, 10, 10], 1);
    sim.add_object(object(100, 1, 2), &[0, 1], &[], true);
    sim.run(30);
    assert_eq!(sim.oracle_committed_count(100), 2);

    sim.kill(1);
    sim.run(200);

    assert_eq!(sim.oracle_committed_count(100), 2, "不足が修復されている");
    assert!(sim.alert_events().is_empty(), "容量が足りるので ALERT は出ない");
    let degraded = sim.count_events(|e| {
        matches!(e, Event::HealthTransition { state: HealthState::Degraded, .. })
    });
    assert!(degraded >= 1, "不足は DEGRADED として観測される");
    for id in [0u32, 2, 3, 4] {
        assert_eq!(sim.health_of(id, 100), HealthState::Satisfied);
    }
    let max_fetch = *sim.max_concurrent_fetch.get(&100).unwrap_or(&0);
    assert_eq!(max_fetch, 1, "不足1に対して取得者は1台(雪崩なし)");
}

/// 容量が構造的に足りない → T_heal を待たずに ALERT(capacity)。遷移でのみ発火する。
#[test]
fn capacity_shortage_alerts_immediately() {
    let params = Params::default();
    let t_heal = params.t_heal;
    let mut sim = Sim::new(params, &[10, 4, 4], 2);
    sim.add_object(object(7, 5, 2), &[0], &[], true);
    sim.run(60);

    for id in 0..3u32 {
        assert_eq!(sim.health_of(id, 7), HealthState::Alert);
    }
    let capacity_alerts = sim.count_events(|e| {
        matches!(
            e,
            Event::HealthTransition { reason: Some(AlertReason::Capacity), .. }
        )
    });
    assert_eq!(capacity_alerts, 3, "観測者ごとに1回だけ(遷移でのみ)発火する");
    for event in sim.alert_events() {
        if let Event::HealthTransition { tick, .. } = event {
            assert!(*tick < t_heal, "capacity は T_heal を待たない: tick={tick}");
        }
    }
    let unknown_alerts = sim.count_events(|e| {
        matches!(
            e,
            Event::HealthTransition { reason: Some(AlertReason::Unknown), .. }
        )
    });
    assert_eq!(unknown_alerts, 0);
}

/// 唯一の保持者が消え、取得元がどこにもない → 修復不能 → T_heal 経過で ALERT(unknown)。
/// 保持者の復帰で SATISFIED に戻る(閉包: 遷移イベントとして観測できる)。
#[test]
fn unrepairable_loss_alerts_unknown_after_t_heal() {
    let params = Params::default();
    let t_heal = params.t_heal;
    let mut sim = Sim::new(params, &[10, 10, 10], 3);
    sim.add_object(object(9, 1, 1), &[0], &[], true);
    sim.run(30);

    sim.kill(0);
    sim.run(300);

    for id in [1u32, 2] {
        assert_eq!(sim.health_of(id, 9), HealthState::Alert);
    }
    let unknown_ticks: Vec<Tick> = sim
        .events
        .iter()
        .filter_map(|e| match e {
            Event::HealthTransition { tick, reason: Some(AlertReason::Unknown), .. } => Some(*tick),
            _ => None,
        })
        .collect();
    assert_eq!(unknown_ticks.len(), 2, "観測者(生存2台)ごとに1回");
    for tick in &unknown_ticks {
        assert!(*tick >= 30 + t_heal, "T_heal より早く unknown を出さない: tick={tick}");
    }

    sim.revive(0);
    sim.run(60);
    for id in 0..3u32 {
        assert_eq!(sim.health_of(id, 9), HealthState::Satisfied, "復帰で回復する");
    }
}

/// 分断中に両側が独立修復して過剰レプリカになり、合流後に降格規則で収束する。
/// 収束過程で実複製数が min_replicas を割らない(スプリットブレインの良性)。
#[test]
fn partition_heals_benignly_after_merge() {
    let mut sim = Sim::new(Params::default(), &[10; 6], 4);
    sim.add_object(object(42, 1, 2), &[0, 3], &[], true);
    sim.run(40);
    assert_eq!(sim.oracle_committed_count(42), 2);

    for node in 0..3u32 {
        sim.set_partition(node, 0);
    }
    for node in 3..6u32 {
        sim.set_partition(node, 1);
    }
    sim.run(160);
    assert_eq!(sim.oracle_committed_count(42), 4, "両側が独立に修復して過剰になる");

    sim.merge_partitions();
    sim.run(300);
    assert_eq!(sim.oracle_committed_count(42), 2, "過剰分は降格して収束する");
    assert_eq!(
        *sim.min_breach_ticks.get(&42).unwrap_or(&0),
        0,
        "収束過程で min_replicas を割らない"
    );
    let demoted = sim.count_events(|e| matches!(e, Event::Demoted { .. }));
    assert_eq!(demoted, 2);
    assert!(sim.alert_events().is_empty());
    for id in 0..6u32 {
        assert_eq!(sim.health_of(id, 42), HealthState::Satisfied);
    }
}

/// min_replicas=0 のオブジェクトは、作成者の複製を含め全所から evict され消滅し得る。
/// これはエラーではなく、警報も出ない(SPEC §8.1 の意味論)。
#[test]
fn replica_zero_may_vanish_from_the_world() {
    let mut sim = Sim::new(Params::default(), &[3, 3, 3], 5);
    sim.add_object(object(500, 1, 0), &[], &[0, 1], true);
    sim.run(5);
    assert!(sim.oracle_any_copy(500));

    for filler in 0..4u64 {
        sim.add_object(object(600 + filler, 1, 0), &[], &[0], false);
        sim.add_object(object(700 + filler, 1, 0), &[], &[1], false);
    }
    sim.run(5);

    assert!(!sim.oracle_any_copy(500), "世界から消滅し得る");
    let transitions = sim.count_events(|e| {
        matches!(e, Event::HealthTransition { object: 500, .. })
    });
    assert_eq!(transitions, 0, "min=0 は健全性評価の対象外");
    assert!(sim.alert_events().is_empty());
}

/// evict は確約層に触れず、ローカルの確約層から参照されている機会層オブジェクトを温存する。
#[test]
fn eviction_spares_committed_and_referenced() {
    let mut sim = Sim::new(Params::default(), &[5], 6);
    sim.add_object(
        ObjectSpec { id: 1, size: 1, min_replicas: 0, references: vec![2] },
        &[0],
        &[],
        false,
    );
    sim.add_object(object(2, 1, 0), &[], &[0], false);
    sim.run(1);

    for filler in 0..6u64 {
        sim.add_object(object(10 + filler, 1, 0), &[], &[0], false);
    }
    sim.run(2);

    assert!(sim.nodes[0].committed.contains(&1), "確約層は evict されない");
    assert!(sim.nodes[0].cache.contains(&2), "被参照オブジェクトは温存される");
    assert_eq!(sim.nodes[0].cache.len(), 4, "容量(5)に収まるまで filler が evict される");
    let evicted_referenced =
        sim.count_events(|e| matches!(e, Event::Evicted { object: 2, .. }));
    assert_eq!(evicted_referenced, 0);
}

/// 反復故障(保持者を落として復帰させ続ける)でも、故障が存在しない瞬間に複製数が
/// min を割ることはない。復帰による過剰レプリカの降格が自傷にならないことの検証。
#[test]
fn repeated_failure_never_self_inflicts_breach() {
    let mut sim = Sim::new(Params::default(), &[10; 8], 11);
    sim.add_object(object(9, 1, 3), &[0, 1, 2], &[], true);
    sim.run(40);
    let mut victim: NodeId = 0;
    for round in 0..8u64 {
        sim.kill(victim);
        sim.run(120);
        sim.revive(victim);
        sim.run(60);
        victim = ((round + 1) % 8) as NodeId;
    }
    sim.run(200);

    assert_eq!(sim.oracle_committed_count(9), 3, "修復と降格が追従して収束する");
    assert_eq!(
        *sim.min_breach_ticks_all_alive.get(&9).unwrap_or(&0),
        0,
        "全DBノード生存中の breach(降格競合などの自傷)は存在しない"
    );
    assert!(sim.alert_events().is_empty(), "T_heal 内に修復されるので ALERT なし");
}

/// 不足2に対して取得者が2台を超えない(rendezvous 自己選出による雪崩の不在)。
#[test]
fn repair_avoids_thundering_herd() {
    let mut sim = Sim::new(Params::default(), &[10; 10], 7);
    sim.add_object(object(77, 1, 3), &[0, 1, 2], &[], true);
    sim.run(40);

    sim.kill(1);
    sim.kill(2);
    sim.run(300);

    assert_eq!(sim.oracle_committed_count(77), 3);
    assert!(sim.alert_events().is_empty());
    let max_fetch = *sim.max_concurrent_fetch.get(&77).unwrap_or(&0);
    assert!(max_fetch <= 2, "取得者は不足数(2)を超えない: {max_fetch}");
}
