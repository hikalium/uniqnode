//! serve の読み口(agent door)の統合テスト
//! (AGENT_DOOR (uuid:02f79aec-2f12-41e6-bede-1557d4719e4d))。
//!
//! 実プロセスの `uniqnode serve <dir> 127.0.0.1:0 --listen-agent 127.0.0.1:0` を起こし、
//! 標準出力の 2 本の `listening on` から主の口と読み口のアドレスを読み、どちらにも curl が
//! 組み立てる形の生 HTTP/1.1 で当てる(should/0138)。
//!
//! should/0137(欠陥を戻して証明する)のために、各試験の冒頭に「許可表のどの行を消すと
//! この試験のどの assert が落ちるか」を記す。許可表は node/src/agent_door.rs の admit の
//! match である。

mod common;
use common::*;

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 日本語文書(検索の統合テストと共用の資材。世代の整合の節が識別しやすい語を持つ)。
const SEARCH_JA: &str = include_str!("assets/search_ja.md");

/// 読み口つきで起こした serve。標準出力は裏のスレッドが読み続ける(読み口の
/// `listening on` は主の口の後、再試行のときは何秒も後に出るので、1 行読んで捨てる
/// 共通ヘルパでは受けられない。捨てると serve 側の標準出力が閉じ、行を書けなくなる)。
struct DoorServer {
    server: Server,
    /// 標準出力に出た行の控え(裏のスレッドが積む)。
    stdout_lines: Arc<Mutex<Vec<String>>>,
}

impl DoorServer {
    fn start(name: &str, extra_args: &[&str]) -> DoorServer {
        let dir = unique_dir(name);
        let mut child = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args(["serve", dir.to_str().expect("utf-8"), "127.0.0.1:0"])
            .args(extra_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn serve");
        let stdout = child.stdout.take().expect("stdout");
        let stdout_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let lines = stdout_lines.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    lines.lock().expect("lock").push(line);
                }
            });
        }
        let address = wait_for_line(&stdout_lines, |line| {
            line.starts_with("listening on ")
                && !line.ends_with(uniqnode::agent_door::LISTENING_LINE_SUFFIX)
        })
        .unwrap_or_else(|| panic!("主の口の listening on が出ない: {:?}", lines_of(&stdout_lines)))
        .strip_prefix("listening on ")
        .expect("listening line")
        .to_string();
        let server = Server { child, address, dir, remove_dir_on_drop: true, stderr: None };
        DoorServer { server, stdout_lines }
    }

    /// 主の口のアドレス。
    fn main(&self) -> &str {
        &self.server.address
    }

    /// 読み口のアドレス(標準出力の `listening on <addr> (agent door)` から)。まだ出て
    /// いなければ、待ち時間の分だけ待つ。
    fn door_within(&self, wait: Duration) -> Option<String> {
        wait_for_line_within(&self.stdout_lines, wait, |line| {
            line.starts_with("listening on ")
                && line.ends_with(uniqnode::agent_door::LISTENING_LINE_SUFFIX)
        })
        .map(|line| {
            line.strip_prefix("listening on ")
                .and_then(|rest| rest.strip_suffix(uniqnode::agent_door::LISTENING_LINE_SUFFIX))
                .expect("agent door line")
                .to_string()
        })
    }

    /// 読み口のアドレス。束縛は起動直後に済むはずなので、長くは待たない。
    fn door(&self) -> String {
        self.door_within(Duration::from_secs(10))
            .unwrap_or_else(|| panic!("読み口の listening on が出ない: {:?}", lines_of(&self.stdout_lines)))
    }

    /// 正常終了させ、標準出力に出た全行を返す(終了後なので、もう増えない)。
    fn finish(mut self) -> Vec<String> {
        self.server.remove_dir_on_drop = true;
        let address = self.server.address.clone();
        let _ = simple(&address, "POST", "/v1/admin/shutdown", b"");
        let _ = self.server.child.wait();
        // 読み手のスレッドが EOF まで積み終えるのを待つ(プロセスは終わっている)。
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            if Arc::strong_count(&self.stdout_lines) == 1 {
                break;
            }
        }
        lines_of(&self.stdout_lines)
    }
}

fn lines_of(lines: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    lines.lock().expect("lock").clone()
}

fn wait_for_line(lines: &Arc<Mutex<Vec<String>>>, wanted: impl Fn(&str) -> bool) -> Option<String> {
    wait_for_line_within(lines, Duration::from_secs(30), wanted)
}

fn wait_for_line_within(
    lines: &Arc<Mutex<Vec<String>>>,
    wait: Duration,
    wanted: impl Fn(&str) -> bool,
) -> Option<String> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(line) = lines.lock().expect("lock").iter().find(|line| wanted(line)) {
            return Some(line.clone());
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn put_document(address: &str, collection: &str, name: &str, body: &[u8]) {
    let path = format!("/v1/collections/{collection}/documents/{name}");
    let response = simple(address, "PUT", &path, body);
    assert_eq!(response.status, 200, "{}", body_text(&response));
}

/// 主の口に文書を入れ、検索で最初のチャンクの ID を得る(読み口の objects・citation の
/// 的にする)。
fn seed_a_chunk(main: &str) -> String {
    put_document(main, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    let found = simple(main, "POST", "/v1/search", "{\"query\":\"世代の整合\"}".as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    json_text_field(&body_text(&found), "id").expect("チャンク ID")
}

/// 許可表の各行が読み口を通る。行を消したときに落ちる assert(should/0137):
/// - `GET /healthz` を消す → healthz の 200。
/// - `GET /v1/status` を消す → status の 200(node_id の比較まで届かない)。
/// - `POST /v1/search` を消す → search の 200。
/// - `GET /v1/objects/{id}` を消す → objects の 200。
/// - `GET /v1/objects/{id}/citation` を消す → citation の 200。
/// - `GET /v1/collections` を消す → collections の「403 でない」。
#[test]
fn every_row_of_the_table_passes_through_the_door() {
    let server = DoorServer::start("door-rows", &["--listen-agent", "127.0.0.1:0"]);
    let main = server.main().to_string();
    let door = server.door();
    assert_ne!(main, door, "読み口は主の口とは別の口である");
    let chunk_id = seed_a_chunk(&main);

    let health = simple(&door, "GET", "/healthz", b"");
    assert_eq!((health.status, body_text(&health).as_str()), (200, "ok\n"));

    let status = simple(&door, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200, "{}", body_text(&status));
    let door_node = json_text_field(&body_text(&status), "node_id").expect("node_id");
    let main_status = simple(&main, "GET", "/v1/status", b"");
    let main_node = json_text_field(&body_text(&main_status), "node_id").expect("node_id");
    assert_eq!(door_node, main_node, "読み口は主の口と同じストアを見せる");

    let found = simple(&door, "POST", "/v1/search", "{\"query\":\"世代の整合\"}".as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    assert!(body_text(&found).contains(&format!("\"id\":\"{chunk_id}\"")), "{}", body_text(&found));

    let object = simple(&door, "GET", &format!("/v1/objects/{chunk_id}"), b"");
    assert_eq!(object.status, 200, "{}", body_text(&object));
    let chunk = body_text(&object);
    assert!(chunk.contains("\"kind\":\"chunk\""), "{chunk}");
    assert!(chunk.contains("世代の整合はオブジェクト数と署名者ごとの最終列番号"), "{chunk}");

    let citation = simple(&door, "GET", &format!("/v1/objects/{chunk_id}/citation"), b"");
    assert_eq!(citation.status, 200, "{}", body_text(&citation));
    assert!(body_text(&citation).contains("\"document\":\"search_ja\""), "{}", body_text(&citation));

    // GET /v1/collections は許可表に載っている(実装は api.rs 側で別に進む)。ここで
    // 見るのは「門が断らない」ことだけ: 403 でなく、本文が門の断りでない。api.rs に
    // 口が無ければ 404、あれば 200 で、どちらも門を通った証拠である。
    let collections = simple(&door, "GET", "/v1/collections", b"");
    assert!(
        collections.status == 200 || collections.status == 404,
        "門を通っていない: {} {}",
        collections.status,
        body_text(&collections)
    );
    assert!(
        !body_text(&collections).contains(uniqnode::agent_door::ERROR_PREFIX),
        "門が断った: {}",
        body_text(&collections)
    );
}

/// 表に無い要求は method と path を言って 403 で断り、ストアには何も起きない。
/// 許可表に行を足す(例: `POST /v1/admin/gc`)と、その行の 403 の assert が落ちる。
#[test]
fn everything_outside_the_table_is_refused_with_403() {
    let server = DoorServer::start("door-refuse", &["--listen-agent", "127.0.0.1:0"]);
    let main = server.main().to_string();
    let door = server.door();
    let chunk_id = seed_a_chunk(&main);

    let refused: Vec<(&str, String, &[u8])> = vec![
        ("POST", "/v1/admin/gc".to_string(), b""),
        ("POST", "/v1/admin/shutdown".to_string(), b""),
        ("POST", "/v1/sync".to_string(), b"{}"),
        ("GET", "/v1/pins".to_string(), b""),
        ("POST", "/v1/pins".to_string(), b"{\"root\":\"x\",\"min_replicas\":1}"),
        ("GET", "/v1/peers".to_string(), b""),
        ("GET", "/v1/refs".to_string(), b""),
        ("POST", "/v1/collections/notes/fetch".to_string(), b"{\"url\":\"http://127.0.0.1:1/\"}"),
        ("PUT", "/v1/collections/notes/documents/door.md".to_string(), b"# door\n\ndoor test\n"),
        ("POST", "/v1/objects".to_string(), b"\"leaf\""),
        ("GET", format!("/v1/objects/{chunk_id}/rendition"), b""),
        ("GET", format!("/v1/objects/{chunk_id}/rendition/source"), b""),
        ("GET", format!("/v1/objects/{chunk_id}/referrers"), b""),
        ("GET", format!("/v1/closure/{chunk_id}"), b""),
        ("POST", "/v1/query".to_string(), b"{}"),
        ("GET", "/v1/objects/not-an-id".to_string(), b""),
        ("DELETE", "/v1/status".to_string(), b""),
    ];
    for (method, path, body) in refused {
        let response = simple(&door, method, &path, body);
        assert_eq!(response.status, 403, "{method} {path}: {}", body_text(&response));
        assert_eq!(
            body_text(&response),
            format!("{{\"error\":\"agent door: {method} {path} は許可されていない\"}}"),
            "{method} {path}"
        );
    }
    // 断った PUT はストアに届いていない: 主の口の検索に door.md は無い。
    let found = simple(&main, "POST", "/v1/search", b"{\"query\":\"door\"}");
    assert_eq!(found.status, 200);
    assert!(!body_text(&found).contains("\"document\":\"door\""), "{}", body_text(&found));
    // 断った shutdown は効いていない: 主の口はまだ答える。
    assert_eq!(simple(&main, "GET", "/healthz", b"").status, 200);
}

/// peers を名指しした search は 400 で断る(ストアに届かない)。admit の peers の検査を
/// 消すと、この 400 の assert が落ちる(主の口の解釈で 200 か 400 の別の本文になる)。
#[test]
fn a_search_that_names_peers_is_refused_with_400() {
    let server = DoorServer::start("door-peers", &["--listen-agent", "127.0.0.1:0"]);
    let door = server.door();
    for body in [
        "{\"query\":\"x\",\"peers\":true}",
        "{\"query\":\"x\",\"peers\":[\"127.0.0.1:1\"]}",
        "{\"query\":\"x\",\"peers\":false}",
    ] {
        let response = simple(&door, "POST", "/v1/search", body.as_bytes());
        assert_eq!(response.status, 400, "{body}: {}", body_text(&response));
        assert_eq!(body_text(&response), "{\"error\":\"agent door: peers は使えない\"}", "{body}");
    }
    // peers を書かなければ通る(主の口と同じ答え)。
    let plain = simple(&door, "POST", "/v1/search", b"{\"query\":\"x\"}");
    assert_eq!(plain.status, 200, "{}", body_text(&plain));
}

/// ID の形が正しくても、チャンクでないオブジェクトは読み口からは取れない。screen の
/// 差し替えを消すと blob と c1 オブジェクトの 403 の assert が落ちる(200 で中身が出る)。
#[test]
fn a_blob_is_refused_even_though_its_id_is_valid() {
    let server = DoorServer::start("door-blob", &["--listen-agent", "127.0.0.1:0"]);
    let main = server.main().to_string();
    let door = server.door();

    // 主の口から入れる: テキストでないバイト列(PDF の原文に相当)と、チャンクでない
    // c1 オブジェクト。
    let blob_id = put_object(&main, &[0xff, 0xfe, 0x00, b'%', b'P', b'D', b'F']);
    let note_id = put_object(&main, b"{\"kind\":\"note\",\"v\":1}");
    for id in [&blob_id, &note_id] {
        let through_main = simple(&main, "GET", &format!("/v1/objects/{id}"), b"");
        assert_eq!(through_main.status, 200, "主の口は制限しない");
        let through_door = simple(&door, "GET", &format!("/v1/objects/{id}"), b"");
        // 中身が漏れたときは UTF-8 でないので、失敗の文は lossy で組む(status の assert
        // 自体が読めるように)。
        assert_eq!(
            through_door.status,
            403,
            "{id}: {}",
            String::from_utf8_lossy(&through_door.body)
        );
        assert_eq!(
            body_text(&through_door),
            "{\"error\":\"agent door: チャンクでないオブジェクトは許可されていない\"}"
        );
    }
    // 持っていない ID は主の口と同じ 404(門は在否を隠さない。不在の言明でもない)。
    let missing = format!("s256:{}", "0".repeat(64));
    let response = simple(&door, "GET", &format!("/v1/objects/{missing}"), b"");
    assert_eq!(response.status, 404, "{}", body_text(&response));
}

/// 読み口は指定したアドレスにしか束縛しない。127.0.0.2 に束縛した読み口の同じポートに
/// 127.0.0.1 から届かない。127.0.0.2 に束縛できない環境では、主の口には許可表が掛かって
/// いない(表に無い GET /v1/refs が主の口では通る)ことで代える。
#[test]
fn the_door_binds_only_to_the_address_it_was_given() {
    let second_loopback_usable = TcpListener::bind("127.0.0.2:0").is_ok();
    if !second_loopback_usable {
        eprintln!("127.0.0.2 に束縛できない環境なので、主の口に許可表が掛からないことで代える");
        let server = DoorServer::start("door-bind-alt", &["--listen-agent", "127.0.0.1:0"]);
        let refs = simple(server.main(), "GET", "/v1/refs", b"");
        assert_eq!(refs.status, 200, "{}", body_text(&refs));
        assert_eq!(simple(&server.door(), "GET", "/v1/refs", b"").status, 403);
        return;
    }
    let server = DoorServer::start("door-bind", &["--listen-agent", "127.0.0.2:0"]);
    let door = server.door();
    let port = door.rsplit(':').next().expect("port");
    assert!(door.starts_with("127.0.0.2:"), "{door}");
    assert_eq!(simple(&door, "GET", "/healthz", b"").status, 200);
    let elsewhere = TcpStream::connect(format!("127.0.0.1:{port}"));
    assert!(elsewhere.is_err(), "127.0.0.1 の同じポートにつながってはいけない: {door}");
    // 主の口には許可表が掛からない。
    let refs = simple(server.main(), "GET", "/v1/refs", b"");
    assert_eq!(refs.status, 200, "{}", body_text(&refs));
}

/// --listen-agent を与えなければ第 2 の口は存在しない: 標準出力に読み口の listening on は
/// 出ず、主の口には許可表が掛からない。
#[test]
fn without_the_flag_there_is_no_second_door() {
    let server = DoorServer::start("door-none", &[]);
    let refs = simple(server.main(), "GET", "/v1/refs", b"");
    assert_eq!(refs.status, 200, "{}", body_text(&refs));
    let lines = server.finish();
    assert_eq!(lines.len(), 1, "標準出力は主の口の 1 行だけ: {lines:?}");
    assert!(lines[0].starts_with("listening on 127.0.0.1:"), "{lines:?}");
    assert!(!lines[0].ends_with(uniqnode::agent_door::LISTENING_LINE_SUFFIX), "{lines:?}");
}

/// 読み口を束縛できなくても主の口は上がる。塞がれたポートを指して起こすと、主の口は
/// 従来どおり答え、失敗は serve.log に残り、ポートが空けば再試行で束縛して listening on
/// を出す。再試行を消すと最後の「空いた後に出る」assert が落ち、失敗で exit するように
/// 戻すと主の口の listening on 自体が出なくなる。
#[test]
fn a_door_that_cannot_bind_does_not_stop_the_main_door() {
    let blocker = TcpListener::bind("127.0.0.1:0").expect("bind blocker");
    let blocked = blocker.local_addr().expect("addr").to_string();
    let server = DoorServer::start("door-bind-fail", &["--listen-agent", &blocked]);
    let health = simple(server.main(), "GET", "/healthz", b"");
    assert_eq!(health.status, 200, "主の口は上がっている");
    assert!(
        server.door_within(Duration::from_millis(500)).is_none(),
        "塞がれたポートに束縛できてしまった: {:?}",
        lines_of(&server.stdout_lines)
    );
    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    assert!(
        log.contains(&format!("読み口 {blocked} に束縛できない")),
        "束縛の失敗が記録されていない: {log}"
    );
    // 塞ぎを外すと、次の再試行で束縛できる。
    drop(blocker);
    let wait = uniqnode::agent_door::BIND_RETRY_INTERVAL * 3;
    let door = server
        .door_within(wait)
        .unwrap_or_else(|| panic!("{wait:?} 待っても読み口が上がらない: {:?}", lines_of(&server.stdout_lines)));
    assert_eq!(door, blocked);
    assert_eq!(simple(&door, "GET", "/healthz", b"").status, 200);
}

/// 読み口への要求は serve.log に `agent <peer addr> <METHOD> <path> <status> <ms>` の
/// 1 行で残る(通した要求も断った要求も)。handle の log_line! を消すとこの試験が落ちる。
#[test]
fn every_request_through_the_door_leaves_one_line_in_the_log() {
    let server = DoorServer::start("door-log", &["--listen-agent", "127.0.0.1:0"]);
    let door = server.door();
    assert_eq!(simple(&door, "GET", "/healthz", b"").status, 200);
    assert_eq!(simple(&door, "POST", "/v1/admin/gc", b"").status, 403);
    // 主の口への要求は agent の行にならない。
    assert_eq!(simple(server.main(), "GET", "/v1/status", b"").status, 200);

    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    let mark = format!(" {} 127.0.0.1:", uniqnode::agent_door::LOG_MARK);
    let agent_lines: Vec<&str> = log.lines().filter(|line| line.contains(&mark)).collect();
    assert_eq!(agent_lines.len(), 2, "読み口の要求 2 本ぶんの行: {log}");
    let fields = |line: &str| -> Vec<String> {
        // `<time> [pid N] agent <peer> <METHOD> <path> <status> <ms>`
        let rest = line.split(&mark).nth(1).expect("mark");
        rest.split(' ').map(|field| field.to_string()).collect()
    };
    let first = fields(agent_lines[0]);
    assert_eq!(
        (first[1].as_str(), first[2].as_str(), first[3].as_str()),
        ("GET", "/healthz", "200"),
        "{first:?}"
    );
    assert!(first[4].parse::<u64>().is_ok(), "ms が整数でない: {first:?}");
    let second = fields(agent_lines[1]);
    assert_eq!(
        (second[1].as_str(), second[2].as_str(), second[3].as_str()),
        ("POST", "/v1/admin/gc", "403"),
        "{second:?}"
    );
}

/// mcp と viewer は読み口を持たないので、--listen-agent を黙って捨てずに断る。
#[test]
fn mcp_and_viewer_refuse_the_flag_they_cannot_honour() {
    for role in ["mcp", "viewer"] {
        let dir = unique_dir(&format!("door-misplaced-{role}"));
        let mut args = vec![role, dir.to_str().expect("utf-8")];
        if role == "viewer" {
            args.push("127.0.0.1:0");
        }
        args.extend(["--listen-agent", "127.0.0.1:0"]);
        let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{role}: {stderr}");
        assert!(stderr.contains("--listen-agent は"), "{role}: {stderr}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
