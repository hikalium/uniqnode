//! 取り込み層のチャンカー(純関数層)。INGEST (uuid:11ff6fec-cf85-4ae9-a24c-6098964f6cce) の
//! チャンカーの段。文書をチャンク(検索と引用の単位)に切る。ストア I/O は扱わない。

/// チャンクの大きさの上限(近似トークン数)。検索スニペットとして一度に読める長さ。
pub const CHUNK_TOKEN_LIMIT: usize = 480;

/// 近似の最小単位は 1/4 トークン。整数演算で扱うため 4 倍した値で数える。
const LIMIT_QUARTERS: usize = CHUNK_TOKEN_LIMIT * 4;

/// 文字ごとの重み(1/4 トークン単位): ASCII は 1、非 ASCII は 4。
fn quarters_of(character: char) -> usize {
    if character.is_ascii() { 1 } else { 4 }
}

fn token_quarters(text: &str) -> usize {
    text.chars().map(quarters_of).sum()
}

/// 近似トークン数(切り上げ)。チャンカーとテストが共用する唯一の見積もり(should/0135)。
pub fn token_estimate(text: &str) -> usize {
    token_quarters(text).div_ceil(4)
}

/// 検索と引用の単位。text が本文、breadcrumbs が見出しの入れ子パス、page は
/// PDF のときだけ物理ページ番号(1 始まり)を持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub text: String,
    pub breadcrumbs: Vec<String>,
    pub page: Option<u32>,
}

/// 段落列を上限まで詰めてチャンクにする。単一の段落が上限を超えるときだけ
/// 文字境界で分割する。
fn pack_paragraphs(
    paragraphs: &[String],
    breadcrumbs: &[String],
    page: Option<u32>,
    out: &mut Vec<Chunk>,
) {
    let mut current = String::new();
    let mut current_quarters = 0usize;
    let separator_quarters = token_quarters("\n\n");
    let flush = |text: &mut String, quarters: &mut usize, out: &mut Vec<Chunk>| {
        if !text.is_empty() {
            out.push(Chunk {
                text: std::mem::take(text),
                breadcrumbs: breadcrumbs.to_vec(),
                page,
            });
            *quarters = 0;
        }
    };
    for paragraph in paragraphs {
        for piece in split_oversized(paragraph) {
            let piece_quarters = token_quarters(piece);
            let added = if current.is_empty() {
                piece_quarters
            } else {
                separator_quarters + piece_quarters
            };
            if !current.is_empty() && current_quarters + added > LIMIT_QUARTERS {
                flush(&mut current, &mut current_quarters, out);
            }
            if !current.is_empty() {
                current.push_str("\n\n");
                current_quarters += separator_quarters;
            }
            current.push_str(piece);
            current_quarters += piece_quarters;
        }
    }
    flush(&mut current, &mut current_quarters, out);
}

/// 上限を超える段落を、上限以下の断片に文字境界で貪欲に分ける。
/// 上限以下の段落はそのまま 1 断片で返す。
fn split_oversized(paragraph: &str) -> Vec<&str> {
    if token_quarters(paragraph) <= LIMIT_QUARTERS {
        return vec![paragraph];
    }
    let mut pieces = Vec::new();
    let mut start = 0usize;
    let mut quarters = 0usize;
    for (offset, character) in paragraph.char_indices() {
        let weight = quarters_of(character);
        if quarters + weight > LIMIT_QUARTERS {
            pieces.push(&paragraph[start..offset]);
            start = offset;
            quarters = 0;
        }
        quarters += weight;
    }
    if start < paragraph.len() {
        pieces.push(&paragraph[start..]);
    }
    pieces
}

/// 行の並びを空行区切りの段落列にまとめる。コードフェンス(```)の内側では
/// 空行でも段落を切らず、フェンスの行も本文の一部として保つ。
fn paragraphs_of(lines: &[&str]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            current.push(line);
            continue;
        }
        if !in_fence && line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join("\n"));
    }
    paragraphs
}

/// 見出し行なら (レベル, 表題) を返す。フェンス外でのみ呼ぶこと。
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let sharps = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&sharps) {
        if let Some(rest) = line[sharps..].strip_prefix(' ') {
            return Some((sharps, rest.trim()));
        }
    }
    None
}

/// Markdown を切る。見出し境界を優先し(見出しをまたぐチャンクは作らない)、
/// 見出しの入れ子パスを各チャンクの breadcrumbs に写す。
pub fn chunk_markdown(text: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut breadcrumbs: Vec<String> = Vec::new();
    let mut section_lines: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let flush_section = |lines: &mut Vec<&str>, breadcrumbs: &[String], out: &mut Vec<Chunk>| {
        let paragraphs = paragraphs_of(lines);
        pack_paragraphs(&paragraphs, breadcrumbs, None, out);
        lines.clear();
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            section_lines.push(line);
            continue;
        }
        if !in_fence {
            if let Some((level, title)) = heading_of(line) {
                flush_section(&mut section_lines, &breadcrumbs, &mut chunks);
                breadcrumbs.truncate(level - 1);
                breadcrumbs.push(title.to_string());
                continue;
            }
        }
        section_lines.push(line);
    }
    flush_section(&mut section_lines, &breadcrumbs, &mut chunks);
    chunks
}

/// プレーンテキストを切る。空行区切りの段落を上限まで詰める。
pub fn chunk_plain_text(text: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let paragraphs = paragraphs_of(&lines);
    let mut chunks = Vec::new();
    pack_paragraphs(&paragraphs, &[], None, &mut chunks);
    chunks
}

/// pdftotext の出力(form feed 区切り)を切る。ページをまたぐチャンクは作らず、
/// 各チャンクに物理ページ番号(1 始まり)を付す。form feed は最終ページの後ろにも
/// 付くので、末尾の空区画は捨てる(途中の空ページは番号だけ進める)。
pub fn chunk_pdf_text(text: &str) -> Vec<Chunk> {
    let sections: Vec<&str> = text.split('\u{c}').collect();
    let last_nonempty = match sections.iter().rposition(|s| !s.trim().is_empty()) {
        Some(index) => index,
        None => return Vec::new(),
    };
    let mut chunks = Vec::new();
    for (index, section) in sections.iter().enumerate().take(last_nonempty + 1) {
        let lines: Vec<&str> = section.lines().collect();
        let paragraphs = paragraphs_of(&lines);
        let page = u32::try_from(index + 1).expect("ページ数が u32 を超えることはない");
        pack_paragraphs(&paragraphs, &[], Some(page), &mut chunks);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// token_estimate の期待値はリテラルで書く(検査対象から導出しない。should/0137)。
    #[test]
    fn token_estimate_uses_quarter_weights() {
        assert_eq!(token_estimate(""), 0);
        assert_eq!(token_estimate("abc"), 1); // 3/4 → 切り上げ 1
        assert_eq!(token_estimate("abcd"), 1);
        assert_eq!(token_estimate("abcde"), 2);
        assert_eq!(token_estimate("日本語"), 3); // 非 ASCII は 1 文字 1 トークン
        assert_eq!(token_estimate("a日"), 2); // 1/4 + 1 → 切り上げ 2
    }

    #[test]
    fn markdown_breadcrumbs_follow_heading_nesting() {
        let text = "# 甲\n\n序文。\n\n## 乙\n\n本文一。\n\n### 丙\n\n本文二。\n\n## 丁\n\n本文三。\n";
        let chunks = chunk_markdown(text);
        let paths: Vec<(Vec<String>, String)> =
            chunks.into_iter().map(|c| (c.breadcrumbs, c.text)).collect();
        assert_eq!(
            paths,
            vec![
                (vec!["甲".to_string()], "序文。".to_string()),
                (vec!["甲".to_string(), "乙".to_string()], "本文一。".to_string()),
                (
                    vec!["甲".to_string(), "乙".to_string(), "丙".to_string()],
                    "本文二。".to_string()
                ),
                (vec!["甲".to_string(), "丁".to_string()], "本文三。".to_string()),
            ]
        );
    }

    #[test]
    fn paragraphs_pack_until_the_limit_and_never_cross_headings() {
        // 各段落 400 文字(100 トークン)。4 つまでは 1 チャンクに入るが、
        // 5 つ目は区切りの分を超えるので次のチャンクへ。
        let paragraph = "a".repeat(400);
        let five = vec![paragraph.clone(); 5].join("\n\n");
        let chunks = chunk_plain_text(&five);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text.matches(&paragraph).count(), 4);
        assert_eq!(chunks[1].text, paragraph);
        for chunk in &chunks {
            assert!(token_estimate(&chunk.text) <= CHUNK_TOKEN_LIMIT);
        }

        // 同じ 2 段落でも、間に見出しが入ればチャンクは分かれる。
        let text = format!("{paragraph}\n\n# 区切り\n\n{paragraph}");
        let chunks = chunk_markdown(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].breadcrumbs, Vec::<String>::new());
        assert_eq!(chunks[1].breadcrumbs, vec!["区切り".to_string()]);
    }

    #[test]
    fn an_oversized_paragraph_is_split_at_character_boundaries() {
        let text = "a".repeat(2000); // 500 トークン > 上限 480
        let chunks = chunk_plain_text(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text.len(), 1920);
        assert_eq!(chunks[1].text.len(), 80);
        let rejoined: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(rejoined, text);
        // 非 ASCII でも文字境界で切れる(バイト境界で壊れない)。
        let text = "あ".repeat(500); // 500 トークン
        let chunks = chunk_plain_text(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text.chars().count(), 480);
        assert_eq!(chunks[1].text.chars().count(), 20);
    }

    #[test]
    fn fenced_code_is_one_paragraph_and_hides_heading_markers() {
        let text = "# 章\n\n```\n# これは見出しではない\n\ncode line\n```\n\n本文。\n";
        let chunks = chunk_markdown(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].breadcrumbs, vec!["章".to_string()]);
        assert!(chunks[0].text.contains("# これは見出しではない"));
        assert!(chunks[0].text.contains("code line"));
        // フェンス内の空行で段落が割れていない(フェンス全体が 1 段落)。
        assert_eq!(chunks[0].text.matches("```").count(), 2);
    }

    #[test]
    fn pdf_pages_keep_numbers_and_drop_the_trailing_empty_section() {
        // 1 ページ目に本文、2 ページ目は空、3 ページ目に本文、末尾に form feed。
        let text = "page one text\u{c}\u{c}page three text\u{c}";
        let chunks = chunk_pdf_text(text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].page, Some(1));
        assert_eq!(chunks[0].text, "page one text");
        assert_eq!(chunks[1].page, Some(3));
        assert_eq!(chunks[1].text, "page three text");
        // ページをまたぐチャンクはない(page が全チャンクで単一値)。
        assert!(chunks.iter().all(|c| c.page.is_some()));
        // 全体が空なら何も出ない。
        assert_eq!(chunk_pdf_text("\u{c}\u{c}"), Vec::new());
    }

    #[test]
    fn chunking_is_deterministic_and_empty_input_yields_nothing() {
        let text = "# 章\n\n本文。\n\n## 節\n\n続き。\n";
        assert_eq!(chunk_markdown(text), chunk_markdown(text));
        assert_eq!(chunk_markdown(""), Vec::new());
        assert_eq!(chunk_plain_text(""), Vec::new());
    }
}
