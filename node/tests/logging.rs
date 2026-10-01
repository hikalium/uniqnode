//! 運用ログの統合テスト。実プロセスの serve を立て、本番の呼び手が組む要求
//! (POST /v1/search)を投げてから、データディレクトリに残った記録を読む
//! (should/0138)。ログの置き場・行の形・回転・上書きの手立て・開けないときの
//! 振る舞い(既定の道なら利用者の書ける場所へ倒し、明示された道なら倒さない)を、
//! どれも実際のファイルで確かめる。

mod common;
use common::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uniqnode::log::{
    default_path, generation_path, MCP_ROLE, RETAINED_GENERATIONS, SERVE_ROLE,
};

/// 劣化を必ず 1 行残す検索(埋め込みを装備していない節点に融合を求める)。本番の
/// 呼び手(MCP アダプタ・curl)が組むのと同じ POST /v1/search の本体である。
const DEGRADING_SEARCH: &[u8] = br#"{"query":"log rotation","method":"hybrid","top_k":5}"#;

fn search(address: &str) {
    let response = simple(address, "POST", "/v1/search", DEGRADING_SEARCH);
    assert_eq!(response.status, 200, "検索が 200 で答えていない: {}", body_text(&response));
}

/// 行頭が UTC の時刻と pid であることを見る。日付そのものは、真夜中をまたいでも
/// 判定が揺れないよう「今」と「2 分前」のどちらかを認める(should/0125: 判定が
/// 揺れるテストは無いより悪い)。
fn assert_stamped_now(line: &str) {
    let now = uniqnode::clock::unix_now();
    let today = uniqnode::clock::format_unix_time(now);
    let just_before = uniqnode::clock::format_unix_time(now - 120);
    assert!(
        line.starts_with(&today[..10]) || line.starts_with(&just_before[..10]),
        "行頭が今の UTC 日付({} か {})で始まっていない: {line}",
        &today[..10],
        &just_before[..10]
    );
    let stamp = line.chars().take(20).collect::<String>();
    assert!(
        stamp.ends_with('Z') && stamp.contains('T') && stamp.len() == 20,
        "行頭が 2026-08-17T04:05:06Z の形でない: {line}"
    );
    assert!(line[20..].starts_with(" [pid "), "時刻の後ろに pid が無い: {line}");
}

/// 何も指定しなくてもログはファイルに残る。中身は標準エラーと同じで、各行に UTC の
/// 時刻が付く。既定で残ることがこの機能の要点なので、引数を一つも与えずに見る。
#[test]
fn serve_saves_its_log_under_the_data_directory_by_default() {
    let mut server = start_server_capturing_stderr("log-default", &[]);
    search(&server.address);

    server.remove_dir_on_drop = false;
    let dir = server.dir.clone();
    let stderr = server.finish();

    let path = default_path(&dir, SERVE_ROLE);
    assert!(path.exists(), "既定でログファイルが作られていない: {}", path.display());
    let logged = std::fs::read_to_string(&path).expect("read log");
    assert!(
        logged.contains("uniqnode: serve:"),
        "起動の記録がログに無い: {logged}"
    );
    assert!(
        logged.contains("uniqnode: search:"),
        "検索の劣化がログに無い(走行中の診断が残っていない): {logged}"
    );
    for line in logged.lines() {
        assert_stamped_now(line);
    }
    // 前景で見た行がそのままファイルに残る(2 つの記録を突き合わせられる)。
    assert_eq!(
        logged, stderr,
        "ログファイルと標準エラーの内容が食い違う"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 上限を越えたら世代を送り、保持する数を越えた古い世代は消える。常駐する serve が
/// 際限なく太らないことを、小さな上限を注入して実際に回してから見る。
#[test]
fn the_log_rotates_by_size_and_keeps_a_bounded_number_of_generations() {
    // 注入する上限は 1 箇所から出す(引数と期待値で別々に書かない。must/0023 と同じ理由)。
    let limit: u64 = 400;
    let limit_text = limit.to_string();
    let mut server =
        start_server_capturing_stderr("log-rotate", &["--log-max-bytes", &limit_text]);
    let path = default_path(&server.dir, SERVE_ROLE);
    let first_line = {
        // 起動の記録は最初の世代に入る。回転が効いていれば、これは最後には消えている。
        // 起動の行だけで上限を越え、読む前に既に世代が送られていることがある(索引の
        // 温めの行は listening on の後に書かれる)ので、今ある一番古い世代の先頭を読む。
        let oldest = (1..=RETAINED_GENERATIONS)
            .rev()
            .map(|generation| generation_path(&path, generation))
            .find(|old| old.exists())
            .unwrap_or_else(|| path.clone());
        let logged = std::fs::read_to_string(&oldest).expect("read log");
        logged.lines().next().expect("起動の記録").to_string()
    };
    assert!(first_line.contains("ログを"), "起動の記録が読めない: {first_line}");

    for _ in 0..40 {
        search(&server.address);
    }
    server.remove_dir_on_drop = false;
    let dir = server.dir.clone();
    server.finish();

    assert!(path.exists(), "回転の後に現行のログが無い: {}", path.display());
    for generation in 1..=RETAINED_GENERATIONS {
        let old = generation_path(&path, generation);
        assert!(old.exists(), "{generation} 世代前が無い: {}", old.display());
    }
    let overflowed = generation_path(&path, RETAINED_GENERATIONS + 1);
    assert!(
        !overflowed.exists(),
        "保持する世代を越えたファイルが残っている: {}",
        overflowed.display()
    );

    let mut total = 0u64;
    for generation in 0..=RETAINED_GENERATIONS {
        let file = if generation == 0 { path.clone() } else { generation_path(&path, generation) };
        let size = std::fs::metadata(&file).expect("metadata").len();
        total += size;
        assert!(
            size <= limit,
            "{} が上限 {limit} バイトを越えた: {size}",
            file.display()
        );
        // 一番古い記録は捨てられている(回転が「消す」ところまで効いている)。
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(
            !text.contains(&first_line),
            "溢れたはずの起動の記録が {} に残っている",
            file.display()
        );
    }
    assert!(
        total <= limit * u64::from(RETAINED_GENERATIONS + 1),
        "ログ全体が上限 {} バイトを越えた: {total}",
        limit * u64::from(RETAINED_GENERATIONS + 1)
    );

    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 保存先は --log で変えられる(既定の道は使われない)。
#[test]
fn the_destination_can_be_moved_with_the_log_option() {
    let elsewhere = unique_dir("log-elsewhere").join("運用").join("serve.log");
    let mut server = start_server_capturing_stderr(
        "log-moved",
        &["--log", elsewhere.to_str().expect("utf-8")],
    );
    search(&server.address);
    server.remove_dir_on_drop = false;
    let dir = server.dir.clone();
    server.finish();

    let moved = std::fs::read_to_string(&elsewhere).expect("指定した道にログが無い");
    assert!(moved.contains("uniqnode: search:"), "移した先に診断が無い: {moved}");
    assert!(
        !default_path(&dir, SERVE_ROLE).exists(),
        "--log を与えたのに既定の道にも書いている"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup");
    std::fs::remove_dir_all(elsewhere.parent().expect("親").parent().expect("祖父"))
        .expect("cleanup");
}

/// --no-log を与えたときだけファイルに残さない。診断そのものは標準エラーに出続け、
/// 残さないことを起動時に言う(黙って記録を止めない)。
#[test]
fn no_log_keeps_the_diagnostics_on_stderr_only() {
    let mut server = start_server_capturing_stderr("log-off", &["--no-log"]);
    search(&server.address);
    server.remove_dir_on_drop = false;
    let dir = server.dir.clone();
    let stderr = server.finish();

    assert!(
        !default_path(&dir, SERVE_ROLE).exists(),
        "--no-log なのにログファイルがある: {}",
        default_path(&dir, SERVE_ROLE).display()
    );
    assert!(
        stderr.contains("uniqnode: search:"),
        "ファイルに残さない指定で診断まで消えている: {stderr}"
    );
    assert!(
        stderr.contains("--no-log"),
        "ファイルに残さないことを言っていない: {stderr}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 書けないディレクトリを作る(system 単位のストア、0750 で所有者 uniqnode、を人間の権限
/// から見た形)。chmod の戻り値ではなく、実際に下へ作れないことで書けなさを確かめる
/// (should/0116)。root では chmod が効かず書けてしまうので、そのときは飛ばして緑にせず
/// 前提を言って失敗する(検証したのか諦めたのかを結果から区別できなくしない。
/// docs/design/TESTING.md の外部の前提の規約)。
fn unwritable_dir(name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = unique_dir(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    if std::fs::create_dir(dir.join("probe")).is_ok() {
        make_removable(&dir);
        std::fs::remove_dir_all(&dir).expect("cleanup");
        panic!(
            "{} を 0500 にしても下に作れる(root で走っている)。このテストは書けない\
             データディレクトリを前提にするので、一般ユーザーで cargo test を走らせる",
            dir.display()
        );
    }
    dir
}

/// 書けなくしたディレクトリを消せるように戻す。
fn make_removable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).expect("chmod back");
}

/// 既定のログ先に書けないとき、利用者の書ける場所($XDG_STATE_HOME/uniqnode/logs/<役割>.log)
/// へ倒す。system 単位のストア(0750、所有者 uniqnode)に人間の権限で mcp を起こす、という
/// 本番の形そのもの: 転送する形の mcp はストアを開かないので、書けないディレクトリを指した
/// まま起こせる。標準エラーだけにすると LLM クライアントが吸って誰も読めないので、倒した
/// 事実と両方の道を最初の 1 行として、ファイルと標準エラーの双方に残す。
#[test]
fn an_unwritable_default_log_falls_back_to_the_users_state_directory() {
    let store = unwritable_dir("log-fallback-store");
    let state_home = unique_dir("log-fallback-state");

    // 標準入力を閉じたまま渡す(EOF は相手が閉じた合図で、起動の記録を残して終える)。
    // 転送先の serve は起動時には要らない(届かないことは要求が来たときに言う)。
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["mcp", store.to_str().expect("utf-8"), "--serve-url", "http://127.0.0.1:1"])
        .env("XDG_STATE_HOME", &state_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run mcp");
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(output.status.success(), "書けないログ先のために mcp が止まった: {stderr}");

    // 期待する倒れ先はリテラルで組む(検査対象の関数から導出しない。should/0137)。
    let fallen = state_home.join("uniqnode").join("logs").join("mcp.log");
    assert!(
        fallen.exists(),
        "倒れ先 {} にログが無い。標準エラー: {stderr}",
        fallen.display()
    );
    let logged = std::fs::read_to_string(&fallen).expect("read fallen log");
    let first = logged.lines().next().expect("倒れ先のログが空");
    assert_stamped_now(first);
    assert!(
        first.contains("既定のログ先") && first.contains("書けない"),
        "最初の行が倒した理由を言っていない: {first}"
    );
    let refused = default_path(&store, MCP_ROLE);
    assert!(
        first.contains(refused.to_str().expect("utf-8")),
        "最初の行に書けなかった既定の道 {} が無い: {first}",
        refused.display()
    );
    assert!(
        first.contains(fallen.to_str().expect("utf-8")),
        "最初の行に倒れ先 {} が無い: {first}",
        fallen.display()
    );
    // 続く起動の記録も、倒れ先を「残す先」として言う(既定の道を名乗らない)。
    let second = logged.lines().nth(1).expect("起動の記録が無い");
    assert!(
        second.contains("ログを") && second.contains(fallen.to_str().expect("utf-8")),
        "起動の記録が倒れ先を残す先として言っていない: {second}"
    );
    // 前景(標準エラー)にも同じ行がある(2 つの記録を突き合わせられる)。
    assert_eq!(logged, stderr, "倒れ先のファイルと標準エラーの内容が食い違う");

    make_removable(&store);
    std::fs::remove_dir_all(&store).expect("cleanup");
    std::fs::remove_dir_all(&state_home).expect("cleanup");
}

/// --log で明示された道を開けなくても serve は提供を続ける。ただし黙って落とさず、どの道
/// でなぜ失敗したかを標準エラーに言う(must/0022)。明示された道は倒さない: 利用者が指した
/// 道を黙って別の場所に変えると、指した所を読みに行った者が何も見つけられない。ログが
/// 書けないことは、答えられる要求を断る理由にはならない。
#[test]
fn an_unopenable_log_is_reported_and_serve_keeps_serving() {
    // 通常ファイルの下にはディレクトリを作れないので、確実に開けない道になる。
    let blocking_file = unique_dir("log-blocked");
    std::fs::create_dir_all(blocking_file.parent().expect("親")).expect("mkdir");
    std::fs::write(&blocking_file, b"file, not a directory").expect("write");
    let unopenable = blocking_file.join("serve.log");
    // 倒れ先になり得る場所を用意しておき、そこへ倒れないことを見る。
    let state_home = unique_dir("log-unopenable-state");

    let mut server = start_server_capturing_stderr_with_env(
        "log-unopenable",
        &[("XDG_STATE_HOME", state_home.to_str().expect("utf-8"))],
        &["--log", unopenable.to_str().expect("utf-8")],
    );
    // 起動している(「listening on」を読めた時点で束縛済み)。要求にも答える。
    let health = simple(&server.address, "GET", "/healthz", b"");
    assert_eq!(health.status, 200, "ログを開けないだけで提供が止まっている");
    search(&server.address);

    server.remove_dir_on_drop = false;
    let dir = server.dir.clone();
    let stderr = server.finish();

    assert!(
        stderr.contains("ログをファイルに残せない"),
        "ログを開けなかったことを黙っている: {stderr}"
    );
    assert!(
        stderr.contains(unopenable.to_str().expect("utf-8")),
        "どの道で失敗したかを言っていない: {stderr}"
    );
    assert!(
        stderr.contains("uniqnode: search:"),
        "ログを開けないと診断まで止まっている: {stderr}"
    );
    assert!(
        !default_path(&dir, SERVE_ROLE).exists(),
        "指定された道に失敗したのに既定の道へ勝手に書いている"
    );
    assert!(
        !state_home.exists(),
        "明示された道に失敗したのに {} へ倒している(倒すのは既定の道のときだけ)",
        state_home.display()
    );

    std::fs::remove_dir_all(&dir).expect("cleanup");
    std::fs::remove_file(&blocking_file).expect("cleanup");
}
