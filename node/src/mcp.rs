//! MCP(Model Context Protocol)アダプタ: 標準入出力の JSON-RPC 2.0 で、読む 2 ツール
//! (search・fetch)と、`--writable` で許したコレクションへ書く 2 ツール(add_document・
//! fetch_url)を公開する(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9))。
//!
//! コアはあくまで REST であり、この層は薄い被せ物である。検索の判断(方式の既定・劣化の
//! 判断・引用の組み立て)は node/src/api.rs の run_search が持ち、全文の取得は
//! fetch_object が持つ。書き込みも同じで、文書の取り込みは api.rs の put_document、URL
//! からの取り込みは fetch_into が持つ(REST の PUT documents・POST fetch と同じ関数)。
//! ここがするのは JSON-RPC の封筒の付け外しと、LLM が読む形への整形だけである(検索と
//! 取り込みの判断を二重に実装しない。should/0135)。
//!
//! 書き込みは既定で閉じている。`--writable <コレクション名>` を与えたコレクションにだけ
//! 書け、1 つも無ければ書くツールは tools/list に載せない(書けない相手に書くツールを
//! 見せない)。許していないコレクションへの書き込みは、ツールの失敗(isError)として理由を
//! 言って断る。
//!
//! 標準出力はプロトコル専用である(MCP の stdio 転送の規定)。1 行が 1 メッセージで、
//! 行に混ざった非メッセージは相手の解析をその場で壊す。ログはすべて標準エラーへ出す。
//! 応答の直列化は c1 の正規形を通すので、改行や制御文字は \u00xx に畳まれ、1 メッセージが
//! 1 行に収まることが直列化の側から保証される(実装を増やさない。should/0135)。
//!
//! 受け取り側の既知の制限: 要求の解析は c1(SPEC §4.1)を使うため、整数しか受けない。
//! JSON-RPC 自体と、これらのツールの引数はすべて文字列・整数・真偽値・オブジェクトなので
//! 足りるが、小数を含む要求は解析誤り(-32700)として拒む。黙って読み飛ばさない。
//!
//! ツールの後ろ盾は二つの形がある(Backend)。ストアを直接開く形(Local)は起動から終了
//! までストアの排他ロックを持つので、常駐している間 CLI の ingest・embed は断られる。走って
//! いる serve の REST へ転送する形(Forward)はロックを一切取らないので、エージェントを繋いだ
//! まま取り込みと埋め込みを回せる。整形は一つで、どちらの形も同じ render_search /
//! render_fetch を通る(検索の判断も整形も二重に実装しない。should/0135)。
//!
//! 実行ファイルが更新されたら、自分を exec で差し替える(StdioServer::replace_if_updated)。
//! execve はプロセスイメージを入れ替えるがファイル記述子は保つので、相手が握るパイプの
//! 反対側と PID は変わらず、差し替えはクライアントから見えない。位置は「応答を書き終えた
//! 直後、次の読み込みの前」だけで、先読みバッファが空であることを確かめてから行う。

use crate::api::{self, ApiContext, Citation, Fetched, SearchRequest, SearchResults};
use crate::c1::{self, Value};
use crate::clock::format_unix_time;
use crate::http;
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// initialize で答えるプロトコル版。相手が別の版を求めても、規定は「サーバが対応する
/// 版を答える」であって誤りにはしない。Claude Code は 2025-11-25 を求めたうえで、この
/// 版への引き下げを受け入れる(実測。接続に成功する)。
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// サーバの名前(登録側が `mcp__<name>__<tool>` の形でツール名を組む)。
pub const SERVER_NAME: &str = "uniqnode";

/// JSON-RPC 2.0 の誤り符号。
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// スニペットではなく全文が要るときの案内(search の結果の末尾に置く)。ツールの説明と
/// 同じ事実なので、文言はこの 1 箇所から出す(must/0023)。
const FETCH_HINT: &str = "To read the full text, pass the chunk ID to the fetch tool.";

/// 転送する形が serve を待つ期限(1 要求ぶん)。初回の検索は索引の構築ぶんだけ待つので
/// (実データ規模で約 9 秒)、埋め込みのクエリ期限(15 秒)より長く採る。期限のない待ちは
/// 作らない(should/0104)。
const SERVE_TIMEOUT: Duration = Duration::from_secs(60);

/// fetch_url で serve を待つ期限。serve は curl が取り終わるまで答えないので、curl の
/// 期限(node/src/fetch.rs の DEFAULT_MAX_SECONDS)を通常の期限に足す。
const FETCH_URL_TIMEOUT: Duration =
    Duration::from_secs(60 + crate::fetch::DEFAULT_MAX_SECONDS);

/// 転送する形の ingest(`uniqnode ingest … --serve-url`)が PUT 1 件を待つ期限。serve は
/// PDF なら pdftotext とセクションの見出しの復元が終わるまで答えず、千ページ級の仕様書ではそれが
/// 数十秒かかる(直接開く形の ingest には期限が無い)。add_document の文書は人が書く
/// 短い本文なので SERVE_TIMEOUT のままにし、ここだけ長く採る(should/0104)。
const INGEST_PUT_TIMEOUT: Duration = Duration::from_secs(600);

/// add_document が受ける media と、PUT documents へ渡す拡張子。種別の判定そのものは
/// 拡張子の表(crate::ingest::media_for_extension)が持ち、ここはその逆引きである。両者が
/// 噛み合うことは単体試験 add_document_media_round_trips_through_the_extension_table が
/// 確かめる(should/0135)。先頭が既定。
const ADD_DOCUMENT_MEDIA: [(&str, &str); 2] = [("markdown", "md"), ("text", "txt")];

/// 自己置換の前に新しいイメージを確かめるサブコマンド。呼ぶ側(このモジュール)と
/// 答える側(node/src/main.rs)で同じ文字列を使う(must/0023)。
pub const SELF_CHECK_COMMAND: &str = "selfcheck";

/// 自己検査が健全なときに標準出力へ書くマーク。exec の前にこのマークを確かめるので、たまたま
/// 終了コード 0 で終わる別の実行ファイル(置き換え事故)を健全とみなさない。
pub const SELF_CHECK_MARKER: &str = "uniqnode selfcheck ok";

/// 自己検査の子プロセスを待つ期限。応答と応答のあいだで待つので短く採る。
const SELF_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// 自己検査の子プロセスの終了を見に行く間隔。
const SELF_CHECK_POLL: Duration = Duration::from_millis(5);

/// 自己置換のとき、ハンドシェイクの事実を新しいイメージへ渡す環境変数。新しいイメージは
/// initialize を受けた事実を忘れており、Claude Code は再送しない。渡す側と読む側で同じ
/// 名前を使う(must/0023)。
pub const HANDSHAKE_PROTOCOL_ENV: &str = "UNIQNODE_MCP_HANDSHAKE_PROTOCOL";
pub const HANDSHAKE_CLIENT_ENV: &str = "UNIQNODE_MCP_HANDSHAKE_CLIENT";

/// ツールの後ろ盾。ストアを直接開く形と、走っている serve の REST へ転送する形の二つ。
///
/// 転送する形はストアのロックを一切取らない。これが要点である: Claude Code に登録した MCP
/// サーバが常駐していても、同じデータディレクトリに対する ingest・embed が通る。検索の
/// 判断(方式の既定・劣化・引用の組み立て)はどちらの形でも serve と同じ 1 箇所
/// (node/src/api.rs の run_search)にあり、転送する形はその答えを REST の応答から
/// 組み直すだけである(should/0135)。
pub enum Backend {
    /// ストアを直接開く形。起動から終了まで排他ロックを持つ(serve と同じ制約)。
    Local(Box<ApiContext>),
    /// 走っている serve へ転送する形。ロックを取らない。
    Forward(ServeClient),
}

impl Backend {
    /// 検索 1 回。誤りは LLM が読む文にして返す(ツール実行の失敗は isError の結果)。
    fn search(&self, request: &SearchRequest) -> Result<SearchResults, String> {
        match self {
            Backend::Local(context) => {
                // 自分のために引くので、共有ポリシーの絞り込みは掛からない(絞りが要る
                // のは、ピアの QUERY に答えるときだけである)。
                api::run_search(context, request, &crate::search::CollectionScope::All)
                    .map_err(|error| format!("{error}"))
            }
            Backend::Forward(client) => client.search(request),
        }
    }

    /// 全文 1 件(ローカルに無ければ None)。
    fn fetch(&self, id: &str) -> Result<Option<Fetched>, String> {
        match self {
            Backend::Local(context) => {
                api::fetch_object(context, id).map_err(|error| format!("{error}"))
            }
            Backend::Forward(client) => client.fetch(id),
        }
    }

    /// 文書 1 件の書き込み(add_document)。file_name は拡張子つきで、種別は api.rs の
    /// put_document が拡張子から判定する(ここでは判定しない。should/0135)。
    fn put_document(
        &self,
        collection: &str,
        file_name: &str,
        body: &[u8],
    ) -> Result<Written, String> {
        match self {
            Backend::Local(context) => {
                // 出所の meta は持たない(ストアを直接開く形の add_document は操作者の
                // 手元の道具で、出所は署名者で足りる)。
                Ok(Written::from(api::put_document(context, collection, file_name, body, &[])))
            }
            Backend::Forward(client) => {
                client.put_document(collection, file_name, body, SERVE_TIMEOUT)
            }
        }
    }

    /// URL からの取り込み(fetch_url)。body は POST fetch のボディと同じ JSON。
    fn fetch_url(&self, collection: &str, body: &[u8]) -> Result<Written, String> {
        match self {
            Backend::Local(context) => Ok(Written::from(api::fetch_into(context, collection, body))),
            Backend::Forward(client) => client.fetch_url(collection, body),
        }
    }

    /// 起動の知らせに載せる、この形の説明。
    fn description(&self) -> String {
        match self {
            Backend::Local(_) => "opening the store directly (holding the exclusive lock)".to_string(),
            Backend::Forward(client) => {
                format!("forwarding to {} (taking no store lock)", client.url)
            }
        }
    }
}

/// 書き込みの口(PUT documents・POST fetch)の答え: 状態と JSON の本文。ストアを直接
/// 開く形は api.rs の handler の Response から、転送する形は serve の HTTP 応答から、同じ
/// 形に写す。読むのは written_document の 1 箇所である(should/0135)。
pub struct Written {
    status: u16,
    body: Vec<u8>,
}

impl From<http::Response> for Written {
    fn from(response: http::Response) -> Written {
        Written { status: response.status, body: response.body }
    }
}

impl From<http::ClientResponse> for Written {
    fn from(response: http::ClientResponse) -> Written {
        Written { status: response.status, body: response.body }
    }
}

/// PUT documents の道(転送する形が呼ぶ REST の道であり、失敗の理由にもこの字面で出す)。
fn document_path(collection: &str, file_name: &str) -> String {
    format!("/v1/collections/{collection}/documents/{file_name}")
}

/// POST fetch の道。
fn fetch_path(collection: &str) -> String {
    format!("/v1/collections/{collection}/fetch")
}

/// 走っている serve への最小のクライアント。検索は POST /v1/search、全文は
/// GET /v1/objects/{id}、その出典は GET /v1/objects/{id}/citation、書き込みは
/// PUT /v1/collections/{c}/documents/{name} と POST /v1/collections/{c}/fetch を呼ぶ。
pub struct ServeClient {
    /// 接続先(host:port)。
    address: String,
    /// 与えられた URL(理由に出す。どこへ届かなかったのかを言うため)。
    url: String,
    /// 転送先の serve が開いているはずのデータディレクトリ(起動コマンドの案内に使う)。
    data_dir: String,
}

impl ServeClient {
    /// URL とデータディレクトリから組む。誤った URL はここで断る。
    pub fn new(url: &str, data_dir: &str) -> Result<ServeClient, String> {
        let (address, path) = http::split_http_url(url)?;
        if path != "/" {
            return Err(format!(
                "{url}: must be a URL to the root of serve (for example http://127.0.0.1:7440)"
            ));
        }
        Ok(ServeClient {
            address,
            url: url.to_string(),
            data_dir: data_dir.to_string(),
        })
    }

    /// 届かないときの文。原因と、その serve を起こすコマンドを添える。黙って失敗せず、
    /// 読んだ者が次の手を打てる形で言う(must/0022)。
    fn unreachable(&self, cause: &str) -> String {
        format!(
            "cannot reach a running serve ({}): {cause}; \
             the forwarding form (--serve-url of mcp and ingest) does not open the store itself, \
             so start serve first: \
             uniqnode serve {} {}",
            self.url, self.data_dir, self.address
        )
    }

    /// 相手が誤りを返したときの文(届いてはいるので、起動の案内は付けない)。
    fn refused(&self, what: &str, status: u16, body: &[u8]) -> String {
        format!("{what} of serve returned {status} ({}): {}", self.url, http::body_head(body))
    }

    fn search(&self, request: &SearchRequest) -> Result<SearchResults, String> {
        let body = api::search_request_body(request);
        let response = http::post_json(&self.address, "/v1/search", &body, SERVE_TIMEOUT)
            .map_err(|error| self.unreachable(&error))?;
        if response.status != 200 {
            return Err(self.refused("POST /v1/search", response.status, &response.body));
        }
        api::parse_search_response(&response.body)
            .map_err(|error| format!("cannot read the search response of serve ({}): {error}", self.url))
    }

    fn fetch(&self, id: &str) -> Result<Option<Fetched>, String> {
        let response = http::get(&self.address, &format!("/v1/objects/{id}"), SERVE_TIMEOUT)
            .map_err(|error| self.unreachable(&error))?;
        match response.status {
            200 => {}
            // 404 は「この serve のストアが持っていない」というローカルな事実である。
            404 => return Ok(None),
            status => {
                return Err(self.refused(&format!("GET /v1/objects/{id}"), status, &response.body))
            }
        }
        match api::classify_object(response.body) {
            // 出典は全文とは別に取る(REST の GET /v1/objects/{id} は生のバイト列だけを
            // 返す)。出典の組み立ては serve 側の索引が持つので、search が示した出典と
            // 一致する。
            Fetched::Chunk { text, .. } => {
                let citation = self.citation(id)?;
                Ok(Some(Fetched::Chunk { text, citation }))
            }
            other => Ok(Some(other)),
        }
    }

    /// 文書 1 件を PUT する。4xx/5xx は読み替えずそのまま返し、読むのは呼び手の
    /// written_document である(届かないときだけがここの失敗)。
    fn put_document(
        &self,
        collection: &str,
        file_name: &str,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Written, String> {
        let path = document_path(collection, file_name);
        http::request(
            &self.address,
            "PUT",
            &path,
            Some(("application/octet-stream", body)),
            timeout,
        )
        .map(Written::from)
        .map_err(|error| self.unreachable(&error))
    }

    /// 転送する形の ingest が文書 1 件を PUT し、応答の欄(doc_rev・new_objects・
    /// ref_updated・previous)を返す。2xx でなければ serve が返した理由(応答の error、
    /// 無ければ本文の頭)を載せて失敗する。読み方は add_document と同じ written_document
    /// の 1 箇所である(should/0135)。
    pub fn ingest_document(
        &self,
        collection: &str,
        file_name: &str,
        body: &[u8],
    ) -> Result<BTreeMap<String, Value>, String> {
        let written = self.put_document(collection, file_name, body, INGEST_PUT_TIMEOUT)?;
        written_document(&format!("PUT {}", document_path(collection, file_name)), written)
    }

    /// URL からの取り込みを POST する。
    fn fetch_url(&self, collection: &str, body: &[u8]) -> Result<Written, String> {
        http::post_json(&self.address, &fetch_path(collection), body, FETCH_URL_TIMEOUT)
            .map(Written::from)
            .map_err(|error| self.unreachable(&error))
    }

    fn citation(&self, id: &str) -> Result<Option<Citation>, String> {
        let path = format!("/v1/objects/{id}/citation");
        let response = http::get(&self.address, &path, SERVE_TIMEOUT)
            .map_err(|error| self.unreachable(&error))?;
        if response.status != 200 {
            return Err(self.refused(&format!("GET {path}"), response.status, &response.body));
        }
        let text = std::str::from_utf8(&response.body)
            .map_err(|_| format!("the response of serve to {path} is not UTF-8"))?;
        let value = crate::json::Json::parse(text)
            .map_err(|error| format!("cannot read the response of serve to {path}: {error}"))?;
        match value.field("citation") {
            None => Err(format!("the response of serve to {path} has no citation")),
            Some(crate::json::Json::Null) => Ok(None),
            Some(found) => api::citation_from_json(found)
                .map(Some)
                .map_err(|error| format!("cannot read the citation from serve's {path}: {error}")),
        }
    }
}

/// 標準入力の読み手。1 行読むことに加えて、先読みバッファに未処理のバイトが残って
/// いるかを答える。自己置換の前にこれを確かめる: BufReader が次のメッセージまで読んで
/// いると、そのバイト列は旧イメージと共に消えるからである。
pub trait MessageInput: BufRead {
    fn has_buffered_bytes(&self) -> bool;
}

impl<R: Read> MessageInput for std::io::BufReader<R> {
    fn has_buffered_bytes(&self) -> bool {
        !self.buffer().is_empty()
    }
}

/// initialize で交渉した内容。自己置換のとき新しいイメージへ引き継ぐ(新しいイメージは
/// initialize を受けた事実を忘れており、Claude Code は再送しない)。
#[derive(Default)]
struct Handshake {
    /// 相手が求めたプロトコル版(こちらが答えるのは PROTOCOL_VERSION)。
    protocol: Option<String>,
    /// 相手の名乗り(`<name>/<version>`)。
    client: Option<String>,
}

impl Handshake {
    /// 前のイメージからの引き継ぎ(環境変数)。空文字は無かったものと同じに扱う。
    fn inherited() -> Handshake {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Handshake { protocol: read(HANDSHAKE_PROTOCOL_ENV), client: read(HANDSHAKE_CLIENT_ENV) }
    }
}

/// 標準入出力で MCP を話すサーバ。後ろ盾(ストアを直接開く形か、serve へ転送する形)と、
/// 起動時に控えた実行ファイルの姿と、交渉した内容を持つ。
pub struct StdioServer {
    backend: Backend,
    /// 書き込みを許すコレクション(`--writable`。与えられた順)。空なら書くツールを
    /// 出さない。
    writable: Vec<String>,
    /// 起動時に控えた実行ファイルの姿。None なら自己置換をしない。
    binary: Option<BinaryStamp>,
    handshake: Handshake,
}

impl StdioServer {
    /// 後ろ盾と、書き込みを許すコレクションを受けて組む。実行ファイルの姿はここで控える:
    /// 置き換えられた後の /proc/self/exe は "(deleted)" 付きの読めない道になるので、
    /// 比較の相手は起動時のパスでなければならない。
    pub fn new(backend: Backend, writable: Vec<String>) -> StdioServer {
        let handshake = Handshake::inherited();
        if let Some(protocol) = &handshake.protocol {
            crate::log_line!(
                "uniqnode: mcp: started with the handshake already done (peer protocol {protocol}, \
                 client {})",
                handshake.client.as_deref().unwrap_or("(unnamed)")
            );
        }
        StdioServer { backend, writable, binary: BinaryStamp::of_current_exe(), handshake }
    }

    /// 起動の知らせ(標準出力はプロトコル専用なので標準エラーへ出す)。出すツールと、
    /// 書き込みを許すコレクションもここで言う。
    pub fn announce(&self, target: &str) {
        let tools = if self.writable.is_empty() {
            "search, fetch (read only; --writable allows writes)".to_string()
        } else {
            format!(
                "search, fetch, add_document, fetch_url (writes allowed to: {})",
                self.writable.join(", ")
            )
        };
        crate::log_line!(
            "uniqnode: mcp: serving {target} on stdio (protocol {PROTOCOL_VERSION}, \
             tools: {tools}, {})",
            self.backend.description()
        );
    }

    /// 標準入出力で話す(相手が標準入力を閉じたら終わる)。
    ///
    /// 1 行読んで 1 行書く。応答を書くたびに flush するのは、相手が次の要求を出す前に
    /// この応答を読み切る必要があるためである(パイプの buffer に残したまま待つと、
    /// 双方が相手を待って止まる)。自己置換を試すのは、応答を書き終えた直後、次の
    /// 読み込みの前だけである。
    pub fn serve(
        &mut self,
        input: &mut dyn MessageInput,
        output: &mut dyn Write,
    ) -> std::io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if input.read_line(&mut line)? == 0 {
                // 標準入力の EOF は相手が閉じたということ。速やかに終える。
                return Ok(());
            }
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(response) = self.handle_message(&trimmed) {
                output.write_all(response.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
            // ここが差し替えの位置である。メッセージの処理の途中で入れ替えると、
            // 読んだ要求が旧イメージと共に消える。
            self.replace_if_updated(input);
        }
    }

    /// メッセージ 1 本の処理。応答を返すのは要求(id を持つもの)だけで、通知(id を持た
    /// ないもの)には何も返さない(返すと相手の解析が壊れる)。
    pub fn handle_message(&mut self, line: &str) -> Option<String> {
        let message = match c1::parse(line) {
            Ok(value) => value,
            // 解析できなければ id も読めないので、規定どおり id は null で答える。
            Err(error) => {
                return Some(failure(&Value::Null, PARSE_ERROR, &format!("invalid JSON: {error}")))
            }
        };
        let Value::Object(map) = &message else {
            return Some(failure(&Value::Null, INVALID_REQUEST, "the request must be an object"));
        };
        // id が無い(または null)のは通知である。以後、応答を返さない道はここで分かれる。
        let id = match map.get("id") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.clone()),
        };
        let method = match map.get("method") {
            Some(Value::Text(method)) => method.as_str(),
            _ => {
                let id = id?;
                return Some(failure(&id, INVALID_REQUEST, "no method (a string)"));
            }
        };
        if map.get("jsonrpc") != Some(&Value::Text("2.0".to_string())) {
            let id = id?;
            return Some(failure(&id, INVALID_REQUEST, "jsonrpc must be \"2.0\""));
        }
        let params = map.get("params");
        let Some(id) = id else {
            // 通知。initialized と cancelled は受理して黙る。知らない通知も、応答を返して
            // はならない以上ここで捨てるほかないが、標準エラーには残す(must/0022)。
            if method != "notifications/initialized" && method != "notifications/cancelled" {
                crate::log_line!("uniqnode: mcp: ignored an unknown notification: {method}");
            }
            return None;
        };
        Some(match method {
            "initialize" => success(&id, self.initialize_result(params)),
            // 生存確認。空の結果が規定の答えである。
            "ping" => success(&id, Value::Object(BTreeMap::new())),
            "tools/list" => success(&id, tools_result(&self.writable)),
            "tools/call" => call_tool(&self.backend, &self.writable, &id, params),
            other => failure(&id, METHOD_NOT_FOUND, &format!("unknown method: {other}")),
        })
    }

    /// initialize の結果。capabilities は実装しているものだけを載せる(tools だけ)。
    /// 相手が求めた版に対応していればその版で、していなければこちらの版で答える。
    /// 交渉した内容は控える(自己置換のとき新しいイメージへ引き継ぐため)。
    fn initialize_result(&mut self, params: Option<&Value>) -> Value {
        if let Some(Value::Object(map)) = params {
            if let Some(Value::Text(requested)) = map.get("protocolVersion") {
                self.handshake.protocol = Some(requested.clone());
                if requested != PROTOCOL_VERSION {
                    crate::log_line!(
                        "uniqnode: mcp: the peer asked for protocol {requested}; \
                         answering with {PROTOCOL_VERSION}"
                    );
                }
            }
            if let Some(Value::Object(info)) = map.get("clientInfo") {
                let field = |name: &str| match info.get(name) {
                    Some(Value::Text(value)) => value.clone(),
                    _ => "(unknown)".to_string(),
                };
                self.handshake.client = Some(format!("{}/{}", field("name"), field("version")));
            }
        }
        object(vec![
            ("protocolVersion", text(PROTOCOL_VERSION)),
            (
                "capabilities",
                object(vec![("tools", object(vec![("listChanged", Value::Bool(false))]))]),
            ),
            (
                "serverInfo",
                object(vec![
                    ("name", text(SERVER_NAME)),
                    ("version", text(env!("CARGO_PKG_VERSION"))),
                    ("title", text("uniqnode RAG storage")),
                ]),
            ),
            ("instructions", text(&self.instructions())),
        ])
    }

    /// initialize の instructions。読む道具の使い方に、書き込みを許しているときだけ書く
    /// 道具の使い方を足す(許していないのに書けると言わない)。
    fn instructions(&self) -> String {
        let mut instructions = "uniqnode is a knowledge store that searches ingested documents \
             chunk by chunk. Ask with search, and add the citations it returns (document, page, \
             headings, retrieval time) to your answer. If a snippet is not enough, pass the chunk \
             ID to fetch to read the full text."
            .to_string();
        if !self.writable.is_empty() {
            instructions.push_str(&format!(
                // run-time-text-exempt: 引用の句は利用者が言う言葉の代わり(should/0141 の範囲外)。
                " When the user says \"覚えておいて\" (remember this) or \"保存して\" (save this), \
                 use add_document; when asked to ingest a URL, use fetch_url; write to the \
                 collections that allow writes ({}).",
                self.writable.join(", ")
            ));
        }
        instructions
    }

    /// 実行ファイルが更新されていたら、自分を exec で差し替える。
    ///
    /// execve はプロセスイメージを入れ替えるがファイル記述子は保つので、相手が握って
    /// いるパイプの反対側と PID は変わらない。呼ぶのは応答を書き終えた直後だけである
    /// (処理の途中で入れ替えると、読んだ要求が旧イメージと共に消える)。
    fn replace_if_updated(&mut self, input: &dyn MessageInput) {
        let Some(stamp) = &self.binary else { return };
        let Some(current) = BinaryStamp::read(&stamp.path) else {
            // 一瞬だけ読めない(置き換えの最中など)ことはある。次の機会に見る。
            return;
        };
        if !stamp.differs_from(&current) {
            return;
        }
        if input.has_buffered_bytes() {
            // 先読みバッファに次のメッセージが載っている。ここで exec すると、その
            // バイト列は旧イメージと共に消える。差し替えは次の機会に回す。
            crate::log_line!(
                "uniqnode: mcp: the executable was updated, but the read-ahead buffer holds the next \
                 message; replacing after the next response"
            );
            return;
        }
        // 壊れたバイナリで自分を置き換えると MCP が死に、結局セッションの再起動が要る。
        // exec の前に、新しいイメージを子プロセスとして起こして健全さを確かめる。
        if let Err(reason) = self_check(&stamp.path) {
            crate::log_line!(
                "uniqnode: mcp: the new executable failed its self-check; not replacing, \
                 continuing with the old image: {reason}"
            );
            // 同じ姿を応答のたびに検査し直さない。次にまた変わったときに試す。
            self.binary = Some(current);
            return;
        }
        crate::log_line!(
            "uniqnode: mcp: the executable was updated; replacing itself with exec: {}",
            stamp.path.display()
        );
        let path = stamp.path.clone();
        let error = self.exec_replacement(&path);
        // exec が返るのは失敗したときだけである(成功すればこの行は無い)。
        crate::log_line!("uniqnode: mcp: exec failed; continuing with the old image: {error}");
        self.binary = Some(current);
    }

    /// 新しいイメージへの exec。引数はそのまま渡し、ハンドシェイクの事実は環境変数で
    /// 引き継ぐ(新しいイメージは initialize を受けたことを忘れており、Claude Code は
    /// 再送しない)。ストアの排他ロックは CLOEXEC 付きの FD なので exec で解放され、新しい
    /// イメージが取り直す(取り直しの隙間は acquire_lock の有界再試行が吸収する)。
    fn exec_replacement(&self, path: &Path) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new(path);
        command.args(std::env::args_os().skip(1));
        if let Some(protocol) = &self.handshake.protocol {
            command.env(HANDSHAKE_PROTOCOL_ENV, protocol);
        }
        if let Some(client) = &self.handshake.client {
            command.env(HANDSHAKE_CLIENT_ENV, client);
        }
        command.exec()
    }
}

/// 起動時に控える実行ファイルの姿(更新の検出に使う)。パスも一緒に控えるのは、
/// 置き換えられた後の /proc/self/exe が "(deleted)" 付きの読めない道になるためである。
struct BinaryStamp {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    len: u64,
    inode: u64,
}

impl BinaryStamp {
    /// 起動時の記録。位置か属性が読めなければ自己置換をしない(黙って諦めず理由を言う。
    /// must/0022)。
    fn of_current_exe() -> Option<BinaryStamp> {
        let path = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                crate::log_line!(
                    "uniqnode: mcp: cannot find the executable; not replacing itself: {error}"
                );
                return None;
            }
        };
        let stamp = BinaryStamp::read(&path);
        if stamp.is_none() {
            crate::log_line!(
                "uniqnode: mcp: cannot read the attributes of {}; not replacing itself",
                path.display()
            );
        }
        stamp
    }

    fn read(path: &Path) -> Option<BinaryStamp> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        Some(BinaryStamp {
            path: path.to_path_buf(),
            modified: metadata.modified().ok(),
            len: metadata.len(),
            inode: metadata.ino(),
        })
    }

    /// 更新の検出。cargo は新しい実行ファイルを別の inode で置くので inode だけでも
    /// 足りるが、同じ inode を書き換える置き方(パッチ当て)も拾えるように更新時刻と
    /// 大きさも見る。
    fn differs_from(&self, other: &BinaryStamp) -> bool {
        self.modified != other.modified || self.len != other.len || self.inode != other.inode
    }
}

/// 自己検査(exec の前の防御)。新しいイメージを子プロセスとして起こし、規定のマークを
/// 標準出力に出して正常終了することを確かめる。終了コードだけを見ないのは、たまたま 0 で
/// 終わる別の実行ファイルを健全とみなさないためである。
fn self_check(path: &Path) -> Result<(), String> {
    let mut child = Command::new(path)
        .arg(SELF_CHECK_COMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", path.display()))?;
    let deadline = Instant::now() + SELF_CHECK_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(format!("cannot wait for the self-check child: {error}")),
        }
        if Instant::now() >= deadline {
            // 終わらない新しいイメージも健全ではない。始末してから断る。
            if let Err(error) = child.kill() {
                crate::log_line!("uniqnode: mcp: cannot kill the self-check child: {error}");
            }
            if let Err(error) = child.wait() {
                crate::log_line!("uniqnode: mcp: cannot wait for the self-check child: {error}");
            }
            return Err(format!("the self-check did not finish within {SELF_CHECK_TIMEOUT:?}"));
        }
        std::thread::sleep(SELF_CHECK_POLL);
    };
    let mut reported = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_string(&mut reported)
            .map_err(|error| format!("cannot read the standard output of the self-check: {error}"))?;
    }
    if !status.success() {
        return Err(format!("{} {SELF_CHECK_COMMAND} ended with {status}", path.display()));
    }
    if !reported.contains(SELF_CHECK_MARKER) {
        return Err(format!(
            "the standard output of {} {SELF_CHECK_COMMAND} lacks \"{SELF_CHECK_MARKER}\": {reported:?}",
            path.display()
        ));
    }
    Ok(())
}

/// 自己検査が標準出力へ書く 1 行(uniqnode selfcheck)。ツールの記述を実際に組み立てて
/// 数えるので、新しいイメージがこの層まで動くことを確かめられる。文言の家はここである
/// (書く側と読む側が同じマークを使う。must/0023)。
pub fn self_check_report() -> String {
    // 書くツールの記述まで組み立てる(起動の指定に関わらず、この層の全部が動くことを
    // 確かめる)。コレクション名は記述の文言に入るだけで、何にも書かない。
    let sample = ["notes".to_string()];
    let tools = match tool_descriptors(&sample) {
        Value::Array(tools) => tools.len(),
        _ => 0,
    };
    format!("{SELF_CHECK_MARKER} version={} tools={tools}", env!("CARGO_PKG_VERSION"))
}

/// tools/list の結果。ツールの一覧は result.tools に入る(result そのものを配列に
/// してはならない)。ページ分割はしないので nextCursor は載せない。
fn tools_result(writable: &[String]) -> Value {
    object(vec![("tools", tool_descriptors(writable))])
}

/// 公開するツールの記述。読む 2 本(search・fetch)は常に、書く 2 本(add_document・
/// fetch_url)は書き込みを許すコレクションが 1 つでもあるときだけ出す。
fn tool_descriptors(writable: &[String]) -> Value {
    let mut tools = read_tool_descriptors();
    if !writable.is_empty() {
        tools.extend(write_tool_descriptors(writable));
    }
    Value::Array(tools)
}

/// 読む 2 ツールの記述。
fn read_tool_descriptors() -> Vec<Value> {
    let search_properties = object(vec![
        (
            "query",
            schema_property("string", "the query (a sentence or keywords)"),
        ),
        (
            "collection",
            schema_property("string", "the collection to search (all collections if omitted)"),
        ),
        (
            "top_k",
            schema_property("integer", "how many results to return (1..=1000; 10 if omitted)"),
        ),
        (
            "method",
            method_property(),
        ),
        (
            "include_low_information",
            schema_property(
                "boolean",
                "whether to keep low-information chunks (table-of-contents pages, running heads only, page numbers only) \
                 in the response (false if omitted, which drops them; the response says how many were dropped)",
            ),
        ),
    ]);
    let fetch_properties = object(vec![(
        "id",
        schema_property("string", "the object ID of the chunk (the s256:... value search returns)"),
    )]);
    let search_description = format!(
        // run-time-text-exempt: 例の問いは利用者か模型が打つ言葉の代わり(should/0141 の範囲外)。
        "Search the ingested documents and return snippets of the text with their citations \
         (document, page number, headings, chunk ID, retrieval time).\n\
         \n\
         How you ask changes what you find. Ask in English and include the English terms and \
         abbreviations the documents use as they are. Measured on real data (25 specification \
         PDFs) on 2026-08-17, the same 14 topics scored a mean reciprocal rank of 0.315 when \
         asked in Japanese only, 0.869 when English terms were mixed into a Japanese sentence, \
         and 0.705 to 0.798 when asked in English only. What matters is not the language \
         itself but whether the query holds the words the documents use.\n\
         Good: which field reports the period at which the HPET main counter increments\n\
         Bad: 高精度イベントタイマの主計数器が増える周期 (the documents only say HPET, so \
         this paraphrase is on no page)\n\
         If you only know how to say it in Japanese, write one query with both Japanese and \
         English (for example: HPET main counter 周期 tick period). If the translation misses, \
         the other half still hits.\n\
         \n\
         The methods are word matching (bm25), closeness of meaning (embedding) and their \
         fusion (hybrid). If omitted, the method follows what this node has: with embeddings, \
         embedding when top_k is at most {}, and hybrid above that.\n\
         Low-information chunks such as table-of-contents pages, running heads only or page \
         numbers only are dropped from the response by default (the response says how many). \
         Set include_low_information to true only when looking for figure pages from which \
         pdftotext got only the running heads.\n\
         Scores only rank the results within one response and cannot be compared across \
         responses.",
        crate::embed::EMBEDDING_ONLY_TOP_K
    );
    vec![
        object(vec![
            ("name", text("search")),
            ("title", text("uniqnode search")),
            ("description", text(&search_description)),
            (
                "inputSchema",
                object(vec![
                    ("type", text("object")),
                    ("properties", search_properties),
                    ("required", Value::Array(vec![text("query")])),
                ]),
            ),
            ("annotations", read_only_annotations("uniqnode search")),
        ]),
        object(vec![
            ("name", text("fetch")),
            ("title", text("uniqnode full text")),
            (
                "description",
                text(
                    "return the full text for a chunk's object ID (use it when the search snippet \
                     is not enough); a chunk in the current view also comes with its citation.",
                ),
            ),
            (
                "inputSchema",
                object(vec![
                    ("type", text("object")),
                    ("properties", fetch_properties),
                    ("required", Value::Array(vec![text("id")])),
                ]),
            ),
            ("annotations", read_only_annotations("uniqnode full text")),
        ]),
    ]
}

/// 書く 2 ツールの記述。説明文は模型が読む唯一の手引きなので、いつ使い・いつ使わないかと、
/// 書ける先をそこに書く。
fn write_tool_descriptors(writable: &[String]) -> Vec<Value> {
    let allowed = writable.join(", ");
    let collection_property = schema_property(
        "string",
        &format!("the collection to write to (allowed: {allowed}; others are refused)"),
    );
    let name_description = format!(
        "the document name without extension (for example hpet-notes): non-empty, without / ? # % or whitespace, not starting with ., \
         at most {} characters; writing the same name again overwrites it and keeps the previous version",
        crate::fetch::MAX_NAME_CHARS
    );
    let media_values: Vec<Value> =
        ADD_DOCUMENT_MEDIA.iter().map(|(media, _)| text(media)).collect();
    let add_document_properties = object(vec![
        ("collection", collection_property.clone()),
        ("name", schema_property("string", &name_description)),
        ("text", schema_property("string", "the text (a non-empty string)")),
        (
            "media",
            object(vec![
                ("type", text("string")),
                (
                    "description",
                    text(&format!(
                        "the kind of the text ({} if omitted; with markdown, headings appear in the chunks' citations)",
                        ADD_DOCUMENT_MEDIA[0].0
                    )),
                ),
                ("enum", Value::Array(media_values)),
            ]),
        ),
    ]);
    let add_document_description = format!(
        // run-time-text-exempt: 引用の句は利用者が言う言葉の代わり(should/0141 の範囲外)。
        "Add any text knowledge (markdown or plain text) to a collection as a document. The \
         document appears with its citation from the next search on.\n\
         When to use: when the user says \"覚えておいて\" (remember this), \"保存して\" (save \
         this) or \"メモしておいて\" (make a note), or to keep facts, decisions and procedures \
         confirmed in the conversation so they can be looked up later.\n\
         When not to use: do not write to collections of specifications or ingested originals \
         (this mcp allows writes only to {allowed}, and others are refused). Do not keep \
         anything the user did not ask for.\n\
         Writing the same name again overwrites it (the previous version is kept); the same \
         content changes nothing."
    );
    let fetch_url_properties = object(vec![
        ("collection", collection_property),
        (
            "url",
            schema_property("string", "the URL to fetch (http or https; file: and ftp: are refused)"),
        ),
        (
            "name",
            schema_property(
                "string",
                &format!(
                    "the document name without extension (same rules as the name of add_document); if omitted it is derived \
                     from the URL (host and path as one word, at most {} characters), so fetching the same URL again \
                     overwrites the same name",
                    crate::fetch::MAX_NAME_CHARS
                ),
            ),
        ),
    ]);
    let fetch_url_description = format!(
        // run-time-text-exempt: 引用の句は利用者が言う言葉の代わり(should/0141 の範囲外)。
        "Fetch the document at a URL and ingest it into a collection. HTML is made \
         self-contained by dropping its external dependencies (scripts, images, fonts, \
         external styles), PDF has its text extracted, and plain text goes in as is. The \
         document appears with its citation from the next search on.\n\
         When to use: when the user says \"このページを取り込んで\" (ingest this page) or \
         \"この URL を保存して\" (save this URL).\n\
         When not to use: when you only want to read it (ingesting is permanent; read with \
         another tool). Only the allowed collections ({allowed}) can be written.\n\
         Only http/https can be fetched, and anything over {} seconds or {} bytes is refused.",
        crate::fetch::DEFAULT_MAX_SECONDS,
        crate::fetch::DEFAULT_MAX_BYTES
    );
    vec![
        object(vec![
            ("name", text("add_document")),
            ("title", text("uniqnode add document")),
            ("description", text(&add_document_description)),
            (
                "inputSchema",
                object(vec![
                    ("type", text("object")),
                    ("properties", add_document_properties),
                    (
                        "required",
                        Value::Array(vec![text("collection"), text("name"), text("text")]),
                    ),
                ]),
            ),
            // 上書きは起こるが消さない(destructiveHint false)。同じ内容は同じ ID に
            // 落ちるので、繰り返しても状態は変わらない(idempotentHint true)。
            ("annotations", write_annotations("uniqnode add document", true, false)),
        ]),
        object(vec![
            ("name", text("fetch_url")),
            ("title", text("uniqnode fetch URL")),
            ("description", text(&fetch_url_description)),
            (
                "inputSchema",
                object(vec![
                    ("type", text("object")),
                    ("properties", fetch_url_properties),
                    ("required", Value::Array(vec![text("collection"), text("url")])),
                ]),
            ),
            // 取るたびに相手の内容が変わりうる(idempotentHint false)し、外の世界(網)に
            // 出る(openWorldHint true)。
            ("annotations", write_annotations("uniqnode fetch URL", false, true)),
        ]),
    ]
}

/// JSON Schema の 1 項目(型と説明)。
fn schema_property(kind: &str, description: &str) -> Value {
    object(vec![("type", text(kind)), ("description", text(description))])
}

/// method 引数の schema(取りうる値は SearchMethod の 3 つ。文字列はそちらの
/// as_str から出す。must/0023)。
fn method_property() -> Value {
    let methods = [
        crate::embed::SearchMethod::Bm25,
        crate::embed::SearchMethod::Embedding,
        crate::embed::SearchMethod::Hybrid,
    ];
    object(vec![
        ("type", text("string")),
        (
            "description",
            text(&format!(
                "the search method (if omitted, it follows what the node has: with embeddings, embedding when top_k is at most {}, \
                 and hybrid above that)",
                crate::embed::EMBEDDING_ONLY_TOP_K
            )),
        ),
        (
            "enum",
            Value::Array(methods.iter().map(|method| text(method.as_str())).collect()),
        ),
    ])
}

/// 読む 2 ツールの annotations: 読むだけで、外の世界を変えない。
fn read_only_annotations(title: &str) -> Value {
    object(vec![
        ("title", text(title)),
        ("readOnlyHint", Value::Bool(true)),
        ("destructiveHint", Value::Bool(false)),
        ("idempotentHint", Value::Bool(true)),
        ("openWorldHint", Value::Bool(false)),
    ])
}

/// 書く 2 ツールの annotations: 書くが消さない(上書きしても前版は残る)。べき等かと、
/// 外の世界に出るかはツールごとに違う。
fn write_annotations(title: &str, idempotent: bool, open_world: bool) -> Value {
    object(vec![
        ("title", text(title)),
        ("readOnlyHint", Value::Bool(false)),
        ("destructiveHint", Value::Bool(false)),
        ("idempotentHint", Value::Bool(idempotent)),
        ("openWorldHint", Value::Bool(open_world)),
    ])
}

/// tools/call の振り分け。知らないツール名と引数の誤りはプロトコルの誤り
/// (-32602)、ツールを実行したうえでの失敗は isError の結果で返す(前者は要求の
/// 組み立てが誤っている話、後者はモデルが読んで次の手を選べる話である)。
fn call_tool(backend: &Backend, writable: &[String], id: &Value, params: Option<&Value>) -> String {
    let Some(Value::Object(map)) = params else {
        return failure(id, INVALID_PARAMS, "no params (an object)");
    };
    let Some(Value::Text(name)) = map.get("name") else {
        return failure(id, INVALID_PARAMS, "no params.name (the tool name)");
    };
    let empty = Value::Object(BTreeMap::new());
    let arguments = map.get("arguments").unwrap_or(&empty);
    match name.as_str() {
        "search" => call_search(backend, id, arguments),
        "fetch" => call_fetch(backend, id, arguments),
        // 書くツールは tools/list に載せたときだけ受ける。載せていないツールを呼ぶのは
        // 要求の組み立ての誤り(知らないツールと同じ区分)で、理由を添える。
        write_tool @ ("add_document" | "fetch_url") if writable.is_empty() => failure(
            id,
            INVALID_PARAMS,
            &format!(
                "{write_tool} is not offered (no --writable was given, so this mcp \
                 only reads)"
            ),
        ),
        "add_document" => call_add_document(backend, writable, id, arguments),
        "fetch_url" => call_fetch_url(backend, writable, id, arguments),
        other => failure(id, INVALID_PARAMS, &format!("unknown tool: {other}")),
    }
}

/// ツールを実行したうえでの失敗。モデルが読んで次の手を選べるように結果(isError)で
/// 返し、同じ理由を標準エラーにも残す(応答を読まない運用者にも見えるように。
/// must/0022)。転送する形で serve に届かないときの案内もこの道を通る。
fn tool_failure(id: &Value, what: &str, reason: &str) -> String {
    crate::log_line!("uniqnode: mcp: {what}: {reason}");
    success(id, tool_text(&format!("{what}: {reason}"), true))
}

/// search ツール。引数の形は POST /v1/search のボディと同じで、読み取りも順位付けも
/// REST と同じ関数を通る(should/0135)。整形も、ストアを直接開く形と転送する形で
/// 同じ render_search を通る。
fn call_search(backend: &Backend, id: &Value, arguments: &Value) -> String {
    let request = match api::parse_search_request(arguments) {
        Ok(request) => request,
        Err(message) => return failure(id, INVALID_PARAMS, &message),
    };
    match backend.search(&request) {
        Ok(results) => success(id, tool_text(&render_search(&request, &results), false)),
        Err(reason) => tool_failure(id, "search failed", &reason),
    }
}

/// fetch ツール。
fn call_fetch(backend: &Backend, id: &Value, arguments: &Value) -> String {
    let Value::Object(map) = arguments else {
        return failure(id, INVALID_PARAMS, "the arguments must be an object");
    };
    let Some(Value::Text(object_id)) = map.get("id") else {
        return failure(id, INVALID_PARAMS, "no id (the object ID of the chunk)");
    };
    if !c1::is_object_id(object_id) {
        return failure(
            id,
            INVALID_PARAMS,
            "malformed object ID (s256: followed by 64 hex digits)",
        );
    }
    match backend.fetch(object_id) {
        Err(reason) => tool_failure(id, "fetch failed", &reason),
        // ローカルに無いのは「このDBノードは持っていない」というローカルな事実で
        // あって、不存在の言明ではない(SPEC §7.2/§10)。
        Ok(None) => success(
            id,
            tool_text(
                &format!("this node does not hold {object_id} (this is not a claim that it does not exist)"),
                true,
            ),
        ),
        Ok(Some(fetched)) => {
            let is_error = matches!(fetched, Fetched::Binary { .. });
            success(id, tool_text(&render_fetch(object_id, &fetched), is_error))
        }
    }
}

/// add_document ツール。引数を検めて PUT documents の形(拡張子つきの名前と本文)にし、
/// 後ろ盾に渡す。種別の判定も取り込みも api.rs の put_document が持つ(should/0135)。
fn call_add_document(backend: &Backend, writable: &[String], id: &Value, arguments: &Value) -> String {
    let Value::Object(map) = arguments else {
        return failure(id, INVALID_PARAMS, "the arguments must be an object");
    };
    let collection = match collection_argument(map) {
        Ok(collection) => collection,
        Err(message) => return failure(id, INVALID_PARAMS, &message),
    };
    let name = match map.get("name") {
        Some(Value::Text(name)) => name,
        _ => return failure(id, INVALID_PARAMS, "no name (the document name, without extension)"),
    };
    if let Some(message) = document_name_error(name) {
        return failure(id, INVALID_PARAMS, &message);
    }
    let body = match map.get("text") {
        Some(Value::Text(body)) if !body.trim().is_empty() => body,
        _ => return failure(id, INVALID_PARAMS, "no text (a non-empty text)"),
    };
    let extension = match map.get("media") {
        None => ADD_DOCUMENT_MEDIA[0].1,
        Some(Value::Text(media)) => match ADD_DOCUMENT_MEDIA.iter().find(|(m, _)| m == media) {
            Some((_, extension)) => extension,
            None => {
                return failure(
                    id,
                    INVALID_PARAMS,
                    &format!(
                        "media must be one of {}: {media}",
                        ADD_DOCUMENT_MEDIA.map(|(media, _)| media).join(" / ")
                    ),
                )
            }
        },
        Some(_) => return failure(id, INVALID_PARAMS, "media must be a string"),
    };
    if let Some(refusal) = write_refusal(collection, writable) {
        return tool_failure(id, "document not added", &refusal);
    }
    let file_name = format!("{name}.{extension}");
    let what = format!("PUT {}", document_path(collection, &file_name));
    let written = backend
        .put_document(collection, &file_name, body.as_bytes())
        .and_then(|written| written_document(&what, written));
    match written {
        Ok(fields) => success(
            id,
            tool_text(&render_added(collection, name, &fields), false),
        ),
        Err(reason) => tool_failure(id, "could not add the document", &reason),
    }
}

/// fetch_url ツール。引数を検めて POST fetch のボディにし、後ろ盾に渡す。取りに行く
/// 判断も取り込みも api.rs の fetch_into(と node/src/fetch.rs)が持つ(should/0135)。
fn call_fetch_url(backend: &Backend, writable: &[String], id: &Value, arguments: &Value) -> String {
    let Value::Object(map) = arguments else {
        return failure(id, INVALID_PARAMS, "the arguments must be an object");
    };
    let collection = match collection_argument(map) {
        Ok(collection) => collection,
        Err(message) => return failure(id, INVALID_PARAMS, &message),
    };
    let url = match map.get("url") {
        Some(Value::Text(url)) if !url.is_empty() => url,
        _ => return failure(id, INVALID_PARAMS, "no url (an http or https URL)"),
    };
    // 取りに行ける URL かの判断は node/src/fetch.rs の 1 箇所に問う(should/0135)。
    // ここで断るのは、要求の組み立ての誤り(-32602)として返すためである。
    if let Err(message) = crate::fetch::validate_url(url) {
        return failure(id, INVALID_PARAMS, &message);
    }
    let mut body = vec![("url", text(url))];
    match map.get("name") {
        None | Some(Value::Null) => {}
        Some(Value::Text(name)) => {
            if let Some(message) = document_name_error(name) {
                return failure(id, INVALID_PARAMS, &message);
            }
            body.push(("name", text(name)));
        }
        Some(_) => return failure(id, INVALID_PARAMS, "name must be a string (if omitted, it is derived from the URL)"),
    }
    if let Some(refusal) = write_refusal(collection, writable) {
        return tool_failure(id, "not ingested", &refusal);
    }
    let what = format!("POST {}", fetch_path(collection));
    let written = backend
        .fetch_url(collection, &c1::to_canonical_bytes(&object(body)))
        .and_then(|written| written_document(&what, written));
    match written {
        Ok(fields) => success(id, tool_text(&render_fetched_url(collection, &fields), false)),
        Err(reason) => tool_failure(id, "could not ingest", &reason),
    }
}

/// 書くツールの共通の引数 collection(空でなく、/ を含まない 1 語)。
fn collection_argument(map: &BTreeMap<String, Value>) -> Result<&str, String> {
    match map.get("collection") {
        Some(Value::Text(collection)) if !collection.is_empty() && !collection.contains('/') => {
            Ok(collection)
        }
        _ => Err("no collection (a collection name: one word without /)".to_string()),
    }
}

/// 書き込みを許していないコレクションへの断り(許しているなら None)。ツール実行の失敗
/// として返す文言で、どこなら書けるかを添える(模型が次の手を選べるように)。
fn write_refusal(collection: &str, writable: &[String]) -> Option<String> {
    if writable.iter().any(|allowed| allowed == collection) {
        return None;
    }
    Some(format!(
        "{collection} is not writable (--writable allows: {})",
        writable.join(", ")
    ))
}

/// 文書名(拡張子なし)の規則に外れていれば、その理由。ref 名の末尾になり、転送する形では
/// URL の道にそのまま載るので、道を壊す字と階層を作る / を断る。拡張子の付いた名前
/// (foo.md)は、PUT documents が拡張子を足すので二重になる前に断る(拡張子の判定は
/// 取り込みと同じ表 crate::ingest::media_for_extension に問う。should/0135)。
fn document_name_error(name: &str) -> Option<String> {
    let rule = format!(
        "name is the document name without extension: non-empty, without / ? # %, whitespace or control characters, not starting with ., \
         at most {} characters",
        crate::fetch::MAX_NAME_CHARS
    );
    if name.is_empty()
        || name.starts_with('.')
        || name.chars().count() > crate::fetch::MAX_NAME_CHARS
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/?#%".contains(c))
    {
        return Some(format!("{rule}: {name:?}"));
    }
    if let Some((_, extension)) = name.rsplit_once('.') {
        if crate::ingest::media_for_extension(&extension.to_ascii_lowercase()).is_some() {
            return Some(format!(
                "name takes no extension (drop .{extension}; give the kind in media): {name:?}"
            ));
        }
    }
    None
}

/// 書き込みの口の答えを読む。200 なら本文の JSON(オブジェクト)を返し、それ以外は
/// handler(転送する形なら serve)が言った理由をそのまま失敗にする(4xx/5xx を黙って
/// 飲まない。must/0022)。ストアを直接開く形と転送する形が同じ読み手を通る(should/0135)。
fn written_document(what: &str, written: Written) -> Result<BTreeMap<String, Value>, String> {
    let parsed = std::str::from_utf8(&written.body)
        .ok()
        .and_then(|text| c1::parse(text).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        });
    if written.status != 200 {
        let reason = match parsed.as_ref().and_then(|map| map.get("error")) {
            Some(Value::Text(message)) => message.clone(),
            _ => http::body_head(&written.body),
        };
        return Err(format!("{what} returned {}: {reason}", written.status));
    }
    parsed.ok_or_else(|| {
        format!("cannot read the response to {what} (not a JSON object): {}", http::body_head(&written.body))
    })
}

/// add_document の応答(1 行)。新規か上書きか、同じ内容で変わらなかったかを言う。
fn render_added(collection: &str, name: &str, fields: &BTreeMap<String, Value>) -> String {
    format!(
        "added: {collection}/{name} ({}); it appears in search.",
        revision_summary(fields)
    )
}

/// fetch_url の応答(1 行)。取り込んだ名前・種別・転送後の URL と、HTML なら落とした
/// 外部依存の数。
fn render_fetched_url(collection: &str, fields: &BTreeMap<String, Value>) -> String {
    let name = field_text(fields, "name").unwrap_or("(no name)");
    let media = field_text(fields, "media").unwrap_or("(no media)");
    let final_url = field_text(fields, "final_url").unwrap_or("(no URL)");
    let mut line = format!(
        "ingested: {collection}/{name} ({media}, final_url {final_url}, {}",
        revision_summary(fields)
    );
    if let Some(Value::Object(dropped)) = fields.get("dropped") {
        let counts: Vec<(&str, i64)> = dropped
            .iter()
            .filter_map(|(kind, count)| match count {
                Value::Integer(count) if *count > 0 => Some((kind.as_str(), *count)),
                _ => None,
            })
            .collect();
        let total: i64 = counts.iter().map(|(_, count)| count).sum();
        line.push_str(&format!(", dropped {total} external dependencies"));
        if !counts.is_empty() {
            let detail: Vec<String> =
                counts.iter().map(|(kind, count)| format!("{kind} {count}")).collect();
            line.push_str(&format!(" ({})", detail.join(", ")));
        }
    }
    line.push_str("); it appears in search.");
    line
}

/// 取り込みの結果の要約: 新規 / 上書き(前版) / 変わらず、doc_rev の短縮、新規オブジェクト数。
fn revision_summary(fields: &BTreeMap<String, Value>) -> String {
    let doc_rev = field_text(fields, "doc_rev").map(short_id).unwrap_or_else(|| "(none)".to_string());
    let new_objects = match fields.get("new_objects") {
        Some(Value::Integer(count)) => *count,
        _ => 0,
    };
    let previous = field_text(fields, "previous");
    let revision = match (fields.get("ref_updated"), previous) {
        (Some(Value::Bool(false)), _) => "unchanged (the same content is already there)".to_string(),
        (_, Some(previous)) => format!("overwritten; previous version {}", short_id(previous)),
        (_, None) => "new".to_string(),
    };
    format!("{revision}; doc_rev {doc_rev}, new objects {new_objects}")
}

fn field_text<'a>(fields: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a str> {
    match fields.get(key) {
        Some(Value::Text(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// オブジェクト ID の短縮(s256: と先頭 8 桁)。応答の 1 行に収めるためで、全文が要る
/// ときは REST で引ける。
fn short_id(id: &str) -> String {
    match id.char_indices().nth(13) {
        Some((cut, _)) => format!("{}...", &id[..cut]),
        None => id.to_string(),
    }
}

/// 検索結果を LLM が読む形に整える。生の JSON を垂れ流さず、抜粋と出典を並べる
/// (完了条件は「出典付きで答えられる」であり、出典の読めない応答は用を成さない)。
fn render_search(request: &SearchRequest, outcome: &SearchResults) -> String {
    let mut out = format!(
        "search: {} (method {}, score meaning {}, results {}",
        request.query,
        outcome.method.as_str(),
        outcome.method.score_semantics(),
        outcome.results.len()
    );
    match &request.collection {
        Some(collection) => out.push_str(&format!(", collection {collection})\n")),
        None => out.push_str(")\n"),
    }
    // 黙って劣化しない(should/0128)。求めた方式で答えられなかったことを、応答の
    // 読み手にも見せる。
    if let Some(reason) = &outcome.degraded {
        out.push_str(&format!("degraded: answered with {} ({reason})\n", outcome.method.as_str()));
    }
    // 捨てたことを黙らない(must/0019 と同じ理由)。落とした件数と、戻す方法を書く。
    if outcome.filtered_low_information > 0 {
        out.push_str(&format!(
            "low-information chunks dropped from the response: {} (table-of-contents pages, running heads only, page numbers only; \
             to keep them, set include_low_information: true)\n",
            outcome.filtered_low_information
        ));
    }
    if outcome.results.is_empty() {
        out.push_str("no matches.\n");
        return out;
    }
    for (rank, result) in outcome.results.iter().enumerate() {
        let citation = &result.citation;
        out.push_str(&format!(
            "\n{}. {}/{}{} position {}\n",
            rank + 1,
            citation.collection,
            citation.document,
            match citation.page {
                Some(page) => format!(" p.{page}"),
                None => String::new(),
            },
            citation.position
        ));
        out.push_str(&format!("   headings: {}\n", breadcrumb_path(&citation.breadcrumbs)));
        out.push_str(&format!("   retrieved at: {}\n", format_unix_time(citation.at)));
        out.push_str(&format!("   chunk ID: {}\n", result.id));
        out.push_str(&format!("   score: {:.4}\n", result.score));
        out.push_str(&format!("   snippet: {}\n", result.snippet));
        // 原本(文書そのもの)への道。serve が言った件にだけ出す。相対の道であり、
        // 根は問い合わせ先の serve である。
        if let Some(url) = &result.source_url {
            out.push_str(&format!("   original (whole document): {url}\n"));
        }
    }
    out.push_str(&format!("\n{FETCH_HINT}\n"));
    out
}

/// 全文の応答。出典は検索と同じ引用から組む。
fn render_fetch(object_id: &str, fetched: &Fetched) -> String {
    match fetched {
        Fetched::Chunk { text, citation } => {
            let source = match citation {
                Some(citation) => format!(
                    "source: {}/{}{} position {} / headings: {} / retrieved at: {}",
                    citation.collection,
                    citation.document,
                    match citation.page {
                        Some(page) => format!(" p.{page}"),
                        None => String::new(),
                    },
                    citation.position,
                    breadcrumb_path(&citation.breadcrumbs),
                    format_unix_time(citation.at)
                ),
                // ID で取れても見えに無いことはある(旧版のチャンクなど)。出典を
                // でっち上げず、そう言う。
                None => "source: a chunk not in the current view's index (an old version, or a chunk of a ref \
                         outside collections/)"
                    .to_string(),
            };
            format!("full text of chunk {object_id}\n{source}\n---\n{text}\n")
        }
        Fetched::Object { text } => format!(
            "{object_id} is a c1 object that is not a chunk (such as a doc_rev or an annotation); \
             shown in canonical form.\n---\n{text}\n"
        ),
        Fetched::Binary { bytes } => format!(
            "{object_id} is bytes that are not text ({bytes} bytes, such as the original blob of a PDF); \
             its full text cannot be shown; get the chunks you need by the IDs search returns."
        ),
    }
}

/// 見出しの入れ子パス(PDF のチャンクは見出しを持たない)。
fn breadcrumb_path(breadcrumbs: &[String]) -> String {
    if breadcrumbs.is_empty() {
        return "(none)".to_string();
    }
    breadcrumbs.join(" > ")
}

/// ツールの応答本体。isError はツールを実行したうえでの失敗を言う(プロトコルの誤りは
/// JSON-RPC の error で返す)。
fn tool_text(body: &str, is_error: bool) -> Value {
    object(vec![
        (
            "content",
            Value::Array(vec![object(vec![("type", text("text")), ("text", text(body))])]),
        ),
        ("isError", Value::Bool(is_error)),
    ])
}

fn success(id: &Value, result: Value) -> String {
    line_of(&object(vec![
        ("jsonrpc", text("2.0")),
        ("id", id.clone()),
        ("result", result),
    ]))
}

fn failure(id: &Value, code: i64, message: &str) -> String {
    line_of(&object(vec![
        ("jsonrpc", text("2.0")),
        ("id", id.clone()),
        (
            "error",
            object(vec![("code", Value::Integer(code)), ("message", text(message))]),
        ),
    ]))
}

/// 1 メッセージ 1 行の直列化。c1 の正規形は空白を持たず、制御文字を \u00xx に畳むので、
/// 本文にどんな改行が混ざっても 1 行に収まる(stdio 転送の要求)。
fn line_of(value: &Value) -> String {
    String::from_utf8(c1::to_canonical_bytes(value)).expect("c1 serialization is UTF-8")
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = BTreeMap::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 見出しの無いチャンク(PDF)でも、出典の欄が空白のまま残らない。
    #[test]
    fn breadcrumbs_render_as_a_path_or_an_explicit_absence() {
        assert_eq!(breadcrumb_path(&[]), "(none)");
        assert_eq!(
            breadcrumb_path(&["分散設計".to_string(), "世代の整合".to_string()]),
            "分散設計 > 世代の整合"
        );
    }

    /// 応答は必ず 1 行に収まる(本文の改行は c1 の直列化がエスケープに畳む)。
    #[test]
    fn a_response_line_never_carries_a_raw_newline() {
        let line = success(&Value::Integer(1), tool_text("一行目\n二行目", false));
        assert!(!line.contains('\n'), "応答に生の改行が残っている: {line}");
        assert!(line.contains("\\u000a"), "改行が畳まれていない: {line}");
    }

    /// 通知(id を持たないメッセージ)には何も返さない。返すと相手の解析が壊れる。
    #[test]
    fn notifications_get_no_response() {
        let (dir, mut server) = local_server("notifications");
        assert_eq!(
            server.handle_message("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}"),
            None
        );
        // 知らない通知も、応答は返さない(標準エラーには残す)。
        assert_eq!(server.handle_message("{\"jsonrpc\":\"2.0\",\"method\":\"x/y\"}"), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 誤りの符号は JSON-RPC の規定どおり。解析できない行は id を読めないので null で
    /// 答える。
    #[test]
    fn malformed_messages_answer_with_the_standard_error_codes() {
        let (dir, mut server) = local_server("malformed");
        let parse_error = server.handle_message("not json").expect("応答");
        assert!(parse_error.contains("\"code\":-32700"), "{parse_error}");
        assert!(parse_error.contains("\"id\":null"), "{parse_error}");
        let unknown = server
            .handle_message("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"x/y\"}")
            .expect("応答");
        assert!(unknown.contains("\"code\":-32601"), "{unknown}");
        let no_version =
            server.handle_message("{\"id\":1,\"method\":\"tools/list\"}").expect("応答");
        assert!(no_version.contains("\"code\":-32600"), "{no_version}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// initialize は版・capabilities・サーバ情報を答え、tools/list は 2 ツールを出す。
    #[test]
    fn the_handshake_declares_the_two_tools() {
        let (dir, mut server) = local_server("handshake");
        let initialized = server
            .handle_message(
                "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":\
             {\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\
             \"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}",
            )
            .expect("応答");
        assert!(initialized.contains("\"protocolVersion\":\"2025-06-18\""), "{initialized}");
        assert!(initialized.contains("\"tools\":{\"listChanged\":false}"), "{initialized}");
        assert!(initialized.contains("\"name\":\"uniqnode\""), "{initialized}");
        // id 0 は「id が無い」ではない(通知と取り違えると応答が消える)。
        assert!(initialized.contains("\"id\":0"), "{initialized}");

        let listed = server
            .handle_message("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}")
            .expect("応答");
        // 一覧は result.tools に入る。result を配列にすると相手は 1 本もツールを
        // 見つけられない(名前だけを探す検査では、この取り違えを見逃す)。
        assert!(listed.contains("\"result\":{\"tools\":["), "{listed}");
        assert!(listed.contains("\"name\":\"search\""), "{listed}");
        assert!(listed.contains("\"name\":\"fetch\""), "{listed}");
        assert!(listed.contains("\"required\":[\"query\"]"), "{listed}");
        assert!(listed.contains("\"required\":[\"id\"]"), "{listed}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 空のストアに対しても、search は空振りの結果を返し、fetch は持っていないことを
    /// isError で言う(黙って空を返さない)。
    #[test]
    fn the_tools_answer_over_an_empty_store() {
        let (dir, mut server) = local_server("empty-store");
        let searched = server
            .handle_message(
                "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":\
             {\"name\":\"search\",\"arguments\":{\"query\":\"世代の整合\"}}}",
            )
            .expect("応答");
        assert!(searched.contains("no matches"), "{searched}");
        assert!(searched.contains("\"isError\":false"), "{searched}");

        let absent = format!("s256:{}", "0".repeat(64));
        let fetched = server
            .handle_message(&format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":\
                 {{\"name\":\"fetch\",\"arguments\":{{\"id\":\"{absent}\"}}}}}}"
            ))
            .expect("応答");
        assert!(fetched.contains("\"isError\":true"), "{fetched}");
        assert!(fetched.contains("does not hold"), "{fetched}");

        // 知らないツールと引数の誤りはプロトコルの誤り。
        let unknown_tool = server
            .handle_message(
                "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":\
                 {\"name\":\"delete\",\"arguments\":{}}}",
            )
            .expect("応答");
        assert!(unknown_tool.contains("\"code\":-32602"), "{unknown_tool}");
        let no_query = server
            .handle_message(
                "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":\
                 {\"name\":\"search\",\"arguments\":{}}}",
            )
            .expect("応答");
        assert!(no_query.contains("\"code\":-32602"), "{no_query}");
        assert!(no_query.contains("query"), "{no_query}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// add_document の media の表は、取り込みの拡張子の表の逆引きである。片方だけを
    /// 直すと、受けた media と入る種別が食い違う(should/0135 の合意の試験)。
    #[test]
    fn add_document_media_round_trips_through_the_extension_table() {
        for (media, extension) in ADD_DOCUMENT_MEDIA {
            assert_eq!(
                crate::ingest::media_for_extension(extension),
                Some(media),
                "media {media} と拡張子 {extension} が取り込みの表と食い違う"
            );
        }
        assert_eq!(ADD_DOCUMENT_MEDIA[0].0, "markdown", "既定は markdown");
    }

    /// 文書名の規則: 拡張子つき・階層・URL を壊す字・空白は断り、日本語の 1 語は通す。
    #[test]
    fn document_names_are_one_word_without_an_extension() {
        assert_eq!(document_name_error("hpet-notes"), None);
        assert_eq!(document_name_error("覚え書き_2026"), None);
        assert_eq!(document_name_error("v1.2-notes"), None, "拡張子でない . は通す");
        for bad in ["", "a/b", "a b", ".hidden", "a?b", "a#b", "a%b", "a\nb"] {
            let error = document_name_error(bad).unwrap_or_else(|| panic!("{bad:?} を通した"));
            assert!(error.contains("name is the document name"), "{error}");
        }
        let extension = document_name_error("notes.md").expect("拡張子つきは断る");
        assert!(extension.contains("takes no extension"), "{extension}");
        assert!(document_name_error("notes.PDF").is_some(), "大文字の拡張子も断る");
        let long = "x".repeat(crate::fetch::MAX_NAME_CHARS + 1);
        assert!(document_name_error(&long).is_some(), "長すぎる名前は断る");
    }

    /// 書くツールは、書き込みを許すコレクションがあるときだけ tools/list に出る。無い
    /// ときに呼べば、載せていないツールとして要求の誤り(-32602)で断る。
    #[test]
    fn write_tools_are_listed_only_when_a_collection_is_writable() {
        let (dir, mut server) = local_server("writable");
        let list = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}";
        let read_only = server.handle_message(list).expect("応答");
        assert!(!read_only.contains("\"name\":\"add_document\""), "{read_only}");
        assert!(!read_only.contains("\"name\":\"fetch_url\""), "{read_only}");
        let refused = server
            .handle_message(
                "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":\
                 {\"name\":\"add_document\",\"arguments\":{\"collection\":\"notes\",\
                 \"name\":\"n\",\"text\":\"t\"}}}",
            )
            .expect("応答");
        assert!(refused.contains("\"code\":-32602"), "{refused}");
        assert!(refused.contains("--writable"), "理由を言うべき: {refused}");

        server.writable = vec!["notes".to_string(), "web".to_string()];
        let listed = server.handle_message(list).expect("応答");
        assert!(listed.contains("\"name\":\"add_document\""), "{listed}");
        assert!(listed.contains("\"name\":\"fetch_url\""), "{listed}");
        assert!(
            listed.contains("\"required\":[\"collection\",\"name\",\"text\"]"),
            "{listed}"
        );
        assert!(listed.contains("\"required\":[\"collection\",\"url\"]"), "{listed}");
        // 書ける先は説明文に出る(模型が読むのはここだけである)。
        assert!(listed.contains("allowed: notes, web"), "{listed}");
        assert!(listed.contains("\"readOnlyHint\":false"), "{listed}");
        // 許していないコレクションはツールの失敗(isError)で、どこなら書けるかを言う。
        let refused = server
            .handle_message(
                "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":\
                 {\"name\":\"add_document\",\"arguments\":{\"collection\":\"specs\",\
                 \"name\":\"n\",\"text\":\"t\"}}}",
            )
            .expect("応答");
        assert!(refused.contains("\"isError\":true"), "{refused}");
        assert!(
            refused.contains("specs is not writable (--writable allows: notes, web)"),
            "{refused}"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 書き込みの口の 4xx/5xx は、handler(転送する形なら serve)の言った理由をそのまま
    /// 失敗にする(黙って飲まない。must/0022)。200 は本文のオブジェクトを返す。
    #[test]
    fn written_document_passes_the_handlers_reason_through() {
        let refused = Written {
            status: 400,
            body: b"{\"error\":\"\xe5\xaf\xbe\xe8\xb1\xa1\xe5\xa4\x96\xe3\x81\xae\xe6\x8b\xa1\xe5\xbc\xb5\xe5\xad\x90\"}".to_vec(),
        };
        let error = written_document("PUT /x", refused).expect_err("400 は失敗");
        assert_eq!(error, "PUT /x returned 400: 対象外の拡張子");
        let plain = Written { status: 502, body: b"bad gateway".to_vec() };
        let error = written_document("POST /y", plain).expect_err("502 は失敗");
        assert!(error.starts_with("POST /y returned 502: bad gateway"), "{error}");
        let ok = Written {
            status: 200,
            body: b"{\"doc_rev\":\"s256:ab\",\"new_objects\":2,\"previous\":null,\"ref_updated\":true}".to_vec(),
        };
        let fields = written_document("PUT /x", ok).expect("200 は成功");
        assert_eq!(fields.get("new_objects"), Some(&Value::Integer(2)));
        let broken = Written { status: 200, body: b"[]".to_vec() };
        assert!(written_document("PUT /x", broken).is_err(), "オブジェクトでない本文は失敗");
    }

    /// 応答の 1 行は新規・上書き・変わらずを言い分ける(期待値はリテラル。should/0137)。
    #[test]
    fn the_write_responses_say_whether_the_document_is_new_or_overwritten() {
        let doc_rev = format!("s256:{}", "1".repeat(64));
        let previous = format!("s256:{}", "2".repeat(64));
        let mut fields = BTreeMap::new();
        fields.insert("doc_rev".to_string(), text(&doc_rev));
        fields.insert("new_objects".to_string(), Value::Integer(3));
        fields.insert("ref_updated".to_string(), Value::Bool(true));
        fields.insert("previous".to_string(), Value::Null);
        assert_eq!(
            render_added("notes", "memo", &fields),
            "added: notes/memo (new; doc_rev s256:11111111..., new objects 3); it appears in search."
        );
        fields.insert("previous".to_string(), text(&previous));
        assert_eq!(
            render_added("notes", "memo", &fields),
            "added: notes/memo (overwritten; previous version s256:22222222...; \
             doc_rev s256:11111111..., new objects 3); it appears in search."
        );
        fields.insert("ref_updated".to_string(), Value::Bool(false));
        fields.insert("new_objects".to_string(), Value::Integer(0));
        assert_eq!(
            render_added("notes", "memo", &fields),
            "added: notes/memo (unchanged (the same content is already there); \
             doc_rev s256:11111111..., new objects 0); it appears in search."
        );
        // fetch_url は名前・種別・転送後の URL と、HTML なら落とした数を添える。
        fields.insert("ref_updated".to_string(), Value::Bool(true));
        fields.insert("previous".to_string(), Value::Null);
        fields.insert("new_objects".to_string(), Value::Integer(2));
        fields.insert("name".to_string(), text("example.com_page"));
        fields.insert("media".to_string(), text("html"));
        fields.insert("final_url".to_string(), text("http://example.com/page.html"));
        let mut dropped = BTreeMap::new();
        for (kind, count) in [("scripts", 2), ("images", 1), ("fonts", 0)] {
            dropped.insert(kind.to_string(), Value::Integer(count));
        }
        fields.insert("dropped".to_string(), Value::Object(dropped));
        assert_eq!(
            render_fetched_url("web", &fields),
            "ingested: web/example.com_page (html, final_url http://example.com/page.html, \
             new; doc_rev s256:11111111..., new objects 2, dropped 3 external dependencies \
             (images 1, scripts 2)); it appears in search."
        );
    }

    /// 先読みバッファの検査(自己置換の前提)。1 行読んだ後に次のメッセージがバッファに
    /// 載っていれば、そのバイト列は exec で消えるので、差し替えは見送らねばならない。
    #[test]
    fn a_buffered_next_message_is_visible_after_reading_a_line() {
        let mut input = std::io::BufReader::new(&b"{\"a\":1}\n{\"b\":2}\n"[..]);
        let mut line = String::new();
        input.read_line(&mut line).expect("read");
        assert_eq!(line, "{\"a\":1}\n");
        assert!(input.has_buffered_bytes(), "次のメッセージがバッファに残っているはず");
        line.clear();
        input.read_line(&mut line).expect("read");
        assert!(!input.has_buffered_bytes(), "読み切れば空のはず");
    }

    /// 自己検査は、起こせない実行ファイルを健全とは言わない(防御が黙って素通りすると、
    /// 壊れたイメージへの exec で MCP が死ぬ)。
    #[test]
    fn the_self_check_refuses_a_binary_it_cannot_start() {
        let missing =
            std::env::temp_dir().join(format!("uniqnode-absent-{}", std::process::id()));
        let error = self_check(&missing).expect_err("起こせないはず");
        assert!(error.contains("cannot start"), "{error}");
    }

    /// 空のストアを直接開いた MCP のサーバ(serve と同じ形の ApiContext を組む。
    /// HTTP は通らない)。
    fn local_server(name: &str) -> (std::path::PathBuf, StdioServer) {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-mcp-unit-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let store = crate::store::Store::open(crate::store::StoreConfig::new(&dir)).expect("open");
        let store = std::sync::Arc::new(std::sync::Mutex::new(store));
        let engine =
            std::sync::Arc::new(crate::query::QueryEngine::new(store.clone(), dir.clone()));
        let context = ApiContext {
            store,
            engine,
            health: None,
            referrers: std::sync::Mutex::new(None),
            search: std::sync::Mutex::new(None),
            search_warmer: None,
            embedding: None,
            reranker: None,
            data_dir: dir.clone(),
            main_door: None,
        };
        (dir, StdioServer::new(Backend::Local(Box::new(context)), Vec::new()))
    }
}
