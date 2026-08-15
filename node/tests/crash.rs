//! クラッシュ耐性の統合テスト(M1 受け入れ基準: 書き込み中の kill -9 から
//! 再起動でエラーなく回復し、fsck が全件パスする)。

use std::path::PathBuf;
use std::process::{Command, Stdio};
use uniqnode::store::{Store, StoreConfig};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("uniqnode-crash-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    dir
}

/// flood 子プロセスをある程度書かせてから SIGKILL し、回復を検証する。これを複数回
/// 繰り返して、切り裂かれる位置(pack 途中・reflog 途中・適用直前)を変える。
#[test]
fn kill9_during_writes_recovers_clean() {
    let binary = env!("CARGO_BIN_EXE_uniqnode");
    let dir = temp_dir("flood");
    let dir_text = dir.to_str().expect("utf-8 path");

    let mut previous_seq = 0u64;
    for round in 0..5 {
        let mut child = Command::new(binary)
            .args(["flood", dir_text])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn flood");

        // 条件待ち(should/0104): reflog がラウンドごとの目標量まで伸びたら kill する。
        // 目標量をラウンドでずらし、切り裂かれる位置を変える。
        let reflog = dir.join("reflog").join("reflog-000001.log");
        let target = 4096 * (round as u64 + 1) + 1234 * (round as u64);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let size = std::fs::metadata(&reflog).map(|m| m.len()).unwrap_or(0);
            if size > target {
                break;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("flood が書き込みを始めない(round {round}, size {size})");
            }
            std::thread::yield_now();
        }
        child.kill().expect("SIGKILL");
        child.wait().expect("wait");

        // 回復して検証。有効データはすべて残り、fsck が全件パスする。
        let store = Store::open(StoreConfig::new(&dir)).expect("crash からの回復");
        assert!(store.object_count() > 0);
        assert!(
            store.last_seq() >= previous_seq,
            "回復のたびに ref が単調に増えている(round {round})"
        );
        previous_seq = store.last_seq();
        let report = store.fsck().expect("fsck 実行");
        assert!(
            report.errors.is_empty(),
            "round {round} fsck errors: {:?}",
            report.errors
        );
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
