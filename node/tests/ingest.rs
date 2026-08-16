//! 取り込み口の統合テスト(INGEST (uuid:11ff6fec-cf85-4ae9-a24c-6098964f6cce) の
//! 「取り込み口の段」の確認)。実プロセスの serve に生 HTTP/1.1 で当て(should/0138)、
//! CLI は実プロセスで起動する。

mod common;
use common::*;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_uniqnode")
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
