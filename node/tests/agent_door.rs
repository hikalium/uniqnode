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
/// `--agent-writable` を与えていないので、PUT は第 1 段と同じ「許可されていない」で断られる
/// (書く口の有無で本文が変わらないことは、この行の全文一致が見張る)。
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

/// 書く口(第 2 段): `--agent-writable notes` で notes への PUT が読み口を通って 200 になり、
/// query の `meta.<key>=<value>` が doc_rev.meta に写る(doc_rev は主の口で読む。読み口の
/// objects はチャンクしか通さない)。検索の citation と /citation に meta は出ない(citation の
/// 形は変えない)。許していないコレクションへの PUT は許した一覧を言う 403、fetch は許した
/// コレクションでも従来の 403、meta の形が違えば 400 で理由を言い、どれもストアに届かない。
/// 読み口の log 行は PUT でも同じ形。
///
/// should/0137: admit の PUT の腕を消すと最初の 200 が 403 に、put_document の extra_meta を
/// `&[]` に戻すと meta の全文一致が、handle の parse_meta_query の 400 を消すと 400 の段が
/// (200 になって)落ち、admit の集合の検査を消すと other の 403 の本文が落ちる。
#[test]
fn an_allowed_collection_takes_a_put_through_the_door_and_records_where_it_came_from() {
    let server = DoorServer::start(
        "door-write",
        &["--listen-agent", "127.0.0.1:0", "--agent-writable", "notes", "--agent-writable", "scratch"],
    );
    let main = server.main().to_string();
    let door = server.door();

    // lamalium が付ける形: ?meta.agent=<id>&meta.task=<id>(値は %XX で符号化)。
    let path = "/v1/collections/notes/documents/memo.md?meta.agent=agent-7&meta.task=task%2F42";
    let written = simple(&door, "PUT", path, "# memo\n\n読み口から書いた覚え書き\n".as_bytes());
    assert_eq!(written.status, 200, "{}", body_text(&written));
    let doc_rev = json_text_field(&body_text(&written), "doc_rev").expect("doc_rev");

    // doc_rev.meta に出所が写る(鍵は正規形の順。name・media は取り込みが決めたまま)。
    let object = simple(&main, "GET", &format!("/v1/objects/{doc_rev}"), b"");
    assert_eq!(object.status, 200, "{}", body_text(&object));
    let text = body_text(&object);
    assert!(
        text.contains(
            "\"meta\":{\"agent\":\"agent-7\",\"media\":\"markdown\",\"name\":\"memo\",\"task\":\"task/42\"}"
        ),
        "{text}"
    );
    // 読み口からは doc_rev は読めない(チャンク限定はそのまま)。
    assert_eq!(simple(&door, "GET", &format!("/v1/objects/{doc_rev}"), b"").status, 403);

    // 検索の citation と /citation に meta は出ない。
    let found = simple(&door, "POST", "/v1/search", "{\"query\":\"覚え書き\"}".as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    let found_text = body_text(&found);
    assert!(found_text.contains("\"document\":\"memo\""), "{found_text}");
    assert!(!found_text.contains("agent-7") && !found_text.contains("task/42"), "{found_text}");
    let chunk_id = json_text_field(&found_text, "id").expect("チャンク ID");
    let citation = simple(&door, "GET", &format!("/v1/objects/{chunk_id}/citation"), b"");
    assert_eq!(citation.status, 200, "{}", body_text(&citation));
    assert!(
        !body_text(&citation).contains("agent") && !body_text(&citation).contains("task"),
        "{}",
        body_text(&citation)
    );

    // 許していないコレクション: 許した一覧を言って 403。
    let other = simple(&door, "PUT", "/v1/collections/other/documents/memo.md", b"# other\n\nother\n");
    assert_eq!(other.status, 403, "{}", body_text(&other));
    assert_eq!(
        body_text(&other),
        "{\"error\":\"agent door: コレクション other は書けない(--agent-writable で許したのは notes, scratch)\"}"
    );
    // fetch は許したコレクションでも表に無い(コンテナが網に出る道)。
    let fetch = simple(&door, "POST", "/v1/collections/notes/fetch", b"{\"url\":\"http://127.0.0.1:1/\"}");
    assert_eq!(fetch.status, 403, "{}", body_text(&fetch));
    assert_eq!(
        body_text(&fetch),
        "{\"error\":\"agent door: POST /v1/collections/notes/fetch は許可されていない\"}"
    );

    // meta の形が違えば 400 で理由を言う(門は越えている: 本文は門の断りでない)。
    for (query, expected) in [
        (
            "meta.Agent=x",
            "{\"error\":\"meta の鍵 \\\"Agent\\\" は [a-z0-9_] の 1..=32 字\"}",
        ),
        (
            "agent=x",
            "{\"error\":\"query の鍵 \\\"agent\\\" は受け付けない(受け付けるのは meta.<key> だけ)\"}",
        ),
        ("meta.agent=", "{\"error\":\"meta.agent の値は 1..=200 字(与えられたのは 0 字)\"}"),
        ("meta.name=x", "{\"error\":\"meta.name は取り込みが決める鍵(query では与えられない)\"}"),
        (
            "meta.agent=%zz",
            "{\"error\":\"meta.agent の値 \\\"%zz\\\": 1 バイト目の % の後に 16 進 2 桁が無い\"}",
        ),
    ] {
        let bad_path = format!("/v1/collections/notes/documents/bad.md?{query}");
        let response = simple(&door, "PUT", &bad_path, b"# bad\n\nbad\n");
        assert_eq!(response.status, 400, "{query}: {}", body_text(&response));
        assert_eq!(body_text(&response), expected, "{query}");
    }

    // 断った PUT はどれもストアに届いていない: 文書は notes の memo 1 件だけ。
    let collections = simple(&main, "GET", "/v1/collections", b"");
    assert_eq!(
        body_text(&collections),
        "{\"collections\":[{\"documents\":1,\"name\":\"notes\"}]}"
    );

    // 読み口の log 行は PUT でも `agent <peer> PUT <path> <status> <ms>` のまま。
    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    let mark = format!(" {} 127.0.0.1:", uniqnode::agent_door::LOG_MARK);
    let put_line = log
        .lines()
        .find(|line| line.contains(&mark) && line.contains(" PUT "))
        .unwrap_or_else(|| panic!("PUT の行が無い: {log}"));
    assert!(put_line.contains(&format!(" PUT {path} 200 ")), "{put_line}");
    // 起動時に、読み口から書けるコレクションを 1 行で言う。
    assert!(log.contains("読み口から書けるコレクション: notes, scratch"), "{log}");
}

/// 読める集合(`--agent-collections`)の試験の資材。許していない側のコレクションに置く
/// 文書で、許した側と同じ語(世代の整合)を持つ: collection を省いた検索が、集合の外の
/// 件を落としていることを見るために要る。
const BORROWED_JA: &str =
    "# 第三者の頁\n\n## 世代の整合\n\n第三者の頁の写しにも世代の整合の話がある。\n";

/// 2 つのコレクション(notes と web)に別々の文書を入れ、それぞれの最初のチャンクの ID を
/// 返す。入れるのは主の口なので、読み口の許可表とは無関係に両方がストアにある。
fn seed_two_collections(main: &str) -> (String, String) {
    put_document(main, "notes", "search_ja.md", SEARCH_JA.as_bytes());
    put_document(main, "web", "borrowed_ja.md", BORROWED_JA.as_bytes());
    (chunk_in(main, "notes"), chunk_in(main, "web"))
}

/// そのコレクションの中で「世代の整合」に当たった最初のチャンクの ID(主の口で引く)。
fn chunk_in(address: &str, collection: &str) -> String {
    let body = format!("{{\"query\":\"世代の整合\",\"collection\":\"{collection}\"}}");
    let found = simple(address, "POST", "/v1/search", body.as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    json_text_field(&body_text(&found), "id")
        .unwrap_or_else(|| panic!("{collection} に当たりが無い: {}", body_text(&found)))
}

/// (a) 許したコレクションを指した検索は読み口を通って結果を返し、その件のチャンクと出典も
/// 読める。should/0137: admit の readable_scope の検査が集合の中まで断つように壊れると、
/// 最初の 200 が 403 になって落ちる。
#[test]
fn a_search_that_names_an_allowed_collection_answers_through_the_door() {
    let server = DoorServer::start(
        "door-read-allowed",
        &["--listen-agent", "127.0.0.1:0", "--agent-collections", "notes"],
    );
    let main = server.main().to_string();
    let door = server.door();
    let (notes_id, _web_id) = seed_two_collections(&main);

    let found = simple(
        &door,
        "POST",
        "/v1/search",
        "{\"query\":\"世代の整合\",\"collection\":\"notes\"}".as_bytes(),
    );
    assert_eq!(found.status, 200, "{}", body_text(&found));
    let text = body_text(&found);
    assert!(text.contains(&format!("\"id\":\"{notes_id}\"")), "{text}");
    assert!(text.contains("\"collection\":\"notes\""), "{text}");
    // 許したコレクションのチャンクは objects も citation も通る。
    assert_eq!(simple(&door, "GET", &format!("/v1/objects/{notes_id}"), b"").status, 200);
    let citation = simple(&door, "GET", &format!("/v1/objects/{notes_id}/citation"), b"");
    assert_eq!(citation.status, 200, "{}", body_text(&citation));
    assert!(body_text(&citation).contains("\"collection\":\"notes\""), "{}", body_text(&citation));
    // 起動時に、読み口から読めるコレクションを 1 行で言う。
    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    assert!(log.contains("読み口から読めるコレクション: notes"), "{log}");
}

/// (b) 許していないコレクションを指した検索は、索引を引く前に 403 で断られ、許した一覧を
/// 言う。should/0137: admit の readable_scope の検査を消すと 403 が 200 になって落ちる
/// (集合の外の件が返る)。
#[test]
fn a_search_that_names_a_collection_outside_the_set_is_refused_with_403() {
    let server = DoorServer::start(
        "door-read-refused",
        &["--listen-agent", "127.0.0.1:0", "--agent-collections", "notes", "--agent-collections", "papers"],
    );
    let main = server.main().to_string();
    let door = server.door();
    seed_two_collections(&main);

    let refused = simple(
        &door,
        "POST",
        "/v1/search",
        "{\"query\":\"世代の整合\",\"collection\":\"web\"}".as_bytes(),
    );
    assert_eq!(refused.status, 403, "{}", body_text(&refused));
    assert_eq!(
        body_text(&refused),
        "{\"error\":\"agent door: コレクション web は読めない(--agent-collections で許したのは notes, papers)\"}"
    );
    // 主の口には掛からない(同じ検索が通り、web の件が返る)。
    let through_main = simple(
        &main,
        "POST",
        "/v1/search",
        "{\"query\":\"世代の整合\",\"collection\":\"web\"}".as_bytes(),
    );
    assert_eq!(through_main.status, 200, "{}", body_text(&through_main));
    assert!(body_text(&through_main).contains("\"collection\":\"web\""), "{}", body_text(&through_main));
}

/// (c) collection を省いた検索の結果に、集合の外のコレクションの件は 1 つも無い(集合は
/// share として run_search の交差に載る)。一覧の口も許した分だけを返す。
/// should/0137: answer が Row::Search に readable_scope を渡すのをやめて api::handle に
/// 戻すと web の件が混ざって落ち、Row::Collections の腕を戻すと一覧に web が出て落ちる。
#[test]
fn a_search_without_a_collection_sees_only_the_allowed_ones() {
    let server = DoorServer::start(
        "door-read-scope",
        &["--listen-agent", "127.0.0.1:0", "--agent-collections", "notes"],
    );
    let main = server.main().to_string();
    let door = server.door();
    let (notes_id, web_id) = seed_two_collections(&main);

    // 主の口では両方の件が返る(資材が両方に入っていることの証拠)。
    let everything = simple(&main, "POST", "/v1/search", "{\"query\":\"世代の整合\"}".as_bytes());
    let everything = body_text(&everything);
    assert!(everything.contains(&format!("\"id\":\"{notes_id}\"")), "{everything}");
    assert!(everything.contains(&format!("\"id\":\"{web_id}\"")), "{everything}");

    let found = simple(&door, "POST", "/v1/search", "{\"query\":\"世代の整合\"}".as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    let text = body_text(&found);
    assert!(text.contains(&format!("\"id\":\"{notes_id}\"")), "{text}");
    assert!(!text.contains(&format!("\"id\":\"{web_id}\"")), "集合の外の件が返った: {text}");
    assert!(!text.contains("\"collection\":\"web\""), "集合の外の件が返った: {text}");

    // 一覧も許した分だけ(主の口は両方を数える)。
    let listed = simple(&door, "GET", "/v1/collections", b"");
    assert_eq!(listed.status, 200, "{}", body_text(&listed));
    assert_eq!(body_text(&listed), "{\"collections\":[{\"documents\":1,\"name\":\"notes\"}]}");
    let through_main = simple(&main, "GET", "/v1/collections", b"");
    assert_eq!(
        body_text(&through_main),
        "{\"collections\":[{\"documents\":1,\"name\":\"notes\"},{\"documents\":1,\"name\":\"web\"}]}"
    );
}

/// (d) 集合の外のコレクションのチャンクは、ID を知っていても読み口からは取れない
/// (objects も citation も 403)。同じ ID が主の口では 200 である。
/// should/0137: answer の source を引く腕(または screen の Object | Citation の腕)を
/// 消すと、この 403 が 200 になって落ちる。
#[test]
fn a_chunk_from_a_collection_outside_the_set_is_refused_even_by_id() {
    let server = DoorServer::start(
        "door-read-by-id",
        &["--listen-agent", "127.0.0.1:0", "--agent-collections", "notes"],
    );
    let main = server.main().to_string();
    let door = server.door();
    let (notes_id, web_id) = seed_two_collections(&main);

    for path in [format!("/v1/objects/{web_id}"), format!("/v1/objects/{web_id}/citation")] {
        let refused = simple(&door, "GET", &path, b"");
        assert_eq!(refused.status, 403, "{path}: {}", body_text(&refused));
        assert_eq!(
            body_text(&refused),
            "{\"error\":\"agent door: コレクション web は読めない(--agent-collections で許したのは notes)\"}",
            "{path}"
        );
        // 主の口は同じ ID を返す(門だけの制限であって、ストアの不在ではない)。
        assert_eq!(simple(&main, "GET", &path, b"").status, 200, "{path}");
    }
    // 許した側の同じ形の要求は通る(集合の判定であって、objects の口を閉じたのではない)。
    assert_eq!(simple(&door, "GET", &format!("/v1/objects/{notes_id}"), b"").status, 200);
    assert_eq!(simple(&door, "GET", &format!("/v1/objects/{notes_id}/citation"), b"").status, 200);
}

/// (e) `--agent-collections` を与えなければ、今までどおり全コレクションが読める(既定は
/// 変えない)。should/0137: readable_scope が空を All でなく Only(空) に直すと、この試験の
/// 検索が 0 件になって落ちる。
#[test]
fn without_the_flag_every_collection_is_readable() {
    let server = DoorServer::start("door-read-default", &["--listen-agent", "127.0.0.1:0"]);
    let main = server.main().to_string();
    let door = server.door();
    let (notes_id, web_id) = seed_two_collections(&main);

    let found = simple(&door, "POST", "/v1/search", "{\"query\":\"世代の整合\"}".as_bytes());
    assert_eq!(found.status, 200, "{}", body_text(&found));
    let text = body_text(&found);
    assert!(text.contains(&format!("\"id\":\"{notes_id}\"")), "{text}");
    assert!(text.contains(&format!("\"id\":\"{web_id}\"")), "{text}");
    let named = simple(
        &door,
        "POST",
        "/v1/search",
        "{\"query\":\"世代の整合\",\"collection\":\"web\"}".as_bytes(),
    );
    assert_eq!(named.status, 200, "{}", body_text(&named));
    assert_eq!(simple(&door, "GET", &format!("/v1/objects/{web_id}"), b"").status, 200);
    assert_eq!(simple(&door, "GET", &format!("/v1/objects/{web_id}/citation"), b"").status, 200);
    let listed = simple(&door, "GET", "/v1/collections", b"");
    assert_eq!(
        body_text(&listed),
        "{\"collections\":[{\"documents\":1,\"name\":\"notes\"},{\"documents\":1,\"name\":\"web\"}]}"
    );
}

/// `--agent-collections` も `--listen-agent` があるときだけ。無いのに与えれば serve は何も
/// 開かずに 2 で終わる。名前の形が違うときも同じ。
/// should/0137: parse_run_options の「--listen-agent が無い」の検査を消すと最初の段が
/// (serve が上がってしまい)落ちる。
#[test]
fn agent_collections_without_a_door_is_refused_with_exit_2() {
    let dir = unique_dir("door-collections-alone");
    let run = |args: &[&str]| -> (Option<i32>, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args(["serve", dir.to_str().expect("utf-8"), "127.0.0.1:0"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).to_string())
    };
    let (code, stderr) = run(&["--agent-collections", "notes"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("--agent-collections notes は --listen-agent があるときだけ"),
        "{stderr}"
    );
    assert!(!dir.exists(), "断るときはストアを作らない");
    let (code, stderr) = run(&["--listen-agent", "127.0.0.1:0", "--agent-collections", "a b"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("--agent-collections はコレクション名"), "{stderr}");
    assert!(!dir.exists(), "断るときはストアを作らない");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--agent-writable` は `--listen-agent` があるときだけ。無いのに与えれば serve は何も
/// 開かずに 2 で終わる(黙って捨てると「許したつもり」が残る)。名前の形が違うときも同じ。
/// should/0137: parse_run_options の「--listen-agent が無い」の検査を消すと最初の段が
/// (serve が上がってしまい)落ちる。
#[test]
fn agent_writable_without_a_door_is_refused_with_exit_2() {
    let dir = unique_dir("door-writable-alone");
    let run = |args: &[&str]| -> (Option<i32>, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args(["serve", dir.to_str().expect("utf-8"), "127.0.0.1:0"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).to_string())
    };
    let (code, stderr) = run(&["--agent-writable", "notes"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("--agent-writable notes は --listen-agent があるときだけ"),
        "{stderr}"
    );
    assert!(!dir.exists(), "断るときはストアを作らない");
    let (code, stderr) = run(&["--listen-agent", "127.0.0.1:0", "--agent-writable", "a/b"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("--agent-writable はコレクション名"), "{stderr}");
    assert!(!dir.exists(), "断るときはストアを作らない");
    let _ = std::fs::remove_dir_all(&dir);
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

/// 記録の行から、読み口の要求 1 本ぶんの欄を切り出す(`<time> [pid N] agent <peer> …` の
/// peer より後ろ)。
fn agent_line_fields(log: &str, wanted: &str) -> Vec<String> {
    let mark = format!(" {} 127.0.0.1:", uniqnode::agent_door::LOG_MARK);
    let line = log
        .lines()
        .filter(|line| line.contains(&mark))
        .find(|line| line.contains(wanted))
        .unwrap_or_else(|| panic!("{wanted} の行が無い: {log}"));
    let rest = line.split(&mark).nth(1).expect("mark");
    rest.split(' ').map(|field| field.to_string()).collect()
}

/// 要求ヘッダ X-Uniqnode-Task と X-Uniqnode-Agent は記録の行の末尾に写る(通した要求にも
/// 断った要求にも)。判断には入らない: 同じ要求は、ヘッダの有無によらず同じ status で答える。
/// should/0137: handle の log_line! を元の書式({recorded} 抜き)に戻すと、欄が 5 つに
/// なってこの試験が落ちる(実験した)。
#[test]
fn the_task_and_agent_headers_ride_at_the_end_of_the_recorded_line() {
    let server = DoorServer::start("door-task-header", &["--listen-agent", "127.0.0.1:0"]);
    let door = server.door();
    let asking = [("X-Uniqnode-Task", "tasks/chat"), ("X-Uniqnode-Agent", "worker-3")];

    let health = with_headers(&door, "GET", "/healthz", &asking, b"");
    assert_eq!((health.status, body_text(&health).as_str()), (200, "ok\n"), "判断には入らない");
    // 断った要求にも同じ末尾が付く(誰が何を試したかを記録から追える)。
    let refused = with_headers(&door, "POST", "/v1/admin/gc", &asking, b"");
    assert_eq!(refused.status, 403, "{}", body_text(&refused));

    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    for wanted in [" GET /healthz 200 ", " POST /v1/admin/gc 403 "] {
        let fields = agent_line_fields(&log, wanted);
        assert_eq!(fields.len(), 7, "{wanted}: {fields:?}");
        assert_eq!(
            (fields[5].as_str(), fields[6].as_str()),
            ("task=tasks/chat", "agent=worker-3"),
            "{wanted}: {fields:?}"
        );
    }
    // 片方だけでも同じ(在るものだけを足す)。
    assert_eq!(
        with_headers(&door, "GET", "/v1/status", &[("X-Uniqnode-Agent", "worker-3")], b"").status,
        200
    );
    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    let fields = agent_line_fields(&log, " GET /v1/status 200 ");
    assert_eq!(fields.len(), 6, "{fields:?}");
    assert_eq!(fields[5], "agent=worker-3", "{fields:?}");
}

/// ヘッダが無いのは正常で、記録の行は今までどおりの 5 欄のままである(足すものが無い)。
/// should/0137: recorded_tail が無いヘッダを飛ばさず既定値(`-` など)で埋めるようにすると、
/// 欄が 7 つになってこの assert が落ちる(実験した)。
#[test]
fn a_request_without_the_headers_leaves_the_line_it_always_left() {
    let server = DoorServer::start("door-task-absent", &["--listen-agent", "127.0.0.1:0"]);
    let door = server.door();
    assert_eq!(simple(&door, "GET", "/healthz", b"").status, 200);

    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    let fields = agent_line_fields(&log, " GET /healthz 200 ");
    assert_eq!(fields.len(), 5, "{fields:?}");
    assert!(fields[4].parse::<u64>().is_ok(), "ms が整数でない: {fields:?}");
    assert!(!log.contains(" task=") && !log.contains(" agent="), "{log}");
}

/// 字種の外の値は 400 で理由を言い、要求は api::handle に届かない(何も起きない)。
/// should/0137: handle が recorded_tail の Err を捨てて admit へ進むようにすると、PUT が
/// 200 で通ってしまい「一覧が空」の assert が落ちる(実験した)。
#[test]
fn a_task_name_outside_the_alphabet_is_refused_before_the_table() {
    let server = DoorServer::start(
        "door-task-bad",
        &["--listen-agent", "127.0.0.1:0", "--agent-writable", "notes"],
    );
    let main = server.main().to_string();
    let door = server.door();

    let path = "/v1/collections/notes/documents/memo.md";
    let refused = with_headers(
        &door,
        "PUT",
        path,
        &[("X-Uniqnode-Task", "tasks chat")],
        "# memo\n\n読み口から書いた覚え書き\n".as_bytes(),
    );
    assert_eq!(refused.status, 400, "{}", body_text(&refused));
    assert_eq!(
        body_text(&refused),
        "{\"error\":\"agent door: X-Uniqnode-Task の値は [A-Za-z0-9_.:/-] の 1..=64 字\"}"
    );
    // api::handle に届いていない: ストアには何も入っていない。
    let collections = simple(&main, "GET", "/v1/collections", b"");
    assert_eq!(body_text(&collections), "{\"collections\":[]}");

    // 通るはずの要求も、同じ理由で断られる(許可表より前に見る)。長すぎる値も空の値も同じ。
    for (name, value) in [
        ("X-Uniqnode-Agent", "a".repeat(65)),
        ("X-Uniqnode-Agent", String::new()),
    ] {
        let health = with_headers(&door, "GET", "/healthz", &[(name, &value)], b"");
        assert_eq!(health.status, 400, "{name}={value:?}: {}", body_text(&health));
        assert_eq!(
            body_text(&health),
            "{\"error\":\"agent door: X-Uniqnode-Agent の値は [A-Za-z0-9_.:/-] の 1..=64 字\"}",
            "{name}={value:?}"
        );
    }
    // 断った 1 本も記録には残る(末尾は付かない: 書けない値だから断った)。
    let log = std::fs::read_to_string(server.server.dir.join("logs").join("serve.log")).expect("serve.log");
    let fields = agent_line_fields(&log, &format!(" PUT {path} 400 "));
    assert_eq!(fields.len(), 5, "{fields:?}");
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
