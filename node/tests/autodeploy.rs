//! 自動更新(docs/mop/SYSTEMD.md の「自動更新」)の試験: serve と viewer の自己再起動、
//! apply の塊の走らせ方、autodeploy の記録と判断、install-autodeploy の drop-in。
//! git・cargo・systemctl を呼ぶ autodeploy のループそのものは、vega での最初の実行で確かめる。

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use uniqnode::autodeploy::{decide, environment_value, service_drop_in, InstallOptions, Next, Record};
use uniqnode::install::Account;
use uniqnode::selfreload::{SELF_RELOAD_ENV, SELF_RELOAD_EXIT, SELF_RELOAD_TICK};

/// テスト用のバイナリの写しを dir に置く(置き換えを試すので、cargo の組んだ実物は触らない)。
fn copy_binary(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mkdir");
    let path = dir.join("uniqnode");
    std::fs::copy(env!("CARGO_BIN_EXE_uniqnode"), &path).expect("copy");
    path
}

/// 隣に書いて rename で置き換える(自動更新と同じ置き換え方)。
fn replace_by_rename(path: &Path) {
    let staging = path.with_file_name(".uniqnode.new");
    std::fs::copy(env!("CARGO_BIN_EXE_uniqnode"), &staging).expect("copy");
    std::fs::rename(&staging, path).expect("rename");
}

fn spawn_listening(binary: &Path, arguments: &[&str], self_reload: bool) -> Child {
    let mut command = Command::new(binary);
    command.args(arguments).stdout(Stdio::piped()).stderr(Stdio::null());
    if self_reload {
        command.env(SELF_RELOAD_ENV, "1");
    } else {
        command.env_remove(SELF_RELOAD_ENV);
    }
    let mut child = command.spawn().expect("spawn");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("read listening line");
    assert!(line.starts_with("listening on "), "{line:?}");
    child
}

fn wait_exit(child: &mut Child, bound: Duration) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    while started.elapsed() < bound {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// 見張る serve と viewer はバイナリの置き換えで SELF_RELOAD_EXIT で終わり、見張らない serve は
/// 終わらない。should/0137: replaced の比較を常に false にすると、最初の assert が赤になる(実験した)。
#[test]
fn a_replaced_binary_ends_the_watching_serve_and_viewer_but_not_an_unwatched_serve() {
    let root = common::unique_dir("selfreload");
    let watched = copy_binary(&root.join("watched"));
    let unwatched = copy_binary(&root.join("unwatched"));
    let store_a = root.join("store-a");
    let store_b = root.join("store-b");
    let mut serve = spawn_listening(&watched, &["serve", store_a.to_str().unwrap(), "127.0.0.1:0"], true);
    let mut viewer = spawn_listening(
        &watched,
        &["viewer", store_a.to_str().unwrap(), "127.0.0.1:0", "--serve-url", "http://127.0.0.1:9"],
        true,
    );
    let mut quiet = spawn_listening(&unwatched, &["serve", store_b.to_str().unwrap(), "127.0.0.1:0"], false);
    replace_by_rename(&watched);
    replace_by_rename(&unwatched);
    let bound = SELF_RELOAD_TICK * 3;
    for (what, child) in [("serve", &mut serve), ("viewer", &mut viewer)] {
        let status = wait_exit(child, bound);
        assert_eq!(
            status.and_then(|s| s.code()),
            Some(SELF_RELOAD_EXIT),
            "見張る {what} は置き換えで {SELF_RELOAD_EXIT} で終わる"
        );
    }
    // 見張る側が終わった後も、見張らない側は少なくとも 1 刻み分は生きている。
    assert!(
        wait_exit(&mut quiet, SELF_RELOAD_TICK + Duration::from_secs(1)).is_none(),
        "{SELF_RELOAD_ENV} の無い serve は置き換えで終わらない"
    );
    let _ = quiet.kill();
    let _ = quiet.wait();
    std::fs::remove_dir_all(&root).expect("cleanup");
}

/// apply は塊を順に走らせ、最初の失敗で止まり、出力をログに残す。
/// should/0137: 失敗で止まらずに続けるようにすると、終了コードの assert が赤になる(実験した)。
#[test]
fn apply_runs_blocks_in_order_and_stops_at_the_first_failure() {
    let root = common::unique_dir("apply");
    std::fs::create_dir_all(&root).expect("mkdir");
    let first = root.join("first");
    let third = root.join("third");
    let file = root.join("test.apply");
    std::fs::write(
        &file,
        format!(
            "# 1 つ目は通る\ninit\n{}\n\n# 2 つ目は在らないストアを見て断られる\nstatus\n{}\n\n# 3 つ目は走らない\ninit\n{}\n",
            first.display(),
            root.join("missing").display(),
            third.display()
        ),
    )
    .expect("write");
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs).expect("mkdir");
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["apply", file.to_str().unwrap(), "--log-dir", logs.to_str().unwrap()])
        .output()
        .expect("run apply");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(first.join("node_key").exists(), "1 つ目の塊は走った");
    assert!(!third.exists(), "失敗の後の塊は走らない");
    let log: Vec<_> = std::fs::read_dir(&logs).expect("read_dir").collect();
    assert_eq!(log.len(), 1, "ログは 1 本");
    let text = std::fs::read_to_string(log[0].as_ref().unwrap().path()).expect("read log");
    assert!(text.contains("node_id: "), "子の出力がログに残る: {text}");
    assert!(text.contains("2 番目の命令が"), "止まった理由がログに残る: {text}");
    std::fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn the_record_round_trips_and_decides_what_a_round_does() {
    let record = Record {
        deployed: Some("aaa".into()),
        published: None,
        failed: Some("bbb".into()),
        rolled_back: Some("bbb".into()),
    };
    assert_eq!(Record::parse(&record.render()).expect("読める"), record);
    assert!(Record::parse("unknown x\n").is_err(), "知らない鍵は断る");
    assert_eq!(decide(&record, "aaa", false), Next::Nothing, "据え済み");
    assert_eq!(decide(&record, "aaa", true), Next::Publish, "据えたが push が残る");
    assert_eq!(decide(&record, "bbb", true), Next::Nothing, "失敗した SHA は試し直さない");
    assert_eq!(decide(&record, "ccc", true), Next::Deploy, "新しい SHA は据える");
    assert_eq!(decide(&Record::default(), "ccc", false), Next::Deploy, "記録が無ければ据える");
}

#[test]
fn the_listen_address_is_read_from_the_unit_environment() {
    let environment = "UNIQNODE_DATA_DIR=/s UNIQNODE_LISTEN=127.0.0.1:7440 UNIQNODE_LISTEN=127.0.0.1:7442 UNIQNODE_SELF_RELOAD=1";
    assert_eq!(environment_value(environment, "UNIQNODE_LISTEN"), Some("127.0.0.1:7442"), "後の指定が勝つ");
    assert_eq!(environment_value(environment, "UNIQNODE_VIEWER_LISTEN"), None);
}

#[test]
fn the_autodeploy_drop_in_names_the_user_the_checkout_and_the_binary() {
    let options = InstallOptions {
        repo: PathBuf::from("/work/uniqnode-deploy"),
        account: Account {
            name: "op".into(),
            uid: 1000,
            gid: 1000,
            group: "op".into(),
            home: PathBuf::from("/home/op"),
        },
        binary: PathBuf::from("/home/op/.local/bin/uniqnode"),
        publish_branch: Some("deployed_on_vega".into()),
        unit_dir: PathBuf::from("/etc/systemd/system"),
        start: false,
    };
    let text = service_drop_in(&options, "/home/op/.cargo/bin:/usr/bin").expect("描ける");
    for line in [
        "User=op",
        "Group=op",
        "Environment=UNIQNODE_AUTODEPLOY_REPO=/work/uniqnode-deploy",
        "Environment=\"UNIQNODE_AUTODEPLOY_OPTIONS=--bin /home/op/.local/bin/uniqnode --publish-branch deployed_on_vega\"",
        "Environment=PATH=/home/op/.cargo/bin:/usr/bin",
        "ExecStart=\n",
        "ExecStart=/home/op/.local/bin/uniqnode autodeploy ${UNIQNODE_AUTODEPLOY_REPO} $UNIQNODE_AUTODEPLOY_OPTIONS",
    ] {
        assert!(text.contains(line), "{line:?} が無い:\n{text}");
    }
    let spaced = InstallOptions { repo: PathBuf::from("/work/a b"), ..options };
    assert!(service_drop_in(&spaced, "/usr/bin").is_err(), "空白のある道は断る");
}

/// 中身が同じなら写さない(rename で inode を替えて、常駐を要らずに起こし直させない)。
#[test]
fn place_binary_leaves_an_identical_destination_alone() {
    use std::os::unix::fs::MetadataExt;
    let root = common::unique_dir("place-binary");
    std::fs::create_dir_all(&root).expect("mkdir");
    let source = root.join("source");
    let destination = root.join("destination");
    std::fs::write(&source, b"one").expect("write");
    uniqnode::install::place_binary(&source, &destination, None).expect("写す");
    let before = std::fs::metadata(&destination).expect("stat").ino();
    let line = uniqnode::install::place_binary(&source, &destination, None).expect("写さない");
    assert!(line.contains("中身が同じ"), "{line}");
    assert_eq!(std::fs::metadata(&destination).expect("stat").ino(), before, "置き換えていない");
    std::fs::write(&source, b"two").expect("write");
    uniqnode::install::place_binary(&source, &destination, None).expect("写す");
    assert_eq!(std::fs::read(&destination).expect("read"), b"two");
    assert_ne!(std::fs::metadata(&destination).expect("stat").ino(), before, "rename で置き換えた");
    std::fs::remove_dir_all(&root).expect("cleanup");
}
