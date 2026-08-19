//! 統合テスト共通ヘルパ: serve プロセスの起動と、curl が組み立てる形の
//! 生 HTTP/1.1 クライアント(should/0138)。
// 統合テストクレートごとに個別コンパイルされ、どのクレートも全ヘルパは使わないため。
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};

pub struct Server {
    pub child: Child,
    pub address: String,
    pub dir: PathBuf,
    /// Drop 時にディレクトリを消すか(再起動テストでは残す)。
    pub remove_dir_on_drop: bool,
    /// 捕まえた標準エラー(start_server_capturing_stderr で起こしたときだけ)。
    pub stderr: Option<ChildStderr>,
}

/// 正常終了を頼む(応答を読み切ってから閉じる)。頼めたら true。Drop と finish が
/// 同じ手順を通る(should/0135)。
fn ask_for_shutdown(address: &str) -> bool {
    TcpStream::connect(address)
        .ok()
        .and_then(|mut stream| {
            stream
                .write_all(
                    b"POST /v1/admin/shutdown HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .ok()?;
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            Some(())
        })
        .is_some()
}

impl Server {
    /// 正常終了させ、標準エラーを読み切って返す(ログファイルとの突き合わせに使う)。
    /// 呼ぶ前に remove_dir_on_drop を false にしておけば、終了後のデータディレクトリを
    /// 読める。
    pub fn finish(mut self) -> String {
        let mut stderr = self.stderr.take().expect("標準エラーを捕まえて起こしていない");
        ask_for_shutdown(&self.address);
        let _ = self.child.wait();
        let mut text = String::new();
        stderr.read_to_string(&mut text).expect("read stderr");
        text
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // まず正常終了を頼む(カバレッジのプロファイル書き出しは正常終了でのみ起きる)。
        // 期限内に終わらなければ kill に切り替える。
        let asked = ask_for_shutdown(&self.address);
        if asked {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::yield_now(),
                    Err(_) => break,
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.remove_dir_on_drop {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// PDF テストの前提確認。pdftotext が無い環境では黙って飛ばさず、導入手順を示して
/// 失敗する(docs/design/TESTING.md の外部コマンドの規約。openssl_interop と同じ扱い)。
pub fn require_pdftotext() {
    if let Err(error) = Command::new("pdftotext").arg("-v").output() {
        panic!(
            "PDF 取り込みのテストには pdftotext コマンドが必要({error})。\
             導入例(sudo なし): apt-get download poppler-utils と dpkg -x で \
             ~/opt/poppler/ へ展開し、PATH の通ったディレクトリへ symlink を置く。\
             libpoppler の無い機械ではライブラリ側も同じ手順で展開して \
             LD_LIBRARY_PATH を通す"
        );
    }
}

/// ページの写しのテストの前提確認。poppler 一式(pdftoppm・pdftocairo)が無い環境では
/// 黙って飛ばさず、導入手順を示して失敗する(require_pdftotext と同じ扱い。飛ばして緑に
/// すると、検証したのか検証を諦めたのかが結果から区別できなくなる)。
///
/// 探索は本番と同じ道(uniqnode::rendition::located_tool)を通す。この機械では PATH に
/// pdftotext だけを symlink してあり、pdftoppm は pdftotext の実体の隣にしかない。
/// Command::new("pdftoppm") で確かめると、serve は見つけられるのにテストだけが落ちる。
pub fn require_poppler() {
    require_pdftotext();
    for tool in [uniqnode::rendition::Tool::Pdftoppm, uniqnode::rendition::Tool::Pdftocairo] {
        if let Err(error) = uniqnode::rendition::located_tool(tool) {
            panic!("ページの写しのテストには poppler 一式が必要: {error}");
        }
    }
}

/// 埋め込みのテストの前提確認。埋め込みサーバが無い環境では黙って飛ばさず、起動の手順を
/// 示して失敗する(docs/design/TESTING.md の外部コマンドの規約。require_pdftotext と同じ
/// 扱い)。飛ばして緑にすると、検証したのか検証を諦めたのかが結果から区別できなくなる。
///
/// 生存の判定は実際に 1 本埋め込んでみることで行う。GET /v1/models の capabilities は
/// この llama.cpp の版では常に ["completion"] を返し、埋め込みに対応しているかどうかの
/// 判定に使えない(実測)ためである(should/0116: 設定ではなく観測された効果で確かめる)。
pub fn require_embedding_server() -> uniqnode::embed::Embedder {
    let embedder = uniqnode::embed::Embedder::new(
        uniqnode::embed::DEFAULT_EMBEDDING_URL,
        uniqnode::embed::DEFAULT_EMBEDDER_ID,
    )
    .expect("既定の埋め込み設定");
    if let Err(error) = embedder.embed_query("疎通確認") {
        panic!(
            "埋め込みのテストには {} で待ち受ける埋め込みサーバが必要({error})。\
             起動例: llama-server --model /work2/llm/models/bge-m3/bge-m3-FP16.gguf \
             --host 127.0.0.1 --port 8083 --embeddings --pooling cls --embd-normalize 2 \
             -ngl 99 --ctx-size 8192 --parallel 4 --batch-size 2048 --ubatch-size 2048",
            embedder.endpoint()
        );
    }
    embedder
}

pub fn unique_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("uniqnode-test-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
    dir
}

pub fn start_server_at(dir: PathBuf) -> Server {
    start_server_at_with(dir, &[], &[], false)
}

/// 標準エラーを捕まえて起こす(ログファイルの内容と突き合わせるテスト用)。捕まえた
/// 標準エラーは Server::finish で読み切る。
pub fn start_server_capturing_stderr(name: &str, args: &[&str]) -> Server {
    start_server_at_with(unique_dir(name), &[], args, true)
}

/// 環境変数を差し替えて serve を起動する(PATH を空にして pdftotext 不在の環境を
/// 再現する用)。
pub fn start_server_with_env(name: &str, envs: &[(&str, &str)]) -> Server {
    start_server_at_with(unique_dir(name), envs, &[], false)
}

/// serve に追加の引数を与えて起動する(--embed など)。
pub fn start_server_with_args(name: &str, args: &[&str]) -> Server {
    start_server_at_with(unique_dir(name), &[], args, false)
}

/// 既存のディレクトリに対して追加の引数つきで起動する(取り込み済みのストアを
/// 使い回すテスト用)。
pub fn start_server_at_with_args(dir: PathBuf, args: &[&str]) -> Server {
    let mut server = start_server_at_with(dir, &[], args, false);
    server.remove_dir_on_drop = false;
    server
}

fn start_server_at_with(
    dir: PathBuf,
    envs: &[(&str, &str)],
    args: &[&str],
    capture_stderr: bool,
) -> Server {
    let mut command = Command::new(env!("CARGO_BIN_EXE_uniqnode"));
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .args(["serve", dir.to_str().expect("utf-8"), "127.0.0.1:0"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(if capture_stderr { Stdio::piped() } else { Stdio::inherit() })
        .spawn()
        .expect("spawn serve");
    let stderr = child.stderr.take();
    let stdout = child.stdout.take().expect("stdout");
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).expect("read listening line");
    let address = line
        .trim()
        .strip_prefix("listening on ")
        .expect("listening line")
        .to_string();
    Server { child, address, dir, remove_dir_on_drop: true, stderr }
}

pub fn start_server(name: &str) -> Server {
    start_server_at(unique_dir(name))
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// 名乗られた Content-Type(ヘッダが無ければ空)。ページの写しは何であるかを
    /// 型で名乗るので、本文だけでなくヘッダも検証の対象になる。
    pub content_type: String,
    /// 名乗られた Content-Security-Policy(ヘッダが無ければ空)。取り込んだ紙面を
    /// 砂場で描かせる印なので、これも検証の対象になる。
    pub content_security_policy: String,
}

pub fn read_response(reader: &mut BufReader<TcpStream>) -> HttpResponse {
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("status line");
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    let mut content_length = 0usize;
    let mut content_type = String::new();
    let mut content_security_policy = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("header line");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().expect("content length");
            }
            if name.trim().eq_ignore_ascii_case("content-type") {
                content_type = value.trim().to_string();
            }
            if name.trim().eq_ignore_ascii_case("content-security-policy") {
                content_security_policy = value.trim().to_string();
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).expect("body");
    HttpResponse { status, body, content_type, content_security_policy }
}

pub fn simple(address: &str, method: &str, path: &str, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(address).expect("connect");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    let mut reader = BufReader::new(stream);
    read_response(&mut reader)
}

pub fn body_text(response: &HttpResponse) -> String {
    String::from_utf8(response.body.clone()).expect("utf-8 body")
}

/// 応答 JSON から "key":"value" の value を取り出す(テスト用の素朴な抽出)。
pub fn json_text_field(text: &str, key: &str) -> Option<String> {
    text.split(&format!("\"{key}\":\""))
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|s| s.to_string())
}

/// 応答 JSON から "key":<integer> を取り出す。
pub fn json_integer_field(text: &str, key: &str) -> Option<i64> {
    let rest = text.split(&format!("\"{key}\":")).nth(1)?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

pub fn put_object(address: &str, bytes: &[u8]) -> String {
    let response = simple(address, "POST", "/v1/objects", bytes);
    assert!(
        response.status == 200 || response.status == 201,
        "put failed: {} {}",
        response.status,
        body_text(&response)
    );
    json_text_field(&body_text(&response), "id").expect("id in response")
}

pub fn put_ref(address: &str, path: &str, target: Option<&str>) {
    let body = match target {
        Some(t) => format!("{{\"target\":\"{t}\"}}"),
        None => "{\"target\":null}".to_string(),
    };
    let response = simple(address, "PUT", &format!("/v1/refs/{path}"), body.as_bytes());
    assert_eq!(response.status, 200, "{}", body_text(&response));
}
