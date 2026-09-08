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
    uniqnode_with_path(arguments, &std::env::var("PATH").expect("PATH"))
}

/// PATH を差し替えて走らせる(install は自分の PATH で外部の道具を探し、その場所を drop-in
/// に書く)。
fn uniqnode_with_path(arguments: &[&str], path: &str) -> CommandOutcome {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .env("PATH", path)
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
    // 偽の pdftotext(実行できる空のファイル)を PATH の先頭のディレクトリに置く。install は
    // これを見つけ、そのディレクトリを unit の PATH の前に足すはずである。
    let tools = work.join("tools");
    std::fs::create_dir_all(&tools).expect("mkdir");
    {
        use std::os::unix::fs::PermissionsExt;
        let fake = tools.join("pdftotext");
        std::fs::write(&fake, "").expect("write");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").expect("PATH")
    );
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
    let outcome = uniqnode_with_path(&arguments, &path);
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
    // unit の PATH は systemd の既定であり、install はその前に、自分の PATH で pdftotext を
    // 見つけたディレクトリを足す。足す先頭のディレクトリは偽の pdftotext を置いた所。
    assert!(
        serve.contains(&format!(
            "\nEnvironment=PATH={}:",
            tools.display()
        )) && serve
            .lines()
            .any(|line| line.starts_with("Environment=PATH=") && line.ends_with(":/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")),
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
    let again = uniqnode_with_path(&arguments, &path);
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
    let outcome = uniqnode(&["install", "/nonexistent/store", "--sytem"]);
    assert_eq!(outcome.status, 2, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(outcome.stderr.contains("usage:"), "{}", outcome.stderr);
}

/// ヘルプの求めは据え付けを実行しない。この行を消すと `uniqnode install --help` が
/// --help をデータディレクトリと読んで unit を書き、ストアを作る(2026-09-08 に実際に
/// 起きた)。使い方は標準出力へ出して 0 で終わる(断りではないので標準エラーではない)。
#[test]
fn asking_for_help_prints_the_usage_without_installing() {
    let work = work_dir("help");
    let outcome = uniqnode_in(&work, &["install", "--help"]);
    assert_eq!(outcome.status, 0, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(outcome.stdout.contains("usage:"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("install <dir>"), "{}", outcome.stdout);
    assert!(outcome.stderr.is_empty(), "断りではない: {}", outcome.stderr);
    assert!(!work.join("--help").exists(), "ストアを作らない");
    assert_eq!(
        std::fs::read_dir(&work).expect("read_dir").count(),
        0,
        "何も置かない"
    );
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// データディレクトリの位置に指定の字面が来ていたら、走らせずに 2 で断る。この行を消すと
/// 打ち間違いがそのまま据え付けになる(must/0022)。
#[test]
fn an_option_in_the_data_directory_position_is_refused() {
    let work = work_dir("misplaced-option");
    let outcome = uniqnode_in(&work, &["install", "--sytem", "--no-start"]);
    assert_eq!(outcome.status, 2, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome.stderr.contains("データディレクトリの位置に --sytem が来ている"),
        "何が起きたかを言う: {}",
        outcome.stderr
    );
    assert!(!work.join("--sytem").exists(), "ストアを作らない");
    assert_eq!(
        std::fs::read_dir(&work).expect("read_dir").count(),
        0,
        "何も置かない"
    );
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// 作業ディレクトリを指して走らせる(相対の道が指されたときに、何がどこへ作られるかを
/// 観測できるようにする)。
fn uniqnode_in(dir: &Path, arguments: &[&str]) -> CommandOutcome {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("spawn uniqnode");
    CommandOutcome {
        status: output.status.code().expect("exit code"),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

/// SUDO_USER を消して走らせる(--system の実行ユーザの既定はそこから来るので、無い状態を
/// 作って「断る」を観測する)。
fn uniqnode_without_sudo_user(arguments: &[&str]) -> CommandOutcome {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .env_remove("SUDO_USER")
        .output()
        .expect("spawn uniqnode");
    CommandOutcome {
        status: output.status.code().expect("exit code"),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

/// このテストは root で走らない前提(root なら --system は断られず、本物の
/// /etc/systemd/system に unit を書きに行く)。root なら理由を出して戻る。
fn not_root() -> bool {
    let euid = uniqnode::install::effective_uid().expect("euid");
    if euid == 0 {
        println!("root で走っているので、root でないときに断るテストは走らせない");
    }
    euid != 0
}

/// --system は root でなければ、何も置かずに断る。文言は sudo で走らせることと、出力を tee で
/// ファイルに残す形を含む(操作者に渡す命令の形。CLAUDE.md)。実行ユーザの名は
/// 引き当てる前に断るので、実在しない名でよい。
/// should/0137: install::scope の euid の検査を外すと、getent が無い利用者で落ちる
/// (別の文言)ので、この 2 つの assert が赤になる。
#[test]
fn a_system_install_without_root_is_refused_with_the_sudo_and_tee_form() {
    if !not_root() {
        return;
    }
    let work = work_dir("system-not-root");
    let unit_dir = work.join("units");
    let outcome = uniqnode(&[
        "install",
        work.join("store").to_str().expect("utf-8"),
        "--system",
        "--user",
        "uniqnode-no-such-user",
        "--unit-dir",
        unit_dir.to_str().expect("utf-8"),
        "--no-start",
    ]);
    assert_eq!(outcome.status, 1, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome.stderr.contains("root で走らせる") && outcome.stderr.contains("sudo "),
        "sudo で走らせることを言う: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("2>&1 | tee /tmp/uniqnode-install-system.log"),
        "出力をファイルに残す形を含む: {}",
        outcome.stderr
    );
    assert!(!unit_dir.exists(), "unit を置かない");
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// --system に --user が無く SUDO_USER も無ければ、root かどうかを見る前に「実行ユーザが要る」
/// で断る(root で常駐させない)。--user root も断る。
/// should/0137: install::scope の `sudo_user` の既定を "root" に替えると 1 つ目が通ってしまい、
/// name == "root" の検査を外すと 2 つ目が root の検査の文言に変わって赤になる。
#[test]
fn a_system_install_needs_a_service_user_and_refuses_root() {
    let work = work_dir("system-no-user");
    let store = work.join("store");
    let outcome = uniqnode_without_sudo_user(&[
        "install",
        store.to_str().expect("utf-8"),
        "--system",
        "--no-start",
    ]);
    assert_eq!(outcome.status, 1, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome.stderr.contains("--user <name>") && outcome.stderr.contains("SUDO_USER"),
        "{}",
        outcome.stderr
    );
    assert!(outcome.stderr.contains("root では常駐させない"), "{}", outcome.stderr);
    let as_root = uniqnode(&[
        "install",
        store.to_str().expect("utf-8"),
        "--system",
        "--user",
        "root",
        "--no-start",
    ]);
    assert_eq!(as_root.status, 1, "{}\n{}", as_root.stdout, as_root.stderr);
    assert!(
        as_root.stderr.contains("root は使えない"),
        "{}",
        as_root.stderr
    );
    assert!(!store.exists(), "ストアを作らない");
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// --user は --system のときだけ、--after も --system のときだけ(user unit は system unit を
/// 待てない)。--listen-agent の形は --listen と同じ検査(ポート 0 は断る)。どれも何も置かない。
/// should/0137: normalize の --after の検査を外すと --after の status の assert が 0 で赤になる
/// (--no-start で unit が書かれてしまう。実験した)。
#[test]
fn after_and_user_are_system_only_and_the_agent_listen_is_checked() {
    let work = work_dir("user-scope-refusals");
    let unit_dir = work.join("units");
    let base = |extra: &[&str]| -> CommandOutcome {
        let mut arguments = vec![
            "install",
            work.join("store").to_str().expect("utf-8"),
            "--bin",
            work.join("bin").join("uniqnode").to_str().expect("utf-8"),
            "--unit-dir",
            unit_dir.to_str().expect("utf-8"),
            "--no-start",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        arguments.extend(extra.iter().map(|s| s.to_string()));
        let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
        uniqnode(&borrowed)
    };
    let with_user = base(&["--user", "op"]);
    assert_eq!(with_user.status, 1, "{}\n{}", with_user.stdout, with_user.stderr);
    assert!(
        with_user.stderr.contains("--user op は --system のときだけ"),
        "{}",
        with_user.stderr
    );
    let with_after = base(&["--after", "wg-quick@wg1.service"]);
    assert_eq!(with_after.status, 1, "{}\n{}", with_after.stdout, with_after.stderr);
    assert!(
        with_after.stderr.contains("user unit は system unit を待てない"),
        "{}",
        with_after.stderr
    );
    assert!(!unit_dir.exists(), "断るときは unit を置かない");
    let zero_port = base(&["--listen-agent", "127.0.0.1:0"]);
    assert_eq!(zero_port.status, 1, "{}\n{}", zero_port.stdout, zero_port.stderr);
    assert!(
        zero_port.stderr.contains("--listen-agent 127.0.0.1:0")
            && zero_port.stderr.contains("ポートは固定する"),
        "{}",
        zero_port.stderr
    );
    let same_as_main = base(&["--listen-agent", "127.0.0.1:7440"]);
    assert_eq!(same_as_main.status, 1, "{}\n{}", same_as_main.stdout, same_as_main.stderr);
    assert!(
        same_as_main.stderr.contains("--listen と同じ"),
        "{}",
        same_as_main.stderr
    );
    assert!(!unit_dir.exists(), "断るときは unit を置かない");
    // 書く口は読み口があるときだけ(should/0137: normalize の agent_writable の検査を消すと
    // ここが「unit が置かれた」で赤になる)。名前の形も検める。
    let writable_alone = base(&["--agent-writable", "notes"]);
    assert_eq!(writable_alone.status, 1, "{}\n{}", writable_alone.stdout, writable_alone.stderr);
    assert!(
        writable_alone.stderr.contains("--agent-writable notes は --listen-agent があるときだけ"),
        "{}",
        writable_alone.stderr
    );
    let bad_name = base(&["--listen-agent", "127.0.0.1:7441", "--agent-writable", "a b"]);
    assert_eq!(bad_name.status, 1, "{}\n{}", bad_name.stdout, bad_name.stderr);
    assert!(
        bad_name.stderr.contains("--agent-writable はコレクション名(空でなく / と空白を含まない 1 語): \"a b\""),
        "{}",
        bad_name.stderr
    );
    // 読める集合も同じ扱い(読み口があるときだけ、名前は 1 語)。
    let readable_alone = base(&["--agent-collections", "notes"]);
    assert_eq!(readable_alone.status, 1, "{}\n{}", readable_alone.stdout, readable_alone.stderr);
    assert!(
        readable_alone.stderr.contains("--agent-collections notes は --listen-agent があるときだけ"),
        "{}",
        readable_alone.stderr
    );
    let bad_readable = base(&["--listen-agent", "127.0.0.1:7441", "--agent-collections", "a b"]);
    assert_eq!(bad_readable.status, 1, "{}\n{}", bad_readable.stdout, bad_readable.stderr);
    assert!(
        bad_readable.stderr.contains("--agent-collections はコレクション名(空でなく / と空白を含まない 1 語): \"a b\""),
        "{}",
        bad_readable.stderr
    );
    assert!(!unit_dir.exists(), "断るときは unit を置かない");
    // 移行と firewall の指定も system 単位だけ(should/0137: normalize の 2 つの検査を消すと
    // ここが「unit が置かれた」で赤になる)。
    let take_over = base(&["--take-over-user-units"]);
    assert_eq!(take_over.status, 1, "{}\n{}", take_over.stdout, take_over.stderr);
    assert!(
        take_over.stderr.contains("--take-over-user-units は --system のときだけ"),
        "{}",
        take_over.stderr
    );
    let firewall = base(&["--firewall-allow", "10.10.128.4"]);
    assert_eq!(firewall.status, 1, "{}\n{}", firewall.stdout, firewall.stderr);
    assert!(
        firewall.stderr.contains("--firewall-allow 10.10.128.4 は --system で --listen-agent が"),
        "{}",
        firewall.stderr
    );
    let bad_address = base(&["--firewall-allow", "orion"]);
    assert_eq!(bad_address.status, 1, "{}\n{}", bad_address.stdout, bad_address.stderr);
    assert!(
        bad_address.stderr.contains("--firewall-allow orion は IPv4 アドレスでない"),
        "{}",
        bad_address.stderr
    );
    assert!(!unit_dir.exists(), "断るときは unit を置かない");
    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// system 単位の指定(root は要らない: drop-in を描く関数を直に呼ぶ)。実行ユーザは引き当てた
/// 実物の形で与える。
fn system_options() -> uniqnode::install::Options {
    use uniqnode::install::{Account, Options, Scope};
    let account = Account {
        name: "op".to_string(),
        uid: 1000,
        gid: 1000,
        group: "op".to_string(),
        home: PathBuf::from("/home/op"),
    };
    let home = account.home.clone();
    let mut options = Options::defaults(
        PathBuf::from("/srv/uniqnode-store"),
        &home,
        Scope::System(account),
    );
    options.after = vec!["wg-quick@wg1.service".to_string()];
    options.agent_listen = Some("10.10.128.1:7441".to_string());
    options
}

/// system 単位の drop-in: [Unit] に After=/Wants=、[Service] の先頭に User=/Group= と
/// StateDirectory= の打ち消し、バイナリと写し先が home の下なので ProtectHome=read-only、
/// serve には UNIQNODE_AGENT_LISTEN と ExecStart= 末尾の --listen-agent。unit の置き場の既定は
/// /etc/systemd/system で、埋め込む unit は docs/mop/systemd/system/ の現物。
/// should/0137: drop_ins の service_head から StateDirectory= の行を消すと 3 つ目の assert が、
/// exec_tail を空にすると serve の ExecStart= の assert が赤になる(どちらも実験した)。
#[test]
fn a_system_drop_in_names_the_user_cancels_the_state_directory_and_waits_for_the_unit() {
    use uniqnode::install::{drop_ins, SYSTEMD_DEFAULT_PATH, SYSTEM_UNIT_DIR};
    let options = system_options();
    assert_eq!(options.unit_dir, Path::new(SYSTEM_UNIT_DIR));
    assert_eq!(options.binary, Path::new("/home/op/.local/bin/uniqnode"));
    assert_eq!(options.backup_dir, Path::new("/home/op/uniqnode-backup"));
    let source = repo_root()
        .join("docs")
        .join("mop")
        .join("systemd")
        .join("system");
    for (name, embedded) in options.units() {
        assert_eq!(
            *embedded,
            text(&source.join(name)),
            "{name} は docs/mop/systemd/system/ の現物と同じ中身"
        );
    }
    let rendered = drop_ins(&options, SYSTEMD_DEFAULT_PATH).expect("描ける");
    let of = |unit: &str| -> String {
        rendered
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, text)| text.clone())
            .expect("3 つの service の 1 つ")
    };
    let serve = of("uniqnode-serve.service");
    assert!(
        serve.contains("[Unit]\nAfter=wg-quick@wg1.service\nWants=wg-quick@wg1.service\n[Service]\n"),
        "{serve}"
    );
    assert!(
        serve.contains("[Service]\nUser=op\nGroup=op\nStateDirectory=\n"),
        "{serve}"
    );
    assert!(
        serve.contains("\nProtectHome=read-only\nEnvironment=PATH="),
        "home の下のバイナリと写し先のために緩める: {serve}"
    );
    assert!(
        serve.contains("\nEnvironment=UNIQNODE_AGENT_LISTEN=10.10.128.1:7441\n"),
        "{serve}"
    );
    assert!(
        serve.ends_with(
            "ExecStart=\nExecStart=/home/op/.local/bin/uniqnode serve ${UNIQNODE_DATA_DIR} \
             ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS --listen-agent ${UNIQNODE_AGENT_LISTEN}\n"
        ),
        "{serve}"
    );
    // viewer と backup も同じ [Unit] と User=/Group= を持ち、読み口は持たない。
    for unit in ["uniqnode-viewer.service", "uniqnode-backup.service"] {
        let text = of(unit);
        assert!(
            text.contains("[Unit]\nAfter=wg-quick@wg1.service\nWants=wg-quick@wg1.service\n"),
            "{unit}:\n{text}"
        );
        assert!(
            text.contains("[Service]\nUser=op\nGroup=op\nStateDirectory=\n"),
            "{unit}:\n{text}"
        );
        assert!(!text.contains("UNIQNODE_AGENT_LISTEN"), "{unit}:\n{text}");
        assert!(!text.contains("--listen-agent"), "{unit}:\n{text}");
    }
    let backup = of("uniqnode-backup.service");
    assert!(
        backup.ends_with(
            "ExecStart=\nExecStart=/home/op/.local/bin/uniqnode backup ${UNIQNODE_DATA_DIR} \
             ${UNIQNODE_BACKUP_DIR}\n"
        ),
        "{backup}"
    );

    // 何も home の下に無ければ ProtectHome= は unit の yes のまま(緩めない)。--after が無ければ
    // [Unit] も無い。user 単位には User=/Group= も StateDirectory= も出ない。
    let mut bare = system_options();
    bare.binary = PathBuf::from("/usr/local/bin/uniqnode");
    bare.backup_dir = PathBuf::from("/var/backups/uniqnode");
    bare.after.clear();
    bare.agent_listen = None;
    let bare_serve = drop_ins(&bare, SYSTEMD_DEFAULT_PATH).expect("描ける")[0].1.clone();
    assert!(!bare_serve.contains("ProtectHome"), "{bare_serve}");
    assert!(!bare_serve.contains("[Unit]"), "{bare_serve}");
    assert!(bare_serve.contains("\n[Service]\nUser=op\nGroup=op\nStateDirectory=\nEnvironment=PATH="), "{bare_serve}");
    let user_serve = {
        let account_home = PathBuf::from("/home/op");
        let options = uniqnode::install::Options::defaults(
            PathBuf::from("/home/op/store"),
            &account_home,
            uniqnode::install::Scope::User,
        );
        drop_ins(&options, SYSTEMD_DEFAULT_PATH).expect("描ける")[0].1.clone()
    };
    assert!(!user_serve.contains("User="), "{user_serve}");
    assert!(!user_serve.contains("StateDirectory="), "{user_serve}");
    assert!(!user_serve.contains("ProtectHome"), "{user_serve}");
}

/// 書く口(`--agent-writable`)の drop-in: 集合は空白で分けた 1 本の環境変数
/// UNIQNODE_AGENT_WRITABLE に写り、ExecStart= の末尾には `--listen-agent ${UNIQNODE_AGENT_LISTEN}`
/// の後に `--agent-writable <c>` が集合の数だけ値のまま並ぶ(環境変数の展開に頼らない)。
/// viewer と backup には出ない。読み口が無ければ normalize が断る。
/// should/0137: drop_ins の agent_writable の分岐を消すと ExecStart= の全文一致が、Environment=
/// の行だけ消すと 1 つ目の assert が赤になる。
#[test]
fn the_writable_collections_are_written_to_the_drop_in_as_values_not_expansions() {
    use uniqnode::install::{drop_ins, normalize, SYSTEMD_DEFAULT_PATH};
    let mut options = system_options();
    options.agent_writable = vec!["lamalium-notes".to_string(), "scratch".to_string()];
    let rendered = drop_ins(&options, SYSTEMD_DEFAULT_PATH).expect("描ける");
    let of = |unit: &str| -> String {
        rendered
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, text)| text.clone())
            .expect("3 つの service の 1 つ")
    };
    let serve = of("uniqnode-serve.service");
    assert!(
        serve.contains(
            "\nEnvironment=UNIQNODE_AGENT_LISTEN=10.10.128.1:7441\n\
             Environment=\"UNIQNODE_AGENT_WRITABLE=lamalium-notes scratch\"\n"
        ),
        "{serve}"
    );
    assert!(
        serve.ends_with(
            "ExecStart=\nExecStart=/home/op/.local/bin/uniqnode serve ${UNIQNODE_DATA_DIR} \
             ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS --listen-agent ${UNIQNODE_AGENT_LISTEN} \
             --agent-writable lamalium-notes --agent-writable scratch\n"
        ),
        "{serve}"
    );
    for unit in ["uniqnode-viewer.service", "uniqnode-backup.service"] {
        let text = of(unit);
        assert!(!text.contains("UNIQNODE_AGENT_WRITABLE"), "{unit}:\n{text}");
        assert!(!text.contains("--agent-writable"), "{unit}:\n{text}");
    }
    // 1 つだけなら環境変数も引用符なしの 1 語。
    options.agent_writable = vec!["lamalium-notes".to_string()];
    let one = drop_ins(&options, SYSTEMD_DEFAULT_PATH).expect("描ける")[0].1.clone();
    assert!(one.contains("\nEnvironment=UNIQNODE_AGENT_WRITABLE=lamalium-notes\n"), "{one}");
    assert!(one.ends_with("${UNIQNODE_AGENT_LISTEN} --agent-writable lamalium-notes\n"), "{one}");

    // 読み口が無ければ書く口は受け付けない(unit を描く前に断る)。
    let mut without_door = system_options();
    without_door.agent_listen = None;
    without_door.after.clear();
    without_door.agent_writable = vec!["lamalium-notes".to_string()];
    let refused = match normalize(without_door) {
        Ok(_) => panic!("読み口が無いのに書く口の指定が通った"),
        Err(message) => message,
    };
    assert_eq!(
        refused,
        "--agent-writable lamalium-notes は --listen-agent があるときだけ受け付ける(書く許可は\
         読み口に掛かるもので、読み口が無ければ効かせる先が無い)"
    );
    let mut with_door = system_options();
    with_door.agent_writable = vec!["lamalium-notes".to_string()];
    let accepted = normalize(with_door).expect("読み口があれば通る");
    assert_eq!(accepted.agent_writable, vec!["lamalium-notes".to_string()]);
}

/// 読める集合(`--agent-collections`)の drop-in: 書く集合と全く同じ流儀で、集合は空白で
/// 分けた 1 本の環境変数 UNIQNODE_AGENT_COLLECTIONS に写り、ExecStart= の末尾には
/// `--agent-collections <c>` が集合の数だけ値のまま並ぶ(書く集合の後ろ)。viewer と backup
/// には出ない。読み口が無ければ normalize が断る。
/// should/0137: drop_ins の agent_collections の分岐を消すと ExecStart= の全文一致が、
/// Environment= の行だけ消すと 1 つ目の assert が赤になる。
#[test]
fn the_readable_collections_are_written_to_the_drop_in_as_values_not_expansions() {
    use uniqnode::install::{drop_ins, normalize, SYSTEMD_DEFAULT_PATH};
    let mut options = system_options();
    options.agent_writable = vec!["lamalium-notes".to_string()];
    options.agent_collections = vec!["articles".to_string(), "lamalium-notes".to_string()];
    let rendered = drop_ins(&options, SYSTEMD_DEFAULT_PATH).expect("描ける");
    let of = |unit: &str| -> String {
        rendered
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, text)| text.clone())
            .expect("3 つの service の 1 つ")
    };
    let serve = of("uniqnode-serve.service");
    assert!(
        serve.contains(
            "\nEnvironment=UNIQNODE_AGENT_WRITABLE=lamalium-notes\n\
             Environment=\"UNIQNODE_AGENT_COLLECTIONS=articles lamalium-notes\"\n"
        ),
        "{serve}"
    );
    assert!(
        serve.ends_with(
            "ExecStart=\nExecStart=/home/op/.local/bin/uniqnode serve ${UNIQNODE_DATA_DIR} \
             ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS --listen-agent ${UNIQNODE_AGENT_LISTEN} \
             --agent-writable lamalium-notes --agent-collections articles \
             --agent-collections lamalium-notes\n"
        ),
        "{serve}"
    );
    for unit in ["uniqnode-viewer.service", "uniqnode-backup.service"] {
        let text = of(unit);
        assert!(!text.contains("UNIQNODE_AGENT_COLLECTIONS"), "{unit}:\n{text}");
        assert!(!text.contains("--agent-collections"), "{unit}:\n{text}");
    }
    // 1 つだけなら環境変数も引用符なしの 1 語。書く集合が無くても読める集合だけ書ける。
    let mut alone = system_options();
    alone.agent_collections = vec!["articles".to_string()];
    let one = drop_ins(&alone, SYSTEMD_DEFAULT_PATH).expect("描ける")[0].1.clone();
    assert!(one.contains("\nEnvironment=UNIQNODE_AGENT_COLLECTIONS=articles\n"), "{one}");
    assert!(one.ends_with("${UNIQNODE_AGENT_LISTEN} --agent-collections articles\n"), "{one}");

    // 読み口が無ければ読める集合の指定も受け付けない(unit を描く前に断る)。
    let mut without_door = system_options();
    without_door.agent_listen = None;
    without_door.after.clear();
    without_door.agent_collections = vec!["articles".to_string()];
    let refused = match normalize(without_door) {
        Ok(_) => panic!("読み口が無いのに読める集合の指定が通った"),
        Err(message) => message,
    };
    assert_eq!(
        refused,
        "--agent-collections articles は --listen-agent があるときだけ受け付ける(読む許可は\
         読み口に掛かるもので、読み口が無ければ効かせる先が無い)"
    );
    let mut with_door = system_options();
    with_door.agent_collections = vec!["articles".to_string()];
    let accepted = normalize(with_door).expect("読み口があれば通る");
    assert_eq!(accepted.agent_collections, vec!["articles".to_string()]);
}

/// --after の unit 名の検査と、system 単位で外部の道具を探す PATH(実行ユーザの ~/.local/bin と
/// ~/bin を後ろに足す。sudo の secure_path には無いため)。
#[test]
fn after_units_need_a_type_suffix_and_the_system_tool_search_reaches_the_users_local_bin() {
    use uniqnode::install::{check_after_unit, tool_search_path, Scope};
    assert!(check_after_unit("wg-quick@wg1.service").is_ok());
    assert!(check_after_unit("srv-store.mount").is_ok());
    let no_suffix = check_after_unit("wg-quick@wg1").err().expect("断る");
    assert!(no_suffix.contains("拡張子"), "{no_suffix}");
    assert!(check_after_unit("").is_err());
    assert!(check_after_unit("a b.service").is_err());
    let options = system_options();
    let path = tool_search_path("/usr/bin:/bin", &options.scope);
    assert_eq!(path, "/usr/bin:/bin:/home/op/.local/bin:/home/op/bin");
    assert_eq!(tool_search_path("/usr/bin:/bin", &Scope::User), "/usr/bin:/bin");
}

/// ロックの探りは別プロセス(実プロセスの serve)が開いているストアを見分ける。install が
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

/// 1 命令の移行が中で打つ字句と、その効果の読み方。root が要る部分(systemctl --user -M、ufw)
/// は起こさず、組み立てる引数と答えの判定だけを見る。should/0137: ufw_rule_listed の
/// `ALLOW` の照合を消すと DENY の行で true になり 3 つ目の assert が赤、ufw_status_of の
/// inactive の腕を消すと 2 つ目が Err で赤になる(どちらも実験した)。
#[test]
fn the_take_over_and_firewall_steps_use_fixed_words_and_judge_by_effect() {
    use uniqnode::install::{
        agent_ip_and_port, check_firewall_allow, normalize, take_over_command,
        ufw_allow_arguments, ufw_rule_listed, ufw_status_of, unit_is_running, user_manager_flags,
        UfwStatus,
    };
    assert_eq!(user_manager_flags("op"), vec!["--user", "-M", "op@"]);
    assert_eq!(
        take_over_command("op"),
        "systemctl --user -M op@ disable --now uniqnode-serve.service uniqnode-viewer.service \
         uniqnode-backup.timer"
    );
    assert!(unit_is_running("active") && unit_is_running("deactivating"));
    assert!(!unit_is_running("inactive") && !unit_is_running("failed") && !unit_is_running(""));

    assert_eq!(ufw_status_of("Status: active\n\nTo  Action  From\n").expect("読める"), UfwStatus::Active);
    assert_eq!(ufw_status_of("Status: inactive\n").expect("読める"), UfwStatus::Inactive);
    assert!(ufw_status_of("ERROR: You need to be root\n").is_err());

    assert_eq!(agent_ip_and_port("10.10.128.1:7441").expect("分けられる"), ("10.10.128.1".to_string(), "7441".to_string()));
    assert_eq!(
        ufw_allow_arguments("10.10.128.4", "10.10.128.1:7441").expect("組める"),
        vec!["allow", "from", "10.10.128.4", "to", "10.10.128.1", "port", "7441", "proto", "tcp"]
    );
    let listed = "Status: active\n\nTo                         Action      From\n--                         ------      ----\n10.10.128.1 7441/tcp       ALLOW       10.10.128.4\n";
    assert!(ufw_rule_listed(listed, "10.10.128.4", "10.10.128.1:7441").expect("読める"));
    let denied = listed.replace("ALLOW", "DENY");
    assert!(!ufw_rule_listed(&denied, "10.10.128.4", "10.10.128.1:7441").expect("読める"));
    assert!(!ufw_rule_listed(listed, "10.10.128.5", "10.10.128.1:7441").expect("読める"));
    assert!(!ufw_rule_listed(listed, "10.10.128.4", "10.10.128.1:7442").expect("読める"));

    assert!(check_firewall_allow("10.10.128.4").is_ok());
    assert!(check_firewall_allow("10.10.128.0/24").is_err(), "範囲は受けない");

    // system 単位でも、読み口が無ければ firewall の指定は断る。
    let mut without_door = system_options();
    without_door.agent_listen = None;
    without_door.after.clear();
    without_door.firewall_allow = Some("10.10.128.4".to_string());
    let refused = match normalize(without_door) {
        Ok(_) => panic!("読み口が無いのに firewall の指定が通った"),
        Err(message) => message,
    };
    assert!(refused.contains("--listen-agent があるときだけ"), "{refused}");
    let mut with_door = system_options();
    with_door.firewall_allow = Some("10.10.128.4".to_string());
    with_door.take_over_user_units = true;
    let accepted = normalize(with_door).expect("読み口があれば通る");
    assert_eq!(accepted.firewall_allow.as_deref(), Some("10.10.128.4"));
    assert!(accepted.take_over_user_units);
}

/// ufw が active でない機械では、規則は nft の自分の表 inet uniqnode に入れ、serve の drop-in の
/// ExecStartPre=+nft -f が起動のたびに入れ直す。規則ファイルは何度読んでも同じ 1 表になる形
/// (空で作る → 消す → 作る)。should/0137: nft_rules_text の `delete table` の行を消すと
/// 2 つ目の assert が赤、drop_ins の ExecStartPre= の分岐を消すと最後の assert が赤になる
/// (どちらも実験した)。
#[test]
fn the_nft_road_writes_one_table_and_lets_the_serve_unit_load_it_on_every_start() {
    use uniqnode::install::{
        drop_ins, nft_rule_listed, nft_rules_text, FirewallBackend, NFT_RULES_NAME, NFT_TABLE,
    };
    let text = nft_rules_text("10.10.128.4", "10.10.128.1:7441").expect("組める");
    assert!(text.contains("table inet uniqnode {}\n"), "{text}");
    assert!(text.contains("delete table inet uniqnode\n"), "{text}");
    assert!(
        text.contains("type filter hook input priority filter; policy accept;"),
        "{text}"
    );
    // 許す相手は from と読み口自身の IP(install の確認が同じ機械から 10.10.128.1 を源に
    // 届くので、自分を締め出さない。実測 2026-09-06: from だけにしたら install の確認が
    // connection timed out で赤になった)。
    assert!(
        text.contains(
            "ip daddr 10.10.128.1 tcp dport 7441 ip saddr != { 10.10.128.4, 10.10.128.1 } counter drop"
        ),
        "{text}"
    );
    assert_eq!(NFT_TABLE, "inet uniqnode");

    // nft list table の答え(counter の数は変わり、集合の並びも変わりうる)。
    let listing = "table inet uniqnode {\n\tchain agent_door {\n\t\ttype filter hook input priority filter; policy accept;\n\t\tip daddr 10.10.128.1 tcp dport 7441 ip saddr != { 10.10.128.1, 10.10.128.4 } counter packets 3 bytes 180 drop\n\t}\n}\n";
    assert!(nft_rule_listed(listing, "10.10.128.4", "10.10.128.1:7441").expect("読める"));
    assert!(!nft_rule_listed(listing, "10.10.128.5", "10.10.128.1:7441").expect("読める"));
    assert!(!nft_rule_listed(listing, "10.10.128.4", "10.10.128.1:7442").expect("読める"));
    assert!(!nft_rule_listed(listing, "10.10.128.40", "10.10.128.1:7441").expect("読める"));
    let accepting = listing.replace(" drop", " accept");
    assert!(!nft_rule_listed(&accepting, "10.10.128.4", "10.10.128.1:7441").expect("読める"));
    let without_self = listing.replace("{ 10.10.128.1, 10.10.128.4 }", "{ 10.10.128.4 }");
    assert!(!nft_rule_listed(&without_self, "10.10.128.4", "10.10.128.1:7441").expect("読める"));

    let mut options = system_options();
    options.firewall_allow = Some("10.10.128.4".to_string());
    options.firewall_backend = Some(FirewallBackend::Nft(PathBuf::from("/usr/sbin/nft")));
    assert_eq!(
        options.nft_rules_path(),
        PathBuf::from("/etc/systemd/system/uniqnode-serve.service.d").join(NFT_RULES_NAME)
    );
    let rendered = drop_ins(&options, "/usr/bin").expect("描ける");
    let serve = &rendered.iter().find(|(unit, _)| *unit == "uniqnode-serve.service").expect("serve").1;
    assert!(
        serve.contains(
            "ExecStartPre=+/usr/sbin/nft -f /etc/systemd/system/uniqnode-serve.service.d/agent-door.nft"
        ),
        "{serve}"
    );
    for (unit, content) in &rendered {
        if *unit != "uniqnode-serve.service" {
            assert!(!content.contains("ExecStartPre="), "{unit} には要らない: {content}");
        }
    }
    let mut ufw = system_options();
    ufw.firewall_allow = Some("10.10.128.4".to_string());
    ufw.firewall_backend = Some(FirewallBackend::Ufw);
    let rendered = drop_ins(&ufw, "/usr/bin").expect("描ける");
    assert!(
        rendered.iter().all(|(_, content)| !content.contains("ExecStartPre=")),
        "ufw の道では unit に足すものは無い"
    );
}
