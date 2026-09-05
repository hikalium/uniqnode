//! 取り込み層: チャンカー・書き込み経路・pdftotext 委譲・注釈の取り込みと照合・訂正の
//! 発行(INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b))。

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
    // 見出しは階数つきで積む。階数を持たずに深さだけで切ると、階を飛ばした紙面
    // (h1 の次が h3 で、その h3 が何本も並ぶ形。HTML から来る紙面によくある)で
    // 兄弟が親子に見えてしまう。
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut section_lines: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let flush_section = |lines: &mut Vec<&str>, stack: &[(usize, String)], out: &mut Vec<Chunk>| {
        let breadcrumbs: Vec<String> =
            stack.iter().map(|(_, title)| title.clone()).collect();
        let paragraphs = paragraphs_of(lines);
        pack_paragraphs(&paragraphs, &breadcrumbs, None, out);
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
                flush_section(&mut section_lines, &stack, &mut chunks);
                while stack.last().is_some_and(|(open, _)| *open >= level) {
                    stack.pop();
                }
                stack.push((level, title.to_string()));
                continue;
            }
        }
        section_lines.push(line);
    }
    flush_section(&mut section_lines, &stack, &mut chunks);
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

// ---- 取り込み口(INGEST の「CLI と API」節) ----

/// 対象拡張子と media の対応。CLI と API が同じ判定を共用する(should/0135)。
pub fn media_for_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "md" | "markdown" => Some("markdown"),
        "txt" => Some("text"),
        "pdf" => Some("pdf"),
        "html" | "htm" => Some("html"),
        _ => None,
    }
}

/// media に応じたチャンカー。PDF の text は pdftotext の抽出テキスト
/// (form feed 区切り)であって PDF バイナリではない。HTML は原本そのもので、素文への
/// 変換(node/src/html.rs)はここで一度だけ行う(should/0135)。
pub fn chunk_for_media(media: &str, text: &str) -> Vec<Chunk> {
    match media {
        "markdown" => chunk_markdown(text),
        "html" => chunk_markdown(&crate::html::to_text(text)),
        "pdf" => chunk_pdf_text(text),
        _ => chunk_plain_text(text),
    }
}

/// 原本のバイト列も見て切る形。PDF のときだけ、節見出しの経路をチャンクの
/// meta.breadcrumbs に載せる(node/src/outline.rs)。
///
/// なぜ要るか: PDF のチャンクは見出しを持たず、索引語は本文だけから出ていた。Markdown の
/// チャンクは見出しを持ち、その語は本文の 2 倍で数えられる(BREADCRUMB_WEIGHT)のに、
/// 規格書だけが節の構造を捨てていた。実測では、同じ語が出る 2 つの紙面 —「4.20
/// Scratchpad Buffers」という節そのものと、レジスタ欄でその語が 1 回出るだけの紙面 —
/// を区別する情報が、見出しにしか無かった。
///
/// 見出しが取れないときは黙って諦める(breadcrumbs 無しの、これまでと同じチャンク)。
/// 取れなかった理由は呼び手が受け取り、記録に残す。
pub fn chunk_for_media_with_source(
    media: &str,
    text: &str,
    source: &[u8],
) -> (Vec<Chunk>, Option<String>) {
    let mut chunks = chunk_for_media(media, text);
    if media != "pdf" {
        return (chunks, None);
    }
    // ページ数は抽出した本文の改頁の数だけ確かに分かる(しおりは自分でページ数を
    // 知らないので、最後のしおり以降のページに経路が付かない)。
    let pages = chunks.iter().filter_map(|chunk| chunk.page).max();
    match crate::outline::outline_of_pdf(source, pages) {
        Ok(outline) => {
            for chunk in chunks.iter_mut() {
                if let Some(page) = chunk.page {
                    let path = outline.path_for_page(page);
                    if !path.is_empty() {
                        chunk.breadcrumbs = path.to_vec();
                    }
                }
            }
            (chunks, Some(outline.reason))
        }
        // 道具が無いだけで取り込みを止めない(見出しは索引を良くするものであって、
        // 文書の本文ではない)。理由は呼び手が記録する。
        Err(reason) => (chunks, Some(reason)),
    }
}

// ---- PDF 抽出(INGEST の「PDF 抽出」節) ----

use std::path::{Path, PathBuf};
use std::process::Command;

/// pdftotext が見つからないときに示す導入手順(sudo なし)。
const PDFTOTEXT_INSTALL_HINT: &str =
    "導入例(sudo なし): apt-get download poppler-utils と dpkg -x で ~/opt/poppler/ へ展開し、\
     --pdftotext で実行ファイルを指すか PATH の通ったディレクトリへ symlink を置く。\
     libpoppler の無い機械ではライブラリ側も同じ手順で展開して LD_LIBRARY_PATH を通す";

/// pdftotext(poppler)への外部プロセス委譲。シェルを経由せず Command で直接起動する
/// (must/0009 と同じ理由)。版は locate が起動確認を兼ねて一度だけ取得し、
/// 以後の抽出で使い回す。
pub struct PdfExtractor {
    command: PathBuf,
    /// doc_rev.meta.extractor に書く文字列(例 "pdftotext 22.02.0")。
    pub extractor: String,
}

impl PdfExtractor {
    /// 実行ファイルを見つけて版を確かめる。explicit(--pdftotext)の明示指定を優先し、
    /// 無指定なら PATH を引く。見つからないときは導入手順を示して明示的に失敗する。
    /// 黙って PDF を飛ばさない(must/0022 の同型)。
    pub fn locate(explicit: Option<&Path>) -> std::result::Result<PdfExtractor, String> {
        let command = match explicit {
            Some(path) => path.to_path_buf(),
            None => PathBuf::from("pdftotext"),
        };
        let probe = Command::new(&command).arg("-v").output().map_err(|error| {
            format!(
                "PDF の取り込みには pdftotext コマンドが必要({}: {error})。{}",
                command.display(),
                PDFTOTEXT_INSTALL_HINT
            )
        })?;
        // 版は -v が stderr に出す先頭行「pdftotext version <版>」から取る。
        let stderr = String::from_utf8_lossy(&probe.stderr);
        let first_line = stderr.lines().next().unwrap_or("");
        let version = first_line
            .strip_prefix("pdftotext version ")
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                format!(
                    "{} -v の出力から版を読めない(先頭行: {first_line:?})",
                    command.display()
                )
            })?;
        Ok(PdfExtractor { command, extractor: format!("pdftotext {version}") })
    }

    /// PDF バイト列からテキストを抽出する(form feed 区切り)。pdftotext は入力に
    /// ファイルパスを要求するため、一時ファイルへ書いてから起動する。
    pub fn extract(&self, pdf: &[u8]) -> Result<String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = std::env::temp_dir()
            .join(format!("uniqnode-pdf-{}-{serial}.pdf", std::process::id()));
        std::fs::write(&temp, pdf)?;
        let output = Command::new(&self.command).arg(&temp).arg("-").output();
        let removed = std::fs::remove_file(&temp);
        let output = output?;
        removed?;
        if !output.status.success() {
            return Err(crate::store::StoreError::Invalid(format!(
                "pdftotext が失敗した({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        String::from_utf8(output.stdout).map_err(|_| {
            crate::store::StoreError::Invalid("pdftotext の出力が UTF-8 でない".to_string())
        })
    }
}

// ---- 書き込み経路(INGEST の「文書モデル」節) ----

use crate::c1::{self, Value};
use crate::store::{Result, Store, StoreError};
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
    /// doc_rev.meta に足す鍵(URL からの取り込みの出所。組むのは node/src/fetch.rs で、
    /// INGEST の「URL からの取り込み」節)。name・media・extractor と同じ鍵は上書き
    /// できない。ファイルからの取り込みは空。
    pub extra_meta: &'a [(String, Value)],
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
    for (key, value) in input.extra_meta {
        meta.entry(key.clone()).or_insert_with(|| value.clone());
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

// ---- 注釈の取り込み(INGEST の「注釈の取り込みと照合」節) ----

use std::collections::BTreeSet;

/// 検証記録の method: 機械照合(must/0023: 生成側と判定側が同じ定数)。
pub const METHOD_TOKEN_MATCH: &str = "token-match";
/// 検証記録の method: 人手確認(--manual の承認リスト経由)。
pub const METHOD_MANUAL: &str = "manual";

/// annotates 型ノードの c1 正規形本文。辺の type はこの本文の ID を指し、生成側
/// (ingest_annotations の辺の発行)と判定側(is_annotates_edge)が同じ定数を使う
/// (must/0023)。
pub const ANNOTATES_TYPE_BODY: &str = "{\"contents\":\"annotates\",\"kind\":\"node\",\"v\":1}";
/// corrects 型ノードの c1 正規形本文。この段では定数定義のみで、発行経路は訂正の段が足す。
pub const CORRECTS_TYPE_BODY: &str = "{\"contents\":\"corrects\",\"kind\":\"node\",\"v\":1}";
/// supersedes 型ノードの c1 正規形本文。この段では定数定義のみ(改版追随は将来の段)。
pub const SUPERSEDES_TYPE_BODY: &str = "{\"contents\":\"supersedes\",\"kind\":\"node\",\"v\":1}";

/// annotates 型ノードの ID。
pub fn annotates_type_id() -> String {
    c1::id_for_bytes(ANNOTATES_TYPE_BODY.as_bytes())
}

/// corrects 型ノードの ID。
pub fn corrects_type_id() -> String {
    c1::id_for_bytes(CORRECTS_TYPE_BODY.as_bytes())
}

/// supersedes 型ノードの ID。
pub fn supersedes_type_id() -> String {
    c1::id_for_bytes(SUPERSEDES_TYPE_BODY.as_bytes())
}

/// 判定側: この c1 値は annotates 型の辺か。生成側と同じ型定数を通る(must/0023)。
pub fn is_annotates_edge(value: &Value) -> bool {
    let Value::Object(map) = value else { return false };
    map.get("kind") == Some(&text_value("edge"))
        && map.get("type") == Some(&text_value(&annotates_type_id()))
}

/// data.md の注釈 1 件(spec_id のページ page に節 title がある、という一次言明の素)。
/// page は PDF の物理ページ番号(1 始まりの通し番号。紙面の刷り番号ではない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationEntry {
    pub spec_id: String,
    pub page: u32,
    pub title: String,
}

/// 「`タイトル`」のようにバッククォートで囲まれていれば外す(裸ならそのまま)。
fn strip_enclosing_backticks(text: &str) -> &str {
    text.strip_prefix('`').and_then(|inner| inner.strip_suffix('`')).unwrap_or(text)
}

/// data.md の実形式(INGEST の「注釈の取り込みと照合」節)だけを受け付けるパーサ(must/0020):
/// spec_id の見出し + 表題・形式・URL の 3 行コードブロック(形式が zip のときだけ
/// 4 行目に書庫内パス)+「- p.N: 節タイトル」の箇条書き(タイトルはバッククォート
/// 囲みと裸の両方)。注釈が 1 件も無い見出しも正常。コードブロックの中身は使わない。
pub fn parse_annotation_index(text: &str) -> std::result::Result<Vec<AnnotationEntry>, String> {
    /// 次に来てよいもの。見出しの後には必ずコードブロックが 1 個来る。
    #[derive(PartialEq)]
    enum Expecting {
        FirstHeading,
        CodeBlock,
        Bullets,
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut entries = Vec::new();
    let mut spec_id = String::new();
    let mut expecting = Expecting::FirstHeading;
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        let number = index + 1;
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            if expecting == Expecting::CodeBlock {
                return Err(format!("{number} 行目: 見出し {spec_id} にコードブロックが無い"));
            }
            spec_id = strip_enclosing_backticks(rest.trim()).to_string();
            if spec_id.is_empty() {
                return Err(format!("{number} 行目: 見出しの spec_id が空"));
            }
            expecting = Expecting::CodeBlock;
            index += 1;
            continue;
        }
        if line == "```" {
            if expecting != Expecting::CodeBlock {
                return Err(format!(
                    "{number} 行目: コードブロックは見出しの直後に 1 個だけ置ける"
                ));
            }
            let mut body_lines = 0usize;
            let mut is_zip = false;
            let mut end = index + 1;
            loop {
                let Some(body) = lines.get(end) else {
                    return Err(format!("{number} 行目: コードブロックが閉じていない"));
                };
                if *body == "```" {
                    break;
                }
                if body_lines == 1 {
                    is_zip = *body == "zip";
                }
                body_lines += 1;
                end += 1;
            }
            let expected = if is_zip { 4 } else { 3 };
            if body_lines != expected {
                return Err(format!(
                    "{number} 行目: コードブロックは表題・形式・URL の 3 行(形式 zip の\
                     ときだけ書庫内パスの 4 行目)のはずが {body_lines} 行ある"
                ));
            }
            expecting = Expecting::Bullets;
            index = end + 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("- p.") {
            if expecting != Expecting::Bullets {
                return Err(format!(
                    "{number} 行目: 注釈の行はコードブロックの後にしか置けない"
                ));
            }
            let Some((digits, raw_title)) = rest.split_once(": ") else {
                return Err(format!(
                    "{number} 行目: 注釈は「- p.N: 節タイトル」の形のはず: {line:?}"
                ));
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{number} 行目: ページ番号が数字でない: {digits:?}"));
            }
            let page: u32 = digits
                .parse()
                .map_err(|_| format!("{number} 行目: ページ番号が大きすぎる: {digits:?}"))?;
            let title = strip_enclosing_backticks(raw_title.trim()).to_string();
            if title.is_empty() {
                return Err(format!("{number} 行目: 節タイトルが空"));
            }
            entries.push(AnnotationEntry { spec_id: spec_id.clone(), page, title });
            index += 1;
            continue;
        }
        return Err(format!(
            "{number} 行目: 形式外の行(見出し・コードブロック・「- p.N: 節タイトル」\
             だけを受け付ける。must/0020): {line:?}"
        ));
    }
    if expecting == Expecting::CodeBlock {
        return Err(format!("見出し {spec_id} にコードブロックが無いまま入力が終わった"));
    }
    Ok(entries)
}

/// --manual の承認リスト(人手確認済みの注釈)を読む。形式は 1 行 1 件で
/// 「spec_id ページ番号」の空白区切り。空行は無視する。この形式だけを受け付ける
/// (must/0020)。
pub fn parse_manual_approvals(
    text: &str,
) -> std::result::Result<BTreeSet<(String, u32)>, String> {
    let mut approvals = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(spec_id), Some(page), None) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!(
                "{number} 行目: 承認は「spec_id ページ番号」の 2 語のはず: {line:?}"
            ));
        };
        if page.is_empty() || !page.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("{number} 行目: ページ番号が数字でない: {page:?}"));
        }
        let page: u32 =
            page.parse().map_err(|_| format!("{number} 行目: ページ番号が大きすぎる: {page:?}"))?;
        approvals.insert((spec_id.to_string(), page));
    }
    Ok(approvals)
}

/// 注釈の照合の結果。evidence は一致に使った本文行(検証記録に残す根拠)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationMatch {
    pub matched: bool,
    pub matched_tokens: usize,
    pub total_tokens: usize,
    pub evidence: Vec<String>,
}

/// 照合で数える「語」: 小文字化して英数字以外で分割し、数字だけの語を除く。同じ語の
/// 繰り返しは 1 語と数える(重複を除く)。チャンカーの token_estimate(近似トークン数)
/// とは別物である(INGEST の「注釈の取り込みと照合」節)。
fn annotation_tokens(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut tokens = Vec::new();
    for raw in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if raw.is_empty() || raw.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let token = raw.to_ascii_lowercase();
        if seen.insert(token.clone()) {
            tokens.push(token);
        }
    }
    tokens
}

/// 注釈の照合(INGEST の「注釈の取り込みと照合」節の照合規則)。タイトル側の語の 6 割以上がページ
/// 本文の語の集合に含まれれば一致。根拠は、一致した各語を最初に含む本文行(ページ順・
/// 重複なし)。取り込み時の検証と訂正のための再検証が共用する(should/0135)。
/// 語が一つも残らないタイトルは空条件として一致になる(実データには存在しない)。
pub fn match_annotation(title: &str, page_lines: &[String]) -> AnnotationMatch {
    let title_tokens = annotation_tokens(title);
    let line_tokens: Vec<BTreeSet<String>> = page_lines
        .iter()
        .map(|line| annotation_tokens(line).into_iter().collect())
        .collect();
    let mut matched = 0usize;
    let mut evidence_lines: BTreeSet<usize> = BTreeSet::new();
    for token in &title_tokens {
        if let Some(index) = line_tokens.iter().position(|set| set.contains(token)) {
            matched += 1;
            evidence_lines.insert(index);
        }
    }
    let total = title_tokens.len();
    AnnotationMatch {
        // 6 割以上(matched / total >= 3/5)を整数演算で判定する。
        matched: matched * 5 >= total * 3,
        matched_tokens: matched,
        total_tokens: total,
        evidence: evidence_lines.into_iter().map(|i| page_lines[i].trim().to_string()).collect(),
    }
}

/// 取り込まれた注釈 1 件(報告と索引の素)。
#[derive(Debug)]
pub struct AcceptedAnnotation {
    pub spec_id: String,
    pub page: u32,
    pub title: String,
    /// METHOD_TOKEN_MATCH か METHOD_MANUAL。
    pub method: &'static str,
    pub matched_tokens: usize,
    pub total_tokens: usize,
    pub edge_id: String,
    pub verification_id: String,
}

/// 取り込まなかった注釈 1 件(どの注釈がどの一致率で落ちたかの報告)。
#[derive(Debug)]
pub struct RejectedAnnotation {
    pub spec_id: String,
    pub page: u32,
    pub title: String,
    pub matched_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug)]
pub struct AnnotationOutcome {
    pub accepted: Vec<AcceptedAnnotation>,
    pub rejected: Vec<RejectedAnnotation>,
    /// コレクションの索引オブジェクトの ID(ref annotations/<コレクション名> の指す先)。
    pub index_id: String,
    /// ref を張り替えたか。false = 索引が前回と同一(no-op)。
    pub ref_updated: bool,
    pub new_objects: usize,
}

/// spec_id 1 本ぶんの解決結果: PDF blob の ID と、必要なページの本文行。
struct ResolvedDocument {
    blob_id: String,
    pages: BTreeMap<u32, Vec<String>>,
}

/// ストア上の c1 オブジェクトを読んで構文解析する。無い・壊れているは黙って飛ばさず
/// 明示的に失敗する(must/0022)。
fn parse_stored_object(store: &Store, id: &str, role: &str) -> Result<Value> {
    let Some(bytes) = store.get_object(id)? else {
        return Err(StoreError::Invalid(format!("{role} {id} がローカルに無い")));
    };
    let text = String::from_utf8(bytes)
        .map_err(|_| StoreError::Invalid(format!("{role} {id} が UTF-8 でない")))?;
    c1::parse(&text)
        .map_err(|error| StoreError::Invalid(format!("{role} {id} を解釈できない: {error}")))
}

/// spec_id を取り込み済み PDF に解決する。ref collections/<コレクション名>/<spec_id> を
/// 引いて doc_rev.source(PDF blob)を得、必要なページの本文行を meta.page の一致する
/// チャンクの text から集める(pdftotext の再実行はしない)。ref が無ければ全体を
/// 失敗させる(must/0022 の同型)。
fn resolve_document(
    store: &Store,
    collection: &str,
    spec_id: &str,
    pages: &BTreeSet<u32>,
) -> Result<ResolvedDocument> {
    let ref_path = format!("collections/{collection}/{spec_id}");
    let full_name = store.own_ref_name(&ref_path);
    let Some(target) = store.get_ref(&full_name).and_then(|state| state.target.clone()) else {
        return Err(StoreError::Invalid(format!(
            "spec_id {spec_id} の PDF が未取り込み(ref {ref_path} が無い)。\
             先に uniqnode ingest で PDF を取り込むこと"
        )));
    };
    let doc_rev = parse_stored_object(store, &target, "doc_rev")?;
    let Value::Object(doc) = &doc_rev else {
        return Err(StoreError::Invalid(format!("doc_rev {target} がオブジェクトでない")));
    };
    let Some(Value::Text(blob_id)) = doc.get("source") else {
        return Err(StoreError::Invalid(format!("doc_rev {target} に source が無い")));
    };
    let page_lines = collect_page_lines(store, &target, doc, pages)?;
    Ok(ResolvedDocument { blob_id: blob_id.clone(), pages: page_lines })
}

/// doc_rev の chunks 列から、要求ページの本文行を集める(meta.page の一致するチャンクの
/// text から取る。pdftotext の再実行はしない)。注釈の取り込みと訂正の再照合が共用する
/// (should/0135)。
fn collect_page_lines(
    store: &Store,
    doc_rev_id: &str,
    doc: &BTreeMap<String, Value>,
    pages: &BTreeSet<u32>,
) -> Result<BTreeMap<u32, Vec<String>>> {
    let Some(Value::Array(chunk_ids)) = doc.get("chunks") else {
        return Err(StoreError::Invalid(format!("doc_rev {doc_rev_id} に chunks 列が無い")));
    };
    let mut page_lines: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for chunk_ref in chunk_ids {
        let Value::Text(chunk_id) = chunk_ref else {
            return Err(StoreError::Invalid(format!(
                "doc_rev {doc_rev_id} の chunks 列に文字列でない要素がある"
            )));
        };
        let chunk = parse_stored_object(store, chunk_id, "chunk")?;
        let Value::Object(chunk_map) = &chunk else {
            return Err(StoreError::Invalid(format!("chunk {chunk_id} がオブジェクトでない")));
        };
        // meta.page を持たないチャンク(PDF 以外)は照合の対象にならない。
        let page = match chunk_map.get("meta") {
            Some(Value::Object(meta)) => meta.get("page"),
            _ => None,
        };
        let Some(Value::Integer(page)) = page else { continue };
        let Ok(page) = u32::try_from(*page) else { continue };
        if !pages.contains(&page) {
            continue;
        }
        if let Some(Value::Text(text)) = chunk_map.get("text") {
            page_lines
                .entry(page)
                .or_default()
                .extend(text.lines().map(|line| line.to_string()));
        }
    }
    Ok(page_lines)
}

/// {v:1, kind:"node", contents:<text>} のノード(節タイトル用)。
fn title_node_value(title: &str) -> Value {
    let mut object = BTreeMap::new();
    object.insert("v".to_string(), Value::Integer(1));
    object.insert("kind".to_string(), text_value("node"));
    object.insert("contents".to_string(), text_value(title));
    Value::Object(object)
}

/// 検証記録: 何を検証したかは持たず、どう検証し何を根拠にしたかだけを持つ
/// (INGEST の「訂正の表現」節。ASSERTIONS の原理 2 と 4)。
fn verification_record_value(method: &str, evidence: &[String]) -> Value {
    let mut contents = BTreeMap::new();
    contents.insert("method".to_string(), text_value(method));
    contents.insert(
        "evidence".to_string(),
        Value::Array(evidence.iter().map(|line| text_value(line)).collect()),
    );
    let mut object = BTreeMap::new();
    object.insert("v".to_string(), Value::Integer(1));
    object.insert("kind".to_string(), text_value("node"));
    object.insert("contents".to_string(), Value::Object(contents));
    Value::Object(object)
}

/// 注釈を annotates 型の辺として取り込み、コレクションの索引を作り直して
/// ref annotations/<コレクション名> を張る(INGEST の「注釈の取り込みと照合」節)。照合に一致した
/// 注釈は method=token-match、機械照合に落ちても承認リストにある (spec_id, ページ) の
/// 注釈は method=manual の検証記録付きで入る。どちらでもない不一致は取り込まず
/// rejected で報告する。辺と検証記録の結びつけは索引の対だけが持つ(検証記録は言明を
/// 指さず、言明も検証記録を持たない。どちらへ参照を張っても壊れる理由は INGEST の
/// 「訂正の表現」節)。
pub fn ingest_annotations(
    store: &mut Store,
    collection: &str,
    entries: &[AnnotationEntry],
    approvals: &BTreeSet<(String, u32)>,
) -> Result<AnnotationOutcome> {
    // 書き込みの前に spec_id を全件解決する(途中まで書いてから失敗する形を作らない)。
    let mut needed: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    for entry in entries {
        needed.entry(&entry.spec_id).or_default().insert(entry.page);
    }
    let mut documents: BTreeMap<&str, ResolvedDocument> = BTreeMap::new();
    for (spec_id, pages) in &needed {
        documents.insert(spec_id, resolve_document(store, collection, spec_id, pages)?);
    }

    let mut new_objects = 0usize;
    let mut accepted: Vec<AcceptedAnnotation> = Vec::new();
    let mut rejected: Vec<RejectedAnnotation> = Vec::new();
    // annotates 型ノードは最初の受理で一度だけ書く(受理ゼロの実行では書かない)。
    let mut type_id: Option<String> = None;
    let no_lines: Vec<String> = Vec::new();
    for entry in entries {
        let document = documents.get(entry.spec_id.as_str()).expect("解決済み");
        let lines = document.pages.get(&entry.page).unwrap_or(&no_lines);
        let outcome = match_annotation(&entry.title, lines);
        let record = if outcome.matched {
            Some((METHOD_TOKEN_MATCH, outcome.evidence))
        } else if approvals.contains(&(entry.spec_id.clone(), entry.page)) {
            // 機械照合に落ちたが人が確認済み。黙った例外ではなく manual の記録を残す。
            Some((METHOD_MANUAL, Vec::new()))
        } else {
            None
        };
        let Some((method, evidence)) = record else {
            rejected.push(RejectedAnnotation {
                spec_id: entry.spec_id.clone(),
                page: entry.page,
                title: entry.title.clone(),
                matched_tokens: outcome.matched_tokens,
                total_tokens: outcome.total_tokens,
            });
            continue;
        };
        if type_id.is_none() {
            let (id, is_new) = store.put_object(ANNOTATES_TYPE_BODY.as_bytes())?;
            new_objects += usize::from(is_new);
            type_id = Some(id);
        }
        let type_id = type_id.as_ref().expect("直前に確保した");
        let title_bytes = c1::to_canonical_bytes(&title_node_value(&entry.title));
        let (title_id, is_new) = store.put_object(&title_bytes)?;
        new_objects += usize::from(is_new);
        // annotates 辺。参照先は文書名ではなく PDF blob のハッシュ(I1・原理 6)。
        let mut edge = BTreeMap::new();
        edge.insert("v".to_string(), Value::Integer(1));
        edge.insert("kind".to_string(), text_value("edge"));
        edge.insert("type".to_string(), text_value(type_id));
        edge.insert(
            "members".to_string(),
            Value::Array(vec![text_value(&title_id), text_value(&document.blob_id)]),
        );
        let mut meta = BTreeMap::new();
        meta.insert("page".to_string(), Value::Integer(i64::from(entry.page)));
        edge.insert("meta".to_string(), Value::Object(meta));
        let (edge_id, is_new) = store.put_object(&c1::to_canonical_bytes(&Value::Object(edge)))?;
        new_objects += usize::from(is_new);
        let record_bytes = c1::to_canonical_bytes(&verification_record_value(method, &evidence));
        let (verification_id, is_new) = store.put_object(&record_bytes)?;
        new_objects += usize::from(is_new);
        accepted.push(AcceptedAnnotation {
            spec_id: entry.spec_id.clone(),
            page: entry.page,
            title: entry.title.clone(),
            method,
            matched_tokens: outcome.matched_tokens,
            total_tokens: outcome.total_tokens,
            edge_id,
            verification_id,
        });
    }

    // 索引は取り込みの実行ごとにまとめて作り直す(1 件ごとに作り直さない)。索引の
    // ref が無いと辺も検証記録も複製や pin の対象にならない(ASSERTIONS の原理 3)。
    // 作り直しは現行索引の訂正の項を持ち越す(落とすと訂正の辺と根拠がどの ref からも
    // 辿れなくなり、複製や pin の対象から外れる)。
    let pairs: Vec<Value> = accepted
        .iter()
        .map(|annotation| {
            let mut pair = BTreeMap::new();
            pair.insert("annotation".to_string(), text_value(&annotation.edge_id));
            pair.insert("verification".to_string(), text_value(&annotation.verification_id));
            Value::Object(pair)
        })
        .collect();
    let (_, corrections) = read_annotation_index(store, collection)?;
    let index = annotation_index_value(&pairs, &corrections);
    let (index_id, ref_updated, index_new) = write_annotation_index(store, collection, &index)?;
    new_objects += index_new;
    Ok(AnnotationOutcome { accepted, rejected, index_id, ref_updated, new_objects })
}

/// 索引オブジェクトを組む(注釈の取り込みと訂正の発行が同じ形を共用する。should/0135)。
/// contents.annotations は {annotation, verification} の対の列、contents.corrections は
/// corrects 辺 ID の列。corrections が空のときは鍵ごと省く(訂正の無い索引は注釈の段の
/// 形のまま変わらない)。
fn annotation_index_value(pairs: &[Value], corrections: &[String]) -> Value {
    let mut contents = BTreeMap::new();
    contents.insert("annotations".to_string(), Value::Array(pairs.to_vec()));
    if !corrections.is_empty() {
        contents.insert(
            "corrections".to_string(),
            Value::Array(corrections.iter().map(|id| text_value(id)).collect()),
        );
    }
    let mut index = BTreeMap::new();
    index.insert("v".to_string(), Value::Integer(1));
    index.insert("kind".to_string(), text_value("node"));
    index.insert("contents".to_string(), Value::Object(contents));
    Value::Object(index)
}

/// 現行の注釈索引(ref annotations/<コレクション名>)の中身を読む。ref が無ければ空。
/// 返り値は (annotations の対の列, corrections の ID 列)。形が崩れていたら黙って
/// 進めず明示的に失敗する(must/0022)。
fn read_annotation_index(
    store: &Store,
    collection: &str,
) -> Result<(Vec<Value>, Vec<String>)> {
    let full_name = store.own_ref_name(&format!("annotations/{collection}"));
    let Some(target) = store.get_ref(&full_name).and_then(|state| state.target.clone()) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let value = parse_stored_object(store, &target, "注釈索引")?;
    let Value::Object(map) = &value else {
        return Err(StoreError::Invalid(format!("注釈索引 {target} がオブジェクトでない")));
    };
    let Some(Value::Object(contents)) = map.get("contents") else {
        return Err(StoreError::Invalid(format!("注釈索引 {target} に contents が無い")));
    };
    let Some(Value::Array(pairs)) = contents.get("annotations") else {
        return Err(StoreError::Invalid(format!("注釈索引 {target} に annotations 列が無い")));
    };
    let corrections = match contents.get("corrections") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Text(id) => Ok(id.clone()),
                _ => Err(StoreError::Invalid(format!(
                    "注釈索引 {target} の corrections 列に文字列でない要素がある"
                ))),
            })
            .collect::<Result<Vec<String>>>()?,
        Some(_) => {
            return Err(StoreError::Invalid(format!(
                "注釈索引 {target} の corrections が列でない"
            )))
        }
    };
    Ok((pairs.clone(), corrections))
}

/// 索引を書いて ref annotations/<コレクション名> を張る。現行と同一なら ref は触らない
/// (no-op)。返り値は (索引 ID, ref を張り替えたか, 新規オブジェクト数)。
fn write_annotation_index(
    store: &mut Store,
    collection: &str,
    index: &Value,
) -> Result<(String, bool, usize)> {
    let (index_id, is_new) = store.put_object(&c1::to_canonical_bytes(index))?;
    let new_objects = usize::from(is_new);
    let ref_path = format!("annotations/{collection}");
    let full_name = store.own_ref_name(&ref_path);
    let current = store.get_ref(&full_name).and_then(|state| state.target.clone());
    let ref_updated = current.as_deref() != Some(index_id.as_str());
    if ref_updated {
        store.set_ref(&ref_path, Some(&index_id))?;
    }
    Ok((index_id, ref_updated, new_objects))
}

// ---- 訂正の発行(INGEST の「訂正の発行」節) ----

/// 訂正 1 件の結果(報告の素)。matched_tokens / total_tokens は新しい言明の再照合の
/// 一致率。
#[derive(Debug)]
pub struct CorrectionOutcome {
    pub corrects_edge_id: String,
    pub verification_id: String,
    pub matched_tokens: usize,
    pub total_tokens: usize,
    /// コレクションの索引オブジェクトの ID(ref annotations/<コレクション名> の指す先)。
    pub index_id: String,
    /// ref を張り替えたか。false = 同じ訂正の再発行(no-op)。
    pub ref_updated: bool,
    pub new_objects: usize,
}

/// annotates 辺の中身(節タイトルのノード ID・blob ID・ページ番号)。
struct AnnotatesParts {
    title_id: String,
    blob_id: String,
    page: u32,
}

/// annotates 型の辺から再照合に要る三つ組を取り出す。形が崩れていたら黙って進めず
/// 明示的に失敗する(must/0022)。
fn annotates_edge_parts(id: &str, value: &Value) -> Result<AnnotatesParts> {
    let Value::Object(map) = value else {
        return Err(StoreError::Invalid(format!("annotates 辺 {id} がオブジェクトでない")));
    };
    let Some(Value::Array(members)) = map.get("members") else {
        return Err(StoreError::Invalid(format!("annotates 辺 {id} に members が無い")));
    };
    let (Some(Value::Text(title_id)), Some(Value::Text(blob_id)), None) =
        (members.first(), members.get(1), members.get(2))
    else {
        return Err(StoreError::Invalid(format!(
            "annotates 辺 {id} の members が [節タイトルのノード, blob] の 2 要素でない"
        )));
    };
    let page = match map.get("meta") {
        Some(Value::Object(meta)) => match meta.get("page") {
            Some(Value::Integer(page)) => u32::try_from(*page).ok(),
            _ => None,
        },
        _ => None,
    };
    let Some(page) = page else {
        return Err(StoreError::Invalid(format!(
            "annotates 辺 {id} に meta.page(1 始まりの物理ページ番号)が無い"
        )));
    };
    Ok(AnnotatesParts { title_id: title_id.clone(), blob_id: blob_id.clone(), page })
}

/// blob を source に持つ現行 doc_rev を ref collections/<コレクション名>/* から探す。
/// 注釈は文書名ではなく blob に張られている(原理 6)ので、逆に blob から本文へ戻るには
/// 現在の見え(ref)を引く。見つからなければ None(呼び手が明示的に失敗する)。
fn find_doc_rev_for_blob(
    store: &Store,
    collection: &str,
    blob_id: &str,
) -> Result<Option<(String, Value)>> {
    let prefix = store.own_ref_name(&format!("collections/{collection}/"));
    for (name, state) in store.list_refs() {
        if !name.starts_with(&prefix) {
            continue;
        }
        // tombstone は現在の見えに無い。
        let Some(target) = &state.target else { continue };
        let value = parse_stored_object(store, target, "doc_rev")?;
        let Value::Object(map) = &value else { continue };
        if map.get("source") == Some(&text_value(blob_id)) {
            return Ok(Some((target.clone(), value)));
        }
    }
    Ok(None)
}

/// 訂正の発行(INGEST の「訂正の発行」節)。wrong_id の言明を誤りとし、new_id の言明が
/// 代わることを主張する corrects 辺を発行する。新しい言明は取り込み時と同じトークン照合
/// (match_annotation。should/0135)で再照合し、根拠行つきの検証記録(method=token-match)
/// を作って辺の meta.verification から指す。meta 内の s256: 文字列も参照なので、訂正の
/// 到達閉包が新旧の言明と根拠ごと運ばれる(原理 3 帰結 1)。索引 ref
/// annotations/<コレクション名> に訂正の項を足す(原理 3 帰結 3)。
pub fn correct_statement(
    store: &mut Store,
    collection: &str,
    wrong_id: &str,
    new_id: &str,
    reason: &str,
) -> Result<CorrectionOutcome> {
    if wrong_id == new_id {
        return Err(StoreError::Invalid(format!(
            "誤った言明と新しい言明が同じ({wrong_id})。言明は自分自身を訂正できない"
        )));
    }
    // 訂正の対象は既にストアにある言明だけ(INGEST の「訂正の表現」節)。無ければ
    // 黙って進めず明示的に失敗する(must/0022)。
    for (role, id) in [("誤った言明", wrong_id), ("新しい言明", new_id)] {
        if !store.has_object(id) {
            return Err(StoreError::Invalid(format!(
                "{role} {id} がストアに無い(訂正は既にストアにある言明にだけ張れる)"
            )));
        }
    }
    // 新しい言明の再照合。照合はタイトルとページ本文のトークン照合なので、適用できるのは
    // annotates 型の辺だけ。それ以外の形は理由を言って失敗する。
    let new_value = parse_stored_object(store, new_id, "新しい言明")?;
    if !is_annotates_edge(&new_value) {
        return Err(StoreError::Invalid(format!(
            "新しい言明 {new_id} が annotates 型の辺でない。再照合(タイトルとページ本文の\
             トークン照合)は annotates 辺にしか適用できないため、この言明では訂正を\
             発行できない"
        )));
    }
    let parts = annotates_edge_parts(new_id, &new_value)?;
    let title_value = parse_stored_object(store, &parts.title_id, "節タイトルのノード")?;
    let title = match &title_value {
        Value::Object(map) => match map.get("contents") {
            Some(Value::Text(title)) => title.clone(),
            _ => {
                return Err(StoreError::Invalid(format!(
                    "節タイトルのノード {} に contents(文字列)が無い",
                    parts.title_id
                )))
            }
        },
        _ => {
            return Err(StoreError::Invalid(format!(
                "節タイトルのノード {} がオブジェクトでない",
                parts.title_id
            )))
        }
    };
    let Some((doc_rev_id, doc_rev)) = find_doc_rev_for_blob(store, collection, &parts.blob_id)?
    else {
        return Err(StoreError::Invalid(format!(
            "新しい言明の参照する blob {} を source に持つ doc_rev が collections/\
             {collection}/ に無い。先に uniqnode ingest で文書を取り込むこと",
            parts.blob_id
        )));
    };
    let Value::Object(doc) = &doc_rev else {
        return Err(StoreError::Invalid(format!("doc_rev {doc_rev_id} がオブジェクトでない")));
    };
    let mut pages = BTreeSet::new();
    pages.insert(parts.page);
    let page_lines = collect_page_lines(store, &doc_rev_id, doc, &pages)?;
    let no_lines: Vec<String> = Vec::new();
    let lines = page_lines.get(&parts.page).unwrap_or(&no_lines);
    let outcome = match_annotation(&title, lines);
    if !outcome.matched {
        return Err(StoreError::Invalid(format!(
            "新しい言明 {new_id} が照合に落ちた(p.{} との一致 {}/{})。照合を通らない言明\
             では訂正を発行しない",
            parts.page, outcome.matched_tokens, outcome.total_tokens
        )));
    }
    // ここから書き込み。すべて content-addressed なので、同じ訂正の再発行は新規
    // オブジェクトを生まない(べき等 = I1)。
    let mut new_objects = 0usize;
    let (type_id, is_new) = store.put_object(CORRECTS_TYPE_BODY.as_bytes())?;
    new_objects += usize::from(is_new);
    let record_bytes =
        c1::to_canonical_bytes(&verification_record_value(METHOD_TOKEN_MATCH, &outcome.evidence));
    let (verification_id, is_new) = store.put_object(&record_bytes)?;
    new_objects += usize::from(is_new);
    // corrects 辺。members は [新しい言明, 誤った言明] の順(INGEST の「訂正の表現」節)。
    let mut edge = BTreeMap::new();
    edge.insert("v".to_string(), Value::Integer(1));
    edge.insert("kind".to_string(), text_value("edge"));
    edge.insert("type".to_string(), text_value(&type_id));
    edge.insert(
        "members".to_string(),
        Value::Array(vec![text_value(new_id), text_value(wrong_id)]),
    );
    let mut meta = BTreeMap::new();
    meta.insert("reason".to_string(), text_value(reason));
    meta.insert("verification".to_string(), text_value(&verification_id));
    edge.insert("meta".to_string(), Value::Object(meta));
    let (edge_id, is_new) = store.put_object(&c1::to_canonical_bytes(&Value::Object(edge)))?;
    new_objects += usize::from(is_new);
    // 索引に訂正の項を足す(訂正の辺も索引の ref から指されないと複製や pin の対象に
    // ならない。原理 3 帰結 3)。同じ訂正の再発行は項を重複させない(no-op)。
    let (pairs, mut corrections) = read_annotation_index(store, collection)?;
    if !corrections.contains(&edge_id) {
        corrections.push(edge_id.clone());
    }
    let index = annotation_index_value(&pairs, &corrections);
    let (index_id, ref_updated, index_new) = write_annotation_index(store, collection, &index)?;
    new_objects += index_new;
    Ok(CorrectionOutcome {
        corrects_edge_id: edge_id,
        verification_id,
        matched_tokens: outcome.matched_tokens,
        total_tokens: outcome.total_tokens,
        index_id,
        ref_updated,
        new_objects,
    })
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
                extra_meta: &[],
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

    // ---- 注釈の段 ----

    /// 照合規則: 小文字化・英数字以外での分割・数字だけの語の除去・6 割以上で一致
    /// (期待値はリテラル。should/0137)。
    #[test]
    fn annotation_matching_follows_the_token_rule() {
        let lines = vec![
            "5.2.3.2 Generic Address Structure".to_string(),
            "The platform uses tables.".to_string(),
        ];
        let outcome = match_annotation("5.2.3.2 Generic Address Structure", &lines);
        assert!(outcome.matched);
        // 数字だけの語(5, 2, 3)は数えない。
        assert_eq!(outcome.matched_tokens, 3);
        assert_eq!(outcome.total_tokens, 3);
        assert_eq!(outcome.evidence, vec!["5.2.3.2 Generic Address Structure".to_string()]);

        // 大文字小文字と区切り記号(em ダッシュ)は照合に影響しない。
        let lines = vec!["CPUID—CPU Identification".to_string()];
        let outcome = match_annotation("cpuid identification", &lines);
        assert!(outcome.matched);
        assert_eq!(outcome.evidence, vec!["CPUID—CPU Identification".to_string()]);

        // 6 割の境界: 5 語中 3 語は一致、4 語中 2 語(5 割)は不一致。
        let lines = vec!["alpha beta gamma".to_string()];
        let at_the_threshold = match_annotation("alpha beta gamma delta epsilon", &lines);
        assert!(at_the_threshold.matched);
        assert_eq!(at_the_threshold.matched_tokens, 3);
        assert_eq!(at_the_threshold.total_tokens, 5);
        let below = match_annotation("alpha beta delta epsilon", &lines);
        assert!(!below.matched);
        assert_eq!(below.matched_tokens, 2);
        assert_eq!(below.total_tokens, 4);

        // 負例: ページに無い語だけのタイトルは落ち、根拠も残らない。
        let missed = match_annotation("Nonexistent widget", &lines);
        assert!(!missed.matched);
        assert_eq!(missed.matched_tokens, 0);
        assert_eq!(missed.total_tokens, 2);
        assert_eq!(missed.evidence, Vec::<String>::new());
    }

    /// 根拠は、一致した各語を最初に含む本文行(ページ順・重複なし)。
    #[test]
    fn annotation_evidence_collects_the_first_line_for_each_matched_token() {
        let lines = vec![
            "first line about alpha".to_string(),
            "second line about beta".to_string(),
            "alpha again".to_string(),
        ];
        let outcome = match_annotation("Alpha Beta", &lines);
        assert!(outcome.matched);
        assert_eq!(
            outcome.evidence,
            vec!["first line about alpha".to_string(), "second line about beta".to_string()]
        );
    }

    /// 型ノード三種の ID を固定で押さえる(must/0023。期待値はリテラル。should/0137)。
    /// 本文は c1 正規形そのもの(パースして再直列化しても変わらない)。
    #[test]
    fn type_node_ids_are_pinned() {
        assert_eq!(
            annotates_type_id(),
            "s256:5338025bc944148dd5ae2ea4fc2ac5807f260f6259cc8fd8b9556615983e0d3c"
        );
        assert_eq!(
            corrects_type_id(),
            "s256:78d9189f91557b5bff27aefd61643ee333f54d63f9bb70a47b9fa18be40a8b4d"
        );
        assert_eq!(
            supersedes_type_id(),
            "s256:7d6e8d3753bb00b3abecdac3875099b6b1f53683cc792b08c3e427f3206112e8"
        );
        for body in [ANNOTATES_TYPE_BODY, CORRECTS_TYPE_BODY, SUPERSEDES_TYPE_BODY] {
            let value = c1::parse(body).expect("型ノード本文は c1");
            assert_eq!(c1::to_canonical_bytes(&value), body.as_bytes());
        }
    }

    /// ページ番号付きチャンク 1 個の擬似 PDF 文書を collections/specs/<name> に
    /// 取り込む(注釈の段のテスト用。照合はチャンクの text だけを見るので PDF の
    /// 実バイナリは不要)。
    fn ingest_page_document(store: &mut Store, name: &str, page_text: &str) -> IngestOutcome {
        let chunks = vec![Chunk {
            text: page_text.to_string(),
            breadcrumbs: Vec::new(),
            page: Some(1),
        }];
        let source = format!("%PDF-fake {name}");
        ingest_document(
            store,
            &DocumentInput {
                collection: "specs",
                name,
                source: source.as_bytes(),
                media: "pdf",
                chunks: &chunks,
                extractor: Some("pdftotext test"),
                extra_meta: &[],
            },
        )
        .expect("ingest")
    }

    /// 生成側(ingest_annotations の辺の発行)と判定側(is_annotates_edge)が同じ
    /// 型定数を通ることを、実際に書かれた辺で見る(must/0023: 定数同士の比較ではなく
    /// 生成経路を通す)。
    #[test]
    fn the_produced_edge_is_recognized_by_the_matcher_through_the_shared_constant() {
        let (dir, mut store) = temp_store("annotation-type");
        ingest_page_document(&mut store, "minispec", "Alpha Beta Gamma\nDelta line");
        let entries = vec![AnnotationEntry {
            spec_id: "minispec".to_string(),
            page: 1,
            title: "Alpha Beta".to_string(),
        }];
        let outcome =
            ingest_annotations(&mut store, "specs", &entries, &BTreeSet::new()).expect("ingest");
        assert_eq!(outcome.accepted.len(), 1);
        let bytes =
            store.get_object(&outcome.accepted[0].edge_id).expect("get").expect("present");
        let value = c1::parse(&String::from_utf8(bytes).expect("utf-8")).expect("c1");
        assert!(is_annotates_edge(&value), "発行した辺を判定側が認識しない");
        // 別の型の辺は認識しない(判定が type を見ている証明)。
        let Value::Object(mut map) = value else { panic!("辺はオブジェクト") };
        map.insert("type".to_string(), text_value(&corrects_type_id()));
        assert!(!is_annotates_edge(&Value::Object(map)));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 一致は入り、不一致は取り込まず報告する(負例を含む両向きの確認)。承認リストに
    /// ある不一致は manual の検証記録付きで入り、無い不一致は入らない。再実行は no-op。
    #[test]
    fn matched_annotations_enter_and_mismatches_are_reported_not_ingested() {
        let (dir, mut store) = temp_store("annotation-both-ways");
        ingest_page_document(&mut store, "minispec", "Generic Address Structure\nOther line");
        let entries = vec![
            AnnotationEntry {
                spec_id: "minispec".to_string(),
                page: 1,
                title: "Generic Address Structure".to_string(),
            },
            AnnotationEntry {
                spec_id: "minispec".to_string(),
                page: 1,
                title: "Completely Unrelated Heading".to_string(),
            },
        ];
        let outcome =
            ingest_annotations(&mut store, "specs", &entries, &BTreeSet::new()).expect("ingest");
        assert_eq!(outcome.accepted.len(), 1);
        assert_eq!(outcome.accepted[0].title, "Generic Address Structure");
        assert_eq!(outcome.accepted[0].method, METHOD_TOKEN_MATCH);
        assert_eq!(outcome.rejected.len(), 1, "不一致が報告されなければならない");
        assert_eq!(outcome.rejected[0].title, "Completely Unrelated Heading");
        assert_eq!(outcome.rejected[0].matched_tokens, 0);
        assert_eq!(outcome.rejected[0].total_tokens, 3);
        assert!(outcome.ref_updated);

        // 辺の形: {v:1, kind:"edge", type:<annotates>, members:[タイトルノード, blob],
        // meta:{page:N}}。blob は擬似 PDF のハッシュ。
        let bytes =
            store.get_object(&outcome.accepted[0].edge_id).expect("get").expect("present");
        let edge = c1::parse(&String::from_utf8(bytes).expect("utf-8")).expect("c1");
        let Value::Object(edge) = edge else { panic!("辺はオブジェクト") };
        assert_eq!(edge.get("kind"), Some(&text_value("edge")));
        assert_eq!(edge.get("type"), Some(&text_value(&annotates_type_id())));
        let title_id = c1::object_id(&title_node_value("Generic Address Structure"));
        let blob_id = c1::id_for_bytes("%PDF-fake minispec".as_bytes());
        assert_eq!(
            edge.get("members"),
            Some(&Value::Array(vec![text_value(&title_id), text_value(&blob_id)]))
        );
        let Some(Value::Object(meta)) = edge.get("meta") else { panic!("meta") };
        assert_eq!(meta.get("page"), Some(&Value::Integer(1)));

        // 検証記録: method=token-match、根拠は一致に使った本文行。何を検証したかは
        // 持たない(言明への参照が無い)。
        let bytes = store
            .get_object(&outcome.accepted[0].verification_id)
            .expect("get")
            .expect("present");
        let record_text = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(
            record_text,
            "{\"contents\":{\"evidence\":[\"Generic Address Structure\"],\
             \"method\":\"token-match\"},\"kind\":\"node\",\"v\":1}"
        );

        // 索引には受理された 1 件だけが、辺と検証記録の対で載る。
        let bytes = store.get_object(&outcome.index_id).expect("get").expect("present");
        let index_text = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(
            index_text,
            format!(
                "{{\"contents\":{{\"annotations\":[{{\"annotation\":\"{}\",\
                 \"verification\":\"{}\"}}]}},\"kind\":\"node\",\"v\":1}}",
                outcome.accepted[0].edge_id, outcome.accepted[0].verification_id
            )
        );
        let full_name = store.own_ref_name("annotations/specs");
        assert_eq!(
            store.get_ref(&full_name).expect("ref").target.as_deref(),
            Some(outcome.index_id.as_str())
        );

        // 再実行は完全な no-op(索引 ID・ref・オブジェクト数が動かない)。
        let objects_before = store.object_count();
        let seq_before = store.get_ref(&full_name).expect("ref").seq;
        let again =
            ingest_annotations(&mut store, "specs", &entries, &BTreeSet::new()).expect("ingest");
        assert_eq!(again.index_id, outcome.index_id);
        assert!(!again.ref_updated, "同じ入力の再実行で ref を張り替えてはならない");
        assert_eq!(again.new_objects, 0);
        assert_eq!(store.object_count(), objects_before);
        assert_eq!(store.get_ref(&full_name).expect("ref").seq, seq_before);

        // 承認リストに載せると、落ちていた注釈が manual の検証記録付きで入る。
        let mut approvals = BTreeSet::new();
        approvals.insert(("minispec".to_string(), 1u32));
        let approved =
            ingest_annotations(&mut store, "specs", &entries, &approvals).expect("ingest");
        assert_eq!(approved.accepted.len(), 2);
        assert!(approved.rejected.is_empty());
        // 機械照合に通る注釈は承認リストがあっても token-match のまま。
        assert_eq!(approved.accepted[0].method, METHOD_TOKEN_MATCH);
        assert_eq!(approved.accepted[1].method, METHOD_MANUAL);
        let bytes = store
            .get_object(&approved.accepted[1].verification_id)
            .expect("get")
            .expect("present");
        assert_eq!(
            String::from_utf8(bytes).expect("utf-8"),
            "{\"contents\":{\"evidence\":[],\"method\":\"manual\"},\"kind\":\"node\",\"v\":1}"
        );
        assert!(approved.ref_updated, "索引が変わったので ref も進む");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// ref の無い spec_id は黙って飛ばさず全体を失敗させ、何も書かない
    /// (must/0022 の同型)。
    #[test]
    fn a_missing_pdf_ref_fails_the_whole_annotation_ingest_before_writing() {
        let (dir, mut store) = temp_store("annotation-missing-ref");
        ingest_page_document(&mut store, "present", "Alpha line");
        let objects_before = store.object_count();
        let entries = vec![
            AnnotationEntry {
                spec_id: "present".to_string(),
                page: 1,
                title: "Alpha".to_string(),
            },
            AnnotationEntry {
                spec_id: "absent".to_string(),
                page: 1,
                title: "Alpha".to_string(),
            },
        ];
        let error = ingest_annotations(&mut store, "specs", &entries, &BTreeSet::new())
            .expect_err("ref の無い spec_id で成功してはならない");
        let message = error.to_string();
        assert!(message.contains("absent"), "{message}");
        assert!(message.contains("先に"), "{message}");
        assert_eq!(store.object_count(), objects_before, "失敗した取り込みが書き残した");
        let full_name = store.own_ref_name("annotations/specs");
        assert!(store.get_ref(&full_name).is_none(), "失敗した取り込みが ref を作った");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    // ---- 訂正の段 ----

    /// 2 ページの擬似 PDF 文書(訂正の段のテスト用)。p.1 と p.2 に別の本文を置く。
    fn ingest_two_page_document(store: &mut Store, name: &str) -> IngestOutcome {
        let chunks = vec![
            Chunk { text: "Alpha Beta Gamma".to_string(), breadcrumbs: Vec::new(), page: Some(1) },
            Chunk {
                text: "Delta Epsilon Zeta".to_string(),
                breadcrumbs: Vec::new(),
                page: Some(2),
            },
        ];
        let source = format!("%PDF-fake {name}");
        ingest_document(
            store,
            &DocumentInput {
                collection: "specs",
                name,
                source: source.as_bytes(),
                media: "pdf",
                chunks: &chunks,
                extractor: Some("pdftotext test"),
                extra_meta: &[],
            },
        )
        .expect("ingest")
    }

    /// p.1 と p.2 の注釈 2 件を取り込む(2 件目が訂正の「新しい言明」役)。
    fn ingest_two_annotations(store: &mut Store) -> AnnotationOutcome {
        ingest_two_page_document(store, "minispec");
        let entries = vec![
            AnnotationEntry {
                spec_id: "minispec".to_string(),
                page: 1,
                title: "Alpha Beta".to_string(),
            },
            AnnotationEntry {
                spec_id: "minispec".to_string(),
                page: 2,
                title: "Delta Epsilon".to_string(),
            },
        ];
        let outcome =
            ingest_annotations(store, "specs", &entries, &BTreeSet::new()).expect("ingest");
        assert_eq!(outcome.accepted.len(), 2, "前提: 2 件とも照合を通る");
        outcome
    }

    /// 訂正の段の本流: corrects 辺が正典仕様の形({v:1, kind:edge, type:<corrects>,
    /// members:[新, 旧], meta:{reason, verification}})で発行され、検証記録は再照合の
    /// 根拠行を持ち、索引に訂正の項が足される。同じ訂正の再発行は完全な no-op。
    #[test]
    fn correct_statement_issues_the_edge_with_reverification_and_updates_the_index() {
        let (dir, mut store) = temp_store("correct-edge");
        let annotations = ingest_two_annotations(&mut store);
        let wrong_id = annotations.accepted[0].edge_id.clone();
        let new_id = annotations.accepted[1].edge_id.clone();

        let outcome =
            correct_statement(&mut store, "specs", &wrong_id, &new_id, "p.1 は p.2 の誤り")
                .expect("correct");
        assert_eq!((outcome.matched_tokens, outcome.total_tokens), (2, 2));
        assert!(outcome.ref_updated);

        // corrects 辺の形(期待値はリテラルに近い正規形の全文。should/0137)。
        let bytes = store.get_object(&outcome.corrects_edge_id).expect("get").expect("present");
        assert_eq!(
            String::from_utf8(bytes).expect("utf-8"),
            format!(
                "{{\"kind\":\"edge\",\"members\":[\"{new_id}\",\"{wrong_id}\"],\
                 \"meta\":{{\"reason\":\"p.1 は p.2 の誤り\",\"verification\":\"{}\"}},\
                 \"type\":\"{}\",\"v\":1}}",
                outcome.verification_id,
                corrects_type_id()
            )
        );
        // 検証記録: 再照合の方法と根拠行(p.2 の本文)だけを持つ。
        let bytes = store.get_object(&outcome.verification_id).expect("get").expect("present");
        assert_eq!(
            String::from_utf8(bytes).expect("utf-8"),
            "{\"contents\":{\"evidence\":[\"Delta Epsilon Zeta\"],\
             \"method\":\"token-match\"},\"kind\":\"node\",\"v\":1}"
        );
        // 索引: 注釈の対は残ったまま、corrections に訂正の辺が載り、ref が指す。
        let bytes = store.get_object(&outcome.index_id).expect("get").expect("present");
        let index_text = String::from_utf8(bytes).expect("utf-8");
        assert!(
            index_text
                .contains(&format!("\"corrections\":[\"{}\"]", outcome.corrects_edge_id)),
            "{index_text}"
        );
        assert!(index_text.contains(&format!("\"annotation\":\"{wrong_id}\"")), "{index_text}");
        assert!(index_text.contains(&format!("\"annotation\":\"{new_id}\"")), "{index_text}");
        let full_name = store.own_ref_name("annotations/specs");
        assert_eq!(
            store.get_ref(&full_name).expect("ref").target.as_deref(),
            Some(outcome.index_id.as_str())
        );

        // 同じ訂正の再発行は完全な no-op(辺も索引もオブジェクト数も動かない)。
        let objects_before = store.object_count();
        let seq_before = store.get_ref(&full_name).expect("ref").seq;
        let again =
            correct_statement(&mut store, "specs", &wrong_id, &new_id, "p.1 は p.2 の誤り")
                .expect("correct again");
        assert_eq!(again.corrects_edge_id, outcome.corrects_edge_id);
        assert_eq!(again.new_objects, 0, "再発行で新オブジェクトを書いてはならない");
        assert!(!again.ref_updated, "再発行で ref を張り替えてはならない");
        assert_eq!(store.object_count(), objects_before);
        assert_eq!(store.get_ref(&full_name).expect("ref").seq, seq_before);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 訂正の負例: 存在しない ID・自分自身・annotates 辺でない言明・blob の引けない
    /// 言明・照合に落ちる言明は、どれも明示的に失敗し(must/0022)、何も書かない。
    #[test]
    fn correct_statement_fails_explicitly_on_bad_input_and_writes_nothing() {
        let (dir, mut store) = temp_store("correct-negative");
        let annotations = ingest_two_annotations(&mut store);
        let wrong_id = annotations.accepted[0].edge_id.clone();
        let absent = format!("s256:{}", "0".repeat(64));

        // 誤った言明がストアに無い(訂正は既にストアにある言明にだけ張れる)。
        let new_id = annotations.accepted[1].edge_id.clone();
        let error = correct_statement(&mut store, "specs", &absent, &new_id, "理由")
            .expect_err("存在しない誤った言明で成功してはならない");
        let message = error.to_string();
        assert!(message.contains("誤った言明"), "{message}");
        assert!(message.contains(&absent), "{message}");
        assert!(message.contains("ストアに無い"), "{message}");

        // 新しい言明がストアに無い。
        let error = correct_statement(&mut store, "specs", &wrong_id, &absent, "理由")
            .expect_err("存在しない新しい言明で成功してはならない");
        let message = error.to_string();
        assert!(message.contains("新しい言明"), "{message}");
        assert!(message.contains("ストアに無い"), "{message}");

        // 自分自身では訂正できない。
        let error = correct_statement(&mut store, "specs", &wrong_id, &wrong_id, "理由")
            .expect_err("自分自身の訂正で成功してはならない");
        assert!(error.to_string().contains("自分自身"), "{error}");

        // annotates 辺でない言明(節タイトルのノード)では再照合できない。
        let title_id = c1::object_id(&title_node_value("Alpha Beta"));
        let error = correct_statement(&mut store, "specs", &wrong_id, &title_id, "理由")
            .expect_err("annotates 辺でない言明で成功してはならない");
        assert!(error.to_string().contains("annotates 型の辺でない"), "{error}");

        // 参照先 blob の doc_rev がコレクションに無い annotates 辺。
        let make_edge = |title_id: &str, blob_id: &str, page: i64| {
            let mut edge = BTreeMap::new();
            edge.insert("v".to_string(), Value::Integer(1));
            edge.insert("kind".to_string(), text_value("edge"));
            edge.insert("type".to_string(), text_value(&annotates_type_id()));
            edge.insert(
                "members".to_string(),
                Value::Array(vec![text_value(title_id), text_value(blob_id)]),
            );
            let mut meta = BTreeMap::new();
            meta.insert("page".to_string(), Value::Integer(page));
            edge.insert("meta".to_string(), Value::Object(meta));
            c1::to_canonical_bytes(&Value::Object(edge))
        };
        let (dangling_id, _) = store.put_object(&make_edge(&title_id, &absent, 1)).expect("put");
        let error = correct_statement(&mut store, "specs", &wrong_id, &dangling_id, "理由")
            .expect_err("blob の引けない言明で成功してはならない");
        assert!(error.to_string().contains("doc_rev"), "{error}");

        // 照合に落ちる新しい言明では訂正を発行しない。失敗までに何も書かないこと。
        let missed_title = title_node_value("Nonexistent widget");
        let (missed_title_id, _) =
            store.put_object(&c1::to_canonical_bytes(&missed_title)).expect("put");
        let blob_id = c1::id_for_bytes("%PDF-fake minispec".as_bytes());
        let (missed_edge_id, _) =
            store.put_object(&make_edge(&missed_title_id, &blob_id, 2)).expect("put");
        let objects_before = store.object_count();
        let error = correct_statement(&mut store, "specs", &wrong_id, &missed_edge_id, "理由")
            .expect_err("照合に落ちる言明で成功してはならない");
        let message = error.to_string();
        assert!(message.contains("照合に落ちた"), "{message}");
        assert!(message.contains("0/2"), "{message}");
        assert_eq!(store.object_count(), objects_before, "失敗した訂正が書き残した");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 索引の作り直し(ingest_annotations の再実行)が訂正の項を落とさない。落とすと
    /// 訂正の辺と根拠がどの ref からも辿れず、複製や pin の対象から外れる(原理 3)。
    #[test]
    fn reingesting_annotations_preserves_recorded_corrections() {
        let (dir, mut store) = temp_store("correct-preserved");
        let annotations = ingest_two_annotations(&mut store);
        let wrong_id = annotations.accepted[0].edge_id.clone();
        let new_id = annotations.accepted[1].edge_id.clone();
        let corrected =
            correct_statement(&mut store, "specs", &wrong_id, &new_id, "p.1 は p.2 の誤り")
                .expect("correct");
        assert_ne!(corrected.index_id, annotations.index_id, "訂正で索引が進む前提");

        // 同じ注釈の再取り込み: 索引は作り直されるが、訂正の項は持ち越されて no-op。
        let entries = vec![
            AnnotationEntry {
                spec_id: "minispec".to_string(),
                page: 1,
                title: "Alpha Beta".to_string(),
            },
            AnnotationEntry {
                spec_id: "minispec".to_string(),
                page: 2,
                title: "Delta Epsilon".to_string(),
            },
        ];
        let again =
            ingest_annotations(&mut store, "specs", &entries, &BTreeSet::new()).expect("ingest");
        assert_eq!(again.index_id, corrected.index_id, "作り直しが訂正の項を落とした");
        assert!(!again.ref_updated);
        assert_eq!(again.new_objects, 0);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 階を飛ばした見出し(h1 の次が h3)で、並んだ h3 が親子にならない。
    #[test]
    fn sibling_headings_do_not_nest_when_a_level_is_skipped() {
        let chunks = chunk_markdown("# 題\n\n### あ\n\n本文あ\n\n### い\n\n本文い\n");
        let paths: Vec<Vec<String>> =
            chunks.iter().map(|chunk| chunk.breadcrumbs.clone()).collect();
        assert_eq!(paths, vec![vec!["題", "あ"], vec!["題", "い"]]);
    }

    /// HTML は原本のまま渡され、チャンクは札の落ちた本文になる(見出しは経路に写る)。
    #[test]
    fn html_is_chunked_from_its_text_not_from_its_tags() {
        assert_eq!(media_for_extension("html"), Some("html"));
        assert_eq!(media_for_extension("htm"), Some("html"));
        let page = "<!DOCTYPE html><html><head><title>題</title>\
                    <style>p { color: red }</style></head><body>\
                    <h2>節</h2><p>本文&amp;続き</p><script>var a = 1;</script></body></html>";
        let chunks = chunk_for_media("html", page);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "本文&続き");
        assert_eq!(chunks[0].breadcrumbs, vec!["題".to_string(), "節".to_string()]);
        assert!(chunks[0].page.is_none(), "HTML に紙面の番号は無い");
    }
}
