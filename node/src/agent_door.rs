//! serve の読み口(agent door。AGENT_DOOR (uuid:02f79aec-2f12-41e6-bede-1557d4719e4d))。
//!
//! `uniqnode serve <dir> <listen> --listen-agent <addr>` で束縛する第 2 の TcpListener。
//! 別の機械で走る LLM エージェントに、ストアを読む口と、許したコレクションへ書く口だけを
//! 見せるためのもので、(method, path) の許可表 1 つを持ち、表に無い要求は 403 で断り、
//! 表にある要求だけを主の口と同じ api::handle に委ねる。書く口は
//! `--agent-writable <collection>`(複数可)で許したコレクションへの
//! `PUT /v1/collections/{c}/documents/{name}` だけで、1 つも許していなければ表に載らない。
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
/// `agent <peer addr> <METHOD> <path> <status> <ms>` の形で、試験はこの頭で行を選ぶ
/// (must/0023)。
pub const LOG_MARK: &str = "agent";

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
}

/// 要求を許可表に当てる。表に無ければ 403、表にある search の本文に peers があれば 400。
/// writable は `--agent-writable` で許したコレクション(与えられた順)。PUT は c がその
/// 集合にあるときだけ表にあり、集合が空なら PUT は表に無い要求として断る。
/// 本文の JSON が壊れているときは断らず通す(壊れた本文を断るのは api::handle の仕事で、
/// ここで断ると同じ判断が 2 箇所に生える)。
pub fn admit(method: &str, path: &str, body: &[u8], writable: &[String]) -> Result<Row, Response> {
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
    Ok(row)
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
pub fn screen(row: Row, response: Response) -> Response {
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

/// 読み口の要求 1 本を捌く: 許可表に当て、通れば主の口と同じ api::handle に委ね、応答を
/// 検め、1 行を記録する。記録は断った要求にも残る(誰が何を試したかは、通した要求と
/// 同じだけ読みたい記録である)。writable は `--agent-writable` の集合(admit に渡す)。
pub fn handle(
    context: &ApiContext,
    request: &Request,
    peer: std::net::SocketAddr,
    writable: &[String],
) -> Response {
    let started = Instant::now();
    let response = match admit(&request.method, &request.path, &request.body, writable) {
        Ok(row) => screen(row, api::handle(context, request)),
        Err(refused) => refused,
    };
    crate::log_line!(
        "{LOG_MARK} {peer} {} {} {} {}",
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

    /// 読むだけの読み口(--agent-writable 無し)。
    const READ_ONLY: &[String] = &[];

    fn writable(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// 許可表の各行は行として当たり、表に無いものは 403 の本文に method と path を
    /// そのまま言う。期待値はリテラルで書く(should/0137)。
    #[test]
    fn the_table_admits_its_rows_and_names_what_it_refuses() {
        let id = format!("s256:{}", "a".repeat(64));
        assert_eq!(admit("GET", "/healthz", b"", READ_ONLY).ok(), Some(Row::Healthz));
        assert_eq!(admit("GET", "/v1/status", b"", READ_ONLY).ok(), Some(Row::Status));
        assert_eq!(admit("POST", "/v1/search", b"{\"query\":\"x\"}", READ_ONLY).ok(), Some(Row::Search));
        assert_eq!(admit("GET", &format!("/v1/objects/{id}"), b"", READ_ONLY).ok(), Some(Row::Object));
        assert_eq!(
            admit("GET", &format!("/v1/objects/{id}/citation"), b"", READ_ONLY).ok(),
            Some(Row::Citation)
        );
        assert_eq!(admit("GET", "/v1/collections", b"", READ_ONLY).ok(), Some(Row::Collections));

        let refused = admit("POST", "/v1/admin/gc", b"", READ_ONLY).expect_err("表に無い");
        assert_eq!(refused.status, 403);
        assert_eq!(
            body(&refused),
            "{\"error\":\"agent door: POST /v1/admin/gc は許可されていない\"}"
        );
        // 同じ path でも method が違えば表に無い。
        assert_eq!(admit("POST", "/v1/status", b"", READ_ONLY).expect_err("表に無い").status, 403);
        assert_eq!(admit("GET", "/v1/search", b"", READ_ONLY).expect_err("表に無い").status, 403);
        // オブジェクトの下の他の口(rendition・referrers)と、ID の形でない id。
        assert_eq!(
            admit("GET", &format!("/v1/objects/{id}/rendition"), b"", READ_ONLY).expect_err("表に無い").status,
            403
        );
        assert_eq!(
            admit("GET", &format!("/v1/objects/{id}/referrers"), b"", READ_ONLY).expect_err("表に無い").status,
            403
        );
        assert_eq!(admit("GET", "/v1/objects/abc", b"", READ_ONLY).expect_err("ID でない").status, 403);
        assert_eq!(admit("GET", "/v1/objects/abc/citation", b"", READ_ONLY).expect_err("ID でない").status, 403);
    }

    /// 書く口(第 2 段): PUT は --agent-writable の集合にあるコレクションだけ通り、集合に
    /// 無いコレクションは許した一覧を言って 403、集合が空なら従来どおり「許可されていない」。
    /// fetch はコレクションを許していても表に無い。should/0137: admit の PUT の腕を消すと
    /// 最初の Ok の assert が落ち、集合の検査を消すと other の 403 の本文の assert が落ちる。
    #[test]
    fn a_put_passes_only_for_a_collection_that_was_allowed() {
        let allowed = writable(&["notes", "web"]);
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
        let read_only = admit("PUT", path, b"", READ_ONLY).expect_err("表に無い");
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
            let refused = admit("POST", "/v1/search", body_text.as_bytes(), READ_ONLY).expect_err(body_text);
            assert_eq!(refused.status, 400, "{body_text}");
            assert_eq!(body(&refused), "{\"error\":\"agent door: peers は使えない\"}");
        }
        assert_eq!(admit("POST", "/v1/search", b"{\"query\":\"x\",", READ_ONLY).ok(), Some(Row::Search));
    }

    /// 応答の検め: オブジェクトの行だけ、チャンク以外を 403 に差し替える。他の行と、
    /// 200 でない応答(404 など)はそのまま通す。
    #[test]
    fn only_chunks_leave_through_the_object_row() {
        let chunk = "{\"kind\":\"chunk\",\"meta\":{},\"text\":\"本文\",\"v\":1}".as_bytes().to_vec();
        let passed = screen(Row::Object, Response::bytes(200, chunk.clone()));
        assert_eq!((passed.status, passed.body), (200, chunk));

        let blob = screen(Row::Object, Response::bytes(200, vec![0xff, 0xfe, 0x00]));
        assert_eq!(blob.status, 403);
        assert_eq!(
            body(&blob),
            "{\"error\":\"agent door: チャンクでないオブジェクトは許可されていない\"}"
        );
        let doc_rev = screen(Row::Object, Response::bytes(200, b"{\"kind\":\"doc_rev\",\"v\":1}".to_vec()));
        assert_eq!(doc_rev.status, 403);

        let missing = screen(Row::Object, Response::text(404, "not held locally"));
        assert_eq!(missing.status, 404);
        let status = screen(Row::Status, Response::bytes(200, vec![0xff]));
        assert_eq!(status.status, 200);
    }
}
