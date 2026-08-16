//! 取り込み口の統合テスト(INGEST (uuid:11ff6fec-cf85-4ae9-a24c-6098964f6cce) の
//! 「取り込み口の段」と「PDF の段」の確認)。実プロセスの serve に生 HTTP/1.1 で当て
//! (should/0138)、CLI は実プロセスで起動する。

mod common;
use common::*;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_uniqnode")
}

/// テスト資材の最小 PDF(3 ページ、手書き。should/0112)。各ページの本文は
/// "Page one of three" の形の 1 文で、pdftotext の抽出はページごとに form feed で
/// 区切られる。
const THREE_PAGE_PDF: &[u8] = include_bytes!("assets/three_pages.pdf");

/// PDF テストの前提確認。pdftotext が無い環境では黙って飛ばさず、導入手順を示して
/// 失敗する(docs/design/TESTING.md の外部コマンドの規約。openssl_interop と同じ扱い)。
fn require_pdftotext() {
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

/// doc_rev の JSON から chunks 列のオブジェクト ID を取り出す(テスト用の素朴な抽出)。
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

/// PUT で入れた文書の引用が、ref から doc_rev、chunks の添字と辿って組めること。
/// 再 PUT が no-op であること。
#[test]
fn put_document_and_resolve_a_citation_then_reput_is_noop() {
    let server = start_server("ingest-api");
    let markdown = "# 甲\n\n## 乙\n\n引用される本文。\n";

    let response = simple(
        &server.address,
        "PUT",
        "/v1/collections/notes/documents/memo.md",
        markdown.as_bytes(),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let first = body_text(&response);
    assert!(first.contains("\"ref_updated\":true"), "{first}");
    let doc_rev = json_text_field(&first, "doc_rev").expect("doc_rev");

    // 再 PUT は no-op。
    let response = simple(
        &server.address,
        "PUT",
        "/v1/collections/notes/documents/memo.md",
        markdown.as_bytes(),
    );
    let second = body_text(&response);
    assert!(second.contains("\"ref_updated\":false"), "{second}");
    assert!(second.contains("\"new_objects\":0"), "{second}");

    // ref 一覧に文書名が載る。
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(refs.contains("collections/notes/memo"), "{refs}");

    // doc_rev を取り、最初のチャンク ID を辿って本文とパンくずに到達する。
    let doc = body_text(&simple(&server.address, "GET", &format!("/v1/objects/{doc_rev}"), b""));
    let chunk_id = doc
        .split("\"chunks\":[\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("chunk id");
    let chunk = body_text(&simple(&server.address, "GET", &format!("/v1/objects/{chunk_id}"), b""));
    assert!(chunk.contains("引用される本文。"), "{chunk}");
    assert!(chunk.contains("\"breadcrumbs\":[\"甲\",\"乙\"]"), "{chunk}");

    // 対象外の拡張子は明示的に拒否される。
    let response = simple(
        &server.address,
        "PUT",
        "/v1/collections/notes/documents/prog.rs",
        b"fn main() {}",
    );
    assert_eq!(response.status, 400, "{}", body_text(&response));
}

/// CLI がディレクトリを再帰で取り込み、対象外を黙って捨てずに報告すること。
#[test]
fn cli_ingest_walks_a_directory_and_reports_skipped_files() {
    let store_dir = unique_dir("ingest-cli-store");
    let corpus = unique_dir("ingest-cli-corpus");
    std::fs::create_dir_all(corpus.join("sub")).expect("mkdir");
    std::fs::write(corpus.join("a.md"), "# 章\n\n本文 A。\n").expect("write");
    std::fs::write(corpus.join("sub/b.txt"), "本文 B。\n").expect("write");
    std::fs::write(corpus.join("c.rs"), "fn main() {}\n").expect("write");

    let output = Command::new(binary())
        .args([
            "ingest",
            store_dir.to_str().expect("utf-8"),
            "notes",
            corpus.to_str().expect("utf-8"),
        ])
        .output()
        .expect("run ingest");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("notes/a: updated"), "{stdout}");
    assert!(stdout.contains("notes/sub/b: updated"), "{stdout}");
    assert!(stdout.contains("対象外(拡張子): c.rs"), "{stdout}");

    // 取り込んだ ref が残っている(別プロセスで一覧)。
    let output = Command::new(binary())
        .args(["refs", store_dir.to_str().expect("utf-8")])
        .output()
        .expect("run refs");
    let refs = String::from_utf8_lossy(&output.stdout);
    assert!(refs.contains("collections/notes/a"), "{refs}");
    assert!(refs.contains("collections/notes/sub/b"), "{refs}");

    std::fs::remove_dir_all(&store_dir).expect("cleanup");
    std::fs::remove_dir_all(&corpus).expect("cleanup");
}

/// PUT で入れた PDF のチャンクに物理ページ番号が付くこと: 先頭チャンクの page が 1、
/// 最終チャンクの page が総ページ数の 3 に一致し、空チャンクが生まれない。再 PUT が
/// no-op で、doc_rev.meta.extractor に pdftotext の名前と版が残る。
#[test]
fn put_pdf_document_pages_are_recorded_and_reput_is_noop() {
    require_pdftotext();
    let server = start_server("ingest-pdf-api");
    let path = "/v1/collections/specs/documents/three_pages.pdf";
    let response = simple(&server.address, "PUT", path, THREE_PAGE_PDF);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let first = body_text(&response);
    assert!(first.contains("\"ref_updated\":true"), "{first}");
    let doc_rev = json_text_field(&first, "doc_rev").expect("doc_rev");

    // 再 PUT は no-op(同じ抽出器なら同じチャンク列に戻る)。
    let second = body_text(&simple(&server.address, "PUT", path, THREE_PAGE_PDF));
    assert!(second.contains("\"ref_updated\":false"), "{second}");
    assert!(second.contains("\"new_objects\":0"), "{second}");

    // doc_rev: media は pdf、extractor は pdftotext の名前と版。
    let doc = body_text(&simple(&server.address, "GET", &format!("/v1/objects/{doc_rev}"), b""));
    assert!(doc.contains("\"media\":\"pdf\""), "{doc}");
    assert!(doc.contains("\"extractor\":\"pdftotext "), "{doc}");

    // 3 ページで 3 チャンク。ページ番号は 1 始まりで最終が総ページ数。空チャンクは無い。
    let chunk_ids = chunk_ids_of(&doc);
    assert_eq!(chunk_ids.len(), 3, "{doc}");
    let mut pages = Vec::new();
    for id in &chunk_ids {
        let chunk =
            body_text(&simple(&server.address, "GET", &format!("/v1/objects/{id}"), b""));
        let text = json_text_field(&chunk, "text").expect("text");
        assert!(!text.is_empty(), "空チャンクが生まれてはならない: {chunk}");
        pages.push(json_integer_field(&chunk, "page").expect("page"));
    }
    assert_eq!(pages, vec![1, 2, 3]);

    // 本文は抽出テキストであって PDF バイナリではない。
    let first_chunk = body_text(&simple(
        &server.address,
        "GET",
        &format!("/v1/objects/{}", chunk_ids[0]),
        b"",
    ));
    assert!(first_chunk.contains("Page one of three"), "{first_chunk}");
}

/// CLI が --pdftotext の明示指定で PDF を取り込み、再取り込みが no-op であること。
/// 名前 "pdftotext" の明示指定は PATH から解決される(明示指定が PATH より優先される
/// ことは、下の不在テストが存在しないパスの失敗で確かめる)。
#[test]
fn cli_ingest_pdf_records_the_extractor_and_reingest_is_noop() {
    require_pdftotext();
    let store_dir = unique_dir("ingest-pdf-cli-store");
    let corpus = unique_dir("ingest-pdf-cli-corpus");
    std::fs::create_dir_all(&corpus).expect("mkdir");
    std::fs::write(corpus.join("three_pages.pdf"), THREE_PAGE_PDF).expect("write");

    let run = || {
        let output = Command::new(binary())
            .args([
                "ingest",
                store_dir.to_str().expect("utf-8"),
                "specs",
                corpus.to_str().expect("utf-8"),
                "--pdftotext",
                "pdftotext",
            ])
            .output()
            .expect("run ingest");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let first = run();
    assert!(first.contains("specs/three_pages: updated chunks=3"), "{first}");
    let second = run();
    assert!(second.contains("specs/three_pages: no-op"), "{second}");
    assert!(second.contains("new_objects=0"), "{second}");

    // doc_rev に extractor(pdftotext の名前と版)が残る(別プロセスで読む)。
    let doc_rev = first
        .split("doc_rev=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("doc_rev id");
    let output = Command::new(binary())
        .args(["get", store_dir.to_str().expect("utf-8"), doc_rev])
        .output()
        .expect("run get");
    let doc = String::from_utf8_lossy(&output.stdout);
    assert!(doc.contains("\"media\":\"pdf\""), "{doc}");
    assert!(doc.contains("\"extractor\":\"pdftotext "), "{doc}");

    std::fs::remove_dir_all(&store_dir).expect("cleanup");
    std::fs::remove_dir_all(&corpus).expect("cleanup");
}

/// pdftotext が見つからないとき、CLI は黙って PDF を飛ばさず、導入手順を示して
/// 失敗すること(must/0022 の同型)。存在しないパスの明示指定で失敗することは、
/// --pdftotext が PATH より優先される証明でもある。
#[test]
fn cli_ingest_without_pdftotext_fails_with_install_instructions() {
    let store_dir = unique_dir("ingest-pdf-missing-store");
    let corpus = unique_dir("ingest-pdf-missing-corpus");
    std::fs::create_dir_all(&corpus).expect("mkdir");
    std::fs::write(corpus.join("doc.pdf"), THREE_PAGE_PDF).expect("write");
    let output = Command::new(binary())
        .args([
            "ingest",
            store_dir.to_str().expect("utf-8"),
            "specs",
            corpus.to_str().expect("utf-8"),
            "--pdftotext",
            "/nonexistent/pdftotext",
        ])
        .output()
        .expect("run ingest");
    assert!(!output.status.success(), "存在しない pdftotext の指定で成功してはならない");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pdftotext コマンドが必要"), "{stderr}");
    assert!(stderr.contains("apt-get download poppler-utils"), "{stderr}");
    std::fs::remove_dir_all(&store_dir).expect("cleanup");
    std::fs::remove_dir_all(&corpus).expect("cleanup");
}

/// serve の PATH に pdftotext が無いとき、PDF の PUT は導入手順を含む 503 で明示的に
/// 失敗し、pdftotext の要らない Markdown の PUT は同じ serve で通ること。
#[test]
fn serve_without_pdftotext_rejects_pdf_with_instructions() {
    let server = start_server_with_env("ingest-pdf-no-path", &[("PATH", "")]);
    let response = simple(
        &server.address,
        "PUT",
        "/v1/collections/specs/documents/doc.pdf",
        THREE_PAGE_PDF,
    );
    assert_eq!(response.status, 503, "{}", body_text(&response));
    let body = body_text(&response);
    assert!(body.contains("pdftotext コマンドが必要"), "{body}");
    assert!(body.contains("apt-get download poppler-utils"), "{body}");

    let response = simple(
        &server.address,
        "PUT",
        "/v1/collections/specs/documents/memo.md",
        "# 章\n\n本文。\n".as_bytes(),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
}
