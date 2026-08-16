//! MCP アダプタの統合テスト(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9))。
//!
//! 実プロセスとして `uniqnode mcp <dir>` を起こし、標準入力に JSON-RPC の要求を書いて
//! 標準出力から応答を読む。エージェント(Claude Code)がこのサーバに話しかける経路
//! そのものであり、要求は Claude Code が実際に送る形で組む(should/0138)。
//!
//! 標準出力の純度も検査する: 1 行 1 メッセージで、JSON でない行が 1 行でも混ざれば
//! 相手の解析はそこで壊れる。ログが標準エラーへ出ていることは、標準エラー側に起動の
//! 知らせがあることで確かめる。

mod common;
use common::*;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

/// テスト資材の最小 PDF(3 ページ。取り込みは CLI に渡すのでファイルとして書き出す。
/// markdown の資材は tests/assets/ のパスをそのまま CLI へ渡す。should/0112)。
const THREE_PAGE_PDF: &[u8] = include_bytes!("assets/three_pages.pdf");

/// 子プロセスとして動く MCP サーバと、その標準入出力。
struct McpProcess {
    child: Child,
    /// 終了させるときに落とす(標準入力の EOF が終了の合図)。
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
    /// 受け取った行の控え(標準出力の純度をまとめて検査するため)。
    received: Vec<String>,
}

impl McpProcess {
    fn start(dir: &Path, arguments: &[&str]) -> McpProcess {
        let mut child = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args(["mcp", dir.to_str().expect("utf-8")])
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let stderr = child.stderr.take().expect("stderr");
        McpProcess { child, stdin: Some(stdin), stdout, stderr: Some(stderr), received: Vec::new() }
    }

    /// 1 本書く(応答は読まない)。通知に使う。
    fn send(&mut self, message: &str) {
        let stdin = self.stdin.as_mut().expect("stdin は開いている");
        stdin.write_all(message.as_bytes()).expect("write request");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush");
    }

    /// 1 本書いて 1 行読む。
    fn request(&mut self, message: &str) -> String {
        self.send(message);
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read response");
        assert!(read > 0, "要求 {message} への応答が無いまま標準出力が閉じた");
        let line = line.trim_end_matches('\n').to_string();
        self.received.push(line.clone());
        line
    }

    /// tools/call を 1 本(引数は JSON のまま渡す)。
    fn call(&mut self, id: u32, tool: &str, arguments: &str) -> String {
        self.request(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"{tool}\",\"arguments\":{arguments}}}}}"
        ))
    }

    /// 標準入力を閉じて終わらせ、標準出力の残りと標準エラーを返す。
    fn finish(mut self) -> McpExit {
        drop(self.stdin.take());
        let status = self.child.wait().expect("wait");
        let mut trailing = String::new();
        self.stdout.read_to_string(&mut trailing).expect("read trailing stdout");
        let mut stderr = String::new();
        self.stderr.as_mut().expect("stderr").read_to_string(&mut stderr).expect("read stderr");
        // 標準出力に流れてよいのは JSON-RPC のメッセージだけである(stdio 転送の規定)。
        // ログが 1 行でも混ざれば、相手の解析はその行で壊れる。
        for line in &self.received {
            let parsed = uniqnode::c1::parse(line)
                .unwrap_or_else(|e| panic!("標準出力に JSON でない行が混ざっている: {line} ({e})"));
            let uniqnode::c1::Value::Object(map) = &parsed else {
                panic!("標準出力の行がオブジェクトでない: {line}");
            };
            assert_eq!(
                map.get("jsonrpc"),
                Some(&uniqnode::c1::Value::Text("2.0".to_string())),
                "標準出力の行が JSON-RPC のメッセージでない: {line}"
            );
        }
        assert!(
            trailing.is_empty(),
            "応答のほかに標準出力へ書かれたものがある: {trailing}"
        );
        assert!(status.success(), "mcp が異常終了した: {status}");
        McpExit { stderr }
    }
}

struct McpExit {
    stderr: String,
}

/// CLI を 1 回動かす(取り込みは serve/mcp を止めた状態のストアの経路。ストアは二重に
/// 開けないので、取り込みは MCP を起こす前に済ませる)。
fn run_cli(arguments: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .output()
        .expect("uniqnode の起動");
    assert!(
        output.status.success(),
        "uniqnode {arguments:?} が失敗した: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

/// 資材を 1 件、コレクション notes へ取り込んだ新しいストアを作る。
fn store_with(name: &str, documents: &[&str]) -> PathBuf {
    let dir = unique_dir(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for document in documents {
        let path = assets().join(document);
        run_cli(&[
            "ingest",
            dir.to_str().expect("utf-8"),
            "notes",
            path.to_str().expect("utf-8"),
        ]);
    }
    dir
}

/// 応答の本文から最初のチャンク ID(s256: と 16 進 64 桁)を取り出す。
fn first_chunk_id(text: &str) -> String {
    let start = text.find("s256:").expect("チャンク ID が応答に無い");
    text[start..start + 69].to_string()
}

/// エージェントが通る経路そのもの(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9)):
/// initialize → tools/list → tools/call が通り、search の答えが出典(文書名・見出し・
/// チャンク ID・取得日時)を持ち、fetch が全文を返す。エージェントが出典付きで答えられる
/// のは、この 3 往復が通るからである。
#[test]
fn the_handshake_lists_the_tools_and_search_answers_with_citations() {
    let dir = store_with("mcp-handshake", &["search_ja.md"]);
    let mut mcp = McpProcess::start(&dir, &[]);

    // (1) initialize。Claude Code が実際に送る形(id 0、protocolVersion 2025-11-25)で
    // 組む(should/0138)。id 0 を「id が無い」と取り違えると応答が消える。
    let initialized = mcp.request(
        "{\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\
         \"capabilities\":{\"roots\":{\"listChanged\":true},\"elicitation\":{}},\
         \"clientInfo\":{\"name\":\"claude-code\",\"version\":\"2.1.232\"}},\
         \"jsonrpc\":\"2.0\",\"id\":0}",
    );
    assert!(initialized.contains("\"id\":0"), "{initialized}");
    assert!(initialized.contains("\"protocolVersion\":\"2025-06-18\""), "{initialized}");
    assert!(initialized.contains("\"tools\":{\"listChanged\":false}"), "{initialized}");
    assert!(initialized.contains("\"name\":\"uniqnode\""), "{initialized}");

    // (2) notifications/initialized には応答を返さない。次の要求の応答が届くことで
    // 「返さなかった」ことを確かめる(応答を 1 本余計に書いていれば、この tools/list の
    // 応答の位置に initialized への応答が居座り、id が食い違う)。
    mcp.send("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}");
    let listed = mcp.request("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}");
    assert!(listed.contains("\"id\":1"), "通知に応答を返している: {listed}");
    // 一覧は result.tools に入る(result を配列にすると、相手はツールを 1 本も
    // 見つけられないまま「接続はできた」状態になる)。
    assert!(listed.contains("\"result\":{\"tools\":["), "{listed}");
    assert!(listed.contains("\"name\":\"search\""), "{listed}");
    assert!(listed.contains("\"name\":\"fetch\""), "{listed}");
    assert!(listed.contains("\"inputSchema\""), "{listed}");

    // (3) search。出典が全部そろっていること(完了条件は「出典付きで答えられる」で
    // あり、出典を欠いた応答は用を成さない)。
    let searched = mcp.call(2, "search", "{\"query\":\"世代の整合\"}");
    assert!(searched.contains("\"id\":2"), "{searched}");
    assert!(searched.contains("\"isError\":false"), "{searched}");
    assert!(searched.contains("方式 bm25"), "実際に使った方式が読めるべき: {searched}");
    assert!(searched.contains("notes/search_ja"), "文書名が出典に無い: {searched}");
    assert!(searched.contains("位置 1"), "チャンクの位置が出典に無い: {searched}");
    assert!(
        searched.contains("見出し: 分散設計 > 世代の整合"),
        "見出しが出典に無い: {searched}"
    );
    assert!(searched.contains("チャンク ID: s256:"), "チャンク ID が出典に無い: {searched}");
    assert!(
        searched.contains("抜粋: 転置索引は導出データであり"),
        "本文の抜粋が無い: {searched}"
    );
    // 取得日時: ref レコードの at。今この場で取り込んだので、日付は今日である。
    let today = uniqnode::mcp::format_unix_time(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("unix 時刻")
            .as_secs() as i64,
    );
    assert!(
        searched.contains(&format!("取得日時: {}", &today[..10])),
        "取得日時が今日の取り込みを指していない(期待する日付 {}): {searched}",
        &today[..10]
    );

    // (4) fetch。抜粋の続き(スニペットの 200 文字より後ろ)まで届く。
    let chunk_id = first_chunk_id(&searched);
    let fetched = mcp.call(3, "fetch", &format!("{{\"id\":\"{chunk_id}\"}}"));
    assert!(fetched.contains("\"id\":3"), "{fetched}");
    assert!(fetched.contains("\"isError\":false"), "{fetched}");
    assert!(
        fetched.contains("世代の整合はオブジェクト数と署名者ごとの最終列番号"),
        "全文が返っていない: {fetched}"
    );
    assert!(fetched.contains("出典: notes/search_ja"), "全文にも出典が要る: {fetched}");
    assert!(fetched.contains("取得日時: 20"), "{fetched}");

    // 起動の知らせは標準エラーへ出る(標準出力の純度は finish が検査する)。
    let exit = mcp.finish();
    assert!(exit.stderr.contains("uniqnode: mcp:"), "起動の知らせが標準エラーに無い");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 誤りの経路: 不正な JSON・未知のメソッド・知らないツール・引数の欠け・存在しない ID。
/// どれも黙って落ちず、JSON-RPC の規定の形で理由を言う(must/0022)。
#[test]
fn malformed_requests_get_json_rpc_errors_and_absent_ids_get_tool_errors() {
    let dir = store_with("mcp-errors", &["search_ja.md"]);
    let mut mcp = McpProcess::start(&dir, &[]);

    // 不正な JSON: id が読めないので id は null(規定)。
    let parse_error = mcp.request("not json at all");
    assert!(parse_error.contains("\"code\":-32700"), "{parse_error}");
    assert!(parse_error.contains("\"id\":null"), "{parse_error}");
    // 壊れた 1 行で接続は終わらない。次の要求は通る。
    let after = mcp.request("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}");
    assert!(after.contains("\"result\":{}"), "{after}");

    // 未知のメソッド。
    let unknown = mcp.request("{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/list\"}");
    assert!(unknown.contains("\"code\":-32601"), "{unknown}");
    assert!(unknown.contains("resources/list"), "何を知らないのかを言うべき: {unknown}");

    // 知らないツールと引数の誤りは、要求の組み立ての誤り(-32602)。
    let unknown_tool = mcp.call(3, "delete_everything", "{}");
    assert!(unknown_tool.contains("\"code\":-32602"), "{unknown_tool}");
    let no_query = mcp.call(4, "search", "{}");
    assert!(no_query.contains("\"code\":-32602"), "{no_query}");
    assert!(no_query.contains("query"), "{no_query}");
    let bad_top_k = mcp.call(5, "search", "{\"query\":\"世代\",\"top_k\":0}");
    assert!(bad_top_k.contains("\"code\":-32602"), "{bad_top_k}");
    assert!(bad_top_k.contains("top_k"), "{bad_top_k}");
    let bad_id = mcp.call(6, "fetch", "{\"id\":\"not-an-object-id\"}");
    assert!(bad_id.contains("\"code\":-32602"), "{bad_id}");

    // 形は正しいが持っていない ID は、ツールを実行したうえでの失敗(isError)。
    // モデルが読んで次の手を選べるように、結果として返す。
    let absent = format!("s256:{}", "a".repeat(64));
    let missing = mcp.call(7, "fetch", &format!("{{\"id\":\"{absent}\"}}"));
    assert!(missing.contains("\"isError\":true"), "{missing}");
    assert!(missing.contains("持っていない"), "{missing}");
    assert!(!missing.contains("\"error\""), "ツールの失敗は JSON-RPC の誤りではない: {missing}");

    mcp.finish();
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 劣化の経路(should/0128): 埋め込みを装備しても届かなければ BM25 だけで答え続ける。
/// MCP 越しでも、黙って劣化しないこと(ツールの応答が理由を言い、標準エラーにも残る
/// こと)を確かめる。埋め込みサーバは要らない: 誰も待ち受けていないポートを指す。
#[test]
fn a_degraded_search_says_so_through_mcp_and_on_stderr() {
    let dir = store_with("mcp-degraded", &["search_ja.md"]);
    let mut mcp = McpProcess::start(&dir, &["--embed", "http://127.0.0.1:1"]);

    // (1) ベクトルが 1 件も無い段階。既定の方式(融合)を求めても BM25 で答え、
    // 理由を言う。
    let first = mcp.call(1, "search", "{\"query\":\"世代の整合\"}");
    assert!(first.contains("方式 bm25"), "劣化後の方式が読めるべき: {first}");
    assert!(first.contains("劣化:"), "劣化を黙っている: {first}");
    assert!(first.contains("ベクトルが 1 件も無い"), "劣化の理由が読めるべき: {first}");
    assert!(first.contains("チャンク ID: s256:"), "劣化しても BM25 の結果は返るべき: {first}");

    // (2) ベクトルはあるが埋め込みサーバに届かない段階。ベクトルはストアの外の導出
    // データなので、MCP を止めずに書ける。
    let chunk_id = first_chunk_id(&first);
    let mut cache = uniqnode::embed::VectorCache::open(
        uniqnode::embed::VectorCache::path_for(&dir, "bge-m3"),
        "bge-m3",
        uniqnode::embed::DEFAULT_EMBEDDING_DIMENSION,
    )
    .expect("open cache");
    let mut vector = vec![0.0f32; uniqnode::embed::DEFAULT_EMBEDDING_DIMENSION];
    vector[0] = 1.0;
    cache.extend(vec![(chunk_id, vector)]).expect("write cache");

    let second = mcp.call(2, "search", "{\"query\":\"世代の整合\"}");
    assert!(second.contains("方式 bm25"), "{second}");
    assert!(second.contains("127.0.0.1:1"), "どのサーバに届かなかったかを言うべき: {second}");
    assert!(second.contains("接続できない"), "劣化の理由が読めるべき: {second}");

    // 方式を明示して BM25 を求めたときは、そもそも埋め込みを試さないので劣化もない。
    let third = mcp.call(3, "search", "{\"query\":\"世代の整合\",\"method\":\"bm25\"}");
    assert!(!third.contains("劣化:"), "劣化していないのに理由を出さない: {third}");

    // 劣化は標準エラーにも残る(応答を読まない運用者にも見えるように)。
    let exit = mcp.finish();
    assert!(
        exit.stderr.contains("uniqnode: search:"),
        "劣化が標準エラーに残っていない: {}",
        exit.stderr
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// PDF のチャンクは出典にページ番号を持ち、見出しは持たない(INGEST の「既知の癖」)。
/// 空欄のまま黙らず、「(なし)」と書く。
#[test]
fn pdf_citations_carry_the_page_number_through_mcp() {
    require_pdftotext();
    let dir = unique_dir("mcp-pdf");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let pdf = dir.join("three_pages.pdf");
    std::fs::write(&pdf, THREE_PAGE_PDF).expect("write pdf");
    run_cli(&[
        "ingest",
        dir.to_str().expect("utf-8"),
        "specs",
        pdf.to_str().expect("utf-8"),
    ]);
    let mut mcp = McpProcess::start(&dir, &[]);
    let searched = mcp.call(1, "search", "{\"query\":\"Page two\",\"top_k\":1}");
    assert!(searched.contains("specs/three_pages p.2"), "ページ番号が出典に無い: {searched}");
    assert!(searched.contains("見出し: (なし)"), "見出しの不在を言うべき: {searched}");
    mcp.finish();
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// コレクションの絞り込みと件数の指定が MCP の引数からも効く(引数の形は
/// POST /v1/search のボディと同じ。読み取りは同じ関数を通る)。
#[test]
fn the_search_tool_honours_collection_and_top_k() {
    let dir = store_with("mcp-arguments", &["search_ja.md", "search_en.md"]);
    let mut mcp = McpProcess::start(&dir, &[]);
    let all = mcp.call(1, "search", "{\"query\":\"の\",\"top_k\":2}");
    assert!(all.contains("2 件"), "top_k で件数を絞れるべき: {all}");
    let narrowed = mcp.call(2, "search", "{\"query\":\"世代の整合\",\"collection\":\"absent\"}");
    assert!(narrowed.contains("一致なし"), "存在しないコレクションは空振り: {narrowed}");
    assert!(narrowed.contains("コレクション absent"), "絞り込みの条件を書き戻すべき: {narrowed}");
    mcp.finish();
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
