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
//! 転送するのは頁が実際に使う 5 つの口だけである。ここを「/v1/ で始まれば何でも通す」に
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
        match (request.method.as_str(), path) {
            ("GET", "/") => Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: PAGE.as_bytes().to_vec(),
                shutdown_after: false,
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
/// 種別は octet-stream で通す。GET /v1/objects/{id} はチャンクの c1 JSON も PDF の原文
/// blob も同じ口から返すので、こちらで JSON だと名乗ると嘘になることがある。頁の側は
/// 本文として読んでから JSON として解こうと試みる。
fn relay(response: http::ClientResponse) -> Response {
    Response {
        status: response.status,
        content_type: "application/octet-stream",
        body: response.body,
        shutdown_after: false,
    }
}

fn not_found(path: &str) -> Response {
    Response::text(
        404,
        &format!(
            "{path} はビューワの口ではない(この頁が使うのは / と \
             /v1/status・/v1/refs・/v1/search・/v1/objects/{{id}}[/citation] だけである)\n"
        ),
    )
}

/// GET /v1/objects/{id} と GET /v1/objects/{id}/citation だけを、形を確かめてから通す。
/// ID の字種を確かめるのは、転送先へ組み立てるパスに要求の文字列をそのまま入れないため
/// である(must/0020: 頼んだ形だけを受け取る)。
fn object_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/v1/objects/")?;
    let (id, suffix) = match rest.strip_suffix("/citation") {
        Some(id) => (id, "/citation"),
        None => (rest, ""),
    };
    crate::c1::is_object_id(id).then(|| format!("/v1/objects/{id}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 頁は外部資源を参照しない(繋がっていない機械でも開ける)。問い合わせ先も同じ
    /// 生成元の /v1/* だけである。
    #[test]
    fn the_page_is_self_contained() {
        assert!(PAGE.starts_with("<!DOCTYPE html>"), "1 枚の HTML である");
        for forbidden in ["http://", "https://", "//cdn", "<img"] {
            // 転送先の既定 URL は Rust 側の定数で、頁には書かない。
            assert!(!PAGE.contains(forbidden), "頁が外部資源 {forbidden} を参照している");
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

    /// 通す口は 5 つだけで、それ以外は 404 になる(素通しにしない)。
    #[test]
    fn only_the_five_endpoints_the_page_uses_are_forwarded() {
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
