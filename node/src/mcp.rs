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

use crate::api::{self, ApiContext, Fetched, SearchRequest, SearchResults};
use crate::c1::{self, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

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

/// 標準入出力で MCP を話す(相手が標準入力を閉じたら終わる)。
///
/// 1 行読んで 1 行書く。応答を書くたびに flush するのは、相手が次の要求を出す前に
/// この応答を読み切る必要があるためである(パイプの buffer に残したまま待つと、
/// 双方が相手を待って止まる)。
pub fn serve_stdio(
    context: &ApiContext,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            // 標準入力の EOF は相手が閉じたということ。速やかに終える。
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_message(context, trimmed) {
            output.write_all(response.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

/// メッセージ 1 本の処理。応答を返すのは要求(id を持つもの)だけで、通知(id を持た
/// ないもの)には何も返さない(返すと相手の解析が壊れる)。
pub fn handle_message(context: &ApiContext, line: &str) -> Option<String> {
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
        "initialize" => success(&id, initialize_result(params)),
        // 生存確認。空の結果が規定の答えである。
        "ping" => success(&id, Value::Object(BTreeMap::new())),
        "tools/list" => success(&id, tools_result()),
        "tools/call" => call_tool(context, &id, params),
        other => failure(&id, METHOD_NOT_FOUND, &format!("知らないメソッド: {other}")),
    })
}

/// initialize の結果。capabilities は実装しているものだけを載せる(tools だけ)。
/// 相手が求めた版に対応していればその版で、していなければこちらの版で答える。
fn initialize_result(params: Option<&Value>) -> Value {
    if let Some(Value::Object(map)) = params {
        if let Some(Value::Text(requested)) = map.get("protocolVersion") {
            if requested != PROTOCOL_VERSION {
                eprintln!(
                    "uniqnode: mcp: 相手は protocol {requested} を求めた。\
                     {PROTOCOL_VERSION} で答える"
                );
            }
        }
    }
    object(vec![
        ("protocolVersion", text(PROTOCOL_VERSION)),
        ("capabilities", object(vec![("tools", object(vec![("listChanged", Value::Bool(false))]))])),
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
    ]);
    let fetch_properties = object(vec![(
        "id",
        schema_property("string", "チャンクのオブジェクト ID(search が返す s256:… の値)"),
    )]);
    Value::Array(vec![
        object(vec![
            ("name", text("search")),
            ("title", text("uniqnode 検索")),
            (
                "description",
                text(
                    "取り込み済みの文書を検索し、本文の抜粋と出典(文書名・ページ番号・\
                     見出し・チャンク ID・取得日時)を返す。方式は語の一致(bm25)・\
                     意味の近さ(embedding)・両者の融合(hybrid)で、省略時はこの\
                     ノードの装備に従う。得点は同じ応答の中の順位付けにだけ意味があり、\
                     応答をまたいだ比較には使えない。",
                ),
            ),
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
            text("検索の方式(省略時はノードの装備に従う。埋め込みがあれば hybrid)"),
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
fn call_tool(context: &ApiContext, id: &Value, params: Option<&Value>) -> String {
    let Some(Value::Object(map)) = params else {
        return failure(id, INVALID_PARAMS, "params がない(オブジェクト)");
    };
    let Some(Value::Text(name)) = map.get("name") else {
        return failure(id, INVALID_PARAMS, "params.name がない(ツール名)");
    };
    let empty = Value::Object(BTreeMap::new());
    let arguments = map.get("arguments").unwrap_or(&empty);
    match name.as_str() {
        "search" => call_search(context, id, arguments),
        "fetch" => call_fetch(context, id, arguments),
        other => failure(id, INVALID_PARAMS, &format!("知らないツール: {other}")),
    }
}

/// search ツール。引数の形は POST /v1/search のボディと同じで、読み取りも順位付けも
/// REST と同じ関数を通る(should/0135)。
fn call_search(context: &ApiContext, id: &Value, arguments: &Value) -> String {
    let request = match api::parse_search_request(arguments) {
        Ok(request) => request,
        Err(message) => return failure(id, INVALID_PARAMS, &message),
    };
    match api::run_search(context, &request) {
        Ok(results) => success(id, tool_text(&render_search(&request, &results), false)),
        // ストアを読めないのはツールの実行の失敗。モデルが読んで判断できる形で返す。
        Err(error) => success(id, tool_text(&format!("検索に失敗した: {error}"), true)),
    }
}

/// fetch ツール。
fn call_fetch(context: &ApiContext, id: &Value, arguments: &Value) -> String {
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
    match api::fetch_object(context, object_id) {
        Err(error) => success(id, tool_text(&format!("取得に失敗した: {error}"), true)),
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
        let (dir, context) = empty_context("notifications");
        assert_eq!(
            handle_message(&context, "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}"),
            None
        );
        // 知らない通知も、応答は返さない(標準エラーには残す)。
        assert_eq!(handle_message(&context, "{\"jsonrpc\":\"2.0\",\"method\":\"x/y\"}"), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 誤りの符号は JSON-RPC の規定どおり。解析できない行は id を読めないので null で
    /// 答える。
    #[test]
    fn malformed_messages_answer_with_the_standard_error_codes() {
        let (dir, context) = empty_context("malformed");
        let parse_error = handle_message(&context, "not json").expect("応答");
        assert!(parse_error.contains("\"code\":-32700"), "{parse_error}");
        assert!(parse_error.contains("\"id\":null"), "{parse_error}");
        let unknown =
            handle_message(&context, "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"x/y\"}")
                .expect("応答");
        assert!(unknown.contains("\"code\":-32601"), "{unknown}");
        let no_version =
            handle_message(&context, "{\"id\":1,\"method\":\"tools/list\"}").expect("応答");
        assert!(no_version.contains("\"code\":-32600"), "{no_version}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// initialize は版・capabilities・サーバ情報を答え、tools/list は 2 ツールを出す。
    #[test]
    fn the_handshake_declares_the_two_tools() {
        let (dir, context) = empty_context("handshake");
        let initialized = handle_message(
            &context,
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

        let listed =
            handle_message(&context, "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}")
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
        let (dir, context) = empty_context("empty-store");
        let searched = handle_message(
            &context,
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":\
             {\"name\":\"search\",\"arguments\":{\"query\":\"世代の整合\"}}}",
        )
        .expect("応答");
        assert!(searched.contains("一致なし"), "{searched}");
        assert!(searched.contains("\"isError\":false"), "{searched}");

        let absent = format!("s256:{}", "0".repeat(64));
        let fetched = handle_message(
            &context,
            &format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":\
                 {{\"name\":\"fetch\",\"arguments\":{{\"id\":\"{absent}\"}}}}}}"
            ),
        )
        .expect("応答");
        assert!(fetched.contains("\"isError\":true"), "{fetched}");
        assert!(fetched.contains("持っていない"), "{fetched}");

        // 知らないツールと引数の誤りはプロトコルの誤り。
        let unknown_tool = handle_message(
            &context,
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":\
             {\"name\":\"delete\",\"arguments\":{}}}",
        )
        .expect("応答");
        assert!(unknown_tool.contains("\"code\":-32602"), "{unknown_tool}");
        let no_query = handle_message(
            &context,
            "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":\
             {\"name\":\"search\",\"arguments\":{}}}",
        )
        .expect("応答");
        assert!(no_query.contains("\"code\":-32602"), "{no_query}");
        assert!(no_query.contains("query"), "{no_query}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 空のストアを開いた MCP の文脈(serve と同じ形を組む。HTTP は通らない)。
    fn empty_context(name: &str) -> (std::path::PathBuf, ApiContext) {
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
        (dir, context)
    }
}
