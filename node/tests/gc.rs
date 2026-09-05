//! `uniqnode gc` の統合テスト。本番の入口(CLI と serve の `POST /v1/admin/gc`)を実プロセス
//! で走らせ、出力と終了コードと、指したストアに何が残ったかを観測する(should/0137)。
//! dry-run は何も書かないこと、回収は孤児だけを消して生きているものを全部残すこと、A と C の
//! 間に孤児を指す ref が入っても消えないこと(生き返り)。定義と手順と出力の読み方は
//! GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)。クラッシュの各点は node/tests/gc_crash.rs。

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use uniqnode::store::{Store, StoreConfig};

/// packs/ のファイルの大きさの合計(ディスク上の量)。
fn packs_disk_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir.join("packs"))
        .expect("read_dir")
        .map(|e| e.expect("entry").metadata().expect("metadata").len())
        .sum()
}

fn pack_files(dir: &Path) -> Vec<String> {
    entries_of(&dir.join("packs"))
}

/// 生きているものを全部読めて fsck が緑であることを、開き直したストアで言う。
fn assert_all_readable_and_green(dir: &Path, live: &[(String, Vec<u8>)], context: &str) {
    let store = Store::open(small_config(dir)).expect("open");
    for (id, body) in live {
        assert_eq!(
            store.get_object(id).expect("get").as_deref(),
            Some(body.as_slice()),
            "{context}: 生きている {id} が読めない"
        );
    }
    let report = store.fsck().expect("fsck");
    assert!(report.errors.is_empty(), "{context}: fsck: {:?}", report.errors);
}

fn temp_dir(name: &str) -> PathBuf {
    common::unique_dir(&format!("gc-{name}"))
}

/// 小さい封印閾値で開く(数件の投入で複数の pack に分かれる)。
fn small_config(dir: &Path) -> StoreConfig {
    let mut config = StoreConfig::new(dir);
    config.pack_seal_bytes = 64;
    config
}

struct CommandOutcome {
    status: i32,
    stdout: String,
    stderr: String,
}

fn uniqnode(arguments: &[&str]) -> CommandOutcome {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .output()
        .expect("spawn uniqnode");
    CommandOutcome {
        status: output.status.code().expect("exit code"),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

fn gc(dir: &Path, arguments: &[&str]) -> CommandOutcome {
    let mut all = vec!["gc", dir.to_str().expect("utf-8")];
    all.extend_from_slice(arguments);
    uniqnode(&all)
}

/// ストアのデータ(packs/ reflog/ MANIFEST)の、名前 → (中身, mtime)。gc の前後で
/// 突き合わせ、1 バイトも書かれていないことを言う。
fn data_snapshot(dir: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    let mut snapshot = BTreeMap::new();
    let mut record = |relative: PathBuf| {
        let path = dir.join(&relative);
        let bytes = std::fs::read(&path).expect("read");
        let modified = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        snapshot.insert(relative.display().to_string(), (bytes, modified));
    };
    for sub in ["packs", "reflog"] {
        for entry in std::fs::read_dir(dir.join(sub)).expect("read_dir") {
            let name = entry.expect("entry").file_name();
            record(PathBuf::from(sub).join(name));
        }
    }
    record(PathBuf::from("MANIFEST"));
    snapshot
}

fn entries_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// 複数の pack に分かれる量を投入して ref を張り、その半分を上書きで孤児にする。返り値は
/// (投入した数, 孤児にした数)。
fn fill_with_garbage(store: &mut Store) -> (usize, usize) {
    let filled = fill_with_garbage_ids(store);
    (filled.live.len() + filled.orphans.len(), filled.orphans.len())
}

struct Filled {
    /// 生きているオブジェクト(ID, 本体)。
    live: Vec<(String, Vec<u8>)>,
    /// 孤児の ID。
    orphans: Vec<String>,
    /// 孤児の本体(orphans と同じ順)。put し直す(べき等)テスト用。
    orphan_bodies: Vec<Vec<u8>>,
    /// 孤児のペイロードの合計。
    orphan_bytes: u64,
}

/// fill_with_garbage の、ID まで返す形。偶数番の旧 target が孤児、奇数番と偶数番の新 target が
/// 生きている。
fn fill_with_garbage_ids(store: &mut Store) -> Filled {
    let count = 20u32;
    let mut first: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..count {
        let body = format!("{{\"i\":{i:02},\"pad\":\"0123456789abcdef\"}}");
        let (id, _) = store.put_object(body.as_bytes()).expect("put");
        store
            .set_ref(&format!("notes/{i}"), Some(&id))
            .expect("set_ref");
        first.push((id, body.into_bytes()));
    }
    let mut live = Vec::new();
    let mut orphans = Vec::new();
    let mut orphan_bodies = Vec::new();
    let mut orphan_bytes = 0u64;
    // 偶数番を再取り込み(上書き)する: 旧 target が孤児になる。
    for (i, (id, body)) in first.into_iter().enumerate() {
        if i % 2 == 0 {
            let rewritten = format!("{{\"i\":{i:02},\"pad\":\"rewritten-0123456\"}}");
            let (new_id, _) = store.put_object(rewritten.as_bytes()).expect("put");
            store
                .set_ref(&format!("notes/{i}"), Some(&new_id))
                .expect("set_ref");
            orphans.push(id);
            orphan_bytes += body.len() as u64;
            orphan_bodies.push(body);
            live.push((new_id, rewritten.into_bytes()));
        } else {
            live.push((id, body));
        }
    }
    Filled { live, orphans, orphan_bodies, orphan_bytes }
}

/// (1) dry-run は pack ごとの行と集計 2 行を出して 0 で終わり、packs/・reflog/・MANIFEST
/// の中身と mtime を 1 つも変えず、tmp/ にも何も残さない。
#[test]
fn a_dry_run_reports_every_pack_and_a_summary_and_writes_nothing() {
    let dir = temp_dir("dry-run");
    let (objects, orphaned) = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    assert!(dir.join("MANIFEST").exists(), "封印が起きている前提");
    let before = data_snapshot(&dir);
    let pack_count = before
        .keys()
        .filter(|name| name.starts_with("packs/"))
        .count();
    assert!(
        pack_count >= 3,
        "複数の pack に分かれている前提: {pack_count}"
    );

    let outcome = gc(&dir, &["--dry-run"]);
    assert_eq!(
        outcome.status, 0,
        "dry-run は 0 で終わるべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    let pack_lines: Vec<&str> = outcome
        .stdout
        .lines()
        .filter(|line| line.starts_with("pack "))
        .collect();
    assert_eq!(
        pack_lines.len(),
        pack_count,
        "pack ごとに 1 行(pack の数 {pack_count}):\n{}",
        outcome.stdout
    );
    assert!(
        outcome
            .stdout
            .contains("pack 000001 sealed: objects 2 bytes "),
        "最初の pack は封印済みで 2 件: {}",
        outcome.stdout
    );
    assert!(
        pack_lines.last().expect("行がある").contains(" active: "),
        "最後の pack は追記中: {}",
        outcome.stdout
    );
    assert!(
        pack_lines.iter().any(|line| line.ends_with("-> compact")),
        "上書きで孤児になった pack が対象になる: {}",
        outcome.stdout
    );
    assert!(
        pack_lines.iter().any(|line| line.ends_with("-> keep")),
        "孤児の無い pack は残す: {}",
        outcome.stdout
    );
    let summary = format!(
        "objects {objects} live {} garbage {orphaned},",
        objects - orphaned
    );
    assert!(
        outcome.stdout.contains(&summary),
        "集計行が数を言う({summary}): {}",
        outcome.stdout
    );
    assert!(
        outcome
            .stdout
            .lines()
            .any(|line| line.starts_with("gc: live set computed in ") && line.contains(" ms ")),
        "集計の 2 行目が生きている集合の計算時間を言う: {}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("threshold 0.25"),
        "既定の閾値を言う: {}",
        outcome.stdout
    );

    let after = data_snapshot(&dir);
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "ファイルが増えても減ってもいない"
    );
    for (name, (bytes, modified)) in &before {
        let (bytes_after, modified_after) = &after[name];
        assert_eq!(bytes_after, bytes, "{name} の中身が変わっている");
        assert_eq!(modified_after, modified, "{name} の mtime が動いている");
    }
    assert!(
        entries_of(&dir.join("tmp")).is_empty(),
        "tmp/ に残したもの: {:?}",
        entries_of(&dir.join("tmp"))
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (2) 閾値を変えると対象が変わる。0 なら孤児を含む封印済み pack が全部対象、1 なら 1 本も
/// 対象にならない(どちらも追記中の pack は対象外)。
#[test]
fn the_threshold_option_changes_which_packs_are_targets() {
    let dir = temp_dir("threshold");
    {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store);
    }
    let everything = gc(&dir, &["--dry-run", "--threshold", "0"]);
    assert_eq!(
        everything.status, 0,
        "{}\n{}",
        everything.stdout, everything.stderr
    );
    let compact_at_zero = everything
        .stdout
        .lines()
        .filter(|line| line.ends_with("-> compact"))
        .count();
    let sealed_with_garbage = everything
        .stdout
        .lines()
        .filter(|line| line.contains(" sealed: ") && !line.contains(", garbage 0 ("))
        .count();
    assert_eq!(
        compact_at_zero, sealed_with_garbage,
        "閾値 0 では孤児を持つ封印済み pack が全部対象: {}",
        everything.stdout
    );
    assert!(compact_at_zero >= 1, "{}", everything.stdout);
    assert!(
        everything.stdout.contains("threshold 0,"),
        "指定した閾値を言う: {}",
        everything.stdout
    );

    let nothing = gc(&dir, &["--dry-run", "--threshold", "1"]);
    assert_eq!(nothing.status, 0, "{}\n{}", nothing.stdout, nothing.stderr);
    assert!(
        !nothing.stdout.contains("-> compact"),
        "閾値 1 は超えられない: {}",
        nothing.stdout
    );
    assert!(nothing.stdout.contains("compact 0)"), "{}", nothing.stdout);

    let out_of_range = gc(&dir, &["--dry-run", "--threshold", "1.5"]);
    assert_eq!(out_of_range.status, 2, "範囲外の閾値は usage で落ちる");
    assert!(
        out_of_range.stderr.contains("0 以上 1 以下"),
        "{}",
        out_of_range.stderr
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (3) 回収: 孤児を含む封印済み pack が書き直され、生きているものは全件読めて fsck が緑、
/// 孤児は消え、used_bytes と packs/ のディスク上の合計が減り、旧 pack が消えて新 pack が
/// MANIFEST に載る。開き直しても同じ。出力は書き直した pack・新 pack・戻ったバイト数・各相の
/// 所要を言う。2 回目は書き直すものが無く、参照表を流用する(derived/refs/ の mtime が動かない)。
#[test]
fn gc_rewrites_packs_with_garbage_and_keeps_every_live_object() {
    let dir = temp_dir("compact");
    let (filled, used_before) = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        let filled = fill_with_garbage_ids(&mut store);
        (filled, store.used_bytes())
    };
    let sealed_before: Vec<u64> = {
        let store = Store::open(small_config(&dir)).expect("open");
        store.sealed_pack_numbers().to_vec()
    };
    let disk_before = packs_disk_bytes(&dir);
    let files_before = pack_files(&dir);

    // 閾値 0: 孤児を 1 バイトでも持つ封印済み pack は全部対象。
    let outcome = gc(&dir, &["--threshold", "0"]);
    assert_eq!(
        outcome.status, 0,
        "回収は 0 で終わるべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    let compacted_line = outcome
        .stdout
        .lines()
        .find(|line| line.starts_with("gc: compacted packs "))
        .unwrap_or_else(|| panic!("書き直した pack を言う行が無い: {}", outcome.stdout));
    assert!(
        compacted_line.contains(" -> new pack ") && compacted_line.contains(" reclaimed "),
        "新 pack と戻ったバイト数を言う: {compacted_line}"
    );
    let phases_line = outcome
        .stdout
        .lines()
        .find(|line| line.starts_with("gc: phases S "))
        .unwrap_or_else(|| panic!("各相の所要を言う行が無い: {}", outcome.stdout));
    for phase in ["S ", " P ", " A ", " B ", " C ", " D ", "locked (S+A+C) "] {
        assert!(phases_line.contains(phase), "{phase} の所要が無い: {phases_line}");
    }
    assert!(!outcome.stdout.contains("dry-run"), "回収は dry-run ではない: {}", outcome.stdout);

    // 生きているものは全件読め、孤児は消えている。
    let store = Store::open(small_config(&dir)).expect("reopen");
    for (id, body) in &filled.live {
        assert_eq!(
            store.get_object(id).expect("get").as_deref(),
            Some(body.as_slice()),
            "生きている {id} が読めない"
        );
    }
    for id in &filled.orphans {
        assert!(store.get_object(id).expect("get").is_none(), "孤児 {id} が残っている");
    }
    let report = store.fsck().expect("fsck");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(store.object_count(), filled.live.len());
    assert_eq!(
        store.used_bytes(),
        used_before - filled.orphan_bytes,
        "used_bytes が孤児の分だけ減る"
    );
    assert!(
        packs_disk_bytes(&dir) < disk_before,
        "packs/ のディスク上の合計が減る: {} -> {}",
        disk_before,
        packs_disk_bytes(&dir)
    );
    let sealed_after = store.sealed_pack_numbers().to_vec();
    let files_after = pack_files(&dir);
    let removed: Vec<&u64> = sealed_before.iter().filter(|n| !sealed_after.contains(n)).collect();
    assert!(!removed.is_empty(), "対象だった pack が MANIFEST から外れる: {sealed_before:?} -> {sealed_after:?}");
    for number in &removed {
        let name = format!("pack-{number:06}.pack");
        assert!(files_before.contains(&name) && !files_after.contains(&name), "旧 {name} が消えている");
    }
    let new_pack = sealed_after.iter().max().expect("封印済みがある");
    assert!(!sealed_before.contains(new_pack), "新 pack {new_pack} は MANIFEST に足された");
    assert!(files_after.contains(&format!("pack-{new_pack:06}.pack")), "新 pack が packs/ に在る: {files_after:?}");
    assert!(entries_of(&dir.join("tmp")).is_empty(), "tmp/ に残したもの: {:?}", entries_of(&dir.join("tmp")));
    drop(store);

    // 2 回目: 書き直すものが無く、参照表は流用される。
    let tables = dir.join("derived").join("refs");
    let mtimes_before: BTreeMap<String, SystemTime> = std::fs::read_dir(&tables)
        .expect("derived/refs")
        .map(|e| {
            let e = e.expect("entry");
            (e.file_name().to_string_lossy().to_string(), e.metadata().expect("m").modified().expect("mtime"))
        })
        .collect();
    assert!(!mtimes_before.is_empty(), "参照表が置かれている");
    let second = gc(&dir, &["--threshold", "0"]);
    assert_eq!(second.status, 0, "{}\n{}", second.stdout, second.stderr);
    assert!(second.stdout.contains("gc: nothing to compact"), "{}", second.stdout);
    assert!(
        second.stdout.contains("tables built 0 reused"),
        "2 回目は参照表を作らず流用する: {}",
        second.stdout
    );
    for (name, before) in &mtimes_before {
        let after = std::fs::metadata(tables.join(name)).expect("table").modified().expect("mtime");
        assert_eq!(&after, before, "参照表 {name} が書き直されている");
    }
    assert_all_readable_and_green(&dir, &filled.live, "2 回目の後");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (3b) 知らない引数は usage で落ちる。
#[test]
fn an_unknown_gc_argument_falls_to_usage() {
    let dir = temp_dir("unknown-arg");
    {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store);
    }
    let before = data_snapshot(&dir);
    let unknown = gc(&dir, &["--force"]);
    assert_eq!(unknown.status, 2, "知らない引数は usage で落ちる");
    assert!(unknown.stderr.contains("usage:"), "{}", unknown.stderr);
    assert_eq!(data_snapshot(&dir), before, "何も書かれていない");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (6) serve の POST /v1/admin/gc。B の後で止めて(テスト用の口 UNIQNODE_GC_WAIT_AFTER_B)、
/// その間に GET が答えること(ロックの外の相)、2 つ目の gc が 409 で断られること、孤児を指す
/// 新しい ref を入れる(生き返り)と、そのオブジェクトが消えずに新 pack へ写し足されることを
/// 見る。孤児を put し直しただけ(ref はまだ。SPEC §5.3 の順序の途中)のものも消えず、後から
/// ref を張れる。再起動しても読める。
#[test]
fn admin_gc_revives_an_orphan_that_gets_a_ref_between_analysis_and_commit() {
    let dir = temp_dir("admin-revive");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage_ids(&mut store)
    };
    let revived = filled.orphans[0].clone();
    let re_put = filled.orphans[1].clone();
    let re_put_body = filled.orphan_bodies[1].clone();
    let go = dir.join("go");
    let ready = PathBuf::from(format!("{}{}", go.display(), uniqnode::gc::WAIT_READY_SUFFIX));
    let mut server = common::start_server_at_with_env(
        dir.clone(),
        &[(uniqnode::gc::WAIT_AFTER_COPY_ENV, go.to_str().expect("utf-8"))],
    );
    server.remove_dir_on_drop = false;
    let address = server.address.clone();
    let gc_thread = std::thread::spawn(move || {
        common::simple(&address, "POST", "/v1/admin/gc", b"{\"threshold\":\"0\"}")
    });
    // 条件待ち(should/0104): gc が B を終えて合図を待っている印。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() {
        assert!(std::time::Instant::now() < deadline, "gc が B に達しない");
        std::thread::yield_now();
    }
    // ロックの外なので他の要求が答える。
    let status = common::simple(&server.address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200, "gc の B の後で GET が答える");
    let second = common::simple(&server.address, "POST", "/v1/admin/gc", b"");
    assert_eq!(second.status, 409, "走っている間の 2 つ目は 409: {}", common::body_text(&second));
    // 孤児を指す新しい ref(生き返り)。target はまだ索引に在るので張れる。
    common::put_ref(&server.address, "notes/revived", Some(&revived));
    // 別の孤児を put し直す(べき等で「既に在る」)。ref を張る前に gc が進む。
    assert_eq!(common::put_object(&server.address, &re_put_body), re_put);
    // 新しいオブジェクトも書く(S 以後の追記。C-1 がアクティブを封印する)。
    let fresh_body = b"{\"kind\":\"node\",\"written\":\"between A and C\",\"v\":1}".to_vec();
    let fresh = common::put_object(&server.address, &fresh_body);
    common::put_ref(&server.address, "notes/fresh", Some(&fresh));
    std::fs::write(&go, b"").expect("go");
    let response = gc_thread.join().expect("gc thread");
    let body = common::body_text(&response);
    assert_eq!(response.status, 200, "{body}");
    assert_eq!(
        common::json_integer_field(&body, "revived_objects"),
        Some(2),
        "差分検査が 2 件(新しい根 1、put し直し 1)生き返らせたと言う: {body}"
    );
    assert!(
        common::json_integer_field(&body, "sealed_active").is_some(),
        "S 以後に追記があったので C-1 がアクティブを封印したと言う: {body}"
    );
    // put し直したものは消えておらず、遅れて来た ref が張れる。
    common::put_ref(&server.address, "notes/re-put", Some(&re_put));
    assert!(body.contains("\"dry_run\":false"), "{body}");
    assert!(body.contains("\"compacted\":["), "{body}");
    let fetched = common::simple(&server.address, "GET", &format!("/v1/objects/{revived}"), b"");
    assert_eq!(fetched.status, 200, "生き返ったオブジェクトが読める");
    for id in &filled.orphans[2..] {
        let gone = common::simple(&server.address, "GET", &format!("/v1/objects/{id}"), b"");
        assert_eq!(gone.status, 404, "他の孤児は消えている: {id}");
    }
    for (id, body) in &filled.live {
        let fetched = common::simple(&server.address, "GET", &format!("/v1/objects/{id}"), b"");
        assert_eq!(fetched.status, 200);
        assert_eq!(&fetched.body, body);
    }
    drop(server);
    // 再起動しても同じ。
    let store = Store::open(small_config(&dir)).expect("reopen");
    assert!(store.get_object(&revived).expect("get").is_some(), "再起動後も生き返ったものが読める");
    assert!(store.get_object(&re_put).expect("get").is_some(), "put し直したものも読める");
    assert_eq!(
        store.get_object(&fresh).expect("get"),
        Some(fresh_body),
        "A と C の間に書いたものも読める"
    );
    let report = store.fsck().expect("fsck");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(store.object_count(), filled.live.len() + 3);
    drop(store);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (7) admin の dry_run は数えるだけで何も書かず、閾値の指定を受け、不正なボディは 400。
#[test]
fn admin_gc_dry_run_counts_without_writing_and_rejects_bad_bodies() {
    let dir = temp_dir("admin-dry-run");
    {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store);
    }
    let before = data_snapshot(&dir);
    let mut server = common::start_server_at(dir.clone());
    server.remove_dir_on_drop = false;
    let response = common::simple(
        &server.address,
        "POST",
        "/v1/admin/gc",
        b"{\"dry_run\":true,\"threshold\":\"0\"}",
    );
    let body = common::body_text(&response);
    assert_eq!(response.status, 200, "{body}");
    assert!(body.contains("\"dry_run\":true"), "{body}");
    assert!(body.contains("\"threshold\":\"0\""), "{body}");
    assert!(body.contains("\"compacted\":[]"), "dry-run は何も書き直さない: {body}");
    assert!(body.contains("\"compact\":true"), "対象になる pack がある: {body}");
    assert!(body.contains("\"phases_ms\":{"), "{body}");
    let bad = common::simple(&server.address, "POST", "/v1/admin/gc", b"{\"threshold\":\"1.5\"}");
    assert_eq!(bad.status, 400, "{}", common::body_text(&bad));
    let bad = common::simple(&server.address, "POST", "/v1/admin/gc", b"{\"dry_run\":\"yes\"}");
    assert_eq!(bad.status, 400, "{}", common::body_text(&bad));
    drop(server);
    let after = data_snapshot(&dir);
    for (name, (bytes, _)) in &before {
        assert_eq!(&after[name].0, bytes, "{name} の中身が変わっている");
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (4) serve がロックを持って走っているストアには、開けないと言って 1 で終わる(backup と
/// 違い、gc はロックを取る)。serve はそのまま生きている。
#[test]
fn a_dry_run_against_a_store_held_by_serve_exits_1() {
    let server = common::start_server("gc-beside-serve");
    let id = common::put_object(&server.address, b"{\"kind\":\"node\",\"v\":1}");
    common::put_ref(&server.address, "notes/live", Some(&id));

    let outcome = gc(&server.dir, &["--dry-run"]);
    assert_eq!(
        outcome.status, 1,
        "ロックを持たれたストアは 1 で終わるべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome.stderr.contains("別プロセスが開いている"),
        "理由を言う: {}",
        outcome.stderr
    );
    assert!(
        !outcome.stdout.contains("pack "),
        "開けていないのに結果を出してはならない: {}",
        outcome.stdout
    );
    let status = common::simple(&server.address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200, "serve は生きている");
}

/// (5) 空のディレクトリと存在しない道には、fsck と同じく初期化せず 1 で断る。
#[test]
fn a_dry_run_on_a_directory_that_is_not_a_store_refuses_and_creates_nothing() {
    let dir = temp_dir("not-a-store");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let outcome = gc(&dir, &["--dry-run"]);
    assert_eq!(outcome.status, 1, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome
            .stderr
            .contains("ストアのデータディレクトリではない"),
        "{}",
        outcome.stderr
    );
    assert!(
        entries_of(&dir).is_empty(),
        "断ったのに作られたもの: {:?}",
        entries_of(&dir)
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
