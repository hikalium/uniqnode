//! 回収(gc)のクラッシュ耐性の統合テスト。本番の入口 `uniqnode gc` を子プロセスとして走らせ、
//! テスト用の口 UNIQNODE_GC_CRASH_AFTER(cfg(debug_assertions) のビルドだけが読む)で各相の
//! 直後に abort させ、開き直した回復と fsck を観測する: 生きているオブジェクトは全件読め、孤児は
//! 消えているか(次回の回収の対象として)残っているかのどちらかで、途中の残骸(tmp/ の書きかけ、
//! MANIFEST に無い pack)は回復が片づける。各点の状態と回復は
//! GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5) の表。
//!
//! MANIFEST の無いストアには「残骸」の規則を使わないこと(セルフレビューの場合分け)もここで
//! 固定する。
//!
//! abort の口は debug ビルドにしか無いので、それに頼る試験と補助の関数は
//! #[cfg(debug_assertions)] で、`cargo test --release` では走らない。MANIFEST の無いストアの
//! 試験は口に頼らないので、release でも走る。

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use uniqnode::store::{Store, StoreConfig};

fn temp_dir(name: &str) -> PathBuf {
    common::unique_dir(&format!("gc-crash-{name}"))
}

fn small_config(dir: &Path) -> StoreConfig {
    let mut config = StoreConfig::new(dir);
    config.pack_seal_bytes = 64;
    config
}

struct Filled {
    live: Vec<(String, Vec<u8>)>,
    orphans: Vec<String>,
}

/// 複数の pack に分かれる量を投入して ref を張り、偶数番を上書きして旧 target を孤児にする。
/// 孤児は全部封印済み pack の中に入る(最後の 1 件だけが追記中の pack に残り、それは生きて
/// いる)。
fn fill_with_garbage(store: &mut Store) -> Filled {
    let count = 20u32;
    let mut first = Vec::new();
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
    for (i, (id, body)) in first.into_iter().enumerate() {
        if i % 2 == 0 {
            let rewritten = format!("{{\"i\":{i:02},\"pad\":\"rewritten-0123456\"}}");
            let (new_id, _) = store.put_object(rewritten.as_bytes()).expect("put");
            store
                .set_ref(&format!("notes/{i}"), Some(&new_id))
                .expect("set_ref");
            orphans.push(id);
            live.push((new_id, rewritten.into_bytes()));
        } else {
            live.push((id, body));
        }
    }
    Filled { live, orphans }
}

fn entries_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

fn pack_files(dir: &Path) -> Vec<String> {
    entries_of(&dir.join("packs"))
}

#[cfg(debug_assertions)]
fn manifest_packs(dir: &Path) -> Vec<u64> {
    let store = Store::open(small_config(dir)).expect("open");
    store.sealed_pack_numbers().to_vec()
}

/// 相の直後に abort する gc を走らせる。返り値は (終了コード, 標準エラー)。abort なので終了
/// コードは無い(シグナル)はず。
#[cfg(debug_assertions)]
fn gc_crashing_after(dir: &Path, phase: &str) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["gc", dir.to_str().expect("utf-8"), "--threshold", "0"])
        .env(uniqnode::gc::CRASH_AFTER_ENV, phase)
        .output()
        .expect("spawn gc");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[cfg(debug_assertions)]
fn gc(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["gc", dir.to_str().expect("utf-8"), "--threshold", "0"])
        .output()
        .expect("spawn gc")
}

/// 開き直して、生きているものが全件読めて fsck が緑であることを言い、孤児がいくつ残って
/// いるかを返す。
#[cfg(debug_assertions)]
fn recover_and_check(dir: &Path, filled: &Filled, context: &str) -> usize {
    let store = Store::open(small_config(dir)).expect("crash からの回復");
    for (id, body) in &filled.live {
        assert_eq!(
            store.get_object(id).expect("get").as_deref(),
            Some(body.as_slice()),
            "{context}: 生きている {id} が読めない"
        );
    }
    let report = store.fsck().expect("fsck");
    assert!(
        report.errors.is_empty(),
        "{context}: fsck: {:?}",
        report.errors
    );
    assert!(
        report.unlisted_packs.is_empty(),
        "{context}: MANIFEST が在るのに MANIFEST に無い pack が残っている: {:?}",
        report.unlisted_packs
    );
    assert!(
        entries_of(&dir.join("tmp")).is_empty(),
        "{context}: 開いた後の tmp/ に残したもの: {:?}",
        entries_of(&dir.join("tmp"))
    );
    filled
        .orphans
        .iter()
        .filter(|id| store.get_object(id).expect("get").is_some())
        .count()
}

/// 落とした後の回復に続けて、もう一度(落とさずに)回収すると孤児が全部消え、生きているものは
/// 全件読める。どの点で落ちても、次の回収が仕事を終える。
#[cfg(debug_assertions)]
fn a_second_gc_finishes_the_job(dir: &Path, filled: &Filled, context: &str) {
    let output = gc(dir);
    assert!(
        output.status.success(),
        "{context}: 2 回目の gc が失敗: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let remaining = recover_and_check(dir, filled, &format!("{context} の 2 回目の後"));
    assert_eq!(
        remaining, 0,
        "{context}: 2 回目の回収の後に孤児が残っている"
    );
}

/// 各相の直後に落としても、開き直した回復は fsck が緑で、生きているものは全件読める。孤児は
/// C-3(MANIFEST の差し替え)の前なら全部残り(次回の対象)、後なら全部消えている。
#[test]
#[cfg(debug_assertions)]
fn a_crash_after_any_phase_recovers_green_with_every_live_object_readable() {
    for phase in [
        uniqnode::gc::CRASH_AFTER_S,
        uniqnode::gc::CRASH_AFTER_B,
        uniqnode::gc::CRASH_AFTER_C1,
        uniqnode::gc::CRASH_AFTER_C2,
        uniqnode::gc::CRASH_AFTER_C3,
    ] {
        let dir = temp_dir(&format!("after-{phase}"));
        let filled = {
            let mut store = Store::open(small_config(&dir)).expect("open");
            fill_with_garbage(&mut store)
        };
        let (code, stderr) = gc_crashing_after(&dir, phase);
        assert_eq!(
            code, None,
            "{phase}: abort で落ちる(シグナル終了)はず: {stderr}"
        );
        assert!(
            stderr.contains("abort する"),
            "{phase}: 落とした理由を言う: {stderr}"
        );
        let remaining = recover_and_check(&dir, &filled, phase);
        if phase == uniqnode::gc::CRASH_AFTER_C3 {
            assert_eq!(
                remaining, 0,
                "{phase}: MANIFEST が新しいので孤児は消えている"
            );
        } else {
            assert_eq!(
                remaining,
                filled.orphans.len(),
                "{phase}: MANIFEST が古いので孤児は全部残る(次回の対象)"
            );
        }
        a_second_gc_finishes_the_job(&dir, &filled, phase);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

/// S の後: アクティブが封印され MANIFEST に載っている。回復後のアクティブは新しい番号。
#[test]
#[cfg(debug_assertions)]
fn a_crash_after_seal_leaves_the_active_pack_sealed_in_the_manifest() {
    let dir = temp_dir("seal-detail");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    let (sealed_before, active_before) = {
        let store = Store::open(small_config(&dir)).expect("open");
        (
            store.sealed_pack_numbers().to_vec(),
            store.active_pack_number(),
        )
    };
    gc_crashing_after(&dir, uniqnode::gc::CRASH_AFTER_S);
    let store = Store::open(small_config(&dir)).expect("recover");
    let mut expected = sealed_before.clone();
    expected.push(active_before);
    assert_eq!(
        store.sealed_pack_numbers(),
        expected.as_slice(),
        "追記中だった pack が封印されている"
    );
    assert_eq!(store.active_pack_number(), active_before + 1);
    drop(store);
    recover_and_check(&dir, &filled, "S");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// B の途中: tmp/ に書きかけの新 pack がある。開くと tmp/ が空になり、旧 pack は無傷。
#[test]
#[cfg(debug_assertions)]
fn a_crash_during_copy_leaves_a_partial_pack_in_tmp_that_open_removes() {
    let dir = temp_dir("copy-detail");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    let packs_before = pack_files(&dir);
    gc_crashing_after(&dir, uniqnode::gc::CRASH_AFTER_B);
    let leftovers = entries_of(&dir.join("tmp"));
    assert!(
        leftovers
            .iter()
            .any(|name| {
                name.starts_with(uniqnode::store::GC_TMP_PREFIX) && name.ends_with(".pack")
            }),
        "B の途中で落ちたので tmp/ に書きかけの新 pack がある: {leftovers:?}"
    );
    assert_eq!(
        pack_files(&dir),
        packs_before,
        "旧 pack は 1 本も動いていない"
    );
    recover_and_check(&dir, &filled, "B");
    assert!(
        entries_of(&dir.join("tmp")).is_empty(),
        "開くと tmp/ が空になる"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// C-2 の後、C-3 の前: packs/ に新 pack があるが MANIFEST に無い。中身は旧 pack と重複なので、
/// 開くとき削除され(残すと以後の追記がその後ろに続き、重複の分が二度と回収されない)、ログに
/// 名が出る。旧 pack は無傷。
#[test]
#[cfg(debug_assertions)]
fn a_crash_between_rename_and_manifest_leaves_a_duplicate_pack_that_open_removes() {
    let dir = temp_dir("rename-detail");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    let packs_before = pack_files(&dir);
    let sealed_before = manifest_packs(&dir);
    gc_crashing_after(&dir, uniqnode::gc::CRASH_AFTER_C2);
    let packs_after_crash = pack_files(&dir);
    let new_packs: Vec<&String> = packs_after_crash
        .iter()
        .filter(|name| !packs_before.contains(name))
        .collect();
    assert_eq!(
        new_packs.len(),
        1,
        "新 pack が 1 本据えられている: {packs_after_crash:?}"
    );
    let new_pack = new_packs[0].clone();
    assert_eq!(
        manifest_packs(&dir).len(),
        sealed_before.len() + 1,
        "MANIFEST は S の封印までで、新 pack は載っていない"
    );
    // 開き直す(manifest_packs が一度開いたので、その時点で削除されている)。
    recover_and_check(&dir, &filled, "C2");
    assert!(
        !pack_files(&dir).contains(&new_pack),
        "MANIFEST に無く中身が重複の新 pack {new_pack} は開くとき削除される: {:?}",
        pack_files(&dir)
    );
    for name in &packs_before {
        assert!(pack_files(&dir).contains(name), "旧 {name} は無傷");
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// C-3 の後、D の前: MANIFEST は新しく、旧 pack がディスクに残っている。開くとき「回収済みの
/// 残骸」として削除され、ログに名が出る。
#[test]
#[cfg(debug_assertions)]
fn a_crash_after_the_manifest_leaves_old_packs_that_open_removes_as_leftovers() {
    let dir = temp_dir("manifest-detail");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    let packs_before = pack_files(&dir);
    gc_crashing_after(&dir, uniqnode::gc::CRASH_AFTER_C3);
    let on_disk = pack_files(&dir);
    assert!(
        packs_before.iter().all(|name| on_disk.contains(name)),
        "D の前なので旧 pack が全部残っている: {on_disk:?}"
    );
    // 開くと残骸が消える。ログは標準エラーに出るので、fsck の子プロセスで観測する。
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["fsck", dir.to_str().expect("utf-8")])
        .output()
        .expect("spawn fsck");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fsck: {}\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("回収済みの残骸") && stderr.contains("削除する"),
        "残骸の削除をログに言う: {stderr}"
    );
    let after = pack_files(&dir);
    let removed = packs_before
        .iter()
        .filter(|name| !after.contains(name))
        .count();
    assert!(
        removed >= 1,
        "旧 pack が削除されている: {packs_before:?} -> {after:?}"
    );
    let remaining = recover_and_check(&dir, &filled, "C3");
    assert_eq!(remaining, 0);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// S と C の間に追記があったときの C-2 の後のクラッシュ(serve の admin で走らせ、B の後で
/// 止めている間に書く)。C-1 が追記中の pack を先に封印して MANIFEST に載せるので、新 pack を
/// 据えた直後に落ちても、追記した分は「MANIFEST に無く最後でもない pack」に見えず、回復が消さ
/// ない。順序が逆(据えてから封印)だと、追記した pack が残骸に見えて消え、書いたものを失う。
#[test]
#[cfg(debug_assertions)]
fn a_crash_after_rename_with_writes_since_seal_keeps_the_writes() {
    let dir = temp_dir("rename-with-writes");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    let go = dir.join("go");
    let ready = PathBuf::from(format!(
        "{}{}",
        go.display(),
        uniqnode::gc::WAIT_READY_SUFFIX
    ));
    let mut server = common::start_server_at_with_env(
        dir.clone(),
        &[
            (
                uniqnode::gc::WAIT_AFTER_COPY_ENV,
                go.to_str().expect("utf-8"),
            ),
            (uniqnode::gc::CRASH_AFTER_ENV, uniqnode::gc::CRASH_AFTER_C2),
        ],
    );
    server.remove_dir_on_drop = false;
    let address = server.address.clone();
    let gc_thread = std::thread::spawn(move || {
        // abort で応答は来ない(接続が切れる)。結果は見ない。
        let _ = std::panic::catch_unwind(|| {
            common::simple(&address, "POST", "/v1/admin/gc", b"{\"threshold\":\"0\"}")
        });
    });
    // 条件待ち(should/0104): gc が B を終えて合図を待っている印。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready.exists() {
        assert!(std::time::Instant::now() < deadline, "gc が B に達しない");
        std::thread::yield_now();
    }
    let fresh_body = b"{\"kind\":\"node\",\"written\":\"between S and C\",\"v\":1}".to_vec();
    let fresh = common::put_object(&server.address, &fresh_body);
    common::put_ref(&server.address, "notes/fresh", Some(&fresh));
    std::fs::write(&go, b"").expect("go");
    gc_thread.join().expect("gc thread");
    // 条件待ち: serve が abort で死ぬ(ロックが解放される)まで。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match server.child.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(
                    status.code().is_none(),
                    "abort で落ちる(シグナル終了)はず: {status}"
                );
                break;
            }
            None => assert!(
                std::time::Instant::now() < deadline,
                "serve が abort しない"
            ),
        }
        std::thread::yield_now();
    }
    drop(server);
    let store = Store::open(small_config(&dir)).expect("recover");
    assert_eq!(
        store.get_object(&fresh).expect("get"),
        Some(fresh_body),
        "S と C の間に書いたものが回復後も読める"
    );
    drop(store);
    let remaining = recover_and_check(&dir, &filled, "C2 with writes");
    assert_eq!(
        remaining,
        filled.orphans.len(),
        "MANIFEST は古いので孤児は残る"
    );
    let mut with_fresh = Filled {
        live: filled.live.clone(),
        orphans: filled.orphans.clone(),
    };
    with_fresh.live.push((
        fresh,
        b"{\"kind\":\"node\",\"written\":\"between S and C\",\"v\":1}".to_vec(),
    ));
    a_second_gc_finishes_the_job(&dir, &with_fresh, "C2 with writes");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// MANIFEST を失ったストアでは「残骸」の規則を使わない: MANIFEST に無く最後でもない pack は
/// 削除せず走査し、fsck が事実として報告する(エラーではない)。全オブジェクトが読める。
#[test]
fn without_a_manifest_unlisted_packs_are_kept_and_only_reported() {
    let dir = temp_dir("no-manifest");
    let filled = {
        let mut store = Store::open(small_config(&dir)).expect("open");
        fill_with_garbage(&mut store)
    };
    let packs_before = pack_files(&dir);
    assert!(packs_before.len() >= 3, "{packs_before:?}");
    std::fs::remove_file(dir.join("MANIFEST")).expect("MANIFEST を失う");
    let store = Store::open(small_config(&dir)).expect("open without manifest");
    assert_eq!(
        pack_files(&dir),
        packs_before,
        "MANIFEST が無ければ 1 本も削除しない"
    );
    for (id, body) in &filled.live {
        assert_eq!(
            store.get_object(id).expect("get").as_deref(),
            Some(body.as_slice())
        );
    }
    for id in &filled.orphans {
        assert!(
            store.get_object(id).expect("get").is_some(),
            "孤児も残っている"
        );
    }
    let report = store.fsck().expect("fsck");
    assert!(
        report.errors.is_empty(),
        "エラーではない: {:?}",
        report.errors
    );
    assert_eq!(
        report.unlisted_packs.len(),
        packs_before.len() - 1,
        "最後以外の全部が MANIFEST に無い pack として報告される"
    );
    drop(store);
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["fsck", dir.to_str().expect("utf-8")])
        .output()
        .expect("spawn fsck");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("unlisted packs"),
        "fsck の出力が事実を言う: {stdout}"
    );
    assert!(stdout.contains("errors: 0"), "{stdout}");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
