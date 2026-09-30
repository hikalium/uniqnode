//! git の木からの取り込み(`ingest-git`。INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の
//! 「git の木からの取り込み」節)の統合テスト。一時の git の木を実物の git で作り、実プロセスの
//! serve に --serve-url で送る(should/0138)。

mod common;
use common::*;
use std::path::Path;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_uniqnode")
}

/// 木で git を走らせる(作者は固定。利用者の設定に頼らない)。
fn git(tree: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(tree)
        .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(tree: &Path, path: &str, text: &str) {
    let file = tree.join(path);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(file, text).expect("write");
}

/// ingest-git を実プロセスで走らせ、(成功したか, 標準出力, 標準エラー) を返す。
fn run_ingest_git(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(binary()).arg("ingest-git").args(args).output().expect("run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// ref が指す doc_rev の最初のチャンクの本文(serve から辿る)。
fn first_chunk_text(address: &str, reference: &str) -> String {
    let refs = body_text(&simple(address, "GET", "/v1/refs", b""));
    let doc_rev = refs
        .split(&format!("/{reference}\""))
        .nth(1)
        .and_then(|rest| rest.split("s256:").nth(1))
        .and_then(|rest| rest.split('"').next())
        .map(|hex| format!("s256:{hex}"))
        .unwrap_or_else(|| panic!("{reference} が refs に無い: {refs}"));
    let doc = body_text(&simple(address, "GET", &format!("/v1/objects/{doc_rev}"), b""));
    let chunk = doc
        .split("\"chunks\":[\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("chunks が無い: {doc}"));
    body_text(&simple(address, "GET", &format!("/v1/objects/{chunk}"), b""))
}

/// lamalium の木を模した一時の木: 取り込む道(DESIGN.md・docs/design・policy)、外す道
/// (docs/analysis)、追跡していない secrets/、コミット前の書きかけ、道の外を指す
/// シンボリックリンクを置く。
fn make_tree(name: &str) -> std::path::PathBuf {
    let tree = unique_dir(name);
    std::fs::create_dir_all(&tree).expect("mkdir");
    git(&tree, &["init", "--quiet", "-b", "main"]);
    write(&tree, "DESIGN.md", "# 設計\n\n全体の設計。\n");
    write(&tree, "docs/design/store.md", "# ストア\n\n版 1 の本文。\n");
    write(&tree, "docs/design/old.md", "# 古い\n\n消される文書。\n");
    write(&tree, "docs/design/figure.png", "not an image\n");
    write(&tree, "docs/analysis/note.md", "# 分析\n\n外す道の文書。\n");
    write(&tree, "policy/rules/first.md", "# 規則\n\n規則の本文。\n");
    std::os::unix::fs::symlink("../analysis/note.md", tree.join("docs/design/link.md"))
        .expect("symlink");
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "--quiet", "-m", "first"]);
    // 追跡していないファイルと、コミット前の書きかけ。どちらも入ってはならない。
    write(&tree, "secrets/token.md", "# 秘密\n\nSECRET-TOKEN\n");
    write(&tree, "docs/design/draft.md", "# 下書き\n\n未追跡の下書き。\n");
    write(&tree, "docs/design/store.md", "# ストア\n\n書きかけの版 2。\n");
    tree
}

/// ref に追跡されている --paths の下のファイルだけが、木の根からの相対パスを文書名にして
/// 走っている serve へ入ること。作業ディレクトリ(未追跡・書きかけ)とシンボリックリンクは
/// 入らない。再実行は全件 no-op、新しいコミットで変えた文書だけが updated になり、git から
/// 消した文書は uniqnode に残る(消さないことを固定する)。
///
/// 壊して確かめた(should/0137): ingest_git::stage で ls-tree・cat-file の代わりに作業
/// ディレクトリのファイルを写すと(std::fs::read(tree.join(&entry.path)))、"版 1 の本文" の
/// assert が赤くなる(--ref のテストも赤くなる)。stage の通常のファイルの腕に "120000" を
/// 足してリンクを書き出すと、"書き出さない(シンボリックリンク" の assert が赤くなる。
#[test]
fn ingest_git_sends_only_tracked_files_under_the_paths_at_the_ref() {
    let server = start_server("ingest-git-serve");
    let serve_url = format!("http://{}", server.address);
    let tree = make_tree("ingest-git-tree");
    let data_dir = server.dir.to_str().expect("utf-8");
    let tree_text = tree.to_str().expect("utf-8");
    let args = [
        data_dir,
        "lamalium",
        tree_text,
        "--paths",
        "DESIGN.md,docs/design,policy",
        "--serve-url",
        serve_url.as_str(),
    ];

    let (ok, first, stderr) = run_ingest_git(&args);
    assert!(ok, "{stderr}\n{first}");
    assert!(first.contains("書き出さない(シンボリックリンク: docs/design/link.md)"), "{first}");
    assert!(first.contains("lamalium/DESIGN: updated"), "{first}");
    assert!(first.contains("lamalium/docs/design/store: updated"), "{first}");
    assert!(first.contains("lamalium/policy/rules/first: updated"), "{first}");
    assert!(first.contains("対象外(拡張子): docs/design/figure.png"), "{first}");
    assert!(first.contains("取り込み: 4 件(updated 4、no-op 0)、対象外 1 件"), "{first}");

    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    for absent in [
        "lamalium/docs/analysis",
        "lamalium/secrets",
        "lamalium/docs/design/draft",
        "lamalium/docs/design/link",
    ] {
        assert!(!refs.contains(absent), "{absent} が入った: {refs}");
    }
    // コミットされた版が入り、作業ディレクトリの書きかけは入らない。
    let chunk = first_chunk_text(&server.address, "collections/lamalium/docs/design/store");
    assert!(chunk.contains("版 1 の本文"), "{chunk}");

    // 再実行は全件 no-op。
    let (ok, second, stderr) = run_ingest_git(&args);
    assert!(ok, "{stderr}");
    assert!(second.contains("取り込み: 4 件(updated 0、no-op 4)、対象外 1 件"), "{second}");

    // 新しいコミット: 1 件を書き換え、1 件を消す。
    git(&tree, &["add", "docs/design/store.md"]);
    git(&tree, &["rm", "--quiet", "docs/design/old.md"]);
    git(&tree, &["commit", "--quiet", "-m", "second"]);
    let (ok, third, stderr) = run_ingest_git(&args);
    assert!(ok, "{stderr}");
    assert!(third.contains("lamalium/docs/design/store: updated"), "{third}");
    assert!(third.contains("lamalium/DESIGN: no-op"), "{third}");
    assert!(third.contains("取り込み: 3 件(updated 1、no-op 2)、対象外 1 件"), "{third}");
    let chunk = first_chunk_text(&server.address, "collections/lamalium/docs/design/store");
    assert!(chunk.contains("書きかけの版 2"), "{chunk}");
    // git から消えた文書は消さない(INGEST の節に書いた既知の限り)。
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(refs.contains("collections/lamalium/docs/design/old"), "{refs}");

    // 置き場は TMPDIR の下に作られ、終われば残らない。
    let tmp = unique_dir("ingest-git-tmpdir");
    std::fs::create_dir_all(&tmp).expect("mkdir");
    let output = Command::new(binary())
        .arg("ingest-git")
        .args(args)
        .env("TMPDIR", &tmp)
        .output()
        .expect("run");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let leftovers: Vec<_> = std::fs::read_dir(&tmp).expect("tmp").flatten().collect();
    assert!(leftovers.is_empty(), "置き場が残った: {leftovers:?}");

    std::fs::remove_dir_all(&tmp).expect("cleanup");
    std::fs::remove_dir_all(&tree).expect("cleanup");
}

/// --ref で別の ref(fetch で最新にする clone なら origin/main)を読むこと。
#[test]
fn ingest_git_reads_the_ref_it_is_given() {
    let server = start_server("ingest-git-ref");
    let serve_url = format!("http://{}", server.address);
    let tree = make_tree("ingest-git-ref-tree");
    git(&tree, &["checkout", "--quiet", "-b", "other"]);
    git(&tree, &["add", "docs/design/store.md"]);
    git(&tree, &["commit", "--quiet", "-m", "on other"]);
    git(&tree, &["checkout", "--quiet", "main"]);
    let data_dir = server.dir.to_str().expect("utf-8");
    let tree_text = tree.to_str().expect("utf-8");

    let (ok, stdout, stderr) = run_ingest_git(&[
        data_dir,
        "lamalium",
        tree_text,
        "--paths",
        "docs/design",
        "--ref",
        "other",
        "--serve-url",
        serve_url.as_str(),
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("の other = "), "{stdout}");
    let chunk = first_chunk_text(&server.address, "collections/lamalium/docs/design/store");
    assert!(chunk.contains("書きかけの版 2"), "{chunk}");

    std::fs::remove_dir_all(&tree).expect("cleanup");
}

/// 木が無い・git の木でない・ref が無い・道がその ref に無い・--paths が無いときは、
/// 理由を言って非 0 で終わり、1 件も送らない(must/0022: 0 件で成功したことにしない)。
///
/// 壊して確かめた(should/0137): stage の missing の検査を消すと、ls-tree は無い道を
/// 黙って無視して残りの道の文書を送り成功するので、assert!(!ok) が赤くなる。rev-parse の
/// 失敗を見ずに進めると、ls-tree の失敗として落ち、"ref nope が木" の assert が赤くなる。
#[test]
fn ingest_git_refuses_a_missing_tree_ref_or_path_and_sends_nothing() {
    let server = start_server("ingest-git-refuse");
    let serve_url = format!("http://{}", server.address);
    let tree = make_tree("ingest-git-refuse-tree");
    let data_dir = server.dir.to_str().expect("utf-8");
    let tree_text = tree.to_str().expect("utf-8");
    let absent = unique_dir("ingest-git-refuse-absent");
    let absent_text = absent.to_str().expect("utf-8");
    let plain = unique_dir("ingest-git-refuse-plain");
    std::fs::create_dir_all(&plain).expect("mkdir");
    let plain_text = plain.to_str().expect("utf-8");

    let cases: [(&[&str], &str); 5] = [
        (&[absent_text, "--paths", "DESIGN.md"], "が存在しない"),
        (&[plain_text, "--paths", "DESIGN.md"], "が git の木でない"),
        (&[tree_text, "--paths", "DESIGN.md", "--ref", "nope"], "ref nope が木"),
        (&[tree_text, "--paths", "DESIGN.md,memory,docs/design"], "に無い道がある"),
        (&[tree_text], "--paths で取り込む道を与える"),
    ];
    for (tail, expected) in cases {
        let mut args = vec![data_dir, "lamalium"];
        args.extend_from_slice(tail);
        args.extend_from_slice(&["--serve-url", serve_url.as_str()]);
        let (ok, stdout, stderr) = run_ingest_git(&args);
        assert!(!ok, "{tail:?} で成功した: {stdout}");
        assert!(stderr.contains(expected), "{tail:?}: {stderr}");
        if expected == "に無い道がある" {
            assert!(stderr.contains("\"memory\""), "{stderr}");
        }
    }
    let refs = body_text(&simple(&server.address, "GET", "/v1/refs", b""));
    assert!(!refs.contains("collections/lamalium"), "{refs}");

    std::fs::remove_dir_all(&tree).expect("cleanup");
    std::fs::remove_dir_all(&plain).expect("cleanup");
}
