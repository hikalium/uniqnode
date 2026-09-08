//! serve の読み口(agent door。AGENT_DOOR (uuid:02f79aec-2f12-41e6-bede-1557d4719e4d))。
//!
//! `uniqnode serve <dir> <listen> --listen-agent <addr>` で束縛する第 2 の TcpListener。
//! 別の機械で走る LLM エージェントに、ストアを読む口と、許したコレクションへ書く口だけを
//! 見せるためのもので、(method, path) の許可表 1 つを持ち、表に無い要求は 403 で断り、
//! 表にある要求だけを主の口と同じ api::handle に委ねる。書く口は
//! `--agent-writable <collection>`(複数可)で許したコレクションへの
//! `PUT /v1/collections/{c}/documents/{name}` だけで、1 つも許していなければ表に載らない。
//! 読める範囲は `--agent-collections <collection>`(複数可)で絞れ、1 つも与えなければ
//! 今までどおり全コレクションが読める。
//! 何を返すかの判断は api.rs の 1 箇所にあり(should/0135)、ここは門でしかない。
//!
//! 境界は 3 つで、この module が持つのはそのうちの 1 つ(許可表)である。残りの 2 つ
//! (束縛先を WireGuard の口に限ること、firewall)は運用が持つ。

use crate::api::{self, ApiContext, Fetched};
use crate::c1;
use crate::http::{PeerHandler, Request, Response};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 束縛に失敗したときの再試行の間隔。読み口のアドレスは WireGuard の口で、wg1 が上がる
/// 前は存在しない(bind が EADDRNOTAVAIL で落ちる)。主の口は先に上げ、読み口はこの間隔で
/// 取りに行き続ける。試験(node/tests/agent_door.rs)はこの定数から待ち時間を組む。
pub const BIND_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// 束縛できたときに標準出力へ出す 1 行の、アドレスの後ろに付く印。主の口の
/// `listening on <addr>` と見分けるためのもので、起動スクリプトと試験はこの印で
/// 2 本目の行を選ぶ(must/0023)。
pub const LISTENING_LINE_SUFFIX: &str = " (agent door)";

/// 読み口への要求 1 本につきログに残す行の頭。行は
/// `agent <peer addr> <METHOD> <path> <status> <ms>` の形で、末尾に、要求ヘッダから写した
/// ` task=<値> agent=<値>` が在るぶんだけ付く。試験はこの頭で行を選ぶ(must/0023)。
pub const LOG_MARK: &str = "agent";

/// 記録の行に写す要求ヘッダ。3 つ組は(応答の 400 に出す字面, 引くときの名, 記録の行の欄の
/// 名)で、引くのが小文字なのは http::Request の headers が小文字化済みだからである。並びは
/// 記録の行に出る順(must/0023: 字面・引く名・欄の名はここだけにある)。
///
/// 誰が要求したかの申告であって、判断には一切入らない: 許可表も検索も、この値を見ない。
pub const RECORDED_HEADERS: [(&str, &str, &str); 2] = [
    ("X-Uniqnode-Task", "x-uniqnode-task", "task"),
    ("X-Uniqnode-Agent", "x-uniqnode-agent", "agent"),
];

/// 記録に写すヘッダの値の字種(応答の 400 の本文にこの字面で出す)と長さの上限。`/` を許す
/// のは、lamalium のタスク id が `tasks/chat` の形だからである。
pub const RECORDED_VALUE_CHARS: &str = "[A-Za-z0-9_.:/-]";
pub const RECORDED_VALUE_MAX: usize = 64;

/// 断りの本文の頭。表に無い要求の 403 と、peers 付きの search の 400 が共に持つ。
pub const ERROR_PREFIX: &str = "agent door: ";

/// peers 付きの search を断る 400 の本文。散布は他のDBノードへ問いの文を配る行為で、
/// 読み口の向こうのエージェントに選ばせない。
pub const PEERS_REFUSED: &str = "agent door: peers は使えない";

/// 許可表の行。表そのものは admit の match で、文書(AGENT_DOOR.md の許可表)と同じ順に
/// 並ぶ。応答側の検査(オブジェクトはチャンクだけ通す)が行を見るので、admit は通した
/// 事実だけでなく行を返す。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Row {
    Healthz,
    Status,
    Search,
    Object,
    Citation,
    Collections,
    /// `PUT /v1/collections/{c}/documents/{name}`。c が `--agent-writable` の集合にある
    /// ときだけ(第 2 段)。
    PutDocument,
    /// グラフ層の読み(`GET /v1/graphs/{g}…`)。g が `--agent-graph` か
    /// `--agent-graph-writable` の集合にあるときだけ(docs/design/GRAPH.md)。
    Graph,
    /// グラフ層の書き(`PUT`・`DELETE /v1/graphs/{g}…`)。g が `--agent-graph-writable` の
    /// 集合にあるときだけ。
    GraphWrite,
}

/// 読み口に許した集合。要求 1 本を捌くのに要る許可がここに揃う。空の集合の意味は種類で
/// 違うので、欄ごとに書いてある(コレクションの「空 = 全部読める」は読み口の既定として
/// 先にあり、グラフは後から足したので既定を閉じた側に置いた)。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Allowed {
    /// 書けるコレクション(`--agent-writable`)。空なら読み口は書けない。
    pub writable: Vec<String>,
    /// 読めるコレクション(`--agent-collections`)。空なら全コレクションが読める。
    pub readable: Vec<String>,
    /// 読めるグラフ(`--agent-graph`)。空ならグラフ層は読み口から見えない。
    pub graphs: Vec<String>,
    /// 書けるグラフ(`--agent-graph-writable`)。書ける名は読めもする(読み戻せない書き手を
    /// 作らない)。
    pub graphs_writable: Vec<String>,
}

impl Allowed {
    /// そのグラフを読めるか。書ける名は読める。
    pub fn reads_graph(&self, graph: &str) -> bool {
        self.graphs.iter().any(|name| name == graph) || self.writes_graph(graph)
    }

    /// そのグラフを書けるか。
    pub fn writes_graph(&self, graph: &str) -> bool {
        self.graphs_writable.iter().any(|name| name == graph)
    }

    /// 読めるグラフの名を、許した順で 1 本に(断りの本文と記録に使う)。
    pub fn readable_graphs(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.graphs.iter().map(String::as_str).collect();
        for name in &self.graphs_writable {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
        names
    }
}

/// 読み口が読めるコレクション(`--agent-collections`)を、検索の共有ポリシー
/// (CollectionScope)に直す。集合が空(指定が無い)なら All で、今までどおり全コレクション
/// が読める。集合を範囲に直す判断はこの 1 箇所にあり、効かせる 4 か所(search・collections・
/// objects・citation)はどれもこれを呼ぶ(should/0135)。
pub fn readable_scope(readable: &[String]) -> crate::search::CollectionScope {
    match readable.is_empty() {
        true => crate::search::CollectionScope::All,
        false => crate::search::CollectionScope::Only(readable.to_vec()),
    }
}

/// 読めないグラフを断る本文。install の確認は期待する本文をこの関数で組む(must/0023)。
pub fn ungraphed_refusal(graph: &str, allowed: &Allowed) -> String {
    let names = allowed.readable_graphs();
    let flag = crate::install::AGENT_GRAPH_FLAG;
    let allowed = match names.is_empty() {
        true => format!("{flag} で許したグラフは無い"),
        false => format!("{flag} で許したのは {}", names.join(", ")),
    };
    format!("{ERROR_PREFIX}グラフ {graph} は読めない({allowed})")
}

/// 書けないグラフを断る本文。
pub fn unwritable_graph_refusal(graph: &str, allowed: &Allowed) -> String {
    let flag = crate::install::AGENT_GRAPH_WRITABLE_FLAG;
    let allowed = match allowed.graphs_writable.is_empty() {
        true => format!("{flag} で許したグラフは無い"),
        false => format!("{flag} で許したのは {}", allowed.graphs_writable.join(", ")),
    };
    format!("{ERROR_PREFIX}グラフ {graph} は書けない({allowed})")
}

/// 読めないコレクションを断る本文。要求が名指しした collection も、オブジェクトの出典の
/// コレクションも、同じ文言で断る(門から見ればどちらも「集合の外を読もうとした」で
/// ある)。install の確認は期待する本文をこの関数で組む(文言の家は 1 つ。must/0023)。
pub fn unreadable_refusal(collection: &str, readable: &[String]) -> String {
    format!(
        "{ERROR_PREFIX}コレクション {collection} は読めない({} で許したのは {})",
        crate::install::AGENT_COLLECTIONS_FLAG,
        readable.join(", ")
    )
}

/// 要求を許可表に当てる。表に無ければ 403、表にある search の本文に peers があれば 400。
/// writable は `--agent-writable` で許したコレクション(与えられた順)。PUT は c がその
/// 集合にあるときだけ表にあり、集合が空なら PUT は表に無い要求として断る。
/// readable は `--agent-collections` で許したコレクション(空なら全部)。search が集合の
/// 外の collection を名指ししていれば、索引を引く前に 403 で断る。
/// 本文の JSON が壊れているときは断らず通す(壊れた本文を断るのは api::handle の仕事で、
/// ここで断ると同じ判断が 2 箇所に生える)。
pub fn admit(
    method: &str,
    path: &str,
    body: &[u8],
    allowed: &Allowed,
) -> Result<Row, Response> {
    let writable = allowed.writable.as_slice();
    let readable = allowed.readable.as_slice();
    // 書く口の的(PUT で、1 つでも許したコレクションがあるときだけ形を見る)。path の分け方は
    // 主の口と同じ api::document_path(should/0135)。
    let put_target = match method {
        "PUT" if !writable.is_empty() => api::document_path(path),
        _ => None,
    };
    let row = match (method, path) {
        ("GET", "/healthz") => Row::Healthz,
        ("GET", "/v1/status") => Row::Status,
        ("POST", "/v1/search") => Row::Search,
        ("GET", "/v1/collections") => Row::Collections,
        ("GET", _) if object_row(path) == Some(Row::Object) => Row::Object,
        ("GET", _) if object_row(path) == Some(Row::Citation) => Row::Citation,
        // グラフ層。名指ししたグラフだけを見て、一覧の口(GET /v1/graphs)は開けない:
        // 許していないグラフの名を知らせないためである(docs/design/GRAPH.md)。
        ("GET", _) if graph_of(path).is_some() => {
            let graph = graph_of(path).expect("直前に見た形");
            if !allowed.reads_graph(graph) {
                return Err(api::error_response(403, &ungraphed_refusal(graph, allowed)));
            }
            Row::Graph
        }
        ("PUT" | "DELETE", _) if graph_of(path).is_some() => {
            let graph = graph_of(path).expect("直前に見た形");
            if !allowed.writes_graph(graph) {
                return Err(api::error_response(
                    403,
                    &unwritable_graph_refusal(graph, allowed),
                ));
            }
            Row::GraphWrite
        }
        ("PUT", _) if put_target.is_some() => {
            let target = put_target.expect("直前に見た形");
            if !writable.iter().any(|allowed| allowed == target.collection) {
                return Err(api::error_response(
                    403,
                    &format!(
                        "{ERROR_PREFIX}コレクション {} は書けない({} で許したのは {})",
                        target.collection,
                        crate::install::AGENT_WRITABLE_FLAG,
                        writable.join(", ")
                    ),
                ));
            }
            Row::PutDocument
        }
        _ => {
            return Err(api::error_response(
                403,
                &format!("{ERROR_PREFIX}{method} {path} は許可されていない"),
            ))
        }
    };
    if row == Row::Search && names_peers(body) {
        return Err(api::error_response(400, PEERS_REFUSED));
    }
    // 集合の外を名指しした検索は、索引を引く前に断る。collection を省いた検索はここを
    // 素通りし、handle が渡す share(readable_scope)として run_search の交差に載る。
    if row == Row::Search {
        if let Some(collection) = requested_collection(body) {
            if !readable_scope(readable).allows(&collection) {
                return Err(api::error_response(
                    403,
                    &unreadable_refusal(&collection, readable),
                ));
            }
        }
    }
    Ok(row)
}

/// `/v1/graphs/{g}…` の g。名の形が違えばグラフの行ではない(表に無い要求として 403 に
/// なる)。形の判断は crate::graph の 1 箇所から借りる(should/0135)。
fn graph_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/v1/graphs/")?;
    let name = rest.split(|c| c == '/' || c == '?').next()?;
    crate::graph::is_valid_name(name).then_some(name)
}

/// `/v1/objects/{id}` と `/v1/objects/{id}/citation` の見分け。id が ID の形でなければ
/// どちらでもない(幻覚の id や切れた id は表に無い要求として 403 になる)。
fn object_row(path: &str) -> Option<Row> {
    let rest = path.strip_prefix("/v1/objects/")?;
    if let Some(id) = rest.strip_suffix("/citation") {
        return c1::is_object_id(id).then_some(Row::Citation);
    }
    c1::is_object_id(rest).then_some(Row::Object)
}

/// objects の行の id(`/v1/objects/{id}` と `/v1/objects/{id}/citation` のどちらからも)。
/// 行が決まった後に呼ぶので、形の検査は object_row が済ませてある。
fn object_id_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/v1/objects/")?;
    Some(rest.strip_suffix("/citation").unwrap_or(rest))
}

/// search の本文が名指しした collection。値が文字列でないときと本文が壊れているときは
/// None である: そこを断るのは api::handle の仕事で、ここで断ると同じ判断が 2 箇所に
/// 生える(names_peers と同じ規律)。
fn requested_collection(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    match c1::parse(text) {
        Ok(c1::Value::Object(map)) => match map.get("collection") {
            Some(c1::Value::Text(collection)) => Some(collection.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// search の本文が peers を名指ししているか。値が何であれ(true・false・null・配列)
/// 鍵があれば名指しとみなす: 主の口の解釈(false と null は「散布しない」)を
/// ここで写すと、その解釈が変わったときに読み口だけ古い答えを出す。
fn names_peers(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else { return false };
    match c1::parse(text) {
        Ok(c1::Value::Object(map)) => map.contains_key("peers"),
        _ => false,
    }
}

/// 委ねた先の応答を、行に応じて検める。`GET /v1/objects/{id}` は kind:"chunk" の
/// オブジェクトだけ通し、blob(PDF の原文など)や他の kind は 403 に差し替える。見分けは
/// api::classify_object の 1 箇所(should/0135)。他の行はそのまま返す。
///
/// source は、その要求が指すオブジェクトの出典のコレクション(objects と citation の行に
/// ついて、集合を絞っているときだけ呼び手が引いてある)。集合の外なら 403 に差し替える:
/// id を知っていれば読めてしまう道を、検索と同じ集合で塞ぐ。出典が引けない id は None で、
/// 今までどおりの答え(404 か、チャンクでないことによる 403)に任せる。
pub fn screen(
    row: Row,
    response: Response,
    source: Option<&str>,
    readable: &[String],
) -> Response {
    // グラフ層の応答は検めない(チャンクの規律はコレクションの側のもので、グラフには
    // 出典が無い)。
    if let (Row::Object | Row::Citation, Some(collection)) = (row, source) {
        if !readable_scope(readable).allows(collection) {
            return api::error_response(403, &unreadable_refusal(collection, readable));
        }
    }
    if row != Row::Object || response.status != 200 {
        return response;
    }
    // classify_object は所有権を取るので写しを渡す。チャンクは数 KB で、写しの代価は
    // 小さい。blob は数 MB になりうるが、その写しは捨てる応答のためのもので、境界の
    // 外へは出ない。
    match api::classify_object(response.body.clone()) {
        Fetched::Chunk { .. } => response,
        Fetched::Object { .. } | Fetched::Binary { .. } => api::error_response(
            403,
            &format!("{ERROR_PREFIX}チャンクでないオブジェクトは許可されていない"),
        ),
    }
}

/// 記録に写すヘッダの値か。字種は RECORDED_VALUE_CHARS、長さは 1..=RECORDED_VALUE_MAX 字。
fn is_recorded_value(value: &str) -> bool {
    let length = value.chars().count();
    (1..=RECORDED_VALUE_MAX).contains(&length)
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-'))
}

/// 記録の行の末尾に足す ` task=<値> agent=<値>`(在るヘッダのぶんだけ、RECORDED_HEADERS の
/// 順)。ヘッダが無いのは正常で、何も足さない。字種の外・長すぎ・空は 400 で断る: 記録に
/// 書けない値を受け取ったまま通すと、その 1 行が誰の要求だったのかを言えなくなる
/// (黙って捨てない。must/0022)。読む口にも書く口にも同じに掛かる(読み口を通る要求すべて)。
pub fn recorded_tail(request: &Request) -> Result<String, Response> {
    let mut tail = String::new();
    for (name, key, field) in RECORDED_HEADERS {
        let Some(value) = request.header(key) else { continue };
        if !is_recorded_value(value) {
            return Err(api::error_response(
                400,
                &format!(
                    "{ERROR_PREFIX}{name} の値は {RECORDED_VALUE_CHARS} の \
                     1..={RECORDED_VALUE_MAX} 字"
                ),
            ));
        }
        tail.push_str(&format!(" {field}={value}"));
    }
    Ok(tail)
}

/// 表を通った 1 本を主の口へ委ね、応答を検める。search と collections だけは、読める集合を
/// 共有ポリシーとして受け取る入口から呼ぶ: 何を返すかの判断は api.rs の 1 箇所のままで、
/// 読み口はそこへ集合を渡すだけである(should/0135)。
fn answer(context: &ApiContext, request: &Request, row: Row, allowed: &Allowed) -> Response {
    let readable = allowed.readable.as_slice();
    let response = match row {
        Row::Search => api::handle_search_within(context, request, &readable_scope(readable)),
        Row::Collections => api::handle_collections(context, &readable_scope(readable)),
        _ => api::handle(context, request),
    };
    // objects と citation は、id さえ知っていれば読めてしまう道である。集合を絞っていれば、
    // その id の出典のコレクションを引いて突き合わせる(引くのは絞っているときだけ: 絞って
    // いない読み口に索引の引き当てを足さない)。
    let source = match (row, readable.is_empty()) {
        (Row::Object | Row::Citation, false) => match object_id_of(&request.path) {
            None => None,
            Some(id) => match api::citation_collection(context, id) {
                Ok(found) => found,
                // 出典を引けないのは門の断りではなくストア側の失敗である。黙って通さず、
                // 500 で理由を言う(must/0022)。
                Err(error) => {
                    crate::log_line!("uniqnode: 読み口: {id} の出典を引けない: {error}");
                    return api::error_response(
                        500,
                        &format!("{ERROR_PREFIX}出典を引けない: {error}"),
                    );
                }
            },
        },
        _ => None,
    };
    screen(row, response, source.as_deref(), readable)
}

/// 読み口の要求 1 本を捌く: 許可表に当て、通れば主の口と同じ api::handle に委ね、応答を
/// 検め、1 行を記録する。記録は断った要求にも残る(誰が何を試したかは、通した要求と
/// 同じだけ読みたい記録である)。writable は `--agent-writable` の集合(admit に渡す)、
/// readable は `--agent-collections` の集合(空なら全コレクションが読める)。
pub fn handle(
    context: &ApiContext,
    request: &Request,
    peer: std::net::SocketAddr,
    allowed: &Allowed,
) -> Response {
    let started = Instant::now();
    // 誰が要求したかの申告(X-Uniqnode-Task・X-Uniqnode-Agent)を先に読む。判断には入れない
    // が、記録に書けない値なら要求ごと断るので、許可表より前に見る。
    let (recorded, response) = match recorded_tail(request) {
        Ok(recorded) => (
            recorded,
            match admit(&request.method, &request.path, &request.body, allowed) {
                Ok(row) => answer(context, request, row, allowed),
                Err(refused) => refused,
            },
        ),
        Err(refused) => (String::new(), refused),
    };
    crate::log_line!(
        "{LOG_MARK} {peer} {} {} {} {}{recorded}",
        request.method,
        request.path,
        response.status,
        started.elapsed().as_millis()
    );
    response
}

/// 読み口を裏のスレッドで開く。束縛できるまで BIND_RETRY_INTERVAL ごとに取りに行き、
/// できたら標準出力に `listening on <addr> (agent door)` の 1 行を出して待ち受けに入る。
/// 主の口の `listening on` は呼び手が先に出している(この関数はそれを待たない)。
///
/// 束縛の失敗は主の口を殺さない: 読み口のアドレスは wg1 が上がって初めて存在し、その
/// 前に serve が起きるのは正常な順序である。失敗は記録し(must/0022)、同じ理由が続く
/// あいだは繰り返さない(5 秒ごとに同じ行を積まない)。理由が変わればまた記す。
pub fn open(address: String, handler: Arc<PeerHandler>) {
    std::thread::spawn(move || {
        let mut last_refusal: Option<String> = None;
        let listener = loop {
            match TcpListener::bind(&address) {
                Ok(listener) => break listener,
                Err(error) => {
                    let refusal = error.to_string();
                    if last_refusal.as_deref() != Some(refusal.as_str()) {
                        crate::log_line!(
                            "uniqnode: serve: 読み口 {address} に束縛できない: {refusal}。\
                             {} 秒ごとに再試行する(主の口はそのまま受け付ける)",
                            BIND_RETRY_INTERVAL.as_secs()
                        );
                        last_refusal = Some(refusal);
                    }
                    std::thread::sleep(BIND_RETRY_INTERVAL);
                }
            }
        };
        let bound = match listener.local_addr() {
            Ok(bound) => bound.to_string(),
            Err(error) => {
                // 束縛はできている。表示に使う実アドレスが取れないだけなので、指定の
                // 字面で言って待ち受けは続ける。
                crate::log_line!("uniqnode: serve: 読み口の束縛先を読めない: {error}");
                address.clone()
            }
        };
        // 主の口と同じ取り決めの 1 行(起動スクリプトと試験が読む)。標準出力が閉じて
        // いても(読み手が先に去っていても)読み口を止める理由にはならないので、
        // println! の panic ではなく記録に倒す。
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        if let Err(error) = writeln!(stdout, "listening on {bound}{LISTENING_LINE_SUFFIX}")
            .and_then(|()| stdout.flush())
        {
            crate::log_line!("uniqnode: serve: 読み口の listening on を標準出力に書けない: {error}");
        }
        drop(stdout);
        crate::log_line!("uniqnode: serve: 読み口 {bound} で待ち受ける");
        crate::http::serve_with_peer(listener, handler);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(response: &Response) -> String {
        String::from_utf8(response.body.clone()).expect("utf-8")
    }

    /// 何も許していない読み口(読むだけ、コレクションは全部読める、グラフは見えない)。
    fn read_only() -> Allowed {
        Allowed::default()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// 書けるコレクションだけを許した読み口。
    fn writes(collections: &[&str]) -> Allowed {
        Allowed {
            writable: names(collections),
            ..Allowed::default()
        }
    }

    /// 読めるコレクションだけを絞った読み口。
    fn reads(collections: &[&str]) -> Allowed {
        Allowed {
            readable: names(collections),
            ..Allowed::default()
        }
    }

    /// 許可表の各行は行として当たり、表に無いものは 403 の本文に method と path を
    /// そのまま言う。期待値はリテラルで書く(should/0137)。
    #[test]
    fn the_table_admits_its_rows_and_names_what_it_refuses() {
        let id = format!("s256:{}", "a".repeat(64));
        assert_eq!(admit("GET", "/healthz", b"", &read_only()).ok(), Some(Row::Healthz));
        assert_eq!(admit("GET", "/v1/status", b"", &read_only()).ok(), Some(Row::Status));
        assert_eq!(admit("POST", "/v1/search", b"{\"query\":\"x\"}", &read_only()).ok(), Some(Row::Search));
        assert_eq!(admit("GET", &format!("/v1/objects/{id}"), b"", &read_only()).ok(), Some(Row::Object));
        assert_eq!(
            admit("GET", &format!("/v1/objects/{id}/citation"), b"", &read_only()).ok(),
            Some(Row::Citation)
        );
        assert_eq!(admit("GET", "/v1/collections", b"", &read_only()).ok(), Some(Row::Collections));

        let refused = admit("POST", "/v1/admin/gc", b"", &read_only()).expect_err("表に無い");
        assert_eq!(refused.status, 403);
        assert_eq!(
            body(&refused),
            "{\"error\":\"agent door: POST /v1/admin/gc は許可されていない\"}"
        );
        // 同じ path でも method が違えば表に無い。
        assert_eq!(admit("POST", "/v1/status", b"", &read_only()).expect_err("表に無い").status, 403);
        assert_eq!(admit("GET", "/v1/search", b"", &read_only()).expect_err("表に無い").status, 403);
        // オブジェクトの下の他の口(rendition・referrers)と、ID の形でない id。
        assert_eq!(
            admit("GET", &format!("/v1/objects/{id}/rendition"), b"", &read_only()).expect_err("表に無い").status,
            403
        );
        assert_eq!(
            admit("GET", &format!("/v1/objects/{id}/referrers"), b"", &read_only()).expect_err("表に無い").status,
            403
        );
        assert_eq!(admit("GET", "/v1/objects/abc", b"", &read_only()).expect_err("ID でない").status, 403);
        assert_eq!(admit("GET", "/v1/objects/abc/citation", b"", &read_only()).expect_err("ID でない").status, 403);
    }

    /// 書く口(第 2 段): PUT は --agent-writable の集合にあるコレクションだけ通り、集合に
    /// 無いコレクションは許した一覧を言って 403、集合が空なら従来どおり「許可されていない」。
    /// fetch はコレクションを許していても表に無い。should/0137: admit の PUT の腕を消すと
    /// 最初の Ok の assert が落ち、集合の検査を消すと other の 403 の本文の assert が落ちる。
    #[test]
    fn a_put_passes_only_for_a_collection_that_was_allowed() {
        let allowed = writes(&["notes", "web"]);
        let path = "/v1/collections/notes/documents/memo.md";
        assert_eq!(admit("PUT", path, b"# memo", &allowed).ok(), Some(Row::PutDocument));
        // query(出所の meta)が付いていても的はコレクションで決まる(query の検査は api.rs)。
        assert_eq!(
            admit("PUT", &format!("{path}?meta.agent=a1"), b"", &allowed).ok(),
            Some(Row::PutDocument)
        );
        assert_eq!(admit("PUT", "/v1/collections/web/documents/p.html", b"", &allowed).ok(), Some(Row::PutDocument));

        let other = admit("PUT", "/v1/collections/other/documents/memo.md", b"", &allowed).expect_err("集合に無い");
        assert_eq!(other.status, 403);
        assert_eq!(
            body(&other),
            "{\"error\":\"agent door: コレクション other は書けない(--agent-writable で許したのは notes, web)\"}"
        );
        // 集合が空なら PUT は表に無い(第 1 段と同じ本文)。
        let read_only = admit("PUT", path, b"", &read_only()).expect_err("表に無い");
        assert_eq!(read_only.status, 403);
        assert_eq!(
            body(&read_only),
            "{\"error\":\"agent door: PUT /v1/collections/notes/documents/memo.md は許可されていない\"}"
        );
        // 許したコレクションでも fetch(網に出る道)と、documents/ の形でない PUT は表に無い。
        let fetch = admit("POST", "/v1/collections/notes/fetch", b"{}", &allowed).expect_err("表に無い");
        assert_eq!(fetch.status, 403);
        assert_eq!(
            body(&fetch),
            "{\"error\":\"agent door: POST /v1/collections/notes/fetch は許可されていない\"}"
        );
        assert_eq!(admit("PUT", "/v1/collections/notes", b"", &allowed).expect_err("表に無い").status, 403);
        assert_eq!(admit("PUT", "/v1/refs/x", b"", &allowed).expect_err("表に無い").status, 403);
    }

    /// 読める集合(--agent-collections): 集合の外を名指しした search は索引を引く前に 403、
    /// 集合の中と collection 省略は通る。出典が集合の外のオブジェクトは応答の側で 403 に
    /// なり、出典が引けない(None)ものは今までどおりの答えが通る。集合が空なら全部読める。
    /// should/0137: admit の readable_scope の検査を消すと最初の 403 が Ok になって落ち、
    /// screen の source の腕を消すと出典の 403 が 200 のまま通って落ちる。
    #[test]
    fn a_collection_outside_the_readable_set_is_refused_on_the_way_in_and_on_the_way_out() {
        let readable = names(&["notes", "papers"]);
        let refused = admit(
            "POST",
            "/v1/search",
            b"{\"query\":\"x\",\"collection\":\"web\"}",
            &reads(&["notes", "papers"]),
        )
        .expect_err("集合の外");
        assert_eq!(refused.status, 403);
        assert_eq!(
            body(&refused),
            "{\"error\":\"agent door: コレクション web は読めない(--agent-collections で許したのは notes, papers)\"}"
        );
        // 集合の中と、collection を省いた検索は通る(省略は share として run_search の交差へ)。
        assert_eq!(
            admit("POST", "/v1/search", b"{\"query\":\"x\",\"collection\":\"notes\"}", &reads(&["notes", "papers"])).ok(),
            Some(Row::Search)
        );
        assert_eq!(
            admit("POST", "/v1/search", b"{\"query\":\"x\"}", &reads(&["notes", "papers"])).ok(),
            Some(Row::Search)
        );
        // 集合が空(指定が無い)なら、どのコレクションを名指ししても通る(既定は全部)。
        assert_eq!(readable_scope(&[]), crate::search::CollectionScope::All);
        assert_eq!(
            admit("POST", "/v1/search", b"{\"query\":\"x\",\"collection\":\"web\"}", &read_only()).ok(),
            Some(Row::Search)
        );

        // 応答の側(objects と citation): 出典のコレクションが集合の外なら 403。
        let chunk = "{\"kind\":\"chunk\",\"meta\":{},\"text\":\"本文\",\"v\":1}".as_bytes().to_vec();
        let outside = screen(Row::Object, Response::bytes(200, chunk.clone()), Some("web"), &readable);
        assert_eq!(outside.status, 403);
        assert_eq!(
            body(&outside),
            "{\"error\":\"agent door: コレクション web は読めない(--agent-collections で許したのは notes, papers)\"}"
        );
        let citation = screen(
            Row::Citation,
            Response::text(200, "{\"citation\":{}}"),
            Some("web"),
            &readable,
        );
        assert_eq!(citation.status, 403);
        // 集合の中の出典と、出典が引けない id はそのまま。
        let inside = screen(Row::Object, Response::bytes(200, chunk.clone()), Some("notes"), &readable);
        assert_eq!((inside.status, inside.body), (200, chunk.clone()));
        let unknown = screen(Row::Object, Response::text(404, "not held locally"), None, &readable);
        assert_eq!(unknown.status, 404);
    }

    /// peers を名指しした search は値によらず 400。壊れた JSON はここでは断らない
    /// (api::handle が断る)。
    #[test]
    fn a_search_that_names_peers_is_refused_before_the_store_sees_it() {
        for body_text in [
            "{\"query\":\"x\",\"peers\":true}",
            "{\"query\":\"x\",\"peers\":false}",
            "{\"query\":\"x\",\"peers\":null}",
            "{\"query\":\"x\",\"peers\":[\"127.0.0.1:1\"]}",
        ] {
            let refused = admit("POST", "/v1/search", body_text.as_bytes(), &read_only()).expect_err(body_text);
            assert_eq!(refused.status, 400, "{body_text}");
            assert_eq!(body(&refused), "{\"error\":\"agent door: peers は使えない\"}");
        }
        assert_eq!(admit("POST", "/v1/search", b"{\"query\":\"x\",", &read_only()).ok(), Some(Row::Search));
    }

    /// 記録に写すヘッダ: 在るものだけを RECORDED_HEADERS の順で末尾に足し、無いヘッダは
    /// 何も足さない。字種の外・長すぎ・空は 400 で、本文が字種と長さを言う。
    /// should/0137: is_recorded_value の字種の検査を消すと空白入りの 400 が Ok になって
    /// 落ち、長さの検査を消すと 65 字の段が落ちる。
    #[test]
    fn the_requester_headers_are_copied_to_the_line_and_refused_when_they_are_not_labels() {
        let asking = |headers: &[(&str, &str)]| -> Request {
            Request {
                method: "GET".to_string(),
                path: "/healthz".to_string(),
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                body: Vec::new(),
            }
        };
        // Response は Debug を持たないので、通った側は match で取り出す。
        let tail = |headers: &[(&str, &str)]| -> String {
            match recorded_tail(&asking(headers)) {
                Ok(tail) => tail,
                Err(refused) => panic!("字種の中のはず: {}", body(&refused)),
            }
        };
        assert_eq!(tail(&[]), "", "ヘッダが無いのは正常(何も足さない)");
        assert_eq!(
            tail(&[("x-uniqnode-agent", "worker-3"), ("x-uniqnode-task", "tasks/chat")]),
            " task=tasks/chat agent=worker-3",
            "行に出る順は RECORDED_HEADERS の順(ヘッダの並び順ではない)"
        );
        assert_eq!(tail(&[("x-uniqnode-task", "a.b:c_d-e/1")]), " task=a.b:c_d-e/1");
        assert_eq!(
            tail(&[("x-uniqnode-task", &"a".repeat(64))]),
            format!(" task={}", "a".repeat(64)),
            "上限ちょうどは通る"
        );
        for bad in ["", "a b", "タスク", &"a".repeat(65), "a\"b", "a;b"] {
            let refused = recorded_tail(&asking(&[("x-uniqnode-task", bad)]))
                .err()
                .unwrap_or_else(|| panic!("{bad:?} は断るはず"));
            assert_eq!(refused.status, 400, "{bad:?}");
            assert_eq!(
                body(&refused),
                "{\"error\":\"agent door: X-Uniqnode-Task の値は [A-Za-z0-9_.:/-] の 1..=64 字\"}",
                "{bad:?}"
            );
        }
        let refused = recorded_tail(&asking(&[("x-uniqnode-agent", "a b")]))
            .err()
            .expect("字種の外は断る");
        assert_eq!(
            body(&refused),
            "{\"error\":\"agent door: X-Uniqnode-Agent の値は [A-Za-z0-9_.:/-] の 1..=64 字\"}"
        );
    }

    /// 応答の検め: オブジェクトの行だけ、チャンク以外を 403 に差し替える。他の行と、
    /// 200 でない応答(404 など)はそのまま通す。
    #[test]
    fn only_chunks_leave_through_the_object_row() {
        let chunk = "{\"kind\":\"chunk\",\"meta\":{},\"text\":\"本文\",\"v\":1}".as_bytes().to_vec();
        let passed = screen(Row::Object, Response::bytes(200, chunk.clone()), None, &[]);
        assert_eq!((passed.status, passed.body), (200, chunk));

        let blob = screen(Row::Object, Response::bytes(200, vec![0xff, 0xfe, 0x00]), None, &[]);
        assert_eq!(blob.status, 403);
        assert_eq!(
            body(&blob),
            "{\"error\":\"agent door: チャンクでないオブジェクトは許可されていない\"}"
        );
        let doc_rev = screen(
            Row::Object,
            Response::bytes(200, b"{\"kind\":\"doc_rev\",\"v\":1}".to_vec()),
            None,
            &[],
        );
        assert_eq!(doc_rev.status, 403);

        let missing = screen(Row::Object, Response::text(404, "not held locally"), None, &[]);
        assert_eq!(missing.status, 404);
        let status = screen(Row::Status, Response::bytes(200, vec![0xff]), None, &[]);
        assert_eq!(status.status, 200);
    }
}
