//! `uniqnode ingest-git`: 手元の git の木の、ある ref に追跡されているファイルだけを一時の
//! 置き場へ書き出す(INGEST の「git の木からの取り込み」節)。書き出した置き場は、そのまま
//! ingest の取り込み起点になる(main.rs の run_ingest_git)。
//!
//! 作業ディレクトリは読まない。読むのは ref が指すコミットの木(`git ls-tree` と
//! `git cat-file`)だけなので、追跡されていないファイル(secrets/ など)も、コミット前の
//! 書きかけも入らない。裸の木(bare repository)でも同じように読める。git の外へは何も
//! 取りに行かない(fetch も clone もしない)。木を最新にするのは、この命令の前に走る別の段
//! の仕事である。
//!
//! 通常のファイル(mode 100644 / 100755)だけを書き出す。シンボリックリンク(120000)と
//! サブモジュール(160000)は書かず、名前を返して呼び手に言わせる(黙って捨てない。
//! must/0022)。シンボリックリンクを書かないのは、取り込みがリンクを辿るので、指定した
//! 道の外(追跡していないファイルを含む)の中身が入りうるからである。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 書き出した置き場。落とすとディレクトリごと消える(失敗で抜けても置き場を残さない)。
pub struct Staged {
    /// 置き場の根。ingest の取り込み起点に渡すと、文書名は根からの相対パスになる。
    pub dir: PathBuf,
    /// ref を解いたコミット ID(40 桁の 16 進)。
    pub commit: String,
    /// 書き出したファイルの数。
    pub written: usize,
    /// 書き出さなかったもの(「理由: 道」の形)。
    pub left_out: Vec<String>,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `--paths` の値(カンマ区切り)を読む。空の要素・絶対パス・`..` を含む道は断る
/// (指定した道の外を読ませない)。末尾の `/` は落とす。
pub fn parse_paths(list: &str) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for raw in list.split(',') {
        let path = raw.trim().trim_end_matches('/');
        if path.is_empty() {
            return Err(format!("--paths {list:?} に空の要素がある"));
        }
        if path.starts_with('/') || path.split('/').any(|part| part == ".." || part.is_empty()) {
            return Err(format!(
                "--paths の {path:?} は木の根からの相対パスでない(絶対パス・`..`・空の段は使えない)"
            ));
        }
        paths.push(path.to_string());
    }
    Ok(paths)
}

/// git を木に対して走らせる。環境の GIT_DIR などは落とし、`-C <木>` だけで木を決める。
/// 木を探して親へ上らないよう、木の親を GIT_CEILING_DIRECTORIES に置く(木の道が別の
/// 木の中の、git の木でないディレクトリを指していたとき、外の木を読まずに断るため)。
/// パス指定は字面どおり(`:(exclude)` のような魔法を効かせない)。
fn git(tree: &Path) -> Command {
    let mut command = Command::new("git");
    if let Some(parent) = tree.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_COMMON_DIR")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(tree);
    command
}

/// git を走らせ、成功なら標準出力を返す。失敗なら git の標準エラーを載せた文を返す。
fn run_git(tree: &Path, args: &[&str], what: &str) -> Result<Vec<u8>, String> {
    let output = git(tree)
        .args(args)
        .output()
        .map_err(|error| format!("git を起こせない({what}): {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{what}: git {} が失敗した({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// ls-tree の 1 項目。
struct Entry {
    mode: String,
    kind: String,
    object: String,
    path: String,
}

/// `git ls-tree -r -z` の出力(`<mode> SP <type> SP <object> TAB <path> NUL` の並び)を読む。
fn parse_ls_tree(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = std::str::from_utf8(record)
            .map_err(|_| format!("ls-tree の項目が UTF-8 でない: {}", String::from_utf8_lossy(record)))?;
        let (head, path) = text
            .split_once('\t')
            .ok_or_else(|| format!("ls-tree の項目の形が違う: {text:?}"))?;
        let mut fields = head.split(' ');
        let (Some(mode), Some(kind), Some(object), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("ls-tree の項目の形が違う: {text:?}"));
        };
        entries.push(Entry {
            mode: mode.to_string(),
            kind: kind.to_string(),
            object: object.to_string(),
            path: path.to_string(),
        });
    }
    Ok(entries)
}

/// 置き場の名。同じ機械で同時に走っても重ならないよう、pid と時刻を入れる。
fn staging_dir(parent: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent.join(format!("uniqnode-ingest-git-{}-{nanos}", std::process::id()))
}

/// 木 `tree` の ref `reference` が指すコミットから、`paths` の下の追跡ファイルを `parent` の
/// 下の新しい置き場へ書き出す。
///
/// 断るもの(何も書き出さない。must/0022: 0 件で成功したことにしない): 木が無い・git の木で
/// ない、ref が無い、`paths` のうちそのコミットに無い道がある、書き出せる通常のファイルが
/// 1 つも無い。
pub fn stage(
    tree: &Path,
    reference: &str,
    paths: &[String],
    parent: &Path,
) -> Result<Staged, String> {
    if paths.is_empty() {
        return Err("取り込む道(--paths)が 1 つも無い".to_string());
    }
    if !tree.exists() {
        return Err(format!(
            "木 {} が存在しない(木を置く・最新にする段がまだ走っていないか、道が違う)",
            tree.display()
        ));
    }
    // 天井(GIT_CEILING_DIRECTORIES)は絶対パスでないと効かないので、正規化した道で走らせる。
    let tree = &std::fs::canonicalize(tree)
        .map_err(|error| format!("木 {} を開けない: {error}", tree.display()))?;
    run_git(tree, &["rev-parse", "--git-dir"], &format!("木 {} が git の木でない", tree.display()))?;
    if reference.is_empty() || reference.starts_with('-') {
        return Err(format!("ref {reference:?} の形が違う"));
    }
    let wanted = format!("{reference}^{{commit}}");
    let commit = git(tree)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options", &wanted])
        .output()
        .map_err(|error| format!("git を起こせない(ref の解決): {error}"))?;
    if !commit.status.success() {
        return Err(format!(
            "ref {reference} が木 {} に無い(コミットを指さない)。clone を fetch で最新にする形なら \
             origin/main のように追跡ブランチを指す",
            tree.display()
        ));
    }
    let commit = String::from_utf8_lossy(&commit.stdout).trim().to_string();

    let mut args = vec!["ls-tree", "-r", "-z", "--full-tree", commit.as_str(), "--"];
    args.extend(paths.iter().map(String::as_str));
    let listing = run_git(tree, &args, &format!("木 {} の {reference}", tree.display()))?;
    let entries = parse_ls_tree(&listing)?;

    let missing: Vec<&str> = paths
        .iter()
        .filter(|path| {
            !entries
                .iter()
                .any(|e| e.path == **path || e.path.starts_with(&format!("{path}/")))
        })
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{reference}({commit})に無い道がある(何も送っていない。道の一覧を直す): {missing:?}"
        ));
    }

    let mut staged = Staged { dir: staging_dir(parent), commit, written: 0, left_out: Vec::new() };
    std::fs::create_dir(&staged.dir)
        .map_err(|error| format!("置き場 {} を作れない: {error}", staged.dir.display()))?;
    for entry in &entries {
        match (entry.mode.as_str(), entry.kind.as_str()) {
            ("100644" | "100755", "blob") => {}
            ("120000", _) => {
                staged.left_out.push(format!("シンボリックリンク: {}", entry.path));
                continue;
            }
            ("160000", _) => {
                staged.left_out.push(format!("サブモジュール: {}", entry.path));
                continue;
            }
            _ => {
                staged.left_out.push(format!("種類 {} {}: {}", entry.mode, entry.kind, entry.path));
                continue;
            }
        }
        // git は `..` や絶対パスを木に入れないが、置き場の外へ書かないことをここでも確かめる。
        if entry.path.starts_with('/')
            || entry.path.split('/').any(|part| part == ".." || part == "." || part.is_empty())
        {
            return Err(format!("木の中の道の形が違う: {:?}", entry.path));
        }
        let bytes = run_git(
            tree,
            &["cat-file", "blob", &entry.object],
            &format!("{} の中身", entry.path),
        )?;
        let target = staged.dir.join(&entry.path);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("{} を作れない: {error}", dir.display()))?;
        }
        std::fs::write(&target, &bytes)
            .map_err(|error| format!("{} を書けない: {error}", target.display()))?;
        staged.written += 1;
    }
    if staged.written == 0 {
        return Err(format!(
            "{reference}({})の {paths:?} に通常のファイルが 1 つも無い(書き出さなかったもの: {:?})",
            staged.commit, staged.left_out
        ));
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_comma_separated_relative_paths() {
        assert_eq!(
            parse_paths("DESIGN.md, docs/design/,policy").unwrap(),
            vec!["DESIGN.md", "docs/design", "policy"]
        );
        assert!(parse_paths("a,,b").is_err());
        assert!(parse_paths("/etc").is_err());
        assert!(parse_paths("docs/../secrets").is_err());
        assert!(parse_paths("").is_err());
    }

    #[test]
    fn ls_tree_records_are_split_on_nul_and_tab() {
        let bytes = b"100644 blob 0123\tdocs/a b.md\0120000 blob 4567\tlink\0";
        let entries = parse_ls_tree(bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "docs/a b.md");
        assert_eq!(entries[0].object, "0123");
        assert_eq!(entries[1].mode, "120000");
        assert!(parse_ls_tree(b"100644 blob\tx\0").is_err());
    }
}
