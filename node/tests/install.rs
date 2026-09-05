//! `uniqnode install` の統合テスト。本番の入口(サブコマンド)を子プロセスとして走らせ、
//! 置かれたファイルを観測する(should/0137)。手順は
//! SYSTEMD (uuid:7de68e4a-e6a6-4930-8cc7-a56f90f522e2)。
//!
//! unit は systemd が読む本物の置き場ではなく --unit-dir で指した作業ディレクトリに置き、
//! --no-start で daemon-reload までにとどめる。daemon-reload は本物の user 単位のマネージャに
//! 掛かる(何も enable しないので害は無い)が、systemctl の無い機械では install 自身が
//! 「systemctl が PATH に無い」で 1 を返す設計なので、そのテストは前提を出力して戻る。
//! 置き場は CARGO_TARGET_TMPDIR(target/ の下)。/tmp の下のストアは install が断るため
//! (unit の PrivateTmp から見えない)、std::env::temp_dir() は使えない。

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn work_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("install-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
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

/// systemctl が PATH に在るか(本番と同じ探し方。should/0135)。無ければ理由を出して
/// 呼び手が戻る。
fn systemctl_available() -> bool {
    let found = uniqnode::rendition::find_in_path("systemctl", std::env::var_os("PATH").as_deref());
    if found.is_none() {
        println!(
            "systemctl が PATH に無いので、daemon-reload まで行う install のテストは走らせない"
        );
    }
    found.is_some()
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読む: {e}", path.display()))
}

/// --no-start の install は、バイナリ・unit 4 本・drop-in 3 本を置き、ストアと写し先を作り、
/// unit の中身は docs/mop/systemd/user/ の現物と一致する。再実行しても同じ結果になる。
#[test]
fn install_without_start_places_the_binary_the_units_and_the_drop_ins() {
    if !systemctl_available() {
        return;
    }
    let work = work_dir("no-start");
    let data_dir = work.join("store");
    let backup_dir = work.join("copy");
    let unit_dir = work.join("units");
    let binary = work.join("bin").join("uniqnode");
    let arguments = [
        "install",
        data_dir.to_str().expect("utf-8"),
        "--listen",
        "127.0.0.1:7443",
        "--viewer-listen",
        "127.0.0.1:7453",
        "--serve-options",
        "--embed http://127.0.0.1:8083/v1/embeddings",
        "--backup-dir",
        backup_dir.to_str().expect("utf-8"),
        "--bin",
        binary.to_str().expect("utf-8"),
        "--unit-dir",
        unit_dir.to_str().expect("utf-8"),
        "--no-start",
    ];
    let outcome = uniqnode(&arguments);
    assert_eq!(
        outcome.status, 0,
        "install は 0 で終わるべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );

    // バイナリ: 走らせた実行ファイルと同じ中身で、実行できる。
    let installed = std::fs::read(&binary).expect("置かれたバイナリ");
    let running = std::fs::read(env!("CARGO_BIN_EXE_uniqnode")).expect("走らせた実行ファイル");
    assert_eq!(
        installed.len(),
        running.len(),
        "同じ中身のバイナリが置かれる"
    );
    assert_eq!(installed, running);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&binary)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "実行できる許可ビット: {mode:o}");
    }

    // unit 4 本: 現物と一致。
    let source = repo_root()
        .join("docs")
        .join("mop")
        .join("systemd")
        .join("user");
    for name in [
        "uniqnode-serve.service",
        "uniqnode-viewer.service",
        "uniqnode-backup.service",
        "uniqnode-backup.timer",
    ] {
        assert_eq!(
            text(&unit_dir.join(name)),
            text(&source.join(name)),
            "{name} は docs/mop/systemd/user/ の現物と同じ中身"
        );
    }

    // drop-in 3 本: 指定した値が載り、ExecStart= は置いたバイナリを指す。
    let serve = text(
        &unit_dir
            .join("uniqnode-serve.service.d")
            .join("override.conf"),
    );
    assert!(
        serve.contains(&format!(
            "\nEnvironment=UNIQNODE_DATA_DIR={}\n",
            data_dir.display()
        )),
        "{serve}"
    );
    assert!(
        serve.contains("\nEnvironment=UNIQNODE_LISTEN=127.0.0.1:7443\n"),
        "{serve}"
    );
    assert!(
        serve.contains(
            "\nEnvironment=\"UNIQNODE_SERVE_OPTIONS=--embed http://127.0.0.1:8083/v1/embeddings\"\n"
        ),
        "{serve}"
    );
    assert!(
        serve.contains(&format!(
            "\nExecStart=\nExecStart={} serve ${{UNIQNODE_DATA_DIR}} ${{UNIQNODE_LISTEN}} $UNIQNODE_SERVE_OPTIONS\n",
            binary.display()
        )),
        "{serve}"
    );
    let viewer = text(
        &unit_dir
            .join("uniqnode-viewer.service.d")
            .join("override.conf"),
    );
    assert!(
        viewer.contains("\nEnvironment=UNIQNODE_VIEWER_LISTEN=127.0.0.1:7453\n"),
        "{viewer}"
    );
    assert!(
        viewer.contains("\nEnvironment=UNIQNODE_SERVE_URL=http://127.0.0.1:7443\n"),
        "{viewer}"
    );
    let backup = text(
        &unit_dir
            .join("uniqnode-backup.service.d")
            .join("override.conf"),
    );
    assert!(
        backup.contains(&format!(
            "\nEnvironment=UNIQNODE_BACKUP_DIR={}\n",
            backup_dir.display()
        )),
        "{backup}"
    );
    assert!(
        backup.contains(&format!(
            "\nReadWritePaths=\nReadWritePaths={}\n",
            backup_dir.display()
        )),
        "{backup}"
    );

    // ReadWritePaths= が要求するディレクトリは作られている。
    assert!(data_dir.is_dir(), "ストアのディレクトリを作る");
    assert!(backup_dir.is_dir(), "写し先のディレクトリを作る");

    // 報告: 置いたものと、起こしていないことを言う。
    assert!(
        outcome.stdout.contains("daemon-reload: 済み"),
        "{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("--no-start なので起こしていない"),
        "{}",
        outcome.stdout
    );
    let lines_starting = |prefix: &str| {
        outcome
            .stdout
            .lines()
            .filter(|l| l.starts_with(prefix))
            .count()
    };
    assert_eq!(
        lines_starting("install: unit "),
        4,
        "unit 4 本を 1 行ずつ言う: {}",
        outcome.stdout
    );
    assert_eq!(
        lines_starting("install: drop-in "),
        3,
        "drop-in 3 本を 1 行ずつ言う: {}",
        outcome.stdout
    );

    // 再実行は更新: 同じ 0 で終わり、置かれたものは同じ。
    let again = uniqnode(&arguments);
    assert_eq!(again.status, 0, "{}\n{}", again.stdout, again.stderr);
    assert_eq!(
        text(
            &unit_dir
                .join("uniqnode-serve.service.d")
                .join("override.conf")
        ),
        serve,
        "再実行しても drop-in は同じ"
    );
    assert_eq!(
        std::fs::read(&binary).expect("binary").len(),
        installed.len()
    );
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// /tmp の下のストアは、何も置かずに断る(unit は PrivateTmp=yes でそこを見られない)。
#[test]
fn a_store_under_tmp_is_refused_and_nothing_is_written() {
    let work = work_dir("refuse-tmp");
    let unit_dir = work.join("units");
    let binary = work.join("bin").join("uniqnode");
    // 断られるはずの道は pid で一意にする(拒否が外れた版が走ると実際に作られ、固定の名前
    // だと次の走行の「作らない」の確認をその残骸が壊す。should/0137 の実験で実測)。
    let refused = format!("/tmp/uniqnode-install-refused-{}", std::process::id());
    let outcome = uniqnode(&[
        "install",
        &refused,
        "--bin",
        binary.to_str().expect("utf-8"),
        "--unit-dir",
        unit_dir.to_str().expect("utf-8"),
        "--no-start",
    ]);
    assert_eq!(
        outcome.status, 1,
        "断るときは 1\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome.stderr.contains("PrivateTmp"),
        "理由を言う: {}",
        outcome.stderr
    );
    assert!(!unit_dir.exists(), "unit を置かない");
    assert!(!binary.exists(), "バイナリを写さない");
    assert!(!Path::new(&refused).exists(), "ストアも作らない");
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// 知らない引数は usage(2)で落ちる。
#[test]
fn an_unknown_option_is_refused_with_usage() {
    let outcome = uniqnode(&["install", "/nonexistent/store", "--system"]);
    assert_eq!(outcome.status, 2, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(outcome.stderr.contains("usage:"), "{}", outcome.stderr);
}

/// 錠の探りは別プロセス(実プロセスの serve)が開いているストアを見分ける。install が
/// 起こす前に「別プロセスが開いている」と言うための判断。
#[test]
fn the_lock_probe_sees_a_store_held_by_a_serving_process() {
    let server = common::start_server("install-lock-probe");
    // serve はアドレスを束縛してから(「listening on」の後で)ストアを開く。/v1/status が
    // 答えるのはストアを開いた後なので、それを待ってから探る。
    let status = common::simple(&server.address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200, "{}", common::body_text(&status));
    assert!(
        uniqnode::store::opened_by_another_process(&server.dir).expect("probe"),
        "serve が開いている間は true"
    );
    let dir = server.dir.clone();
    drop(server);
    assert!(
        !uniqnode::store::opened_by_another_process(&dir).expect("probe"),
        "serve が終われば false"
    );
}
