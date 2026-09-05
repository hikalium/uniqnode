//! 検査の命令が状態を作らないことの統合テスト。本番の入口 `uniqnode fsck` を子プロセスと
//! して走らせ、終了コードと、指した場所に何が残ったかを観測する(should/0137)。
//!
//! 直した欠陥: fsck を空のディレクトリに向けると新しいストアを初期化して node_key を作り、
//! objects 0 の緑を返していた。復元先を先に fsck した写しは別ノードになり、backup に
//! 「別のノードの写し」と断られる(BACKUP (uuid:e026a5e7-1ece-4f4e-b6b8-ee96c62883a2) の
//! 「復元」)。

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use uniqnode::store::{Store, StoreConfig};

fn temp_dir(name: &str) -> PathBuf {
    common::unique_dir(&format!("fsck-{name}"))
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

fn fsck(dir: &Path) -> CommandOutcome {
    uniqnode(&["fsck", dir.to_str().expect("utf-8")])
}

fn entries_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// (a) 空のディレクトリへの fsck は、理由を言って 1(開けない)で終わり、そこに node_key も
/// MANIFEST も packs/ も作らない。緑(objects: 0)を返してはならない。
#[test]
fn fsck_on_an_empty_directory_refuses_and_creates_nothing() {
    let dir = temp_dir("empty");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let outcome = fsck(&dir);
    assert_eq!(
        outcome.status, 1,
        "空のディレクトリは「開けない」の 1 で終わるべき\nstdout: {}\nstderr: {}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome
            .stderr
            .contains("ストアのデータディレクトリではない"),
        "断りの理由を言う: {}",
        outcome.stderr
    );
    assert!(
        !outcome.stdout.contains("objects:"),
        "検査の結果を出してはならない(空のストアを作って検査したことになる): {}",
        outcome.stdout
    );
    let left = entries_of(&dir);
    assert!(left.is_empty(), "断ったのに作られたもの: {left:?}");
    assert!(!dir.join("node_key").exists(), "node_key が作られている");
    assert!(!dir.join("MANIFEST").exists(), "MANIFEST が作られている");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (b) 存在しない道でも同じ: 1 で終わり、ディレクトリ自体を作らない。
#[test]
fn fsck_on_a_missing_directory_refuses_and_does_not_create_it() {
    let dir = temp_dir("missing");
    assert!(!dir.exists());
    let outcome = fsck(&dir);
    assert_eq!(
        outcome.status, 1,
        "存在しない道は 1 で終わるべき\nstdout: {}\nstderr: {}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome
            .stderr
            .contains("ストアのデータディレクトリではない"),
        "断りの理由を言う: {}",
        outcome.stderr
    );
    assert!(!dir.exists(), "断ったのにディレクトリが作られている");
}

/// (c) 在るストアには従来どおり緑。封印が起きておらず MANIFEST が無い小さなストアでも、
/// 一度開いたなら在るストアである(判定は MANIFEST ではなく node_key と packs/)。
#[test]
fn fsck_on_an_existing_store_is_green_even_before_the_first_seal() {
    let dir = temp_dir("existing");
    {
        let mut store = Store::open(StoreConfig::new(&dir)).expect("init");
        for i in 0..3u32 {
            let (id, _) = store
                .put_object(format!("{{\"i\":{i}}}").as_bytes())
                .expect("put");
            store
                .set_ref(&format!("notes/{i}"), Some(&id))
                .expect("set_ref");
        }
    }
    assert!(
        !dir.join("MANIFEST").exists(),
        "封印前なので MANIFEST は無い"
    );
    let outcome = fsck(&dir);
    assert_eq!(outcome.status, 0, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome.stdout.contains("objects: 3 refs: 3 errors: 0"),
        "{}",
        outcome.stdout
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 直した罠そのもの: 復元先を先に fsck してしまっても、そこには何も作られないので、続く
/// 逆向きの backup は断られず、復元されたノードは元と同じ node id を名乗る。
#[test]
fn a_restore_destination_checked_first_by_fsck_is_still_accepted_by_backup() {
    let source = temp_dir("trap-source");
    let copy = temp_dir("trap-copy");
    let restored = temp_dir("trap-restored");
    let node_id = {
        let mut store = Store::open(StoreConfig::new(&source)).expect("open source");
        let (id, _) = store.put_object(b"the one object").expect("put");
        store.set_ref("notes/one", Some(&id)).expect("set_ref");
        store.node_id_hex().to_string()
    };
    let taken = uniqnode(&[
        "backup",
        source.to_str().expect("utf-8"),
        copy.to_str().expect("utf-8"),
    ]);
    assert_eq!(taken.status, 0, "{}\n{}", taken.stdout, taken.stderr);

    // 運用者が復元先を「まず確かめよう」と fsck する。断られ、何も作られない。
    std::fs::create_dir_all(&restored).expect("mkdir");
    let checked = fsck(&restored);
    assert_eq!(checked.status, 1, "{}\n{}", checked.stdout, checked.stderr);
    assert!(
        entries_of(&restored).is_empty(),
        "{:?}",
        entries_of(&restored)
    );

    let restoring = uniqnode(&[
        "backup",
        copy.to_str().expect("utf-8"),
        restored.to_str().expect("utf-8"),
    ]);
    assert_eq!(
        restoring.status, 0,
        "先に fsck した復元先へ写し戻せるべき\nstdout: {}\nstderr: {}",
        restoring.stdout, restoring.stderr
    );
    let store = Store::open_existing(StoreConfig::new(&restored)).expect("復元先が開ける");
    assert_eq!(store.node_id_hex(), node_id, "同じノードとして復元される");
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&copy).expect("cleanup");
    std::fs::remove_dir_all(&restored).expect("cleanup");
}

/// 閲覧の命令も同じ道を通る: status を空のディレクトリに向けても初期化しない
/// (init だけが作る)。
#[test]
fn status_on_an_empty_directory_refuses_but_init_creates_the_store() {
    let dir = temp_dir("status-empty");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let text = dir.to_str().expect("utf-8");
    let status = uniqnode(&["status", text]);
    assert_eq!(status.status, 1, "{}\n{}", status.stdout, status.stderr);
    assert!(entries_of(&dir).is_empty(), "{:?}", entries_of(&dir));

    let init = uniqnode(&["init", text]);
    assert_eq!(init.status, 0, "{}\n{}", init.stdout, init.stderr);
    assert!(dir.join("node_key").exists(), "init は node_key を作る");
    let status = uniqnode(&["status", text]);
    assert_eq!(status.status, 0, "{}\n{}", status.stdout, status.stderr);
    assert!(status.stdout.contains("objects: 0"), "{}", status.stdout);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
