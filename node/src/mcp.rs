//! MCP(Model Context Protocol)アダプタ: 標準入出力の JSON-RPC 2.0 で search と fetch の
//! 2 ツールを公開する(MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9))。
//!
//! コアはあくまで REST であり、この層は薄い被せ物である。検索の判断(方式の既定・劣化の
//! 判断・引用の組み立て)は node/src/api.rs の run_search が持ち、全文の取得は
//! fetch_object が持つ。ここがするのは JSON-RPC の封筒の付け外しと、LLM が読む形への
//! 整形だけである(検索の判断を二重に実装しない。should/0135)。
//!
//! 標準出力はプロトコル専用である(MCP の stdio 転送の規定)。1 行が 1 メッセージで、
//! 行に混ざった非メッセージは相手の解析をその場で壊す。ログはすべて標準エラーへ出す。
//! 応答の直列化は c1 の正規形を通すので、改行や制御文字は \u00xx に畳まれ、1 メッセージが
//! 1 行に収まることが直列化の側から保証される(実装を増やさない。should/0135)。
//!
//! 受け取り側の既知の制限: 要求の解析は c1(SPEC §4.1)を使うため、整数しか受けない。
//! JSON-RPC 自体と、この 2 ツールの引数はすべて文字列・整数・真偽値・オブジェクトなので
//! 足りるが、小数を含む要求は解析誤り(-32700)として拒む。黙って読み飛ばさない。
//!
//! ツールの後ろ盾は二つの形がある(Backend)。ストアを直接開く形(Local)は起動から終了
//! までストアの排他錠を持つので、常駐している間 CLI の ingest・embed は断られる。走って
//! いる serve の REST へ転送する形(Forward)は錠を一切取らないので、エージェントを繋いだ
//! まま取り込みと埋め込みを回せる。整形は一つで、どちらの形も同じ render_search /
//! render_fetch を通る(検索の判断も整形も二重に実装しない。should/0135)。
//!
//! 実行ファイルが更新されたら、自分を exec で差し替える(StdioServer::replace_if_updated)。
//! execve はプロセスイメージを入れ替えるがファイル記述子は保つので、相手が握るパイプの
//! 反対側と PID は変わらず、差し替えはクライアントから見えない。位置は「応答を書き終えた
//! 直後、次の読み込みの前」だけで、先読みバッファが空であることを確かめてから行う。

use crate::api::{self, ApiContext, Citation, Fetched, SearchRequest, SearchResults};
use crate::c1::{self, Value};
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
const FETCH_HINT: &str = "全文が要るときは fetch ツールにチャンク ID を渡す。";

/// 転送する形が serve を待つ期限(1 要求ぶん)。初回の検索は索引の構築ぶんだけ待つので
/// (実データ規模で約 9 秒)、埋め込みのクエリ期限(15 秒)より長く採る。期限のない待ちは
/// 作らない(should/0104)。
const SERVE_TIMEOUT: Duration = Duration::from_secs(60);

/// 自己置換の前に新しいイメージを確かめるサブコマンド。呼ぶ側(このモジュール)と
/// 答える側(node/src/main.rs)で同じ文字列を使う(must/0023)。
pub const SELF_CHECK_COMMAND: &str = "selfcheck";

/// 自己検査が健全なときに標準出力へ書く印。exec の前にこの印を確かめるので、たまたま
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
/// 転送する形はストアの錠を一切取らない。これが要点である: Claude Code に登録した MCP
/// サーバが常駐していても、同じデータディレクトリに対する ingest・embed が通る。検索の
/// 判断(方式の既定・劣化・引用の組み立て)はどちらの形でも serve と同じ 1 箇所
/// (node/src/api.rs の run_search)にあり、転送する形はその答えを REST の応答から
/// 組み直すだけである(should/0135)。
pub enum Backend {
    /// ストアを直接開く形。起動から終了まで排他錠を持つ(serve と同じ制約)。
    Local(Box<ApiContext>),
    /// 走っている serve へ転送する形。錠を取らない。
    Forward(ServeClient),
}

impl Backend {
    /// 検索 1 回。誤りは LLM が読む文にして返す(ツール実行の失敗は isError の結果)。
    fn search(&self, request: &SearchRequest) -> Result<SearchResults, String> {
        match self {
            Backend::Local(context) => {
                api::run_search(context, request).map_err(|error| format!("{error}"))
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

    /// 起動の知らせに載せる、この形の説明。
    fn description(&self) -> String {
        match self {
            Backend::Local(_) => "ストアを直接開く形(排他錠を持つ)".to_string(),
            Backend::Forward(client) => {
                format!("{} へ転送する形(ストアの錠を取らない)", client.url)
            }
        }
    }
}

/// 走っている serve への最小のクライアント。検索は POST /v1/search、全文は
/// GET /v1/objects/{id}、その出典は GET /v1/objects/{id}/citation を呼ぶ。
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
                "{url}: serve の根を指す URL であるべき(例 http://127.0.0.1:7440)"
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
            "走っている serve に届かない({}): {cause}。\
             転送する形の MCP は自分でストアを開かないので、先に serve を起こす: \
             uniqnode serve {} {}",
            self.url, self.data_dir, self.address
        )
    }

    /// 相手が誤りを返したときの文(届いてはいるので、起動の案内は付けない)。
    fn refused(&self, what: &str, status: u16, body: &[u8]) -> String {
        format!("serve の {what} が {status} を返した({}): {}", self.url, http::body_head(body))
    }

    fn search(&self, request: &SearchRequest) -> Result<SearchResults, String> {
        let body = api::search_request_body(request);
        let response = http::post_json(&self.address, "/v1/search", &body, SERVE_TIMEOUT)
            .map_err(|error| self.unreachable(&error))?;
        if response.status != 200 {
            return Err(self.refused("POST /v1/search", response.status, &response.body));
        }
        api::parse_search_response(&response.body)
            .map_err(|error| format!("serve の検索応答を読めない({}): {error}", self.url))
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

    fn citation(&self, id: &str) -> Result<Option<Citation>, String> {
        let path = format!("/v1/objects/{id}/citation");
        let response = http::get(&self.address, &path, SERVE_TIMEOUT)
            .map_err(|error| self.unreachable(&error))?;
        if response.status != 200 {
            return Err(self.refused(&format!("GET {path}"), response.status, &response.body));
        }
        let text = std::str::from_utf8(&response.body)
            .map_err(|_| format!("serve の {path} の応答が UTF-8 でない"))?;
        let value = crate::json::Json::parse(text)
            .map_err(|error| format!("serve の {path} の応答を読めない: {error}"))?;
        match value.field("citation") {
            None => Err(format!("serve の {path} の応答に citation がない")),
            Some(crate::json::Json::Null) => Ok(None),
            Some(found) => api::citation_from_json(found)
                .map(Some)
                .map_err(|error| format!("serve の {path} の出典を読めない: {error}")),
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
    /// 起動時に控えた実行ファイルの姿。None なら自己置換をしない。
    binary: Option<BinaryStamp>,
    handshake: Handshake,
}

impl StdioServer {
    /// 後ろ盾を受けて組む。実行ファイルの姿はここで控える: 置き換えられた後の
    /// /proc/self/exe は "(deleted)" 付きの読めない道になるので、比較の相手は
    /// 起動時のパスでなければならない。
    pub fn new(backend: Backend) -> StdioServer {
        let handshake = Handshake::inherited();
        if let Some(protocol) = &handshake.protocol {
            eprintln!(
                "uniqnode: mcp: ハンドシェイク済みとして起動した(相手の protocol {protocol}、\
                 client {})",
                handshake.client.as_deref().unwrap_or("(名乗りなし)")
            );
        }
        StdioServer { backend, binary: BinaryStamp::of_current_exe(), handshake }
    }

    /// 起動の知らせ(標準出力はプロトコル専用なので標準エラーへ出す)。
    pub fn announce(&self, target: &str) {
        eprintln!(
            "uniqnode: mcp: {target} を stdio で提供する(protocol {PROTOCOL_VERSION}、\
             tools: search・fetch、{})",
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
                return Some(failure(&Value::Null, PARSE_ERROR, &format!("JSON が不正: {error}")))
            }
        };
        let Value::Object(map) = &message else {
            return Some(failure(&Value::Null, INVALID_REQUEST, "要求はオブジェクトであるべき"));
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
                return Some(failure(&id, INVALID_REQUEST, "method がない(文字列)"));
            }
        };
        if map.get("jsonrpc") != Some(&Value::Text("2.0".to_string())) {
            let id = id?;
            return Some(failure(&id, INVALID_REQUEST, "jsonrpc は \"2.0\" であるべき"));
        }
        let params = map.get("params");
        let Some(id) = id else {
            // 通知。initialized と cancelled は受理して黙る。知らない通知も、応答を返して
            // はならない以上ここで捨てるほかないが、標準エラーには残す(must/0022)。
            if method != "notifications/initialized" && method != "notifications/cancelled" {
                eprintln!("uniqnode: mcp: 知らない通知を無視した: {method}");
            }
            return None;
        };
        Some(match method {
            "initialize" => success(&id, self.initialize_result(params)),
            // 生存確認。空の結果が規定の答えである。
            "ping" => success(&id, Value::Object(BTreeMap::new())),
            "tools/list" => success(&id, tools_result()),
            "tools/call" => call_tool(&self.backend, &id, params),
            other => failure(&id, METHOD_NOT_FOUND, &format!("知らないメソッド: {other}")),
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
                    eprintln!(
                        "uniqnode: mcp: 相手は protocol {requested} を求めた。\
                         {PROTOCOL_VERSION} で答える"
                    );
                }
            }
            if let Some(Value::Object(info)) = map.get("clientInfo") {
                let field = |name: &str| match info.get(name) {
                    Some(Value::Text(value)) => value.clone(),
                    _ => "(不明)".to_string(),
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
                    ("title", text("uniqnode RAG ストレージ")),
                ]),
            ),
            (
                "instructions",
                text(
                    "uniqnode は取り込んだ文書をチャンク単位で検索できる知識ストアである。\
                     search で問い、返った出典(文書名・ページ・見出し・取得日時)を答えに\
                     添える。抜粋で足りなければ fetch にチャンク ID を渡して全文を読む。",
                ),
            ),
        ])
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
            eprintln!(
                "uniqnode: mcp: 実行ファイルが更新されたが、先読みバッファに次の\
                 メッセージがある。差し替えを次の応答の後に回す"
            );
            return;
        }
        // 壊れたバイナリで自分を置き換えると MCP が死に、結局セッションの再起動が要る。
        // exec の前に、新しいイメージを子プロセスとして起こして健全さを確かめる。
        if let Err(reason) = self_check(&stamp.path) {
            eprintln!(
                "uniqnode: mcp: 新しい実行ファイルの自己検査に落ちた。差し替えず、\
                 旧イメージのまま続ける: {reason}"
            );
            // 同じ姿を応答のたびに検査し直さない。次にまた変わったときに試す。
            self.binary = Some(current);
            return;
        }
        eprintln!(
            "uniqnode: mcp: 実行ファイルが更新された。自分を exec で差し替える: {}",
            stamp.path.display()
        );
        let path = stamp.path.clone();
        let error = self.exec_replacement(&path);
        // exec が返るのは失敗したときだけである(成功すればこの行は無い)。
        eprintln!("uniqnode: mcp: exec に失敗した。旧イメージのまま続ける: {error}");
        self.binary = Some(current);
    }

    /// 新しいイメージへの exec。引数はそのまま渡し、ハンドシェイクの事実は環境変数で
    /// 引き継ぐ(新しいイメージは initialize を受けたことを忘れており、Claude Code は
    /// 再送しない)。ストアの排他錠は CLOEXEC 付きの FD なので exec で解放され、新しい
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
                eprintln!(
                    "uniqnode: mcp: 実行ファイルの位置が分からない。自己置換をしない: {error}"
                );
                return None;
            }
        };
        let stamp = BinaryStamp::read(&path);
        if stamp.is_none() {
            eprintln!(
                "uniqnode: mcp: {} の属性を読めない。自己置換をしない",
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

/// 自己検査(exec の前の防御)。新しいイメージを子プロセスとして起こし、規定の印を
/// 標準出力に出して正常終了することを確かめる。終了コードだけを見ないのは、たまたま 0 で
/// 終わる別の実行ファイルを健全とみなさないためである。
fn self_check(path: &Path) -> Result<(), String> {
    let mut child = Command::new(path)
        .arg(SELF_CHECK_COMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{} を起こせない: {error}", path.display()))?;
    let deadline = Instant::now() + SELF_CHECK_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(format!("自己検査の子を待てない: {error}")),
        }
        if Instant::now() >= deadline {
            // 終わらない新しいイメージも健全ではない。始末してから断る。
            if let Err(error) = child.kill() {
                eprintln!("uniqnode: mcp: 自己検査の子を kill できない: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("uniqnode: mcp: 自己検査の子を待てない: {error}");
            }
            return Err(format!("自己検査が {SELF_CHECK_TIMEOUT:?} で終わらない"));
        }
        std::thread::sleep(SELF_CHECK_POLL);
    };
    let mut reported = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_string(&mut reported)
            .map_err(|error| format!("自己検査の標準出力を読めない: {error}"))?;
    }
    if !status.success() {
        return Err(format!("{} {SELF_CHECK_COMMAND} が {status} で終わった", path.display()));
    }
    if !reported.contains(SELF_CHECK_MARKER) {
        return Err(format!(
            "{} {SELF_CHECK_COMMAND} の標準出力に「{SELF_CHECK_MARKER}」が無い: {reported:?}",
            path.display()
        ));
    }
    Ok(())
}

/// 自己検査が標準出力へ書く 1 行(uniqnode selfcheck)。ツールの記述を実際に組み立てて
/// 数えるので、新しいイメージがこの層まで動くことを確かめられる。文言の家はここである
/// (書く側と読む側が同じ印を使う。must/0023)。
pub fn self_check_report() -> String {
    let tools = match tool_descriptors() {
        Value::Array(tools) => tools.len(),
        _ => 0,
    };
    format!("{SELF_CHECK_MARKER} version={} tools={tools}", env!("CARGO_PKG_VERSION"))
}

/// tools/list の結果。ツールの一覧は result.tools に入る(result そのものを配列に
/// してはならない)。ページ分割はしないので nextCursor は載せない。
fn tools_result() -> Value {
    object(vec![("tools", tool_descriptors())])
}

/// 公開する 2 ツールの記述。
fn tool_descriptors() -> Value {
    let search_properties = object(vec![
        (
            "query",
            schema_property("string", "問い合わせ文(自然文でも語でもよい)"),
        ),
        (
            "collection",
            schema_property("string", "絞り込むコレクション名(省略時は全コレクション)"),
        ),
        (
            "top_k",
            schema_property("integer", "返す件数(1..=1000。省略時は 10)"),
        ),
        (
            "method",
            method_property(),
        ),
        (
            "include_low_information",
            schema_property(
                "boolean",
                "目次の紙面・柱だけ・ページ番号だけの低情報チャンクを応答に残すか\
                 (省略時は false で落とす。落とした件数は応答に出る)",
            ),
        ),
    ]);
    let fetch_properties = object(vec![(
        "id",
        schema_property("string", "チャンクのオブジェクト ID(search が返す s256:… の値)"),
    )]);
    let search_description = format!(
        "取り込み済みの文書を検索し、本文の抜粋と出典(文書名・ページ番号・見出し・\
         チャンク ID・取得日時)を返す。\n\
         \n\
         問い方で当たり方が大きく変わる。英語で問い、文書が使っている英語の術語と略語を\
         そのまま入れること。実データ(仕様書 PDF 25 本)での実測 2026-08-17 では、\
         同じ 14 主題を純日本語で問うと平均逆順位 0.315、日本語の文に英語術語を混ぜると \
         0.869、英語だけで問うと 0.705〜0.798 だった。効くのは言語そのものではなく、\
         文書が使う語がクエリに入っているかである。\n\
         良い例: which field reports the period at which the HPET main counter increments\n\
         悪い例: 高精度イベントタイマの主計数器が増える周期(文書は HPET としか書かない\
         ので、この言い換えはどのページにも無い)\n\
         日本語の言い方しか分からないときは、日英を併記した 1 本のクエリにする\
         (例: HPET main counter 周期 tick period)。訳が外れても片方が当たる。\n\
         \n\
         方式は語の一致(bm25)・意味の近さ(embedding)・両者の融合(hybrid)。省略時は\
         このノードの装備に従い、埋め込みがあれば top_k が {} 以下で embedding、\
         それより多ければ hybrid になる。\n\
         目次の紙面・柱だけ・ページ番号だけといった低情報のチャンクは既定で応答から落とす\
         (落とした件数は応答に出る)。pdftotext が柱しか採れなかった図版のページを探す\
         ときだけ include_low_information を true にする。\n\
         得点は同じ応答の中の順位付けにだけ意味があり、応答をまたいだ比較には使えない。",
        crate::embed::EMBEDDING_ONLY_TOP_K
    );
    Value::Array(vec![
        object(vec![
            ("name", text("search")),
            ("title", text("uniqnode 検索")),
            ("description", text(&search_description)),
            (
                "inputSchema",
                object(vec![
                    ("type", text("object")),
                    ("properties", search_properties),
                    ("required", Value::Array(vec![text("query")])),
                ]),
            ),
            ("annotations", read_only_annotations("uniqnode 検索")),
        ]),
        object(vec![
            ("name", text("fetch")),
            ("title", text("uniqnode 全文取得")),
            (
                "description",
                text(
                    "チャンクのオブジェクト ID を受けて全文を返す(search の抜粋で\
                     足りないときに使う)。見えにあるチャンクなら出典も添える。",
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
            ("annotations", read_only_annotations("uniqnode 全文取得")),
        ]),
    ])
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
                "検索の方式(省略時はノードの装備に従う。埋め込みがあれば top_k が {} 以下で\
                 embedding、それより多ければ hybrid)",
                crate::embed::EMBEDDING_ONLY_TOP_K
            )),
        ),
        (
            "enum",
            Value::Array(methods.iter().map(|method| text(method.as_str())).collect()),
        ),
    ])
}

/// どちらのツールも読むだけで、外の世界を変えない。
fn read_only_annotations(title: &str) -> Value {
    object(vec![
        ("title", text(title)),
        ("readOnlyHint", Value::Bool(true)),
        ("destructiveHint", Value::Bool(false)),
        ("idempotentHint", Value::Bool(true)),
        ("openWorldHint", Value::Bool(false)),
    ])
}

/// tools/call の振り分け。知らないツール名と引数の誤りはプロトコルの誤り
/// (-32602)、ツールを実行したうえでの失敗は isError の結果で返す(前者は要求の
/// 組み立てが誤っている話、後者はモデルが読んで次の手を選べる話である)。
fn call_tool(backend: &Backend, id: &Value, params: Option<&Value>) -> String {
    let Some(Value::Object(map)) = params else {
        return failure(id, INVALID_PARAMS, "params がない(オブジェクト)");
    };
    let Some(Value::Text(name)) = map.get("name") else {
        return failure(id, INVALID_PARAMS, "params.name がない(ツール名)");
    };
    let empty = Value::Object(BTreeMap::new());
    let arguments = map.get("arguments").unwrap_or(&empty);
    match name.as_str() {
        "search" => call_search(backend, id, arguments),
        "fetch" => call_fetch(backend, id, arguments),
        other => failure(id, INVALID_PARAMS, &format!("知らないツール: {other}")),
    }
}

/// ツールを実行したうえでの失敗。モデルが読んで次の手を選べるように結果(isError)で
/// 返し、同じ理由を標準エラーにも残す(応答を読まない運用者にも見えるように。
/// must/0022)。転送する形で serve に届かないときの案内もこの道を通る。
fn tool_failure(id: &Value, what: &str, reason: &str) -> String {
    eprintln!("uniqnode: mcp: {what}: {reason}");
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
        Err(reason) => tool_failure(id, "検索に失敗した", &reason),
    }
}

/// fetch ツール。
fn call_fetch(backend: &Backend, id: &Value, arguments: &Value) -> String {
    let Value::Object(map) = arguments else {
        return failure(id, INVALID_PARAMS, "引数はオブジェクトであるべき");
    };
    let Some(Value::Text(object_id)) = map.get("id") else {
        return failure(id, INVALID_PARAMS, "id がない(チャンクのオブジェクト ID)");
    };
    if !c1::is_object_id(object_id) {
        return failure(
            id,
            INVALID_PARAMS,
            "オブジェクトIDの形式が不正(s256: と16進64桁)",
        );
    }
    match backend.fetch(object_id) {
        Err(reason) => tool_failure(id, "取得に失敗した", &reason),
        // ローカルに無いのは「このDBノードは持っていない」というローカルな事実で
        // あって、不存在の言明ではない(SPEC §7.2/§10)。
        Ok(None) => success(
            id,
            tool_text(
                &format!("{object_id} はこのノードが持っていない(不存在の言明ではない)"),
                true,
            ),
        ),
        Ok(Some(fetched)) => {
            let is_error = matches!(fetched, Fetched::Binary { .. });
            success(id, tool_text(&render_fetch(object_id, &fetched), is_error))
        }
    }
}

/// 検索結果を LLM が読む形に整える。生の JSON を垂れ流さず、抜粋と出典を並べる
/// (完了条件は「出典付きで答えられる」であり、出典の読めない応答は用を成さない)。
fn render_search(request: &SearchRequest, outcome: &SearchResults) -> String {
    let mut out = format!(
        "検索: {}(方式 {}、得点の意味 {}、{} 件",
        request.query,
        outcome.method.as_str(),
        outcome.method.score_semantics(),
        outcome.results.len()
    );
    match &request.collection {
        Some(collection) => out.push_str(&format!("、コレクション {collection})\n")),
        None => out.push_str(")\n"),
    }
    // 黙って劣化しない(should/0128)。求めた方式で答えられなかったことを、応答の
    // 読み手にも見せる。
    if let Some(reason) = &outcome.degraded {
        out.push_str(&format!("劣化: {} で答えた({reason})\n", outcome.method.as_str()));
    }
    // 捨てたことを黙らない(must/0019 と同じ理由)。落とした件数と、戻す方法を書く。
    if outcome.filtered_low_information > 0 {
        out.push_str(&format!(
            "低情報チャンク {} 件を応答から落とした(目次の紙面・柱だけ・ページ番号だけ。\
             残すには include_low_information: true)\n",
            outcome.filtered_low_information
        ));
    }
    if outcome.results.is_empty() {
        out.push_str("一致なし。\n");
        return out;
    }
    for (rank, result) in outcome.results.iter().enumerate() {
        let citation = &result.citation;
        out.push_str(&format!(
            "\n{}. {}/{}{} 位置 {}\n",
            rank + 1,
            citation.collection,
            citation.document,
            match citation.page {
                Some(page) => format!(" p.{page}"),
                None => String::new(),
            },
            citation.position
        ));
        out.push_str(&format!("   見出し: {}\n", breadcrumb_path(&citation.breadcrumbs)));
        out.push_str(&format!("   取得日時: {}\n", format_unix_time(citation.at)));
        out.push_str(&format!("   チャンク ID: {}\n", result.id));
        out.push_str(&format!("   得点: {:.4}\n", result.score));
        out.push_str(&format!("   抜粋: {}\n", result.snippet));
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
                    "出典: {}/{}{} 位置 {} / 見出し: {} / 取得日時: {}",
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
                None => "出典: 見えの索引に無いチャンク(旧版か、collections/ 配下でない \
                         ref のチャンク)"
                    .to_string(),
            };
            format!("チャンク {object_id} の全文\n{source}\n---\n{text}\n")
        }
        Fetched::Object { text } => format!(
            "{object_id} はチャンクではない c1 オブジェクト(doc_rev・注釈など)。\
             正規形のまま示す。\n---\n{text}\n"
        ),
        Fetched::Binary { bytes } => format!(
            "{object_id} はテキストでないバイト列({bytes} バイト。PDF の原文 blob など)。\
             全文は示せない。本文の要るチャンクは search が返す ID で取る。"
        ),
    }
}

/// 見出しの入れ子パス(PDF のチャンクは見出しを持たない)。
fn breadcrumb_path(breadcrumbs: &[String]) -> String {
    if breadcrumbs.is_empty() {
        return "(なし)".to_string();
    }
    breadcrumbs.join(" > ")
}

/// unix 秒を UTC の日時にする(例 2026-08-17T04:05:06Z)。取得日時は LLM が読む出典の
/// 一部であり、整数のままでは日付として読めない。外部クレートは使えない(must/0008)
/// ので暦の計算はここに置く。
pub fn format_unix_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60
    )
}

/// 1970-01-01 からの日数を暦の年月日にする(Howard Hinnant の civil_from_days。
/// グレゴリオ暦の 400 年周期を使う閉じた式で、閏日の表を持たない)。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // 3 月始まりの年に移すと、閏日が年の最後に来て場合分けが消える。
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
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
    String::from_utf8(c1::to_canonical_bytes(value)).expect("c1 直列化は UTF-8")
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

    /// 暦の期待値はリテラルで書く(検査対象から導出しない。should/0137)。値は
    /// date -u -d @<秒> で突き合わせたもの。
    #[test]
    fn unix_seconds_render_as_utc_timestamps() {
        assert_eq!(format_unix_time(0), "1970-01-01T00:00:00Z");
        // 閏年の 2 月 29 日(400 年周期の閏年)。
        assert_eq!(format_unix_time(951_782_400), "2000-02-29T00:00:00Z");
        // 平年の 3 月 1 日(1900 は閏年ではない周期の側)。
        assert_eq!(format_unix_time(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(format_unix_time(1_755_000_000), "2025-08-12T12:00:00Z");
        // 1970 より前は負の秒。境界で 1 日ずれないことを見る。
        assert_eq!(format_unix_time(-1), "1969-12-31T23:59:59Z");
    }

    /// 見出しの無いチャンク(PDF)でも、出典の欄が空白のまま残らない。
    #[test]
    fn breadcrumbs_render_as_a_path_or_an_explicit_absence() {
        assert_eq!(breadcrumb_path(&[]), "(なし)");
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
        assert!(searched.contains("一致なし"), "{searched}");
        assert!(searched.contains("\"isError\":false"), "{searched}");

        let absent = format!("s256:{}", "0".repeat(64));
        let fetched = server
            .handle_message(&format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":\
                 {{\"name\":\"fetch\",\"arguments\":{{\"id\":\"{absent}\"}}}}}}"
            ))
            .expect("応答");
        assert!(fetched.contains("\"isError\":true"), "{fetched}");
        assert!(fetched.contains("持っていない"), "{fetched}");

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
        assert!(error.contains("起こせない"), "{error}");
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
            embedding: None,
        };
        (dir, StdioServer::new(Backend::Local(Box::new(context))))
    }
}
