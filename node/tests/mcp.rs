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
        McpProcess::start_program(Path::new(env!("CARGO_BIN_EXE_uniqnode")), dir, arguments)
    }

    /// 実行ファイルを指定して起こす(自己置換の検査は、実物ではなく複製を的にする)。
    fn start_program(program: &Path, dir: &Path, arguments: &[&str]) -> McpProcess {
        let mut child = Command::new(program)
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
    // 問い方の手引きはツールの説明にしか無い(モデルが読むのはここだけである)。
    // 実データで測った差(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9) の
    // 「公開するツール」)を、良い例と悪い例つきで載せていることを確かめる。
    assert!(listed.contains("英語で問い"), "問い方の手引きが説明に無い: {listed}");
    assert!(
        listed.contains("which field reports the period at which the HPET main counter increments"),
        "良い例が説明に無い: {listed}"
    );
    assert!(listed.contains("高精度イベントタイマ"), "悪い例が説明に無い: {listed}");
    assert!(listed.contains("日英を併記"), "訳が分からないときの逃げ道が説明に無い: {listed}");

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
    let today = uniqnode::clock::format_unix_time(uniqnode::clock::unix_now());
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

/// mcp の標準エラーは登録した LLM クライアントが吸うので、利用者の目には触れない。
/// そこで serve と同じく、既定で <dir>/logs/mcp.log にも残す。標準出力はプロトコル
/// 専用なので、ログを足しても 1 行も混ざらないこと(純度は finish が検査する)。
#[test]
fn mcp_saves_its_log_to_a_file_by_default_without_touching_stdout() {
    let dir = store_with("mcp-log", &["search_ja.md"]);
    let mut mcp = McpProcess::start(&dir, &[]);
    let searched = mcp.call(1, "search", "{\"query\":\"世代の整合\"}");
    assert!(searched.contains("\"isError\":false"), "{searched}");
    let exit = mcp.finish();

    let path = uniqnode::log::default_path(&dir, uniqnode::log::MCP_ROLE);
    assert!(path.exists(), "既定で mcp のログファイルが作られていない: {}", path.display());
    let logged = std::fs::read_to_string(&path).expect("read log");
    assert!(
        logged.contains("uniqnode: mcp:"),
        "起動の知らせがログに残っていない: {logged}"
    );
    // 標準エラーで見えるものと同じ内容が残る(片方だけの記録を作らない)。
    assert_eq!(logged, exit.stderr, "ログファイルと標準エラーの内容が食い違う");
    // 行頭に UTC の時刻が付く(後から読む記録なので、時刻の無い行は作らない)。
    for line in logged.lines() {
        assert!(
            line.len() > 20 && line[..20].ends_with('Z') && line[20..].starts_with(" [pid "),
            "行頭に UTC の時刻と pid が無い: {line}"
        );
    }
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
    // 資材の 1 ページは 4 語しかなく、低情報の判定に落ちる(実データで柱だけの紙面を
    // 落とすための判定であり、この資材はそれと見分けがつかない)。ここで見たいのは
    // 出典の形なので、include_low_information で戻して測る。
    let searched = mcp.call(
        1,
        "search",
        "{\"query\":\"Page two\",\"top_k\":1,\"include_low_information\":true}",
    );
    assert!(searched.contains("specs/three_pages p.2"), "ページ番号が出典に無い: {searched}");
    assert!(searched.contains("見出し: (なし)"), "見出しの不在を言うべき: {searched}");
    mcp.finish();
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 低情報チャンクを落としたことは、エージェントの読む応答にも標準エラーにも出る
/// (黙って捨てない。must/0019 と同じ理由)。落とす判断そのものは REST と同じ
/// run_search が持つので、ここで見るのは伝わり方だけである。
#[test]
fn the_search_tool_says_how_many_low_information_chunks_it_dropped() {
    let dir = store_with("mcp-lowinfo", &["search_lowinfo.md"]);
    let mut mcp = McpProcess::start(&dir, &[]);
    // 「索引の構築」は目次の節(点線の紙面)と本文の節の両方に現れる。
    let dropped = mcp.call(1, "search", "{\"query\":\"索引の構築\",\"top_k\":10}");
    assert!(dropped.contains("1 件"), "本文の節だけが残るべき: {dropped}");
    assert!(
        dropped.contains("低情報チャンク 1 件を応答から落とした"),
        "落とした件数を言うべき: {dropped}"
    );
    assert!(
        dropped.contains("include_low_information"),
        "戻し方を言うべき: {dropped}"
    );
    // 戻せば 2 件になり、落としたという行も出ない。
    let kept = mcp.call(
        2,
        "search",
        "{\"query\":\"索引の構築\",\"top_k\":10,\"include_low_information\":true}",
    );
    assert!(kept.contains("2 件"), "目次の節も返るべき: {kept}");
    assert!(!kept.contains("落とした"), "落としていないのに言わない: {kept}");
    let exit = mcp.finish();
    assert!(
        exit.stderr.contains("低情報チャンク 1 件を応答から落とした"),
        "標準エラーにも残すべき: {}",
        exit.stderr
    );
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

/// Claude Code が実際に送る initialize(should/0138)。自己置換の検査でも、引き継ぎの
/// 中身(protocol とクライアントの名乗り)を見るために同じ形を送る。
const CLAUDE_CODE_INITIALIZE: &str = "{\"method\":\"initialize\",\"params\":\
    {\"protocolVersion\":\"2025-11-25\",\"capabilities\":{\"roots\":{\"listChanged\":true},\
    \"elicitation\":{}},\"clientInfo\":{\"name\":\"claude-code\",\"version\":\"2.1.232\"}},\
    \"jsonrpc\":\"2.0\",\"id\":0}";

/// initialize の応答に出る表示名。自己置換の検査は、この文字列を同じ長さの別の文字列に
/// 差し替えた複製を「新しいイメージ」として置き、応答がどちらから来たのかを見る。
const SERVER_TITLE: &str = "uniqnode RAG ストレージ";
const PATCHED_TITLE: &str = "uniqnode NEW ストレージ";

/// CLI を 1 回動かして結果をそのまま返す(失敗する経路を検査する側で読むため)。
fn cli_output(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .output()
        .expect("uniqnode の起動")
}

/// 転送する形(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9))。走っている serve の
/// REST へ回す形で initialize / tools/list / tools/call が通り、出典はストアを直接開く形と
/// 同じものが出る。serve が落ちていれば、黙って空を返さず、原因と起動コマンドを言う。
#[test]
fn the_forwarding_form_answers_through_serve_and_names_the_cause_when_serve_is_down() {
    let dir = store_with("mcp-forward", &["search_ja.md"]);
    let server = start_server_at_with_args(dir.clone(), &[]);
    let serve_url = format!("http://{}", server.address);
    let mut mcp = McpProcess::start(&dir, &["--serve-url", &serve_url]);

    let initialized = mcp.request(CLAUDE_CODE_INITIALIZE);
    assert!(initialized.contains("\"protocolVersion\":\"2025-06-18\""), "{initialized}");
    assert!(initialized.contains("\"name\":\"uniqnode\""), "{initialized}");
    let listed = mcp.request("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}");
    assert!(listed.contains("\"result\":{\"tools\":["), "{listed}");
    assert!(listed.contains("\"name\":\"search\""), "{listed}");

    // 検索。整形はストアを直接開く形と同じ関数を通るので、出典の見え方も同じである。
    let searched = mcp.call(2, "search", "{\"query\":\"世代の整合\"}");
    assert!(searched.contains("\"isError\":false"), "{searched}");
    assert!(searched.contains("方式 bm25"), "{searched}");
    assert!(searched.contains("notes/search_ja"), "文書名が出典に無い: {searched}");
    assert!(searched.contains("位置 1"), "{searched}");
    assert!(
        searched.contains("見出し: 分散設計 > 世代の整合"),
        "見出しが出典に無い: {searched}"
    );
    assert!(searched.contains("チャンク ID: s256:"), "{searched}");
    assert!(searched.contains("取得日時: 20"), "取得日時が出典に無い: {searched}");
    assert!(searched.contains("抜粋: 転置索引は導出データであり"), "{searched}");

    // 全文の取得。出典(取得日時を含む)は GET /v1/objects/{id}/citation から組む。
    let chunk_id = first_chunk_id(&searched);
    let fetched = mcp.call(3, "fetch", &format!("{{\"id\":\"{chunk_id}\"}}"));
    assert!(fetched.contains("\"isError\":false"), "{fetched}");
    assert!(
        fetched.contains("世代の整合はオブジェクト数と署名者ごとの最終列番号"),
        "全文が返っていない: {fetched}"
    );
    assert!(fetched.contains("出典: notes/search_ja"), "全文にも出典が要る: {fetched}");
    assert!(fetched.contains("取得日時: 20"), "{fetched}");

    // 持っていない ID は、転送する形でも「持っていない」と言う(404 の読み替え)。
    let absent = format!("s256:{}", "a".repeat(64));
    let missing = mcp.call(4, "fetch", &format!("{{\"id\":\"{absent}\"}}"));
    assert!(missing.contains("\"isError\":true"), "{missing}");
    assert!(missing.contains("持っていない"), "{missing}");

    // serve を止める。以後は黙って失敗せず、原因と起動コマンドを言う(must/0022)。
    drop(server);
    let unreachable = mcp.call(5, "search", "{\"query\":\"世代の整合\"}");
    assert!(unreachable.contains("\"isError\":true"), "{unreachable}");
    assert!(unreachable.contains("serve に届かない"), "{unreachable}");
    assert!(unreachable.contains("接続できない"), "原因を言うべき: {unreachable}");
    assert!(
        unreachable.contains("uniqnode serve"),
        "起動コマンドを添えるべき: {unreachable}"
    );
    let unreachable_fetch = mcp.call(6, "fetch", &format!("{{\"id\":\"{chunk_id}\"}}"));
    assert!(unreachable_fetch.contains("serve に届かない"), "{unreachable_fetch}");

    // 同じ理由は標準エラーにも残る(応答を読まない運用者にも見えるように)。
    let exit = mcp.finish();
    assert!(
        exit.stderr.contains("serve に届かない"),
        "届かない理由が標準エラーに無い: {}",
        exit.stderr
    );
    assert!(
        exit.stderr.contains("へ転送する形(ストアのロックを取らない)"),
        "どの形で起きたのかを起動時に言うべき: {}",
        exit.stderr
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 転送する形はストアのロックを取らない。これが改善の要点である: MCP サーバを常駐させた
/// まま、同じデータディレクトリに対して CLI の取り込みと埋め込みが通る(ストアを直接
/// 開く形なら、どちらも「別プロセスが開いている」で断られる)。
#[test]
fn the_forwarding_form_holds_no_store_lock_so_the_cli_can_ingest_and_embed() {
    let dir = store_with("mcp-forward-lock", &["search_ja.md"]);
    // serve は起こさない。ここで見たいのは「MCP がロックを取らないこと」だけであり、
    // ロックを取っていればこの後の ingest が断られる。
    let mcp = McpProcess::start(&dir, &["--serve-url", "http://127.0.0.1:7440"]);
    let path = assets().join("search_en.md");
    let text = dir.to_str().expect("utf-8");

    // 取り込み: MCP の常駐中に成功する。
    run_cli(&["ingest", text, "notes", path.to_str().expect("utf-8")]);
    run_cli(&["status", text]);

    // 埋め込み: 埋め込みサーバは要らない。ストアを開けたかどうかを見るので、開いた後に
    // 出る cache の行があり、断りの文言が出ていないことを確かめる(この試験環境に
    // 埋め込みサーバが居るとは限らないので、届かない先を指して失敗させる)。
    let embedded = cli_output(&["embed", text, "--embed", "http://127.0.0.1:1"]);
    let out = String::from_utf8_lossy(&embedded.stdout);
    let error = String::from_utf8_lossy(&embedded.stderr);
    assert!(out.contains("cache:"), "ストアを開けていない: {out} {error}");
    assert!(
        !error.contains("別プロセスが開いている"),
        "MCP がロックを持っている: {error}"
    );

    // 取り込んだ結果は、走っている MCP からも見える(索引は世代で作り直される)。
    mcp.finish();
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// ストアを直接開く形は serve と同じ装備を受ける。--rerank を受けておきながら装備して
/// いなければ、届かないリランカーを指しても応答は何も言わず、運用者は装備したつもりで
/// 装備の無い順位を読む(明示された指定の黙殺。must/0022 の同型)。届かない先を指して
/// 検索し、劣化の理由(どのリランカーに届かなかったか)が応答に出ることで装備を確かめる
/// (SEARCH の劣化の経路)。リランカーは要らない: 誰も待ち受けていないポートを指す。
#[test]
fn the_local_form_equips_the_reranker_and_says_when_it_cannot_reach_it() {
    let dir = store_with("mcp-rerank", &["search_ja.md"]);
    let mut mcp = McpProcess::start(&dir, &["--rerank", "http://127.0.0.1:1/v1/rerank"]);

    // 候補が 2 件無いと取り直しは往復しない(1 件では順位が動かない)ので、2 つの節に
    // 当たる問いを選ぶ。
    let searched = mcp.call(1, "search", "{\"query\":\"レプリカ 世代\"}");
    assert!(searched.contains("\"isError\":false"), "{searched}");
    assert!(searched.contains("2 件"), "候補が 2 件無いと取り直しを試みない: {searched}");
    assert!(searched.contains("劣化:"), "届かないリランカーを黙っている: {searched}");
    assert!(
        searched.contains("127.0.0.1:1"),
        "どのリランカーに届かなかったかを言うべき: {searched}"
    );
    assert!(searched.contains("接続できない"), "劣化の理由が読めるべき: {searched}");
    // 取り直せなくても一次検索の結果はそのまま返る(検索を失敗させない)。
    assert!(searched.contains("方式 bm25"), "{searched}");
    assert!(searched.contains("チャンク ID: s256:"), "{searched}");

    // 装備したことは起動時に標準エラーへ言う(serve と同じ行)。
    let exit = mcp.finish();
    assert!(
        exit.stderr.contains("uniqnode: rerank: bge-reranker-v2-m3 (http://127.0.0.1:1/v1/rerank)"),
        "装備の知らせが標準エラーに無い: {}",
        exit.stderr
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 転送する形に --rerank / --reranker を渡すのは --embed と同じ矛盾である(順位の取り直し
/// を装備するのは転送先の serve)。受けて捨てると、装備したつもりの運用者が装備の無い
/// 検索を読む。理由を標準エラーへ出して exit 2 で終わり、標準出力には何も書かない
/// (MCP の標準出力はプロトコル専用)。
#[test]
fn the_forwarding_form_refuses_a_reranker_it_cannot_equip() {
    let dir = unique_dir("mcp-forward-rerank");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let text = dir.to_str().expect("utf-8");
    let arguments =
        [("--rerank", "http://127.0.0.1:1/v1/rerank"), ("--reranker", "bge-reranker-v2-m3")];
    for (flag, value) in arguments {
        let output =
            cli_output(&["mcp", text, "--serve-url", "http://127.0.0.1:7440", flag, value]);
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{flag} を受けて起動している: {error}");
        assert!(output.stdout.is_empty(), "標準出力はプロトコル専用: {:?}", output.stdout);
        assert!(
            error.contains(&format!("--serve-url と {flag} は併用しない")),
            "断りの理由が標準エラーに無い: {error}"
        );
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// --embedder だけを転送する形に渡すのも同じ矛盾である(装備の可否は --embed だけで
/// 決まるが、模型名を明示した運用者は「その模型で装備した」つもりになる)。--embed と
/// --rerank / --reranker は断るのに --embedder だけが黙って通っていた穴を、同じ形で塞ぐ。
#[test]
fn the_forwarding_form_refuses_an_embedder_it_cannot_equip() {
    let dir = unique_dir("mcp-forward-embedder");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let text = dir.to_str().expect("utf-8");
    let output = cli_output(&[
        "mcp",
        text,
        "--serve-url",
        "http://127.0.0.1:7440",
        "--embedder",
        "multilingual-e5-large",
    ]);
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "--embedder を受けて起動している: {error}");
    assert!(output.stdout.is_empty(), "標準出力はプロトコル専用: {:?}", output.stdout);
    assert!(
        error.contains("--serve-url と --embedder は併用しない"),
        "断りの理由が標準エラーに無い: {error}"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// embed は serve と引数の読み手を共有するので --rerank も字面としては通るが、ベクトルを
/// 作るだけの命令に順位の取り直しの口は無い。受けて捨てずに、ストアを開く前に理由を
/// 言って exit 2 で終わる(標準出力に cache の行が出ていれば、断らずに仕事を始めている)。
#[test]
fn embed_refuses_the_reranker_arguments_instead_of_ignoring_them() {
    let dir = store_with("embed-rerank", &["search_ja.md"]);
    let text = dir.to_str().expect("utf-8");
    let arguments =
        [("--rerank", "http://127.0.0.1:1/v1/rerank"), ("--reranker", "bge-reranker-v2-m3")];
    for (flag, value) in arguments {
        let output = cli_output(&["embed", text, flag, value]);
        let out = String::from_utf8_lossy(&output.stdout);
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{flag} を受けて進んでいる: {out} {error}");
        assert!(out.is_empty(), "断る前に仕事を始めている: {out}");
        assert!(
            error.contains(&format!("{flag} は embed の引数ではない")),
            "断りの理由が標準エラーに無い: {error}"
        );
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 自己置換(バイナリの更新を検出して自分を exec で差し替える)。実プロセスで、
/// 差し替えの前後で応答がどちらのイメージから来ているのかを見る。壊れたイメージを
/// 置いたときは差し替えず、旧イメージのまま答え続ける(負例。発火を一度も見ていない
/// 防御は未検証である)。
#[test]
fn the_server_execs_a_healthy_new_binary_and_refuses_a_broken_one() {
    let dir = store_with("mcp-self-replace", &["search_ja.md"]);
    let binary_dir = unique_dir("mcp-self-replace-bin");
    std::fs::create_dir_all(&binary_dir).expect("mkdir");
    let binary = binary_dir.join("uniqnode");
    // 実物は cargo が管理するので触らない。複製を差し替えの的にする。
    write_executable(&binary, &std::fs::read(env!("CARGO_BIN_EXE_uniqnode")).expect("read"));

    let mut mcp = McpProcess::start_program(&binary, &dir, &[]);
    let initialized = mcp.request(CLAUDE_CODE_INITIALIZE);
    assert!(initialized.contains(SERVER_TITLE), "起動時のイメージが答えるべき: {initialized}");

    // (1) 壊れたイメージ。自己検査のサブコマンド名を潰した複製を置く。exec してしまえば
    // 表示名が変わるので、変わらないことが「差し替えなかった」ことの証拠になる。
    replace_binary(
        &binary,
        &[(SERVER_TITLE, PATCHED_TITLE), ("selfcheck", "selfchecx")],
    );
    let ping = mcp.request("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}");
    assert!(ping.contains("\"result\":{}"), "{ping}");
    let after_broken = mcp.request(CLAUDE_CODE_INITIALIZE);
    assert!(
        after_broken.contains(SERVER_TITLE),
        "壊れたイメージに差し替えてはならない: {after_broken}"
    );

    // (2) 健全なイメージ。表示名だけを変えた複製を置く。
    replace_binary(&binary, &[(SERVER_TITLE, PATCHED_TITLE)]);
    let ping = mcp.request("{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}");
    assert!(ping.contains("\"result\":{}"), "{ping}");
    let after_good = mcp.request(CLAUDE_CODE_INITIALIZE);
    assert!(
        after_good.contains(PATCHED_TITLE),
        "差し替えた後は新しいイメージが答えるべき: {after_good}"
    );
    // 差し替えは相手から見えない: 同じ標準入出力で会話が続き、検索もそのまま通る。
    let searched = mcp.call(3, "search", "{\"query\":\"世代の整合\"}");
    assert!(searched.contains("notes/search_ja"), "差し替え後も検索が通るべき: {searched}");

    let exit = mcp.finish();
    assert!(
        exit.stderr.contains("自己検査に落ちた"),
        "壊れたイメージを断った記録が無い: {}",
        exit.stderr
    );
    assert!(
        exit.stderr.contains("exec で差し替える"),
        "差し替えの記録が無い(黙って入れ替えない): {}",
        exit.stderr
    );
    // 新しいイメージは initialize を受けた事実を環境変数で引き継ぐ(Claude Code は
    // 再送しない)。
    assert!(
        exit.stderr.contains("ハンドシェイク済みとして起動した(相手の protocol 2025-11-25、client claude-code/2.1.232)"),
        "ハンドシェイクの引き継ぎが無い: {}",
        exit.stderr
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
    std::fs::remove_dir_all(&binary_dir).expect("cleanup");
}

/// 自己置換の防御が呼ぶ命令(uniqnode selfcheck)は、健全な実行ファイルなら規定の印を
/// 標準出力に出して正常終了する。防御はこの印と終了コードの両方を見る。
#[test]
fn selfcheck_reports_the_marker_on_stdout() {
    let output = cli_output(&["selfcheck"]);
    let reported = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "selfcheck は正常終了すべき: {}", output.status);
    assert!(
        reported.contains(uniqnode::mcp::SELF_CHECK_MARKER),
        "印が無い: {reported}"
    );
    assert!(reported.contains("tools=4"), "読む 2 本と書く 2 本の記述を組み立てるべき: {reported}");
}

/// tools/list の応答に載るツール名を、載っている順に取り出す。
fn listed_tool_names(listed: &str) -> Vec<String> {
    listed
        .split("\"name\":\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|name| ["search", "fetch", "add_document", "fetch_url"].contains(name))
        .map(|name| name.to_string())
        .collect()
}

/// (a) 書くツール(add_document・fetch_url)は、`--writable` を 1 つでも与えたときだけ
/// tools/list に載る。与えなければ今までどおり search と fetch の 2 本で、書けない相手に
/// 書くツールを見せない。載せていないツールを呼べば要求の誤り(-32602)で理由を言う。
/// ビューワは同じ読み手を共有するが --writable を受けない(黙って捨てない)。
#[test]
fn the_write_tools_are_listed_only_when_a_collection_is_writable() {
    let dir = store_with("mcp-writable-list", &["search_ja.md"]);
    let server = start_server_at_with_args(dir.clone(), &[]);
    let serve_url = format!("http://{}", server.address);
    let list = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}";

    let mut writable = McpProcess::start(&dir, &["--serve-url", &serve_url, "--writable", "notes"]);
    let listed = writable.request(list);
    assert_eq!(
        listed_tool_names(&listed),
        ["search", "fetch", "add_document", "fetch_url"],
        "--writable があれば 4 本: {listed}"
    );
    assert!(listed.contains("許すのは: notes"), "書ける先が説明に無い: {listed}");
    let exit = writable.finish();
    assert!(
        exit.stderr.contains("add_document・fetch_url(書き込みを許す: notes)"),
        "起動の知らせに書ける先が無い: {}",
        exit.stderr
    );

    let mut read_only = McpProcess::start(&dir, &["--serve-url", &serve_url]);
    let listed = read_only.request(list);
    assert_eq!(listed_tool_names(&listed), ["search", "fetch"], "--writable 無しは 2 本: {listed}");
    let refused = read_only.call(
        2,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"memo\",\"text\":\"x\"}",
    );
    assert!(refused.contains("\"code\":-32602"), "載せていないツールは要求の誤り: {refused}");
    assert!(refused.contains("--writable"), "理由を言うべき: {refused}");
    read_only.finish();

    let output = cli_output(&["viewer", dir.to_str().expect("utf-8"), "127.0.0.1:0", "--writable", "notes"]);
    assert_eq!(output.status.code(), Some(2), "viewer が --writable を受けて起動している");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--writable はビューワの引数ではない"),
        "断りの理由が標準エラーに無い: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (b) add_document で notes に markdown を入れると、応答が成功(新規)を言い、直後の
/// search で本文が出典(コレクション/文書名・見出し)付きで出る。同じ内容の再実行は
/// 「変わらず」、違う内容は「上書き」で前版を言う。引数の誤りは要求の誤り(-32602)。
#[test]
fn add_document_puts_markdown_into_a_writable_collection_and_search_finds_it() {
    let dir = store_with("mcp-add-document", &["search_ja.md"]);
    let server = start_server_at_with_args(dir.clone(), &[]);
    let serve_url = format!("http://{}", server.address);
    let mut mcp = McpProcess::start(&dir, &["--serve-url", &serve_url, "--writable", "notes"]);

    let added = mcp.call(
        1,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"hpet-memo\",\"text\":\
         \"# HPET の覚え書き\\n\\nHPET main counter の周期は GCAP_ID の COUNTER_CLK_PERIOD が言う。\"}",
    );
    assert!(added.contains("\"isError\":false"), "{added}");
    assert!(added.contains("入れた: notes/hpet-memo(新規。doc_rev s256:"), "{added}");
    assert!(added.contains("検索に出る"), "{added}");

    let searched = mcp.call(2, "search", "{\"query\":\"HPET main counter\",\"collection\":\"notes\"}");
    assert!(searched.contains("notes/hpet-memo 位置 0"), "入れた文書が検索に出ない: {searched}");
    assert!(searched.contains("見出し: HPET の覚え書き"), "markdown の見出しが出典に無い: {searched}");
    assert!(searched.contains("COUNTER_CLK_PERIOD"), "本文の抜粋が無い: {searched}");

    // 同じ内容は同じ ID に落ち、何も変わらない。
    let same = mcp.call(
        3,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"hpet-memo\",\"text\":\
         \"# HPET の覚え書き\\n\\nHPET main counter の周期は GCAP_ID の COUNTER_CLK_PERIOD が言う。\"}",
    );
    assert!(same.contains("変わらず(同じ内容が既にある)"), "{same}");
    assert!(same.contains("新規オブジェクト 0"), "{same}");

    // 違う内容は上書きで、前版の doc_rev を言う。素文(media text)も受ける。
    let overwritten = mcp.call(
        4,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"hpet-memo\",\"media\":\"text\",\
         \"text\":\"HPET の周期はフェムト秒単位で GCAP_ID に載る。\"}",
    );
    assert!(overwritten.contains("上書き。前版 s256:"), "{overwritten}");
    let searched = mcp.call(5, "search", "{\"query\":\"フェムト秒\",\"collection\":\"notes\"}");
    assert!(searched.contains("notes/hpet-memo"), "上書き後の本文が検索に出ない: {searched}");
    assert!(searched.contains("見出し: (なし)"), "素文は見出しを持たない: {searched}");

    // 引数の誤りは要求の組み立ての誤り(-32602)。ストアには何も書かない。
    let with_extension = mcp.call(
        6,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"memo.md\",\"text\":\"x\"}",
    );
    assert!(with_extension.contains("\"code\":-32602"), "{with_extension}");
    assert!(with_extension.contains("拡張子なし"), "{with_extension}");
    let no_text = mcp.call(7, "add_document", "{\"collection\":\"notes\",\"name\":\"memo\"}");
    assert!(no_text.contains("\"code\":-32602"), "{no_text}");
    assert!(no_text.contains("text"), "{no_text}");
    let bad_media = mcp.call(
        8,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"memo\",\"text\":\"x\",\"media\":\"pdf\"}",
    );
    assert!(bad_media.contains("\"code\":-32602"), "{bad_media}");
    assert!(bad_media.contains("markdown / text"), "{bad_media}");
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(!refs.contains("collections/notes/memo"), "誤った要求で ref ができている: {refs}");

    mcp.finish();
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (c) 許していないコレクションへの add_document は、ツールの失敗(isError)として理由と
/// 書ける先を言い、serve の状態は変わらない(ref が増えない)。
#[test]
fn add_document_refuses_a_collection_that_is_not_writable_without_touching_the_store() {
    let dir = store_with("mcp-add-refused", &["search_ja.md"]);
    let server = start_server_at_with_args(dir.clone(), &[]);
    let serve_url = format!("http://{}", server.address);
    let refs_before = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    let mut mcp = McpProcess::start(&dir, &["--serve-url", &serve_url, "--writable", "notes"]);

    let refused = mcp.call(
        1,
        "add_document",
        "{\"collection\":\"specs\",\"name\":\"memo\",\"text\":\"勝手に書く\"}",
    );
    assert!(refused.contains("\"isError\":true"), "{refused}");
    assert!(!refused.contains("\"error\""), "ツールの失敗は JSON-RPC の誤りではない: {refused}");
    assert!(
        refused.contains("specs は書き込みを許していない(--writable で許すのは: notes)"),
        "理由と書ける先を言うべき: {refused}"
    );
    let refused_fetch = mcp.call(
        2,
        "fetch_url",
        "{\"collection\":\"specs\",\"url\":\"http://127.0.0.1:1/x\"}",
    );
    assert!(refused_fetch.contains("\"isError\":true"), "{refused_fetch}");
    assert!(refused_fetch.contains("specs は書き込みを許していない"), "{refused_fetch}");

    let refs_after = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(!refs_after.contains("collections/specs/"), "断ったのに ref ができている: {refs_after}");
    assert_eq!(refs_before, refs_after, "断った書き込みで serve の状態が変わった");

    // 断りは標準エラーにも残る(応答を読まない運用者にも見えるように)。
    let exit = mcp.finish();
    assert!(
        exit.stderr.contains("specs は書き込みを許していない"),
        "断りが標準エラーに無い: {}",
        exit.stderr
    );
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// fetch_url に取りに行かせる紙面(script と img を含む。落とした数が 0 でないように)。
const FETCHED_PAGE_HTML: &str = "<!DOCTYPE html>\n<html><head><title>紙面</title>\
    <script src=\"/app.js\"></script></head><body>\n<h2>節</h2>\n\
    <p>取り込みの本文はここにある。</p>\n<img src=\"pic.png\" alt=\"絵\">\n\
    <script>var a = 1;</script>\n</body></html>\n";

/// (d) fetch_url がローカルの HTTP サーバから HTML を取り込み、応答に名前・種別・転送後の
/// URL・落とした数(dropped)が出て、本文が search に出る。name を省くと URL から導く。
/// 取れない URL(404)は serve の 502 の理由をそのまま失敗で伝え、file: は要求の誤り。
#[test]
fn fetch_url_ingests_html_from_a_local_site_and_reports_what_it_dropped() {
    require_curl();
    let site = TestSite::start(vec![(
        "/page.html",
        Canned::ok(Some("text/html; charset=utf-8"), FETCHED_PAGE_HTML.as_bytes()),
    )]);
    let dir = store_with("mcp-fetch-url", &["search_ja.md"]);
    let server = start_server_at_with_args(dir.clone(), &[]);
    let serve_url = format!("http://{}", server.address);
    let mut mcp = McpProcess::start(&dir, &["--serve-url", &serve_url, "--writable", "web"]);
    let url = site.url("/page.html");

    let fetched = mcp.call(
        1,
        "fetch_url",
        &format!("{{\"collection\":\"web\",\"url\":\"{url}\",\"name\":\"page\"}}"),
    );
    assert!(fetched.contains("\"isError\":false"), "{fetched}");
    assert!(fetched.contains("取り込んだ: web/page(html、final_url "), "{fetched}");
    assert!(fetched.contains(&url), "転送後の URL を言うべき: {fetched}");
    assert!(fetched.contains("新規。doc_rev s256:"), "{fetched}");
    assert!(
        fetched.contains("落とした外部依存 3 件(images 1・scripts 2)"),
        "落とした数を言うべき: {fetched}"
    );
    let searched = mcp.call(2, "search", "{\"query\":\"取り込みの本文\",\"collection\":\"web\"}");
    assert!(searched.contains("web/page 位置 0"), "取り込んだ紙面が検索に出ない: {searched}");
    assert!(searched.contains("見出し: 紙面 > 節"), "{searched}");

    // name を省くと URL から導く(ホストとパスを 1 語に)。
    let derived = mcp.call(3, "fetch_url", &format!("{{\"collection\":\"web\",\"url\":\"{url}\"}}"));
    assert!(derived.contains("\"isError\":false"), "{derived}");
    let host = site.address.replace(':', "_");
    assert!(derived.contains(&format!("取り込んだ: web/{host}_page(html")), "{derived}");

    // 404 は serve が 502 で言う理由をそのまま伝える(黙って飲まない)。
    let missing = mcp.call(
        4,
        "fetch_url",
        &format!("{{\"collection\":\"web\",\"url\":\"{}\"}}", site.url("/absent")),
    );
    assert!(missing.contains("\"isError\":true"), "{missing}");
    assert!(missing.contains("が 502 を返した"), "{missing}");
    assert!(missing.contains("404"), "相手の状態を言うべき: {missing}");
    // file: は取りに行く前に断る(要求の誤り)。
    let file = mcp.call(5, "fetch_url", "{\"collection\":\"web\",\"url\":\"file:///etc/hosts\"}");
    assert!(file.contains("\"code\":-32602"), "{file}");

    mcp.finish();
    drop(server);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// (e) ストアを直接開く形(--serve-url 無し)でも add_document は同じ関数(api.rs の
/// put_document)を通って動き、同じプロセスの search が本文を返す。
#[test]
fn the_local_form_adds_documents_too() {
    let dir = store_with("mcp-add-local", &["search_ja.md"]);
    let mut mcp = McpProcess::start(&dir, &["--writable", "notes"]);
    let added = mcp.call(
        1,
        "add_document",
        "{\"collection\":\"notes\",\"name\":\"local-memo\",\"text\":\"# 直接開く形\\n\\nロックを持ったまま書く。\"}",
    );
    assert!(added.contains("\"isError\":false"), "{added}");
    assert!(added.contains("入れた: notes/local-memo(新規。doc_rev s256:"), "{added}");
    let searched = mcp.call(2, "search", "{\"query\":\"ロックを持ったまま\",\"collection\":\"notes\"}");
    assert!(searched.contains("notes/local-memo 位置 0"), "{searched}");
    assert!(searched.contains("見出し: 直接開く形"), "{searched}");
    let refused = mcp.call(
        3,
        "add_document",
        "{\"collection\":\"specs\",\"name\":\"x\",\"text\":\"y\"}",
    );
    assert!(refused.contains("specs は書き込みを許していない"), "{refused}");
    // 標準出力の純度は finish が検査する(書き込みの記録も標準エラーへ出ている)。
    let exit = mcp.finish();
    assert!(exit.stderr.contains("uniqnode: mcp:"), "{}", exit.stderr);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// 実行ファイルを書く(実行の許可を付ける)。
fn write_executable(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, bytes).expect("write binary");
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

/// 実行ファイルを、文字列を同じ長さの別の文字列に差し替えた複製で置き換える。
///
/// 置き換えは一時ファイルへ書いてから rename する。走っているイメージのファイルはその場
/// では書けない(ETXTBSY)し、cargo が新しい実行ファイルを置くのも rename である。
/// 長さを変えないのは、ELF の位置を動かさずに応答の見た目だけを変えるためである。
fn replace_binary(path: &Path, patches: &[(&str, &str)]) {
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_uniqnode")).expect("read binary");
    for (from, to) in patches {
        assert_eq!(from.len(), to.len(), "同じ長さでなければ ELF が壊れる");
        let mut replaced = 0usize;
        let mut at = 0usize;
        while at + from.len() <= bytes.len() {
            if &bytes[at..at + from.len()] == from.as_bytes() {
                bytes[at..at + from.len()].copy_from_slice(to.as_bytes());
                replaced += 1;
                at += from.len();
            } else {
                at += 1;
            }
        }
        assert!(replaced > 0, "{from} が実行ファイルに見つからない");
    }
    let staged = path.with_extension("staged");
    write_executable(&staged, &bytes);
    std::fs::rename(&staged, path).expect("rename");
}
