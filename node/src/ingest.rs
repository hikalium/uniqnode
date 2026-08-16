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

// ---- 取り込み口(INGEST の「取り込み口の段」) ----

/// 対象拡張子と media の対応。CLI と API が同じ判定を共用する(should/0135)。
/// PDF は「PDF の段」で加わる。
pub fn media_for_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "md" | "markdown" => Some("markdown"),
        "txt" => Some("text"),
        _ => None,
    }
}

pub fn chunk_for_media(media: &str, text: &str) -> Vec<Chunk> {
    if media == "markdown" {
        chunk_markdown(text)
    } else {
        chunk_plain_text(text)
    }
}

// ---- 書き込み経路(INGEST の「書き込み経路の段」) ----

use crate::c1::{self, Value};
use crate::store::{Result, Store};
use std::collections::BTreeMap;

/// 取り込む文書 1 件の入力。name は取り込み起点からの相対パスから拡張子を除いたもの。
pub struct DocumentInput<'a> {
    pub collection: &'a str,
    pub name: &'a str,
    /// 原文そのもの(blob として保存される)。
    pub source: &'a [u8],
    /// "markdown" | "text" | "pdf"
    pub media: &'a str,
    pub chunks: &'a [Chunk],
    /// 抽出器の名前と版(PDF のとき。例 "pdftotext 22.02")。
    pub extractor: Option<&'a str>,
}

pub struct IngestOutcome {
    pub doc_rev_id: String,
    /// 新規に書かれたオブジェクト数(再取り込みでは 0)。
    pub new_objects: usize,
    /// ref を張り替えたか。false = 完全な no-op。
    pub ref_updated: bool,
}

fn text_value(text: &str) -> Value {
    Value::Text(text.to_string())
}

fn chunk_value(chunk: &Chunk) -> Value {
    let mut object = BTreeMap::new();
    object.insert("v".to_string(), Value::Integer(1));
    object.insert("kind".to_string(), text_value("chunk"));
    object.insert("text".to_string(), text_value(&chunk.text));
    let mut meta = BTreeMap::new();
    if !chunk.breadcrumbs.is_empty() {
        meta.insert(
            "breadcrumbs".to_string(),
            Value::Array(chunk.breadcrumbs.iter().map(|b| text_value(b)).collect()),
        );
    }
    if let Some(page) = chunk.page {
        meta.insert("page".to_string(), Value::Integer(i64::from(page)));
    }
    if !meta.is_empty() {
        object.insert("meta".to_string(), Value::Object(meta));
    }
    Value::Object(object)
}

/// 現行 doc_rev と同じ内容かを (source, chunks 列) で判定する(previous を含む ID の
/// 比較では、同じ入力でも別 ID になり no-op が壊れるため)。
fn same_revision(current: &Value, blob_id: &str, chunk_ids: &[String]) -> bool {
    let Value::Object(map) = current else { return false };
    if map.get("source") != Some(&text_value(blob_id)) {
        return false;
    }
    match map.get("chunks") {
        Some(Value::Array(items)) => {
            items.len() == chunk_ids.len()
                && items.iter().zip(chunk_ids).all(|(item, id)| item == &text_value(id))
        }
        _ => false,
    }
}

/// blob + chunk 群 + doc_rev + ref を一括で書く。順序は blob と chunk 群 → doc_rev →
/// ref(set_ref は存在しない target を拒否するため ref は最後)。同一内容の再取り込みは
/// 新しいオブジェクトを書かず ref も触らない(べき等 = I1 の検証)。
pub fn ingest_document(store: &mut Store, input: &DocumentInput) -> Result<IngestOutcome> {
    let mut new_objects = 0usize;
    let (blob_id, blob_new) = store.put_object(input.source)?;
    new_objects += usize::from(blob_new);

    let mut chunk_ids = Vec::with_capacity(input.chunks.len());
    for chunk in input.chunks {
        let bytes = c1::to_canonical_bytes(&chunk_value(chunk));
        let (id, is_new) = store.put_object(&bytes)?;
        new_objects += usize::from(is_new);
        chunk_ids.push(id);
    }

    let ref_path = format!("collections/{}/{}", input.collection, input.name);
    let full_name = store.own_ref_name(&ref_path);
    let current_target = store.get_ref(&full_name).and_then(|state| state.target.clone());
    if let Some(current_id) = &current_target {
        if let Some(bytes) = store.get_object(current_id)? {
            if let Ok(text) = String::from_utf8(bytes) {
                if let Ok(value) = c1::parse(&text) {
                    if same_revision(&value, &blob_id, &chunk_ids) {
                        return Ok(IngestOutcome {
                            doc_rev_id: current_id.clone(),
                            new_objects,
                            ref_updated: false,
                        });
                    }
                }
            }
        }
    }

    let mut object = BTreeMap::new();
    object.insert("v".to_string(), Value::Integer(1));
    object.insert("kind".to_string(), text_value("doc_rev"));
    object.insert("source".to_string(), text_value(&blob_id));
    object.insert(
        "chunks".to_string(),
        Value::Array(chunk_ids.iter().map(|id| text_value(id)).collect()),
    );
    let mut meta = BTreeMap::new();
    meta.insert("name".to_string(), text_value(input.name));
    meta.insert("media".to_string(), text_value(input.media));
    if let Some(extractor) = input.extractor {
        meta.insert("extractor".to_string(), text_value(extractor));
    }
    object.insert("meta".to_string(), Value::Object(meta));
    if let Some(previous) = &current_target {
        object.insert("previous".to_string(), text_value(previous));
    }
    let bytes = c1::to_canonical_bytes(&Value::Object(object));
    let (doc_rev_id, is_new) = store.put_object(&bytes)?;
    new_objects += usize::from(is_new);
    store.set_ref(&ref_path, Some(&doc_rev_id))?;
    Ok(IngestOutcome { doc_rev_id, new_objects, ref_updated: true })
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

    // ---- 書き込み経路 ----

    use crate::store::StoreConfig;
    use std::path::PathBuf;

    fn temp_store(name: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-ingest-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let store = Store::open(StoreConfig::new(&dir)).expect("open store");
        (dir, store)
    }

    fn ingest_markdown(store: &mut Store, name: &str, text: &str) -> IngestOutcome {
        let chunks = chunk_markdown(text);
        ingest_document(
            store,
            &DocumentInput {
                collection: "notes",
                name,
                source: text.as_bytes(),
                media: "markdown",
                chunks: &chunks,
                extractor: None,
            },
        )
        .expect("ingest")
    }

    /// 段 2 の合格条件: 同一内容の再取り込みで ID 集合と ref の seq が変わらない。
    #[test]
    fn reingesting_the_same_document_is_a_complete_noop() {
        let (dir, mut store) = temp_store("noop");
        let text = "# 章\n\n本文。\n";
        let first = ingest_markdown(&mut store, "memo", text);
        assert!(first.ref_updated);
        assert!(first.new_objects > 0);
        let objects_after_first = store.object_count();
        let full_name = store.own_ref_name("collections/notes/memo");
        let seq_after_first = store.get_ref(&full_name).expect("ref").seq;

        let second = ingest_markdown(&mut store, "memo", text);
        assert!(!second.ref_updated, "再取り込みで ref を張り替えてはならない");
        assert_eq!(second.new_objects, 0, "再取り込みで新オブジェクトを書いてはならない");
        assert_eq!(second.doc_rev_id, first.doc_rev_id);
        assert_eq!(store.object_count(), objects_after_first);
        assert_eq!(store.get_ref(&full_name).expect("ref").seq, seq_after_first);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 内容が変わったら新しい doc_rev が前版への previous を持ち、ref が進む。
    #[test]
    fn a_changed_document_creates_a_new_revision_with_previous() {
        let (dir, mut store) = temp_store("revision");
        let first = ingest_markdown(&mut store, "memo", "# 章\n\n初版。\n");
        let second = ingest_markdown(&mut store, "memo", "# 章\n\n改訂版。\n");
        assert!(second.ref_updated);
        assert_ne!(second.doc_rev_id, first.doc_rev_id);

        let bytes = store.get_object(&second.doc_rev_id).expect("get").expect("present");
        let value = c1::parse(&String::from_utf8(bytes).expect("utf-8")).expect("c1");
        let Value::Object(map) = value else { panic!("doc_rev はオブジェクト") };
        assert_eq!(map.get("previous"), Some(&text_value(&first.doc_rev_id)));

        let full_name = store.own_ref_name("collections/notes/memo");
        let state = store.get_ref(&full_name).expect("ref");
        assert_eq!(state.target.as_deref(), Some(second.doc_rev_id.as_str()));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 引用が doc_rev 経由で組める: ref から doc_rev、chunks の添字と辿り、
    /// 文書名(ref パス)・位置(添字)・見出し(meta.breadcrumbs)の三つが揃う。
    #[test]
    fn a_citation_is_resolvable_through_the_doc_rev() {
        let (dir, mut store) = temp_store("citation");
        let outcome =
            ingest_markdown(&mut store, "memo", "# 甲\n\n## 乙\n\n引用される本文。\n");
        let bytes = store.get_object(&outcome.doc_rev_id).expect("get").expect("present");
        let value = c1::parse(&String::from_utf8(bytes).expect("utf-8")).expect("c1");
        let Value::Object(map) = value else { panic!("doc_rev はオブジェクト") };
        let Some(Value::Array(chunk_ids)) = map.get("chunks") else { panic!("chunks 列") };
        assert_eq!(chunk_ids.len(), 1);
        let Value::Text(chunk_id) = &chunk_ids[0] else { panic!("chunk id は文字列") };

        let bytes = store.get_object(chunk_id).expect("get").expect("present");
        let value = c1::parse(&String::from_utf8(bytes).expect("utf-8")).expect("c1");
        let Value::Object(chunk) = value else { panic!("chunk はオブジェクト") };
        assert_eq!(chunk.get("text"), Some(&text_value("引用される本文。")));
        let Some(Value::Object(meta)) = chunk.get("meta") else { panic!("meta") };
        assert_eq!(
            meta.get("breadcrumbs"),
            Some(&Value::Array(vec![text_value("甲"), text_value("乙")]))
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
