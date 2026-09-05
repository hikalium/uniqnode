//! `uniqnode gc --dry-run` の統合テスト。本番の入口を子プロセスとして走らせ、出力と
//! 終了コードと、指したストアに何も書かれなかったことを観測する(should/0137)。定義と
//! 出力の読み方は GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)。

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use uniqnode::store::{Store, StoreConfig};

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
    let count = 20u32;
    for i in 0..count {
        let body = format!("{{\"i\":{i:02},\"pad\":\"0123456789abcdef\"}}");
        let (id, _) = store.put_object(body.as_bytes()).expect("put");
        store
            .set_ref(&format!("notes/{i}"), Some(&id))
            .expect("set_ref");
    }
    // 偶数番を再取り込み(上書き)する: 旧 target が孤児になる。
    let mut orphaned = 0;
    for i in (0..count).step_by(2) {
        let body = format!("{{\"i\":{i:02},\"pad\":\"rewritten-0123456\"}}");
        let (id, _) = store.put_object(body.as_bytes()).expect("put");
        store
            .set_ref(&format!("notes/{i}"), Some(&id))
            .expect("set_ref");
        orphaned += 1;
    }
    (count as usize + orphaned, orphaned)
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

/// (3) --dry-run 無しは、回収が未実装であることを言って 2 で終わり、黙って dry-run 扱いに
/// しない(pack の行を出さない)。ストアにも触れない。
#[test]
fn gc_without_dry_run_refuses_because_reclaiming_is_not_implemented() {
    let dir = temp_dir("no-dry-run");
    {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store);
    }
    let before = data_snapshot(&dir);
    let outcome = gc(&dir, &[]);
    assert_eq!(
        outcome.status, 2,
        "回収は未実装なので 2 で終わるべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome.stderr.contains("まだ実装していない"),
        "理由を言う: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("PACK_GC"),
        "設計文書を指す: {}",
        outcome.stderr
    );
    assert!(
        !outcome.stdout.contains("pack "),
        "黙って dry-run 扱いにしてはならない: {}",
        outcome.stdout
    );
    assert_eq!(data_snapshot(&dir), before, "何も書かれていない");

    let with_threshold_only = gc(&dir, &["--threshold", "0.5"]);
    assert_eq!(
        with_threshold_only.status, 2,
        "閾値だけ与えても回収にはならない: {}",
        with_threshold_only.stdout
    );
    let unknown = gc(&dir, &["--dry-run", "--force"]);
    assert_eq!(unknown.status, 2, "知らない引数は usage で落ちる");
    assert!(unknown.stderr.contains("usage:"), "{}", unknown.stderr);
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
