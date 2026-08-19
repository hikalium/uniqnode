//! RAG ビューワ(VIEWER (uuid:4cd4c71a-ecf3-44a8-a97b-bb2c8d8fe847))。1 枚の HTML を
//! ブラウザへ出し、その頁が呼ぶ /v1/* を、走っている serve へ転送するだけの薄い被せ物で
//! ある。
//!
//! 判断は何も持たない。検索の方式の既定・劣化の判断・引用の組み立ては serve の側
//! (node/src/api.rs の run_search)にあり、この層は要求と応答をそのまま運ぶ
//! (検索の判断を二重に実装しない。should/0135)。MCP アダプタの転送する形
//! (MCP (uuid:dacd474d-424a-45d5-a278-766fc2465dd9))と同じ立ち位置で、相手が LLM か
//! ブラウザかだけが違う。
//!
//! 自分でストアを開かないので排他錠を取らない。serve が常駐したままビューワを起こせる。
//!
//! 転送するのは頁が実際に使う口だけである。ここを「/v1/ で始まれば何でも通す」に
//! すると、ビューワの口がストア API 全体への素通しになる(書き込みの口も含めて)。

use crate::http::{self, Request, Response};

/// ブラウザへ出す 1 枚。資材は別ファイルで、コンパイル時に埋め込む(should/0112)。
pub const PAGE: &str = include_str!("viewer.html");

/// 転送先の serve の既定(uniqnode serve の案内と同じ待ち受け先)。
pub const DEFAULT_SERVE_URL: &str = "http://127.0.0.1:7440";

/// 1 本の転送に掛ける期限。初回の検索は serve 側の索引構築を待つので、MCP の転送する形と
/// 同じ長さを採る。
const SERVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub struct Viewer {
    /// 転送先(host:port)。
    address: String,
    /// 与えられた URL(届かないときの理由に出す)。
    url: String,
    /// 転送先の serve が開いているはずのデータディレクトリ(起こし方の案内に使う)。
    data_dir: String,
}

impl Viewer {
    /// URL とデータディレクトリから組む。誤った URL はここで断る(起動時に落とす方が、
    /// ブラウザを開いてから 500 を見るより早く直せる)。
    pub fn new(serve_url: &str, data_dir: &str) -> Result<Viewer, String> {
        let (address, path) = http::split_http_url(serve_url)?;
        if path != "/" {
            return Err(format!(
                "{serve_url}: serve の根を指す URL であるべき(例 {DEFAULT_SERVE_URL})"
            ));
        }
        Ok(Viewer {
            address,
            url: serve_url.to_string(),
            data_dir: data_dir.to_string(),
        })
    }

    pub fn serve_url(&self) -> &str {
        &self.url
    }

    /// 届かないときの文。原因と、その serve を起こすコマンドを添える(黙って失敗せず、
    /// 読んだ者が次の手を打てる形で言う。must/0022)。
    fn unreachable(&self, cause: &str) -> String {
        format!(
            "走っている serve に届かない({}): {cause}。ビューワは自分でストアを開かないので、\
             先に serve を起こす: uniqnode serve {} {}",
            self.url, self.data_dir, self.address
        )
    }

    pub fn handle(&self, request: &Request) -> Response {
        let path = request.path.as_str();
        // 問いは頁の道の問い合わせ部分に残る(`/?q=…`)。リロードで同じ画面が出るように
        // するための頁側の仕掛けで、ビューワはその部分を読まない — 検索は頁が
        // POST /v1/search で投げ直す。ここで落とすのは、道の照合を素の `/` と同じに
        // するためだけである。
        let path = path.split('?').next().unwrap_or(path);
        match (request.method.as_str(), path) {
            ("GET", "/") => Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: PAGE.as_bytes().to_vec(),
                shutdown_after: false,
                // 頁は自分自身であって取り込んだ文書ではない。自分の生成元の /v1/* を
                // 叩けなければ検索そのものが動かないので、砂場には入れない。
                sandbox: false,
            },
            ("GET", "/healthz") => Response::text(200, "ok\n"),
            // 検索。要求の本文はそのまま渡す(読み取りと検証の家は serve 側の
            // parse_search_request の一箇所である)。
            ("POST", "/v1/search") => self.forward_post("/v1/search", &request.body),
            ("GET", "/v1/status") => self.forward_get("/v1/status"),
            ("GET", "/v1/refs") => self.forward_get("/v1/refs"),
            ("GET", _) => match object_path(path) {
                Some(forwarded) => self.forward_get(&forwarded),
                None => not_found(path),
            },
            _ => not_found(path),
        }
    }

    fn forward_get(&self, path: &str) -> Response {
        match http::get(&self.address, path, SERVE_TIMEOUT) {
            Ok(response) => relay(response),
            Err(error) => Response::text(502, &self.unreachable(&error)),
        }
    }

    fn forward_post(&self, path: &str, body: &[u8]) -> Response {
        match http::post_json(&self.address, path, body, SERVE_TIMEOUT) {
            Ok(response) => relay(response),
            Err(error) => Response::text(502, &self.unreachable(&error)),
        }
    }
}

/// 相手の応答をそのまま返す。中身は読まない: 誤りの本文も含めて、serve が言ったことが
/// そのままブラウザに届く(理由を途中で握り潰さない。must/0022)。
///
/// 種別も serve が名乗ったものを写す。ここで octet-stream に名乗り直すと、serve が
/// image/jpeg と言った写しがブラウザに届く頃には型を失い、頁の <img> も PDF の埋め込みも
/// 動かない。ビューワは運ぶだけの層なので、途中で名乗りを書き換えない。
fn relay(response: http::ClientResponse) -> Response {
    let content_type = relayed_content_type(upstream_content_type(&response));
    Response {
        status: response.status,
        content_type,
        body: response.body,
        shutdown_after: false,
        // 取り込んだ紙面は、この生成元の権限を持たせずに描く(serve が同じ印を付ける
        // のと同じ理由。node/src/http.rs の sandbox)。
        sandbox: content_type.starts_with("text/html"),
    }
}

/// 上流の応答が名乗った Content-Type。相手が名乗らなければ None である。
///
/// 名乗りが無いときに、パスの拡張子や別名から型を決めることはしない。頼んだ形と、実際に
/// 返ってきたバイト列は別のものである(must/0020: 推測でバイト列に型を付けない)。
fn upstream_content_type(response: &http::ClientResponse) -> Option<&str> {
    response.content_type.as_deref()
}

/// serve が名乗る型のうち、ビューワが写せるもの(この 4 つの口の契約が返しうる型)。
/// 表に無い型と、上流が名乗らなかったときは application/octet-stream で通す。
///
/// 表と照合してから載せる理由は二つある。写し先の http::Response が持つのが
/// &'static str であること、そして上流の言い分をそのまま応答ヘッダの字面にしないこと
/// (相手の文字列を自分のヘッダに素通しさせない)である。
const RELAYED_CONTENT_TYPES: [&str; 6] = [
    "application/json",
    "application/pdf",
    "image/jpeg",
    "image/png",
    "text/html; charset=utf-8",
    "text/plain; charset=utf-8",
];

fn relayed_content_type(upstream: Option<&str>) -> &'static str {
    let Some(named) = upstream else {
        return "application/octet-stream";
    };
    let named = named.trim();
    RELAYED_CONTENT_TYPES
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(named))
        .unwrap_or("application/octet-stream")
}

fn not_found(path: &str) -> Response {
    Response::text(
        404,
        &format!(
            "{path} はビューワの口ではない(この頁が使うのは / と \
             /v1/status・/v1/refs・/v1/search・\
             /v1/objects/{{id}}[/citation|/rendition[/{{別名}}]] だけである)\n"
        ),
    )
}

/// GET で通すのは、頁が実際に引く 4 つの形だけである:
///   /v1/objects/{id}                     チャンクの全文(c1 JSON か原文 blob)
///   /v1/objects/{id}/citation            引用
///   /v1/objects/{id}/rendition           写しの目録(生成しない)
///   /v1/objects/{id}/rendition/{別名}    写しそのもの
///
/// ID の字種も別名も確かめるのは、転送先へ組み立てるパスに要求の文字列をそのまま入れない
/// ためである(must/0020: 頼んだ形だけを受け取る)。別名は serve と同じ許可表
/// (node/src/rendition.rs)と照合し、パスに置くのは表の側の字面である。表に無い別名
/// (寸法指定・ページ番号の細工)はここで止まる。
fn object_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/v1/objects/")?;
    let (id, tail) = match rest.split_once('/') {
        Some((id, tail)) => (id, Some(tail)),
        None => (rest, None),
    };
    if !crate::c1::is_object_id(id) {
        return None;
    }
    match tail {
        None => Some(format!("/v1/objects/{id}")),
        Some("citation") => Some(format!("/v1/objects/{id}/citation")),
        Some("rendition") => Some(format!("/v1/objects/{id}/rendition")),
        Some(tail) => {
            let alias = crate::rendition::Recipe::from_alias(tail.strip_prefix("rendition/")?)?;
            Some(format!("/v1/objects/{id}/rendition/{}", alias.alias()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 頁は外部資源を参照しない(繋がっていない機械でも開ける)。問い合わせ先も同じ
    /// 生成元の /v1/* だけである。
    ///
    /// 以前はここで `<img` の字面そのものを禁じていた。いまは頁が紙面のサムネを出すが、
    /// その画像も同じ生成元の /v1/objects/{id}/rendition/thumb から来る。禁じたいのは
    /// 画像という要素ではなく外への参照なので、検査を意図の側に寄せ、参照先が /v1/ で
    /// 始まることを見る(字面を禁じたままだと、意図を満たす頁も落ちてしまう)。
    #[test]
    fn the_page_is_self_contained() {
        assert!(PAGE.starts_with("<!DOCTYPE html>"), "1 枚の HTML である");
        for forbidden in ["http://", "https://", "src=\"//", "href=\"//", "url(//", "//cdn"] {
            // 転送先の既定 URL は Rust 側の定数で、頁には書かない。
            assert!(!PAGE.contains(forbidden), "頁が外部資源 {forbidden} を参照している");
        }
        // 参照先を字面で書いているところは、どれも同じ生成元の /v1/ で始まる(字面で
        // ないところ = 変数は renditionUrls が組み立てた道で、その形は
        // the_page_builds_the_rendition_paths_itself が固定している)。
        for assignment in [".src = ", ".href = ", "src=", "href="] {
            for (offset, _) in PAGE.match_indices(assignment) {
                let value = &PAGE[offset + assignment.len()..];
                let Some(literal) = value.strip_prefix(['`', '"']) else {
                    continue; // 変数を渡している(下の試験が形を固定する)
                };
                assert!(
                    literal.starts_with("/v1/") || literal.starts_with('#'),
                    "頁が同じ生成元でない参照先を書いている: {}",
                    &literal[..40.min(literal.len())]
                );
            }
        }
        assert!(PAGE.contains("fetch(\"/v1/status\")") || PAGE.contains("api(\"/v1/status\")"));
    }

    /// 頁に流し込む値は、必ず escapeText を通してから innerHTML に入る。チャンクの本文も
    /// 文書名も取り込んだ文書から来るので、`<script>` を含む本文を素通しにすると、
    /// 取り込んだ文書が頁の上で走る。
    #[test]
    fn every_value_from_the_store_is_escaped_before_it_reaches_the_page() {
        for field in ["hit.snippet", "hit.id", "cite.collection", "cite.document", "crumbs"] {
            assert!(
                PAGE.contains(&format!("escapeText({field})")),
                "{field} が escapeText を通っていない"
            );
            assert!(
                !PAGE.contains(&format!("${{{field}}}")),
                "{field} が escapeText を通さずに埋め込まれている"
            );
        }
        // エスケープの実装そのもの(& < > の 3 文字。値は本文の位置にしか入らないので、
        // 属性用の引用符の変換は要らない)。
        assert!(PAGE.contains("\"&\": \"&amp;\""), "& のエスケープ");
        assert!(PAGE.contains("\"<\": \"&lt;\""), "< のエスケープ");
        assert!(PAGE.contains("\">\": \"&gt;\""), "> のエスケープ");
    }

    /// 入力欄と選択肢は、背景と文字を色の組で指定する。片方だけを指定すると、開いた
    /// 選択肢が地の色を失って白地に白文字になる(暗い配色で実際に起きた)。開いた選択肢は
    /// 別の描画面に出て頁の色を継がないので、option にも同じ組を与える。
    #[test]
    fn form_controls_set_both_background_and_foreground() {
        assert!(
            PAGE.contains("option { background: Canvas; color: CanvasText; }"),
            "開いた選択肢に色の組が無い"
        );
        assert!(
            !PAGE.contains("background: transparent"),
            "地の色を持たない入力欄が残っている"
        );
        // 明暗のどちらでも噛み合わせるための宣言(システム色はこれに追従する)。
        assert!(PAGE.contains("color-scheme: light dark"), "配色の申告が無い");
    }

    /// 問いは道の問い合わせ部分に載る(`/?q=…`)。頁はそこから入力欄を復元して同じ検索を
    /// やり直すので、リロードでも戻るでも画面が変わらない。ビューワ側は、その部分が付いた
    /// 道でも素の `/` と同じく頁を返す(付いた瞬間に 404 になっては元も子もない)。
    #[test]
    fn the_page_is_served_with_a_query_string_too() {
        let viewer = Viewer::new("http://127.0.0.1:1", "/tmp/store").expect("組める");
        for path in ["/", "/?q=%E4%B8%96%E4%BB%A3", "/?q=x&method=bm25&top_k=5"] {
            let request = Request {
                method: "GET".into(),
                path: path.into(),
                headers: Vec::new(),
                body: Vec::new(),
            };
            let response = viewer.handle(&request);
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(response.content_type, "text/html; charset=utf-8", "{path}");
        }
        // 問い合わせ部分は道の照合から落とすだけで、転送の許可を広げない。
        let request = Request {
            method: "GET".into(),
            path: "/v1/peers?x=1".into(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert_eq!(viewer.handle(&request).status, 404);
    }

    /// 頁が道に写す欄と、道から読み戻す欄が同じであること。片方だけ足すと、その欄だけ
    /// リロードで消える(画面の状態は入力欄そのものであり、道はその写しである)。
    #[test]
    fn the_page_round_trips_the_search_form_through_the_query_string() {
        for field in ["collection", "method", "top_k", "include_low_information"] {
            assert!(
                PAGE.contains(&format!("params.set(\"{field}\"")),
                "{field} を道に写していない"
            );
            assert!(PAGE.contains(&format!("params.get(\"{field}\")")), "{field} を読み戻していない");
        }
        assert!(PAGE.contains("params.set(\"q\"") && PAGE.contains("params.get(\"q\")"), "問い");
        // リロードと戻るの両方で同じ道を通る。
        assert!(PAGE.contains("history.pushState"), "押すたびに履歴を積む");
        assert!(PAGE.contains("popstate"), "戻る・進むで復元する");
        assert!(PAGE.contains("if (urlToForm()) search(null);"), "読み込み時にやり直す");
        // 結果そのものは道に載せない(載せると、ストアが変わった後の再読み込みで古い順位を
        // 復元してしまう)。
        assert!(!PAGE.contains("params.set(\"results\""), "結果を道に残さない");
    }

    /// 頁は全文をチャンク ID そのままの道で取る。この API のパスは生のまま扱う規約
    /// (パーセントデコードしない。node/src/http.rs)なので、`s256:` のコロンを %3A に
    /// 直すと ID の形の検査に落ちて 404 になる(実際にそうなった)。
    #[test]
    fn the_page_asks_for_an_object_by_its_raw_id() {
        assert!(PAGE.contains("`/v1/objects/${id}`"), "全文を取る道が ID そのままでない");
        assert!(
            !PAGE.contains("encodeURIComponent"),
            "パーセント符号化した ID は、受け側の形の検査を通らない"
        );
        // 受け側が実際にその形を通すこと(頁と受け側が同じ形を見ている)。
        let id = format!("s256:{}", "ab".repeat(32));
        assert_eq!(object_path(&format!("/v1/objects/{id}")), Some(format!("/v1/objects/{id}")));
        assert_eq!(object_path(&format!("/v1/objects/{}", id.replace(':', "%3A"))), None);
    }

    /// 通す口は頁が引くものだけで、それ以外は 404 になる(素通しにしない)。
    #[test]
    fn only_the_endpoints_the_page_uses_are_forwarded() {
        let id = format!("s256:{}", "ab".repeat(32));
        assert_eq!(object_path(&format!("/v1/objects/{id}")), Some(format!("/v1/objects/{id}")));
        assert_eq!(
            object_path(&format!("/v1/objects/{id}/citation")),
            Some(format!("/v1/objects/{id}/citation"))
        );
        // ID の形でないものは通さない(パスの組み立てに要求の文字列を入れない)。
        assert_eq!(object_path("/v1/objects/../../etc/passwd"), None);
        assert_eq!(object_path("/v1/objects/s256:zz/citation"), None);
        assert_eq!(object_path(&format!("/v1/objects/{id}/referrers")), None);
        assert_eq!(object_path("/v1/refs/x"), None);
    }

    /// 写しの口は、目録と、許可表にある 4 つの別名だけを通す。別名を要求の文字列から
    /// 組み立てないので、寸法指定やページ番号らしき細工はここで止まる(must/0020)。
    #[test]
    fn only_the_four_rendition_aliases_are_forwarded() {
        let id = format!("s256:{}", "ab".repeat(32));
        assert_eq!(
            object_path(&format!("/v1/objects/{id}/rendition")),
            Some(format!("/v1/objects/{id}/rendition")),
            "目録は通る"
        );
        for alias in ["source", "thumb", "page", "pagepdf"] {
            assert_eq!(
                object_path(&format!("/v1/objects/{id}/rendition/{alias}")),
                Some(format!("/v1/objects/{id}/rendition/{alias}")),
                "{alias} は許可表にある"
            );
        }
        // 許可表と、serve 側のレシピ表は同じものである(片方だけ増える事故を防ぐ)。
        assert_eq!(
            crate::rendition::Recipe::aliases(),
            vec!["source", "thumb", "page", "pagepdf"],
            "許可表が serve 側のレシピ表とずれている"
        );
        for unknown in [
            "w1200",             // 任意の寸法(ストアが永久である以上、口の広さは容量の広さ)
            "1414",              // ページ番号らしき細工
            "thumb/1414",
            "thumb/../citation",
            "THUMB",
            "",
        ] {
            assert_eq!(
                object_path(&format!("/v1/objects/{id}/rendition/{unknown}")),
                None,
                "許可表に無い別名 {unknown:?} が通っている"
            );
        }
        assert_eq!(object_path(&format!("/v1/objects/{id}/rendition/")), None);
        assert_eq!(object_path(&format!("/v1/objects/{id}/renditions")), None);
        assert_eq!(object_path(&format!("/v1/objects/{id}/rendition/thumb/extra")), None);
        // ID の形が違えば、別名が正しくても通らない。
        assert_eq!(object_path("/v1/objects/s256:zz/rendition/thumb"), None);
    }

    /// 頁は写しの道を自分で組み立てる。この形は serve 側の口の契約そのものなので、
    /// 応答から来た道(source_url)は、頁が形を検めてから href に入る。検めないと、
    /// `javascript:` を返す相手に繋いだ頁の上でそれが走る。
    #[test]
    fn the_page_checks_the_shape_of_the_path_it_was_told() {
        assert!(
            PAGE.contains("sameOriginPath(hit.source_url)"),
            "応答の道を検めずに使っている"
        );
        assert!(
            PAGE.contains("url.startsWith(\"/v1/objects/\")"),
            "検めの条件が同じ生成元の口に絞られていない"
        );
    }

    /// 字面で固定する(統合テストは頁の JavaScript を走らせないため、頁が
    /// encodeURIComponent していた事故を一度見逃している。should/0138)。
    #[test]
    fn the_page_builds_the_rendition_paths_itself() {
        for path in [
            "`/v1/objects/${id}/rendition`",
            "`/v1/objects/${id}/rendition/source`",
            "`/v1/objects/${id}/rendition/thumb`",
            "`/v1/objects/${id}/rendition/page`",
            "`/v1/objects/${id}/rendition/pagepdf`",
        ] {
            assert!(PAGE.contains(path), "頁が {path} を組み立てていない");
        }
        // 原本は当該ページを開く形で参照する(何も生成しないので必ず開く)。
        assert!(PAGE.contains("#page=${entry.page}"), "原本の当該ページへ行く道が無い");
        // サムネは見えたときに取りに行く(10 件ぶんの画像を一度に頼まない)。
        assert!(PAGE.contains("img.loading = \"lazy\""), "サムネが lazy でない");
        // 受け側が実際にその形を通すこと(頁と受け側が同じ形を見ている)。
        let id = format!("s256:{}", "ab".repeat(32));
        for alias in ["source", "thumb", "page", "pagepdf"] {
            let path = format!("/v1/objects/{id}/rendition/{alias}");
            assert_eq!(object_path(&path), Some(path.clone()), "{path}");
        }
    }

    /// 目録から来た文言(出せない理由・注記)は、テキストの位置にしか入らない。理由の
    /// 文言には取り込んだ文書の名前や外部コマンドの出力が混じるので、印付けの位置に
    /// 入れると頁の上で走る。
    #[test]
    fn words_from_the_catalog_reach_the_page_as_text_only() {
        for (number, line) in PAGE.lines().enumerate() {
            let uses_catalog_words = line.contains(".reason") || line.contains(".note");
            let is_comment = line.trim_start().starts_with("//");
            if !uses_catalog_words || is_comment {
                continue;
            }
            assert!(
                line.contains("why(") || line.contains("textContent"),
                "{} 行目: 目録の文言が印付けの位置に入っている: {line}",
                number + 1
            );
        }
        // 文言を置く関数そのものが、テキストとして置いている。
        assert!(PAGE.contains("div.textContent = `${label}: ${text"), "why が textContent でない");
    }

    /// 目録の口が無い serve(旧版)に繋いだときは、頁は何も足さない。写しの箱を作るのは
    /// 目録が返ってきた道の中だけなので、要求が通らなければ従来どおりの画面のままである
    /// (壊れた見た目にしない)。頁の JavaScript は Rust からは走らせられないので、
    /// ここで見るのは道の順序である(実際に 404 が返ることは統合テストが見る)。
    #[test]
    fn a_serve_without_the_catalog_leaves_the_page_as_it_was() {
        let body = PAGE.split("async function attachRenditions").nth(1).expect("写しを足す道");
        let probe = body.find("loadCatalog(").expect("目録を引く");
        let refusal = body.find("return;").expect("引けないときに返す道");
        assert!(refusal > probe, "目録を引く前に返している");
        assert!(refusal - probe < 200, "目録が引けなかったときにそのまま返していない");
        let box_call = body.find("renditionBox(").expect("写しの箱を作る道");
        assert!(box_call > refusal, "目録が返る前に写しの箱を作っている");
        assert!(body.contains("if (!shape) return;"), "一度も目録が返らなければ何も足さない");
    }

    /// 上流が名乗った型をそのまま写す。ここで名乗り直すと、serve が image/jpeg と言った
    /// 写しがブラウザに届く頃には型を失う。表に無い型と、名乗りが無いときだけ
    /// octet-stream になる(パスや別名から型を決めない)。
    #[test]
    fn the_content_type_is_the_one_the_serve_named() {
        assert_eq!(relayed_content_type(Some("image/jpeg")), "image/jpeg");
        assert_eq!(relayed_content_type(Some("image/png")), "image/png");
        assert_eq!(relayed_content_type(Some("application/pdf")), "application/pdf");
        assert_eq!(relayed_content_type(Some("application/json")), "application/json");
        assert_eq!(relayed_content_type(Some(" image/jpeg ")), "image/jpeg");
        assert_eq!(relayed_content_type(Some("IMAGE/JPEG")), "image/jpeg");
        // 取り込んだ紙面は型を保って運ぶ(砂場の印は relay が付ける)。
        assert_eq!(
            relayed_content_type(Some("text/html; charset=utf-8")),
            "text/html; charset=utf-8"
        );
        // 表に無い型は、その字面のまま応答ヘッダに載せない。
        assert_eq!(relayed_content_type(Some("text/html")), "application/octet-stream");
        assert_eq!(
            relayed_content_type(Some("image/jpeg\r\nX-Injected: 1")),
            "application/octet-stream"
        );
        assert_eq!(relayed_content_type(None), "application/octet-stream");
        // serve 側のレシピが返す型は、どれもこの表にある(写せない型を作らない)。
        for alias in crate::rendition::Recipe::aliases() {
            let recipe = crate::rendition::Recipe::from_alias(alias).expect("許可表にある");
            assert_eq!(
                relayed_content_type(Some(recipe.content_type())),
                recipe.content_type(),
                "{alias} の型が写せない"
            );
        }
    }

    /// 誤った転送先は起動時に断る。
    #[test]
    fn a_bad_serve_url_is_refused_when_the_viewer_is_built() {
        assert!(Viewer::new(DEFAULT_SERVE_URL, "/tmp/x").is_ok());
        assert!(Viewer::new("http://127.0.0.1:7440/v1", "/tmp/x").is_err(), "根を指すべき");
        assert!(Viewer::new("https://example.com", "/tmp/x").is_err(), "TLS は未対応");
    }

    /// 届かない転送先は、原因と起こし方を添えた 502 になる(黙って空を返さない)。
    #[test]
    fn an_unreachable_serve_answers_with_the_command_that_starts_it() {
        let viewer = Viewer::new("http://127.0.0.1:1", "/tmp/store").expect("組める");
        let request = Request {
            method: "GET".into(),
            path: "/v1/status".into(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let response = viewer.handle(&request);
        assert_eq!(response.status, 502);
        let text = String::from_utf8(response.body).expect("utf-8");
        assert!(text.contains("uniqnode serve /tmp/store 127.0.0.1:1"), "{text}");
    }

    /// 頁そのものは serve が無くても出る(まず画面が出て、届かないことは画面が言う)。
    #[test]
    fn the_page_is_served_without_asking_the_serve() {
        let viewer = Viewer::new("http://127.0.0.1:1", "/tmp/store").expect("組める");
        let request = Request {
            method: "GET".into(),
            path: "/".into(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let response = viewer.handle(&request);
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "text/html; charset=utf-8");
    }
}
