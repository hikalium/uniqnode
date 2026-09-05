//! URL からの取り込みの統合テスト(INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の
//! 「URL からの取り込み」節)。網には出ない: テストの中で 127.0.0.1 の別ポートに小さな
//! HTTP サーバ(TestSite)を立て、固定の HTML・PDF・素文・転送・404 を返す。実プロセスの
//! serve に生 HTTP/1.1 で POST /v1/collections/{c}/fetch を投げ(should/0138)、CLI は
//! 実プロセスで起こす。curl と pdftotext は要る(無ければ導入手順を示して落ちる。
//! should/0128)。

mod common;
use common::*;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_uniqnode")
}

/// テスト資材の最小 PDF(3 ページ。node/tests/ingest.rs と同じもの)。
const THREE_PAGE_PDF: &[u8] = include_bytes!("assets/three_pages.pdf");

/// 取りに行かせる紙面。script(外部と内側)・外部 css の link・img・相対リンクを含む。
/// 本文の語(取り込みの本文)は検索で引く。
const PAGE_HTML: &str = "<!DOCTYPE html>\n<html><head><title>紙面</title>\n\
    <link rel=\"stylesheet\" href=\"/style.css\">\n\
    <script src=\"/app.js\"></script>\n\
    </head><body>\n<h2>節</h2>\n\
    <p>取り込みの本文はここにある。</p>\n\
    <a href=\"/other\">相対リンク</a>\n\
    <img src=\"pic.png\" alt=\"絵\">\n\
    <script>var a = 1;</script>\n\
    </body></html>\n";

/// 相手が返す 1 件。
#[derive(Clone)]
struct Canned {
    status: u16,
    content_type: Option<&'static str>,
    body: Vec<u8>,
    location: Option<String>,
}

impl Canned {
    fn ok(content_type: Option<&'static str>, body: &[u8]) -> Canned {
        Canned {
            status: 200,
            content_type,
            body: body.to_vec(),
            location: None,
        }
    }
    fn redirect(status: u16, location: &str) -> Canned {
        Canned {
            status,
            content_type: None,
            body: Vec::new(),
            location: Some(location.to_string()),
        }
    }
}

/// 127.0.0.1 の別ポートで待つ小さな HTTP サーバ。パスごとに決めた応答を返し、受けた要求の
/// 頭(要求行とヘッダ)を記録する(curl が実際に何を送ったかを検査する。should/0138)。
/// 知らないパスは 404。応答ごとに接続を閉じる。
struct TestSite {
    address: String,
    routes: Arc<Mutex<BTreeMap<String, Canned>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestSite {
    fn start(routes: Vec<(&str, Canned)>) -> TestSite {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test site");
        let address = listener.local_addr().expect("addr").to_string();
        let routes: Arc<Mutex<BTreeMap<String, Canned>>> = Arc::new(Mutex::new(
            routes
                .into_iter()
                .map(|(path, canned)| (path.to_string(), canned))
                .collect(),
        ));
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (routes_for_thread, requests_for_thread) = (routes.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (routes, requests) = (routes_for_thread.clone(), requests_for_thread.clone());
                std::thread::spawn(move || answer(stream, &routes, &requests));
            }
        });
        TestSite {
            address,
            routes,
            requests,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// 応答を差し替える(同じ URL の 2 回目が別の内容になる場合を作る)。
    fn set(&self, path: &str, canned: Canned) {
        self.routes
            .lock()
            .expect("lock")
            .insert(path.to_string(), canned);
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("lock").clone()
    }
}

fn answer(
    stream: TcpStream,
    routes: &Mutex<BTreeMap<String, Canned>>,
    requests: &Mutex<Vec<String>>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line.trim_end_matches(['\r', '\n']).is_empty() {
            break;
        }
        head.push_str(&line);
    }
    requests.lock().expect("lock").push(head.clone());
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("/")
        .to_string();
    let canned = routes
        .lock()
        .expect("lock")
        .get(&path)
        .cloned()
        .unwrap_or(Canned {
            status: 404,
            content_type: Some("text/plain"),
            body: b"not found".to_vec(),
            location: None,
        });
    let mut response = format!(
        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
        canned.status,
        canned.body.len()
    );
    if let Some(content_type) = canned.content_type {
        response.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(location) = &canned.location {
        response.push_str(&format!("Location: {location}\r\n"));
    }
    response.push_str("\r\n");
    let mut writer = stream;
    let _ = writer.write_all(response.as_bytes());
    let _ = writer.write_all(&canned.body);
    let _ = writer.flush();
}

/// POST /v1/collections/{collection}/fetch を実プロセスの serve に投げる。
fn post_fetch(server: &Server, collection: &str, body: &str) -> HttpResponse {
    simple(
        &server.address,
        "POST",
        &format!("/v1/collections/{collection}/fetch"),
        body.as_bytes(),
    )
}

fn get_object(server: &Server, id: &str) -> String {
    body_text(&simple(
        &server.address,
        "GET",
        &format!("/v1/objects/{id}"),
        b"",
    ))
}

/// doc_rev の JSON から chunks 列のオブジェクト ID を取り出す。
fn chunk_ids_of(doc: &str) -> Vec<String> {
    doc.split("\"chunks\":[")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .map(|list| {
            list.split('"')
                .filter(|piece| piece.starts_with("s256:"))
                .map(|piece| piece.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// (a) HTML の URL を取り込むと、HTML として入り、本文が BM25 の検索に出て、原本が
/// text/html で返り、doc_rev.meta に出所(source_url・final_url・fetcher・content_type・
/// fetched_at・dropped)が入る。curl が名乗る User-Agent は uniqnode の版である。
#[test]
fn fetch_html_ingests_the_page_with_provenance_and_it_is_searchable() {
    require_curl();
    let site = TestSite::start(vec![(
        "/page.html",
        Canned::ok(Some("text/html; charset=utf-8"), PAGE_HTML.as_bytes()),
    )]);
    let server = start_server("fetch-html");
    let url = site.url("/page.html");

    let response = post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{url}\",\"name\":\"page\"}}"),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("\"ref_updated\":true"), "{body}");
    assert!(body.contains("\"media\":\"html\""), "{body}");
    assert!(body.contains("\"name\":\"page\""), "{body}");
    assert_eq!(
        json_text_field(&body, "final_url").as_deref(),
        Some(url.as_str()),
        "{body}"
    );
    assert!(
        body.contains("\"dropped\":{"),
        "HTML の応答は落としたものの数を持つ: {body}"
    );
    let doc_rev = json_text_field(&body, "doc_rev").expect("doc_rev");

    // ref 名は指定した名前。
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(refs.contains("collections/site/page"), "{refs}");

    // doc_rev.meta の出所。
    let doc = get_object(&server, &doc_rev);
    assert!(doc.contains("\"media\":\"html\""), "{doc}");
    assert_eq!(
        json_text_field(&doc, "source_url").as_deref(),
        Some(url.as_str()),
        "{doc}"
    );
    assert_eq!(
        json_text_field(&doc, "final_url").as_deref(),
        Some(url.as_str()),
        "{doc}"
    );
    assert!(
        json_text_field(&doc, "fetcher")
            .expect("fetcher")
            .starts_with("curl "),
        "{doc}"
    );
    assert_eq!(
        json_text_field(&doc, "content_type").as_deref(),
        Some("text/html; charset=utf-8"),
        "{doc}"
    );
    assert!(
        json_integer_field(&doc, "fetched_at").expect("fetched_at") > 1_700_000_000,
        "{doc}"
    );
    assert!(doc.contains("\"dropped\":{\"fonts\":"), "{doc}");
    assert!(
        !doc.contains("extractor"),
        "HTML に extractor は付かない: {doc}"
    );

    // 本文は札の落ちた素文としてチャンクになり、BM25 の検索に出る。
    let chunk_id = chunk_ids_of(&doc).into_iter().next().expect("chunk id");
    let chunk = get_object(&server, &chunk_id);
    assert!(chunk.contains("取り込みの本文はここにある。"), "{chunk}");
    assert!(
        chunk.contains("\"breadcrumbs\":[\"紙面\",\"節\"]"),
        "{chunk}"
    );
    let search = body_text(&simple(
        &server.address,
        "POST",
        "/v1/search",
        b"{\"query\":\"\xe5\x8f\x96\xe3\x82\x8a\xe8\xbe\xbc\xe3\x81\xbf\xe3\x81\xae\xe6\x9c\xac\xe6\x96\x87\",\"collection\":\"site\",\"method\":\"bm25\"}",
    ));
    assert!(search.contains(&chunk_id), "検索に本文が出るべき: {search}");
    assert!(search.contains("\"document\":\"page\""), "{search}");

    // 原本(source レシピ)は text/html で、砂場の印つきで返る。
    let source = simple(
        &server.address,
        "GET",
        &format!("/v1/objects/{chunk_id}/rendition/source"),
        b"",
    );
    assert_eq!(source.status, 200);
    assert_eq!(source.content_type, "text/html; charset=utf-8");
    assert_eq!(source.content_security_policy, "sandbox allow-scripts");
    assert!(body_text(&source).contains("取り込みの本文はここにある。"));

    // curl は uniqnode の版を名乗って取りに来る。
    let requests = site.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with("GET /page.html HTTP/1.1\r\n"),
        "{}",
        requests[0]
    );
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("user-agent: uniqnode/"),
        "{}",
        requests[0]
    );
}

/// (a') 取り込んだ HTML は自足した 1 枚である: 原本を GET すると script も外部 css の link も
/// 無く、meta uniqnode-source が入り、相対リンクは絶対になっている(node/src/web.rs の
/// self_contain の仕事。スタブのままでは赤で、実装が入ると通る)。
#[test]
fn fetched_html_is_self_contained_with_absolute_links() {
    require_curl();
    let site = TestSite::start(vec![(
        "/dir/page.html",
        Canned::ok(Some("text/html"), PAGE_HTML.as_bytes()),
    )]);
    let server = start_server("fetch-html-self-contained");
    let url = site.url("/dir/page.html");
    let response = post_fetch(&server, "site", &format!("{{\"url\":\"{url}\"}}"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    let doc_rev = json_text_field(&body, "doc_rev").expect("doc_rev");
    let doc = get_object(&server, &doc_rev);
    let chunk_id = chunk_ids_of(&doc).into_iter().next().expect("chunk id");
    let source = body_text(&simple(
        &server.address,
        "GET",
        &format!("/v1/objects/{chunk_id}/rendition/source"),
        b"",
    ));
    assert!(!source.contains("<script"), "script は落ちる: {source}");
    assert!(
        !source.contains("rel=\"stylesheet\""),
        "外部 css の link は落ちる: {source}"
    );
    assert!(
        source.contains(&format!(
            "<meta name=\"{}\"",
            uniqnode::web::SOURCE_META_NAME
        )),
        "出所の meta が入る: {source}"
    );
    assert!(
        source.contains(&format!("href=\"{}\"", site.url("/other"))),
        "相対リンクは絶対になる: {source}"
    );
    assert!(
        source.contains("取り込みの本文はここにある。"),
        "本文は残る: {source}"
    );
    // 落としたものの数は応答と meta に出る(script 2 本、外部 css 1 本、img 1 枚)。
    assert!(body.contains("\"scripts\":2"), "{body}");
    assert!(body.contains("\"stylesheets\":1"), "{body}");
    assert!(body.contains("\"images\":1"), "{body}");
    assert!(doc.contains("\"scripts\":2"), "{doc}");
}

/// `<html>` も DOCTYPE も無い断片を text/html で配る相手でも、収めた原本は HTML と名乗れる
/// 頭(DOCTYPE 前置)になり、原本を GET すると text/html で返る。自足化の出力は `<head>` で
/// 始まるので、前置が無いと写しの Content-Type 判定が text/plain に落ちる。
#[test]
fn fetched_html_fragment_is_served_back_as_html() {
    require_curl();
    let fragment = "<h2>断片</h2><p>札の無い紙面の本文。</p><script>var x = 1;</script>";
    let site = TestSite::start(vec![(
        "/fragment",
        Canned::ok(Some("text/html; charset=utf-8"), fragment.as_bytes()),
    )]);
    let server = start_server("fetch-html-fragment");
    let body = body_text(&post_fetch(
        &server,
        "site",
        &format!(
            "{{\"url\":\"{}\",\"name\":\"fragment\"}}",
            site.url("/fragment")
        ),
    ));
    assert!(body.contains("\"media\":\"html\""), "{body}");
    let doc = get_object(
        &server,
        &json_text_field(&body, "doc_rev").expect("doc_rev"),
    );
    let chunk_id = chunk_ids_of(&doc).into_iter().next().expect("chunk id");
    let source = simple(
        &server.address,
        "GET",
        &format!("/v1/objects/{chunk_id}/rendition/source"),
        b"",
    );
    assert_eq!(source.status, 200);
    assert_eq!(
        source.content_type,
        "text/html; charset=utf-8",
        "{}",
        body_text(&source)
    );
    let text = body_text(&source);
    assert!(text.starts_with("<!DOCTYPE html>\n"), "{text}");
    assert!(text.contains("札の無い紙面の本文。"), "{text}");
    assert!(!text.contains("<script"), "{text}");
}

/// (b) PDF の URL は PDF として取り込まれる(pdftotext が要る)。application/pdf と名乗る
/// 相手も、名乗らない(octet-stream の)相手も、中身が %PDF- なら PDF の道へ流れる。
#[test]
fn fetch_pdf_takes_the_pdf_road_even_without_a_content_type() {
    require_curl();
    require_pdftotext();
    let site = TestSite::start(vec![
        (
            "/paper.pdf",
            Canned::ok(Some("application/pdf"), THREE_PAGE_PDF),
        ),
        (
            "/blob",
            Canned::ok(Some("application/octet-stream"), THREE_PAGE_PDF),
        ),
        (
            "/raw",
            Canned::ok(Some("application/octet-stream"), PAGE_HTML.as_bytes()),
        ),
    ]);
    let server = start_server("fetch-pdf");

    let response = post_fetch(
        &server,
        "specs",
        &format!("{{\"url\":\"{}\"}}", site.url("/paper.pdf")),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("\"media\":\"pdf\""), "{body}");
    assert!(
        !body.contains("dropped"),
        "PDF の応答に dropped は無い: {body}"
    );
    let name = json_text_field(&body, "name").expect("name");
    assert!(name.ends_with("_paper"), "拡張子は残さない: {name}");
    let doc = get_object(
        &server,
        &json_text_field(&body, "doc_rev").expect("doc_rev"),
    );
    assert!(doc.contains("\"media\":\"pdf\""), "{doc}");
    assert!(doc.contains("\"extractor\":\"pdftotext "), "{doc}");
    assert!(doc.contains("\"fetcher\":\"curl "), "{doc}");
    let chunk_ids = chunk_ids_of(&doc);
    assert_eq!(chunk_ids.len(), 3, "{doc}");
    let first = get_object(&server, &chunk_ids[0]);
    assert!(first.contains("Page one of three"), "{first}");
    assert!(first.contains("\"page\":1"), "{first}");

    // 名乗らない相手でも、中身の頭が %PDF- なら PDF。
    let response = post_fetch(
        &server,
        "specs",
        &format!("{{\"url\":\"{}\"}}", site.url("/blob")),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("\"media\":\"pdf\""), "{body}");
    // octet-stream と名乗る HTML も、中身の頭(<!DOCTYPE html)で HTML の道へ流れる。
    let response = post_fetch(
        &server,
        "specs",
        &format!("{{\"url\":\"{}\"}}", site.url("/raw")),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("\"media\":\"html\""), "{body}");
}

/// (c) 404 の紙面は 502 で断られ、何も取り込まれない(黙って空を取り込まない。must/0022)。
/// 画像は 415 で断られる。
#[test]
fn fetch_refuses_404_pages_and_images_without_ingesting_anything() {
    require_curl();
    let site = TestSite::start(vec![(
        "/pic.png",
        Canned::ok(Some("image/png"), b"\x89PNG\r\n\x1a\n...."),
    )]);
    let server = start_server("fetch-404");

    let response = post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{}\"}}", site.url("/missing")),
    );
    assert_eq!(response.status, 502, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("HTTP 404"), "{body}");

    let response = post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{}\"}}", site.url("/pic.png")),
    );
    assert_eq!(response.status, 415, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("image/png"), "{body}");

    // 何も入っていない。
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(!refs.contains("collections/site/"), "{refs}");
}

/// (d) http と https 以外(file: と ftp:)は 400 で断り、curl を呼ばない。url の無い
/// ボディも 400。
#[test]
fn fetch_refuses_file_and_ftp_urls_with_400() {
    let server = start_server("fetch-schemes");
    for url in [
        "file:///etc/hostname",
        "ftp://example.invalid/x",
        "example.invalid/x",
        "http://",
    ] {
        let response = post_fetch(&server, "site", &format!("{{\"url\":\"{url}\"}}"));
        assert_eq!(response.status, 400, "{url}: {}", body_text(&response));
        let body = body_text(&response);
        assert!(body.contains("error"), "{body}");
    }
    let response = post_fetch(&server, "site", "{\"name\":\"x\"}");
    assert_eq!(response.status, 400, "{}", body_text(&response));
    assert!(body_text(&response).contains("url がない"));
    let response = post_fetch(
        &server,
        "site",
        "{\"url\":\"http://127.0.0.1:1/x\",\"name\":7}",
    );
    assert_eq!(response.status, 400, "{}", body_text(&response));
    let response = simple(&server.address, "GET", "/v1/collections/site/fetch", b"");
    assert_eq!(response.status, 405, "{}", body_text(&response));
}

/// (e) 名前を省くと URL から導かれる。同じ URL の 2 回目は同じ名前へ向き、内容が同じなら
/// no-op、内容が変われば上書き(ref_updated、previous が前版を指す)になる。
#[test]
fn fetch_without_a_name_derives_it_from_the_url_and_refetch_overwrites() {
    require_curl();
    let site = TestSite::start(vec![(
        "/notes/today.txt",
        Canned::ok(Some("text/plain"), "一日目の本文。\n".as_bytes()),
    )]);
    let server = start_server("fetch-name");
    let url = site.url("/notes/today.txt");
    let expected_name = format!("{}_notes_today", site.address.replace(':', "_"));

    let first = body_text(&post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{url}\"}}"),
    ));
    assert_eq!(
        json_text_field(&first, "name").as_deref(),
        Some(expected_name.as_str()),
        "{first}"
    );
    assert!(first.contains("\"media\":\"text\""), "{first}");
    assert!(first.contains("\"ref_updated\":true"), "{first}");
    let first_doc_rev = json_text_field(&first, "doc_rev").expect("doc_rev");
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(
        refs.contains(&format!("collections/site/{expected_name}")),
        "{refs}"
    );

    // 同じ内容の再取得は no-op(fetched_at が違っても、同一性は source と chunks で見る)。
    let second = body_text(&post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{url}\"}}"),
    ));
    assert_eq!(
        json_text_field(&second, "name").as_deref(),
        Some(expected_name.as_str()),
        "{second}"
    );
    assert!(second.contains("\"ref_updated\":false"), "{second}");
    assert!(second.contains("\"new_objects\":0"), "{second}");

    // 内容が変わった再取得は同じ名前への上書き。
    site.set(
        "/notes/today.txt",
        Canned::ok(Some("text/plain"), "二日目の本文。\n".as_bytes()),
    );
    let third = body_text(&post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{url}\"}}"),
    ));
    assert_eq!(
        json_text_field(&third, "name").as_deref(),
        Some(expected_name.as_str()),
        "{third}"
    );
    assert!(third.contains("\"ref_updated\":true"), "{third}");
    let third_doc_rev = json_text_field(&third, "doc_rev").expect("doc_rev");
    assert_ne!(third_doc_rev, first_doc_rev);
    let doc = get_object(&server, &third_doc_rev);
    assert_eq!(
        json_text_field(&doc, "previous").as_deref(),
        Some(first_doc_rev.as_str()),
        "{doc}"
    );
    assert_eq!(
        json_text_field(&doc, "name").as_deref(),
        Some(expected_name.as_str()),
        "{doc}"
    );
    // ref はひとつだけ(同じ URL が別名で増えていない)。
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert_eq!(refs.matches("collections/site/").count(), 1, "{refs}");
}

/// (f) 転送(301)を追い、転送後の URL が meta.final_url と応答に入る。source_url は
/// 要求した URL のまま。
#[test]
fn fetch_follows_redirects_and_records_the_final_url() {
    require_curl();
    let site = TestSite::start(vec![
        ("/old", Canned::redirect(301, "/new")),
        ("/new", Canned::ok(Some("text/html"), PAGE_HTML.as_bytes())),
    ]);
    let server = start_server("fetch-redirect");
    let old = site.url("/old");
    let new = site.url("/new");
    let body = body_text(&post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{old}\",\"name\":\"moved\"}}"),
    ));
    assert!(body.contains("\"ref_updated\":true"), "{body}");
    assert_eq!(
        json_text_field(&body, "final_url").as_deref(),
        Some(new.as_str()),
        "{body}"
    );
    let doc = get_object(
        &server,
        &json_text_field(&body, "doc_rev").expect("doc_rev"),
    );
    assert_eq!(
        json_text_field(&doc, "source_url").as_deref(),
        Some(old.as_str()),
        "{doc}"
    );
    assert_eq!(
        json_text_field(&doc, "final_url").as_deref(),
        Some(new.as_str()),
        "{doc}"
    );
    let requests = site.requests();
    assert_eq!(requests.len(), 2, "転送で 2 回取りに来る: {requests:?}");
}

/// (g) CLI の fetch が serve 停止中のストアで同じ結果になる: 名前を導き、meta に出所を
/// 残し、別プロセスの refs と get で読める。--name の指定も効く。
#[test]
fn cli_fetch_ingests_into_a_stopped_store_with_the_same_shape() {
    require_curl();
    let site = TestSite::start(vec![
        (
            "/page.html",
            Canned::ok(Some("text/html"), PAGE_HTML.as_bytes()),
        ),
        (
            "/memo.md",
            Canned::ok(Some("text/markdown"), "# 題\n\n本文。\n".as_bytes()),
        ),
    ]);
    let store_dir = unique_dir("fetch-cli-store");
    let store = store_dir.to_str().expect("utf-8");
    let expected_name = format!("{}_page", site.address.replace(':', "_"));

    let output = Command::new(binary())
        .args(["fetch", store, "site", &site.url("/page.html")])
        .output()
        .expect("run fetch");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("site/{expected_name}: updated chunks=")),
        "{stdout}"
    );
    assert!(stdout.contains("media=html"), "{stdout}");
    assert!(
        stdout.contains(&format!("final_url={}", site.url("/page.html"))),
        "{stdout}"
    );
    assert!(stdout.contains("dropped: scripts="), "{stdout}");
    let doc_rev = stdout
        .split("doc_rev=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("doc_rev id")
        .to_string();

    // --name で名前を与える。markdown は markdown として入る。
    let output = Command::new(binary())
        .args([
            "fetch",
            store,
            "site",
            &site.url("/memo.md"),
            "--name",
            "memo",
        ])
        .output()
        .expect("run fetch");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("site/memo: updated chunks=1"), "{stdout}");
    assert!(stdout.contains("media=markdown"), "{stdout}");

    // 別プロセスで読む: ref と doc_rev.meta の出所。
    let output = Command::new(binary())
        .args(["refs", store])
        .output()
        .expect("run refs");
    let refs = String::from_utf8_lossy(&output.stdout);
    assert!(
        refs.contains(&format!("collections/site/{expected_name}")),
        "{refs}"
    );
    assert!(refs.contains("collections/site/memo"), "{refs}");
    let output = Command::new(binary())
        .args(["get", store, &doc_rev])
        .output()
        .expect("run get");
    let doc = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(doc.contains("\"media\":\"html\""), "{doc}");
    assert_eq!(
        json_text_field(&doc, "source_url").as_deref(),
        Some(site.url("/page.html").as_str()),
        "{doc}"
    );
    assert!(
        json_text_field(&doc, "fetcher")
            .expect("fetcher")
            .starts_with("curl "),
        "{doc}"
    );
    assert!(doc.contains("\"dropped\":{"), "{doc}");

    // 取れない URL は理由を言って 1 で終わり、ストアには何も足さない。
    let output = Command::new(binary())
        .args(["fetch", store, "site", &site.url("/missing")])
        .output()
        .expect("run fetch");
    assert!(!output.status.success(), "404 の URL で成功してはならない");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 404"), "{stderr}");
    let output = Command::new(binary())
        .args(["fetch", store, "site", "file:///etc/hostname"])
        .output()
        .expect("run fetch");
    assert!(!output.status.success(), "file: で成功してはならない");
    assert!(String::from_utf8_lossy(&output.stderr).contains("http と https だけ"));
    // 知らない引数は usage で落ちる。
    let output = Command::new(binary())
        .args([
            "fetch",
            store,
            "site",
            &site.url("/memo.md"),
            "--bogus",
            "x",
        ])
        .output()
        .expect("run fetch");
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&store_dir).expect("cleanup");
}

/// serve の PATH に curl が無いとき、fetch は導入手順を含む 503 で明示的に失敗し、curl の
/// 要らない PUT は同じ serve で通る(must/0022 の同型。pdftotext の不在と同じ扱い)。
#[test]
fn serve_without_curl_rejects_fetch_with_instructions() {
    let site = TestSite::start(vec![(
        "/page.html",
        Canned::ok(Some("text/html"), PAGE_HTML.as_bytes()),
    )]);
    let server = start_server_with_env("fetch-no-curl", &[("PATH", "")]);
    let response = post_fetch(
        &server,
        "site",
        &format!("{{\"url\":\"{}\"}}", site.url("/page.html")),
    );
    assert_eq!(response.status, 503, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("curl コマンドが必要"), "{body}");
    assert!(body.contains("apt-get install curl"), "{body}");
    assert!(
        site.requests().is_empty(),
        "curl が無いのに取りに来てはならない"
    );

    let response = simple(
        &server.address,
        "PUT",
        "/v1/collections/site/documents/memo.md",
        "# 章\n\n本文。\n".as_bytes(),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
}
