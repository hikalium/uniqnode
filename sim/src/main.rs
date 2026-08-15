//! シナリオ掃引。分断合流と反復故障を多数の種で回し、SPEC.md §8 の規則が
//! 観測ゆらぎ(gossip 遅延・判断タイミングの競合)の下でも安全か測定する。
//! 結果の分析は docs/analysis/ に記録する。

use uniqnode_sim::*;

struct SweepOutcome {
    seed: u64,
    final_count: usize,
    breach_ticks: u64,
    breach_all_alive: u64,
    episode_count: usize,
    episode_max: u64,
    demotions: usize,
    max_fetch: usize,
    alerts: usize,
}

fn breach_stats(sim: &Sim, object: ObjectId) -> (u64, usize, u64) {
    let all_alive = *sim.min_breach_ticks_all_alive.get(&object).unwrap_or(&0);
    let episodes = sim.breach_episodes.get(&object);
    let count = episodes.map(|e| e.len()).unwrap_or(0);
    let max_len = episodes
        .and_then(|e| e.iter().map(|ep| ep.end - ep.start + 1).max())
        .unwrap_or(0);
    (all_alive, count, max_len)
}

fn partition_sweep(seed: u64) -> SweepOutcome {
    let mut params = Params::default();
    // 種ごとに遅延条件を振って、観測ゆらぎの幅を広げる。
    params.gossip_delay_max = 1 + seed % 6;
    params.demotion_grace = 5 + (seed / 7) % 30;
    let mut sim = Sim::new(params, &[10; 6], seed);
    sim.add_object(
        ObjectSpec { id: 42, size: 1, min_replicas: 2, references: Vec::new() },
        &[0, 3],
        &[],
        true,
    );
    sim.run(40);
    for node in 0..3u32 {
        sim.set_partition(node, 0);
    }
    for node in 3..6u32 {
        sim.set_partition(node, 1);
    }
    sim.run(160);
    sim.merge_partitions();
    sim.run(400);
    let (breach_all_alive, episode_count, episode_max) = breach_stats(&sim, 42);
    SweepOutcome {
        seed,
        final_count: sim.oracle_committed_count(42),
        breach_ticks: *sim.min_breach_ticks.get(&42).unwrap_or(&0),
        breach_all_alive,
        episode_count,
        episode_max,
        demotions: sim.count_events(|e| matches!(e, Event::Demoted { .. })),
        max_fetch: *sim.max_concurrent_fetch.get(&42).unwrap_or(&0),
        alerts: sim.alert_events().len(),
    }
}

/// 反復故障: 保持者を周期的に落として復帰させ続ける。修復が追従し続けるかを見る。
fn repeated_failure_sweep(seed: u64) -> SweepOutcome {
    let mut params = Params::default();
    params.gossip_delay_max = 1 + seed % 6;
    let mut sim = Sim::new(params, &[10; 8], seed ^ 0xDEAD);
    sim.add_object(
        ObjectSpec { id: 9, size: 1, min_replicas: 3, references: Vec::new() },
        &[0, 1, 2],
        &[],
        true,
    );
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
    let (breach_all_alive, episode_count, episode_max) = breach_stats(&sim, 9);
    SweepOutcome {
        seed,
        final_count: sim.oracle_committed_count(9),
        breach_ticks: *sim.min_breach_ticks.get(&9).unwrap_or(&0),
        breach_all_alive,
        episode_count,
        episode_max,
        demotions: sim.count_events(|e| matches!(e, Event::Demoted { .. })),
        max_fetch: *sim.max_concurrent_fetch.get(&9).unwrap_or(&0),
        alerts: sim.alert_events().len(),
    }
}

fn summarize(name: &str, outcomes: &[SweepOutcome], expected_final: usize) {
    let runs = outcomes.len();
    let converged = outcomes.iter().filter(|o| o.final_count == expected_final).count();
    let breached = outcomes.iter().filter(|o| o.breach_ticks > 0).count();
    let self_inflicted = outcomes.iter().filter(|o| o.breach_all_alive > 0).count();
    let max_breach = outcomes.iter().map(|o| o.breach_ticks).max().unwrap_or(0);
    let max_episode = outcomes.iter().map(|o| o.episode_max).max().unwrap_or(0);
    let total_episodes: usize = outcomes.iter().map(|o| o.episode_count).sum();
    let total_breach: u64 = outcomes.iter().map(|o| o.breach_ticks).sum();
    let mean_episode = if total_episodes > 0 { total_breach / total_episodes as u64 } else { 0 };
    let max_fetch = outcomes.iter().map(|o| o.max_fetch).max().unwrap_or(0);
    let max_demotions = outcomes.iter().map(|o| o.demotions).max().unwrap_or(0);
    let alerts: usize = outcomes.iter().map(|o| o.alerts).sum();
    println!("== {name} ({runs} seeds)");
    println!("  converged to {expected_final}: {converged}/{runs}");
    println!("  seeds with min-breach: {breached} (max breach ticks: {max_breach})");
    println!("  seeds with SELF-INFLICTED breach (all nodes alive): {self_inflicted}");
    println!(
        "  breach episodes: total={total_episodes} mean_len={mean_episode} max_len={max_episode}"
    );
    println!("  max concurrent fetch: {max_fetch}");
    println!("  max demotions in a run: {max_demotions}");
    println!("  total alert transitions: {alerts}");
    for o in outcomes {
        if o.final_count != expected_final {
            println!(
                "    NOT CONVERGED: seed={} final={} demotions={}",
                o.seed, o.final_count, o.demotions
            );
        }
    }
}

fn main() {
    let seeds: Vec<u64> = (0..200).collect();
    let partition: Vec<SweepOutcome> = seeds.iter().map(|s| partition_sweep(*s)).collect();
    summarize("partition-merge (min=2, 6 nodes)", &partition, 2);
    let repeated: Vec<SweepOutcome> = seeds.iter().map(|s| repeated_failure_sweep(*s)).collect();
    summarize("repeated-failure (min=3, 8 nodes)", &repeated, 3);
}
