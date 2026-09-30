//! リポジトリ衛生検査。lamalium の
//! check-policy / check-docs の uniqnode 版。機械的に検査できる範囲:
//! - policy/ の形式(ファイル名・番号帯・Level 行と置き場所の一致・必須節・廃止スタブ)
//! - policy 参照の解決(policy/ の外のファイルが存在しないポリシー番号を参照しない。
//!   policy/ 本体は lamalium からの逐語コピーであり、未採用ポリシーへの相互参照を含む
//!   ことが policy/README.md の Provenance に記録済みなので対象外)
//! - markdown の強調記法禁止(must/0021。太字 ** と __ を検査。単一マーカーの斜体は
//!   グロブ・識別子との誤検知があるためレビュー領域に残す)
//! - docs/ の UUID アンカー(must/0013。H1 直下のアンカー定義、重複なし、#uuid 参照の解決、
//!   docs/ 配下の .md へのパスリンク禁止)

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("repo root").to_path_buf()
}

fn markdown_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            markdown_files_under(&path, out);
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// 検査対象の markdown 一覧(policy/ + docs/ + トップレベル)。
fn tracked_markdown() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = vec![root.join("README.md"), root.join("SPEC.md")];
    markdown_files_under(&root.join("policy"), &mut files);
    markdown_files_under(&root.join("docs"), &mut files);
    files.sort();
    files
}

fn display(path: &Path) -> String {
    path.strip_prefix(repo_root()).unwrap_or(path).display().to_string()
}

// ---- policy/ の形式 ----

struct PolicyLevel {
    directory: &'static str,
    band: std::ops::Range<u32>,
    keywords: &'static [&'static str],
}

const LEVELS: [PolicyLevel; 3] = [
    PolicyLevel {
        directory: "must",
        band: 0..100,
        keywords: &["MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT"],
    },
    PolicyLevel {
        directory: "should",
        band: 101..200,
        keywords: &["SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED"],
    },
    PolicyLevel { directory: "may", band: 201..300, keywords: &["MAY", "OPTIONAL"] },
];

fn live_policy_numbers() -> BTreeSet<(String, u32)> {
    let root = repo_root();
    let mut numbers = BTreeSet::new();
    for level in &LEVELS {
        let dir = root.join("policy").join(level.directory);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            let name = entry.expect("entry").file_name().to_string_lossy().to_string();
            if let Some(number) = name.split('-').next().and_then(|n| n.parse::<u32>().ok()) {
                if !name.ends_with("-obsoleted.md") {
                    numbers.insert((level.directory.to_string(), number));
                }
            }
        }
    }
    numbers
}

#[test]
fn the_policy_corpus_is_well_formed() {
    let root = repo_root();
    let mut failures = Vec::new();
    for level in &LEVELS {
        let dir = root.join("policy").join(level.directory);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            let name = path.file_name().expect("name").to_string_lossy().to_string();
            let number = match name.split('-').next().and_then(|n| n.parse::<u32>().ok()) {
                Some(n) if name.ends_with(".md") && name.split('-').next().expect("head").len() == 4 => n,
                _ => {
                    failures.push(format!("{}: ファイル名が NNNN-title.md でない", display(&path)));
                    continue;
                }
            };
            if !level.band.contains(&number) {
                failures.push(format!(
                    "{}: 番号 {number} が {} の帯({:?})にない",
                    display(&path),
                    level.directory,
                    level.band
                ));
            }
            let text = std::fs::read_to_string(&path).expect("read policy");
            if name.ends_with("-obsoleted.md") {
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                if first.trim() != "This policy is obsoleted. Remove reference to this policy." {
                    failures.push(format!("{}: 廃止スタブの1行目が規定文でない", display(&path)));
                }
                continue;
            }
            let expected_heading = format!("# {number:04} ");
            if !text.starts_with(&expected_heading) {
                failures.push(format!(
                    "{}: H1 が「{expected_heading}…」で始まらない",
                    display(&path)
                ));
            }
            let level_line = text.lines().find(|l| l.starts_with("Level:"));
            match level_line {
                None => failures.push(format!("{}: Level: 行がない", display(&path))),
                Some(line) => {
                    let keyword = line.trim_start_matches("Level:").trim();
                    if !level.keywords.contains(&keyword) {
                        failures.push(format!(
                            "{}: Level 「{keyword}」は {} 段に置けない",
                            display(&path),
                            level.directory
                        ));
                    }
                }
            }
            for required in ["Scope:", "Rationale", "Enforcement"] {
                if !text.lines().any(|l| l.starts_with(required)) {
                    failures.push(format!("{}: {required} 節がない", display(&path)));
                }
            }
        }
    }
    assert!(failures.is_empty(), "policy corpus:\n{}", failures.join("\n"));
}

/// policy/ の外(SPEC・docs・ソース)からのポリシー参照はすべて実在の番号を指す。
#[test]
fn policy_references_outside_the_corpus_resolve() {
    let root = repo_root();
    let live = live_policy_numbers();
    let mut failures = Vec::new();
    let mut sources: Vec<PathBuf> = vec![root.join("README.md"), root.join("SPEC.md")];
    markdown_files_under(&root.join("docs"), &mut sources);
    for sub in ["node/src", "node/tests", "sim/src"] {
        let dir = root.join(sub);
        for entry in std::fs::read_dir(&dir).expect("src dir") {
            let path = entry.expect("entry").path();
            if path.extension().map(|e| e == "rs").unwrap_or(false) {
                sources.push(path);
            }
        }
    }
    for path in sources {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for level in &LEVELS {
            let marker = format!("{}/", level.directory);
            for (position, _) in text.match_indices(&marker) {
                let rest = &text[position + marker.len()..];
                let digits: String =
                    rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.len() != 4 {
                    continue; // ポリシー番号の形をしていない(ふつうのパス等)
                }
                let number: u32 = digits.parse().expect("digits");
                if !level.band.contains(&number) {
                    continue; // 番号帯の外はポリシー参照ではない
                }
                if !live.contains(&(level.directory.to_string(), number)) {
                    failures.push(format!(
                        "{}: {}{digits} への参照が実在のポリシーに解決しない",
                        display(&path),
                        marker
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "policy references:\n{}", failures.join("\n"));
}

// ---- markdown 強調禁止(must/0021) ----

/// コードフェンスとバッククォート区間を除いた行の断片を返す。
fn prose_fragments(text: &str) -> Vec<(usize, String)> {
    let mut fragments = Vec::new();
    let mut in_fence = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // バッククォート区間(奇数番目の区画)を落とす。
        let mut kept = String::new();
        for (segment_index, segment) in line.split('`').enumerate() {
            if segment_index % 2 == 0 {
                kept.push_str(segment);
                kept.push(' ');
            }
        }
        fragments.push((index + 1, kept));
    }
    fragments
}

#[test]
fn tracked_markdown_uses_no_bold_emphasis() {
    let mut failures = Vec::new();
    for path in tracked_markdown() {
        let text = std::fs::read_to_string(&path).expect("read markdown");
        for (line_number, fragment) in prose_fragments(&text) {
            // 太字マーカーの対(** と __)。単一マーカーの斜体は誤検知が多く
            // レビュー領域に残す(検査本文のコメント参照)。
            if fragment.matches("**").count() >= 2 || fragment.matches("__").count() >= 2 {
                failures.push(format!(
                    "{}:{line_number}: 強調記法(must/0021): {}",
                    display(&path),
                    fragment.trim()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "emphasis:\n{}", failures.join("\n"));
}

// ---- docs/ の UUID アンカー(must/0013) ----

#[test]
fn docs_anchors_are_defined_unique_and_resolvable() {
    let root = repo_root();
    let mut failures = Vec::new();
    let mut defined: BTreeMap<String, String> = BTreeMap::new();

    let mut docs_files = Vec::new();
    markdown_files_under(&root.join("docs"), &mut docs_files);

    // 定義: docs/ の各文書は H1 の直後に <a id="uuid"></a> を持つ。加えて、参照される
    // 節見出しの直下にも節アンカーを置ける(must/0013)。定義表は両方を集める。
    for path in &docs_files {
        let text = std::fs::read_to_string(path).expect("read doc");
        let mut head_anchor_found = false;
        for (line_index, line) in text.lines().enumerate() {
            let Some(id) =
                line.trim().strip_prefix("<a id=\"").and_then(|rest| rest.split('"').next())
            else {
                continue;
            };
            if line_index < 5 {
                head_anchor_found = true;
            }
            if let Some(previous) = defined.insert(id.to_string(), display(path)) {
                failures.push(format!(
                    "アンカー {id} が {previous} と {} で重複",
                    display(path)
                ));
            }
        }
        if !head_anchor_found {
            failures.push(format!(
                "{}: H1 直下に <a id=\"uuid\"></a> がない(must/0013)",
                display(path)
            ));
        }
    }

    // 参照: #uuid 形式のリンクは定義済みアンカーに解決し、文書名を名乗る字面はその
    // アンカーを定義した文書と一致する。docs/ 配下 .md へのパスリンクは禁止(アンカーで
    // 参照する)。
    let uuid_like = |s: &str| {
        s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    };
    for path in tracked_markdown() {
        // policy/ 本体は lamalium からの逐語コピーで、lamalium 側 docs のアンカーを
        // 引用している(policy/README.md の Provenance が先例として記録)。参照解決の
        // 検査対象は自前の文書だけとする。README は自前なので対象。
        let under_policy = path.strip_prefix(root.join("policy")).is_ok();
        if under_policy && path.file_name().map(|n| n != "README.md").unwrap_or(true) {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read markdown");
        for (position, _) in text.match_indices("](") {
            let rest = &text[position + 2..];
            let target = &rest[..rest.find(')').unwrap_or(0)];
            if let Some(fragment) = target.strip_prefix('#') {
                if !uuid_like(fragment) {
                    continue;
                }
                let Some(home) = defined.get(fragment) else {
                    failures.push(format!(
                        "{}: アンカー参照 #{fragment} が未定義",
                        display(&path)
                    ));
                    continue;
                };
                // リンクの字面が文書名を名乗るなら、その文書がアンカーの住み処である
                // こと。字面と行き先が食い違うと、読み手は開くまで気づけない。
                let label = text[..position].rsplit('[').next().unwrap_or("");
                if label.ends_with(".md") && !home.ends_with(label) {
                    failures.push(format!(
                        "{}: リンクの字面 {label} と、アンカー #{fragment} の住み処 {home} が食い違う",
                        display(&path)
                    ));
                }
            } else if target.ends_with(".md")
                && (target.starts_with("docs/") || target.contains("/docs/")
                    || (path.starts_with(root.join("docs"))
                        && !target.starts_with("../../")))
            {
                failures.push(format!(
                    "{}: docs/ 配下へのパスリンク {target} (アンカーで参照する。must/0013)",
                    display(&path)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "docs anchors:\n{}", failures.join("\n"));
}

// ---- 用語の裁定: lock は「ロック」(CLAUDE.md) ----

/// 用語検査の対象: 追跡している散文に、ソース(node/・sim/)と systemd の unit を足す。
fn tracked_prose_and_source() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = tracked_markdown();
    for dir in ["node/src", "node/tests", "sim/src", "sim/tests", "docs/mop/systemd"] {
        let mut stack = vec![root.join(dir)];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("rs" | "md" | "html" | "service" | "timer")
                ) {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// プログラム上の lock は「ロック」と書く。U+9320 の字は利用者の裁定で使わない(CLAUDE.md の
/// 用語の裁定)。禁じる字そのものをこのファイルに書くと自分が引っかかるので、符号位置で持つ。
#[test]
fn locks_are_called_rokku_not_jou() {
    let forbidden = char::from_u32(0x9320).expect("禁じる字の符号位置");
    let mut failures = Vec::new();
    for path in tracked_prose_and_source() {
        let text = std::fs::read_to_string(&path).expect("read tracked file");
        for (index, line) in text.lines().enumerate() {
            if line.contains(forbidden) {
                failures.push(format!(
                    "{}:{}: lock は「ロック」と書く(CLAUDE.md の用語の裁定): {}",
                    display(&path),
                    index + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "terminology:\n{}", failures.join("\n"));
}

/// docs/mop/SYSTEMD.md の「旧い名の unit から移る」が操作者に貼らせる外す行は、install の断りが
/// 添える命令(`legacy_units_removal_command`)と同じ字句である(must/0023)。片方だけを直すと、
/// 文書の命令と install の文言が食い違う。
#[test]
fn systemd_doc_carries_the_legacy_units_removal_command() {
    use uniqnode::install::{legacy_units_removal_command, Account, Scope, LEGACY_UNITS, SYSTEM_UNIT_DIR};
    let scope = Scope::System(Account {
        name: "hikalium".to_string(),
        uid: 1000,
        gid: 1000,
        group: "hikalium".to_string(),
        home: PathBuf::from("/home/hikalium"),
    });
    let command = legacy_units_removal_command(&scope, Path::new(SYSTEM_UNIT_DIR), &LEGACY_UNITS);
    let path = repo_root().join("docs/mop/SYSTEMD.md");
    let text = std::fs::read_to_string(&path).expect("read SYSTEMD.md");
    assert!(
        text.lines().any(|line| line.trim() == command),
        "{} に install の外す命令と同じ字句の行が無い:\n{command}",
        display(&path)
    );
}
