//! PDF の節見出しの表(物理ページ番号 → 節見出しの経路)。
//!
//! Markdown のチャンクは見出しの入れ子(breadcrumbs)を持ち、索引はそれに本文の 2 倍の
//! 重みを掛ける。PDF のチャンクだけがその構造を持っていない。この層は、PDF から
//! 「そのページに効いている節見出しの経路」を作り、取り込み側が breadcrumbs に写せる形
//! (`Vec<PageHeadings>`)で返す。
//!
//! なぜ要るか(実測 2026-08-17)。日本語「スクラッチパッド」で引くと、正解の
//! xhci_1_2 p.334(本文が「4.20 Scratchpad Buffers / The Scratchpad Allocation
//! mechanism of the xHCI ...」で始まる節)が上位に出ず、レジスタのビット定義が並ぶ
//! p.434 が 1 位になる。両者の本文はどちらも Scratch/Pad/Buffer を含み、**区別できる
//! 情報は節見出しにしかない**(p.334 は `4.20 Scratchpad Buffers`、p.434 は
//! `5.7.2 VTIO Common Assignment Register 1`)。
//!
//! 2 系統ある。
//!
//! 1. しおり(`pdftohtml -f 1 -l 1 -c` が吐く `-outline.html`)。節番号・題・階層・
//!    物理ページ番号が文書全体ぶん揃っている。実測 0.6 秒/本。ただし写し取りを禁じた
//!    PDF(xhci_1_2・hut1_12v2)では pdftohtml 自体が拒まれ、しおりの無い PDF も多い。
//! 2. 語の高さ(`pdftotext -bbox`)。写し取りを禁じた PDF でも通る(実測: xhci 645
//!    ページで 1.9 秒)。語ごとの yMax-yMin がフォントの大きさの代理になり、本文より
//!    背の高い行のうち先頭語が節番号のものを見出しとみなす。
//!
//! しおりを先に試し、駄目なら高さで拾う。どちらを使ったかは返り値
//! (`DocumentOutline::source`)に必ず入る。黙って劣化しない(must/0019 の同型)。
//! どちらも駄目なら `pages` は空で、`reason` に何を試して何が起きたかが入る。
//!
//! 相互検証(実測 2026-08-17): 高さ系統が sdm_vol3 p.400 に `11.5.4 APIC Timer` を
//! 出し、しおり系統も同じページに同じ題を出す。uefi_2_9 p.400 の
//! `10.3.5 Media Device Path` も一致する。独立な 2 手法が一致している。

use crate::rendition::{located_tool, Tool, POPPLER_INSTALL_HINT};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

// ---- 返り値 ----

/// 1 ページに効いている見出しの経路。path は親からの列で、
/// 例 `["4 Operational Model", "4.20 Scratchpad Buffers"]`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageHeadings {
    /// 物理ページ番号(1 始まり)。ingest::chunk_pdf_text が付ける page と同じ数え方。
    pub page: u32,
    pub path: Vec<String>,
}

/// どちらの系統で拾ったか。呼び手はこれを見て、黙って劣化していないかを確かめられる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineSource {
    /// PDF のしおり(pdftohtml)。
    Bookmarks,
    /// 語の高さ(pdftotext -bbox)。
    FontSize,
    /// どちらも拾えなかった。pages は空で、reason に理由が入る。
    Missing,
}

impl OutlineSource {
    /// ログや meta に書くための短い名前。生成側と読み側が同じ表を見る(must/0023)。
    pub fn as_str(self) -> &'static str {
        match self {
            OutlineSource::Bookmarks => "bookmarks",
            OutlineSource::FontSize => "font-size",
            OutlineSource::Missing => "none",
        }
    }
}

/// 1 文書ぶんの見出しの表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentOutline {
    pub source: OutlineSource,
    /// ページ番号の昇順。見出しの効いていないページ(表紙・目次など、最初の見出しより
    /// 前)は入らない。
    pub pages: Vec<PageHeadings>,
    /// 何を試して何が起きたか。source が Missing のときは、そのまま理由である。
    pub reason: String,
}

impl DocumentOutline {
    /// ページ番号から経路を引く。pages は昇順なので二分探索する。
    pub fn path_for_page(&self, page: u32) -> &[String] {
        match self.pages.binary_search_by_key(&page, |entry| entry.page) {
            Ok(index) => &self.pages[index].path,
            Err(_) => &[],
        }
    }
}

/// 見出し 1 件(系統によらない中間の形)。2 系統はどちらもこれを作り、ページへの
/// 割り当ては attribute_headings_to_pages 1 箇所で行う(should/0135)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// 物理ページ番号(1 始まり)。
    pub page: u32,
    /// 1 始まりの階層。1 = 章、2 = 節、3 = 小節。
    pub level: usize,
    /// 節番号を含む題。例 `4.20 Scratchpad Buffers`。
    pub title: String,
}

// ---- 判定に使う定数 ----

/// 見出しの経路の深さの上限。文書名 > 節 > 小節 の 3 段のうち、文書名は取り込み側が
/// 既に持っている(チャンクの ref パス)ので、この層が返すのは 2 段までである。
/// 4 段以上に細かく切ると断片化で検索が劣化する、との報告に合わせてある。
pub const HEADING_PATH_DEPTH: usize = 2;

/// 同じ行とみなす yMin の差(pt)。
const LINE_TOLERANCE_PT: f32 = 1.5;

/// 本文の何倍から見出しとみなすか。行の高さは量子化している(xhci は 26.9 / 17.2 /
/// 14.7 / 本文 12.2)ので、ごく僅かに大きければよい。
const HEADING_HEIGHT_RATIO: f32 = 1.02;

/// 見出しとして通す行の長さの上限(文字)。
const HEADING_MAX_CHARS: usize = 90;

/// 柱・ノンブルとして落とすページ上下の帯(ページ高さに対する割合)。
const MARGIN_BAND_RATIO: f32 = 0.08;

/// 末尾の数字を「柱・目次のページ番号」とみなす、その手前の空き(pt)。語間の空きは
/// 3pt 前後なので、点リーダや右寄せで開いた空きだけが超える。
const RUNNING_HEAD_GAP_PT: f32 = 12.0;

/// 折り返しの続きとみなす、行の高さの一致の許容(pt)。
const WRAP_HEIGHT_TOLERANCE_PT: f32 = 0.3;

/// 折り返しの続きとみなす、行間の上限(その行の高さに対する倍率)。
const WRAP_LINE_GAP_RATIO: f32 = 1.8;

/// 折り返しとして繋ぐ続きの行数の上限。実在するのは 2 行に割れた見出しなので 1 で
/// 足りる。上限を置かないと、判定を 1 件でも取り違えたときに段落を丸ごと飲み込む。
const WRAP_MAX_CONTINUATIONS: usize = 1;

/// 「よく出る高さ」とみなす、全行数に対する割合。
const BODY_HEIGHT_SHARE: f32 = 0.15;

/// 見出しの表が「使える」とみなす最小件数。2 系統に同じ値を使う(should/0135)。
///
/// pci_22 が実例である。しおりは `Return to Contents` の 1 件だけで行き先を持たず、
/// 本文はフォントの符号表が壊れていて pdftotext が字化けを返す(`7KHPDVWHU...`)ので
/// 高さでも 1 件しか拾えない。その 1 件を全ページに被せると、80 ページに同じ経路が
/// 付いて何も区別しなくなる。取れないときは取れないと言う方がよい。
const MIN_HEADINGS: usize = 5;

// ---- 入口 ----

/// PDF のバイト列から見出しの表を作る。一時ファイルへ書いてから
/// outline_of_pdf_file を呼ぶ(poppler はどれも入力にファイルの道を要求する)。
///
/// page_count は、最後の見出しより後ろのページまで経路を伸ばすために使う。None なら
/// 最後の見出しのページまでで止める(しおり系統は自分でページ数を知らない。高さ系統は
/// 数えられるので、None でも全ページを覆う)。取り込み側は pdftotext の出力を form feed
/// で割った数を持っているので、それを渡すとよい。
pub fn outline_of_pdf(pdf: &[u8], page_count: Option<u32>) -> Result<DocumentOutline, String> {
    let dir = work_dir()?;
    let source = dir.join("source.pdf");
    let written = std::fs::write(&source, pdf)
        .map_err(|error| format!("一時ファイル {} を書けない: {error}", source.display()));
    let outcome = match written {
        Ok(()) => outline_in_work_dir(&dir, &source, page_count),
        Err(error) => Err(error),
    };
    remove_work_dir(&dir)?;
    outcome
}

/// 既にファイルとして置いてある PDF から見出しの表を作る。
pub fn outline_of_pdf_file(
    path: &Path,
    page_count: Option<u32>,
) -> Result<DocumentOutline, String> {
    let dir = work_dir()?;
    let outcome = outline_in_work_dir(&dir, path, page_count);
    remove_work_dir(&dir)?;
    outcome
}

/// 2 系統の使い分け。しおりを先に試し、駄目なら高さで拾う。
///
/// Err を返すのは「この機械に poppler が無い」ときだけである(導入すれば同じ要求が
/// 通る)。「この PDF には見出しが無い」は誤りではないので、source = Missing と理由を
/// 付けて Ok で返す。
fn outline_in_work_dir(
    dir: &Path,
    pdf: &Path,
    page_count: Option<u32>,
) -> Result<DocumentOutline, String> {
    let mut notes: Vec<String> = Vec::new();
    match bookmark_headings(dir, pdf) {
        Ok((headings, version)) if headings.len() >= MIN_HEADINGS => {
            let pages = attribute_headings_to_pages(&headings, page_count);
            return Ok(DocumentOutline {
                source: OutlineSource::Bookmarks,
                pages,
                reason: format!("しおり {} 件から作った(pdftohtml {version})", headings.len()),
            });
        }
        Ok((headings, _)) => notes.push(format!(
            "しおりは {} 件しか無い(最低 {MIN_HEADINGS} 件)",
            headings.len()
        )),
        Err(error) => notes.push(format!("しおり: {error}")),
    }
    let version = located_tool(Tool::Pdftotext)?.version.as_str();
    let (headings, pages_seen) = font_size_headings(pdf)?;
    let count = page_count.or(pages_seen);
    if headings.len() < MIN_HEADINGS {
        notes.push(format!(
            "語の高さでも節番号つきの見出しは {} 件しか無い(最低 {MIN_HEADINGS} 件)",
            headings.len()
        ));
        return Ok(DocumentOutline {
            source: OutlineSource::Missing,
            pages: Vec::new(),
            reason: notes.join(" / "),
        });
    }
    notes.push(format!("語の高さから {} 件(pdftotext {version})", headings.len()));
    Ok(DocumentOutline {
        source: OutlineSource::FontSize,
        pages: attribute_headings_to_pages(&headings, count),
        reason: notes.join(" / "),
    })
}

// ---- 系統 1: しおり ----

/// pdftohtml にしおりを吐かせて読む。`-f 1 -l 1` で紙面の変換は 1 ページに抑える
/// (しおりは文書全体ぶん出る)。`-c` は 1 ページを 1 つの html にまとめる指定で、
/// これが無いと `-outline.html` が作られない。
fn bookmark_headings(dir: &Path, pdf: &Path) -> Result<(Vec<Heading>, &'static str), String> {
    let command = pdftohtml()?;
    let prefix = dir.join("outline");
    let output = Command::new(&command.command)
        .args(["-f", "1", "-l", "1", "-c"])
        .arg(pdf)
        .arg(&prefix)
        .output()
        .map_err(|error| format!("{} を起動できない: {error}", command.command.display()))?;
    if !output.status.success() {
        return Err(format!(
            "pdftohtml が失敗した({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let path = dir.join("outline-outline.html");
    let html = std::fs::read_to_string(&path)
        .map_err(|_| "この PDF にしおりが無い(-outline.html が作られない)".to_string())?;
    Ok((headings_from_outline_html(&html), command.version.as_str()))
}

/// pdftohtml が吐く `-outline.html` を読む。形は入れ子の `<ul>/<li>` で、各項目が
/// `<a href="<接頭辞>-<物理ページ>.html">題</a>` である。`<ul>` の入れ子の深さが
/// そのまま階層になる。
///
/// 汎用の HTML 解析は書かない(must/0020: 頼んだ形だけを読む)。読むのは
/// `<ul>`・`</ul>`・`<a href=...>...</a>` の 3 つだけで、他の字句は読み飛ばす。
pub fn headings_from_outline_html(html: &str) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut depth: usize = 0;
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<ul>") {
            depth += 1;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("</ul>") {
            depth = depth.saturating_sub(1);
            rest = after;
        } else if rest.starts_with("<a ") {
            let (heading, after) = anchor_heading(rest, depth.max(1));
            if let Some(heading) = heading {
                headings.push(heading);
            }
            rest = after;
        } else {
            rest = &rest[1..];
        }
    }
    headings
}

/// `<a href="...-<ページ>.html">題</a>` を 1 件読み、残りを返す。href もページ番号も
/// 無い錨(`<a name="outline">`)は None になる。
fn anchor_heading(rest: &str, level: usize) -> (Option<Heading>, &str) {
    let Some(close) = rest.find('>') else { return (None, &rest[1..]) };
    let tag = &rest[..close];
    let after_tag = &rest[close + 1..];
    let (text, after) = match after_tag.find("</a>") {
        Some(end) => (&after_tag[..end], &after_tag[end + 4..]),
        None => (after_tag, ""),
    };
    let Some(href) = attribute_text(tag, "href") else { return (None, after) };
    let Some(page) = page_of_href(&href) else { return (None, after) };
    let title = normalize_title(&unescape_html(strip_tags(text)));
    if title.is_empty() {
        return (None, after);
    }
    (Some(Heading { page, level, title }), after)
}

/// `<接頭辞>-<ページ>.html` からページ番号を取る。接頭辞にも `-` が入りうるので、
/// 最後の `-` から読む。
fn page_of_href(href: &str) -> Option<u32> {
    let stem = href.strip_suffix(".html")?;
    let (_, number) = stem.rsplit_once('-')?;
    number.parse().ok()
}

/// タグの中の `name="値"` を取る。
fn attribute_text(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(unescape_html(tag[start..end].to_string()))
}

/// 題の中に混ざる入れ子のタグ(`<i>` など)を落とす。
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// HTML の実体参照を戻す。pdftohtml が出すのは `&amp; &lt; &gt; &quot; &#NN;` である。
pub fn unescape_html(text: String) -> String {
    if !text.contains('&') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix('#')
                .and_then(|number| match number.strip_prefix('x').or(number.strip_prefix('X')) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- 系統 2: 語の高さ ----

/// pdftotext -bbox を走らせ、語の高さから見出しを拾う。返す 2 つ目はページ数
/// (`<page>` の数)で、最後の見出しより後ろのページまで経路を伸ばすのに使う。
fn font_size_headings(pdf: &Path) -> Result<(Vec<Heading>, Option<u32>), String> {
    let tool = located_tool(Tool::Pdftotext)?;
    let output = Command::new(&tool.command)
        .arg("-bbox")
        .arg(pdf)
        .arg("-")
        .output()
        .map_err(|error| format!("{} を起動できない: {error}", tool.command.display()))?;
    if !output.status.success() {
        return Err(format!(
            "pdftotext -bbox が失敗した({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let xml = String::from_utf8_lossy(&output.stdout);
    let pages = pages_from_bbox_xml(&xml);
    let count = u32::try_from(pages.len()).ok();
    Ok((headings_from_pages(&pages), count))
}

/// 1 行(yMin の近い語をまとめたもの)。見出しの判定に要るものだけを持つ。純関数の
/// 試験がこの形を直に組める(外部コマンドを呼ばずに判定を試せる)。
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub y_min: f32,
    pub y_max: f32,
    /// 行の中で最も背の高い語の yMax-yMin。フォントの大きさの代理。
    pub height: f32,
    /// 語を空白 1 つで繋いだもの。
    pub text: String,
    /// 末尾の語と、その手前の語との横の空き。目次の点リーダや右寄せの柱を見分ける。
    pub trailing_gap: f32,
}

/// 1 ページぶんの行。
#[derive(Clone, Debug, PartialEq)]
pub struct TextPage {
    pub height: f32,
    pub lines: Vec<TextLine>,
}

/// pdftotext -bbox の出力を行に畳む。`<page width= height=>` と
/// `<word xMin= yMin= xMax= yMax=>語</word>` だけを読む(must/0020)。
pub fn pages_from_bbox_xml(xml: &str) -> Vec<TextPage> {
    let mut pages = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<page ") {
        rest = &rest[start..];
        let Some(close) = rest.find('>') else { break };
        let height = attribute_number(&rest[..close], "height").unwrap_or(0.0);
        rest = &rest[close + 1..];
        let (body, after) = match rest.find("</page>") {
            Some(end) => (&rest[..end], &rest[end + 7..]),
            None => (rest, ""),
        };
        pages.push(TextPage { height, lines: lines_of_words(body) });
        rest = after;
    }
    pages
}

/// 1 語ぶんの箱。
struct Word {
    x_min: f32,
    x_max: f32,
    y_min: f32,
    y_max: f32,
    text: String,
}

/// `<word ...>` を順に読み、yMin が LINE_TOLERANCE_PT 以内の連なりを 1 行にまとめる。
/// 語の並びは poppler が組版の順で出すので、並べ替えはしない。
fn lines_of_words(body: &str) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<word ") {
        rest = &rest[start..];
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        let (text, after) = match rest.find("</word>") {
            Some(end) => (&rest[..end], &rest[end + 7..]),
            None => (rest, ""),
        };
        rest = after;
        let (Some(x_min), Some(x_max), Some(y_min), Some(y_max)) = (
            attribute_number(tag, "xMin"),
            attribute_number(tag, "xMax"),
            attribute_number(tag, "yMin"),
            attribute_number(tag, "yMax"),
        ) else {
            continue;
        };
        let word =
            Word { x_min, x_max, y_min, y_max, text: unescape_html(text.to_string()) };
        let same_line = current
            .first()
            .is_some_and(|first| (first.y_min - word.y_min).abs() <= LINE_TOLERANCE_PT);
        if !same_line {
            if let Some(line) = line_of(&current) {
                lines.push(line);
            }
            current.clear();
        }
        current.push(word);
    }
    if let Some(line) = line_of(&current) {
        lines.push(line);
    }
    lines
}

fn line_of(words: &[Word]) -> Option<TextLine> {
    let first = words.first()?;
    let mut y_min = first.y_min;
    let mut y_max = first.y_max;
    let mut height: f32 = 0.0;
    let mut text = String::new();
    for word in words {
        y_min = y_min.min(word.y_min);
        y_max = y_max.max(word.y_max);
        height = height.max(word.y_max - word.y_min);
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&word.text);
    }
    let trailing_gap = match words.len() >= 2 {
        true => words[words.len() - 1].x_min - words[words.len() - 2].x_max,
        false => 0.0,
    };
    Some(TextLine { y_min, y_max, height, text: normalize_title(&text), trailing_gap })
}

/// 題の字面をそろえる。改行しない空白(U+00A0)や連なった空白を普通の空白 1 つにして
/// 前後を落とす。2 系統が同じ形の題を返すよう、両方がここを通る(should/0135)。
///
/// 節番号の切り出しにも要る。pcie_40 のしおりは `4.\u{a0}Physical Layer Specification`
/// と、番号と題の間を U+00A0 で繋いでいる。そのままだと先頭の字句が
/// `4.\u{a0}Physical` になり、節番号として読めない。
pub fn normalize_title(text: &str) -> String {
    if !text.chars().any(|ch| ch.is_whitespace() && ch != ' ')
        && !text.contains("  ")
        && text.trim() == text
    {
        return text.to_string();
    }
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// タグの中の `name="数"` を取る。
fn attribute_number(tag: &str, name: &str) -> Option<f32> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    tag[start..end].parse().ok()
}

/// 本文の高さ。文書全体の行の高さを 0.1pt に丸めて数え、「よく出る高さ」
/// (全体の BODY_HEIGHT_SHARE 以上を占めるもの)のうち最も背の高いものを採る。
///
/// 単純な最頻値では駄目である(実測 2026-08-17)。ich9 は表の細かい字が 7.7pt で
/// 26566 行、地の文が 8.6pt で 14915 行あり、最頻値は 7.7pt になる。すると地の文が
/// まるごと「本文より背が高い」側に回り、番号で始まる箇条書き
/// (`1. Advanced Power Management (APM) Wakeup`)や表の枠内の `32 bit` まで見出しに
/// なって、884 ページに 2081 件の見出しが立った。hid_1_11(8.5pt と 9.5pt)と
/// hpet_1_0a(8.1pt と 9.0pt)も同じ形をしている。本文が 2 種類の大きさで組まれて
/// いるのが常態なので、そのどちらよりも背が高いことを見出しの条件にする。
pub fn body_height_of(pages: &[TextPage]) -> f32 {
    let mut counts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    let mut total = 0usize;
    for page in pages {
        for line in &page.lines {
            if line.height <= 0.0 {
                continue;
            }
            *counts.entry((line.height * 10.0).round() as u32).or_insert(0) += 1;
            total += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    let floor = (total as f32 * BODY_HEIGHT_SHARE).ceil() as usize;
    counts
        .into_iter()
        .filter(|(_, count)| *count >= floor.max(1))
        .map(|(key, _)| key as f32 / 10.0)
        .next_back()
        .unwrap_or(0.0)
}

/// 節番号として通る字句か。返すのは階層の深さ。`4` は 1、`4.20` は 2。PCIe は節番号の
/// 末尾にピリオドが付く(`5.5.3.3. L1.2.Exit`)ので、末尾の 1 つは剥がして数える。
/// これを許さないと pcie_40 の 1053 ページが全滅する(実測)。
pub fn section_number_depth(token: &str) -> Option<usize> {
    let core = token.strip_suffix('.').unwrap_or(token);
    if core.is_empty() {
        return None;
    }
    let mut depth = 0;
    for part in core.split('.') {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        depth += 1;
    }
    Some(depth)
}

/// 柱・ノンブルの帯にある行か(ページ上下 MARGIN_BAND_RATIO)。
pub fn in_margin_band(line: &TextLine, page_height: f32) -> bool {
    if page_height <= 0.0 {
        return false;
    }
    let band = page_height * MARGIN_BAND_RATIO;
    line.y_min < band || line.y_max > page_height - band
}

/// 目次の行や柱に見えるか。末尾が裸の数字で、その手前が大きく空いている(点リーダや
/// 右寄せ)ものだけを弾く。
///
/// 空きを見るのは、見出しにも数字で終わるものがあるからである
/// (`5.7.2 VTIO Common Assignment Register 1`)。末尾の数字だけで弾くと、その手の
/// レジスタ名の節を全部落とす。逆に空きを見ないと ACPI の柱
/// (`6.2. Device Configuration Objects 376`)が見出しとして通る。
pub fn looks_like_running_head(text: &str, trailing_gap: f32) -> bool {
    if has_dot_leader(text) {
        return true;
    }
    if trailing_gap <= RUNNING_HEAD_GAP_PT {
        return false;
    }
    match text.rsplit_once(' ') {
        Some((head, tail)) => {
            !head.is_empty() && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// 点リーダ(目次の `.....`)を含むか。ピリオドが DOT_LEADER_RUN 個以上続く箇所を
/// 探す。節番号の `5.5.3.3.` には連続したピリオドが無いので当たらない。
///
/// 要るのは、目次の行が空きではなく点で埋まっていて、末尾の空きを見る規則をすり抜ける
/// からである(実測: uefi_2_9 の目次 p.46-49 が
/// `24 Network Protocols — SNP, PXE, BIS and HTTP Boot ......... 1017` を見出しとして
/// 通し、しおり系統の言う `Table of Contents` と食い違った)。
pub fn has_dot_leader(text: &str) -> bool {
    let mut run = 0usize;
    for byte in text.bytes() {
        run = if byte == b'.' { run + 1 } else { 0 };
        if run >= DOT_LEADER_RUN {
            return true;
        }
    }
    false
}

/// 点リーダとみなすピリオドの連なり。
const DOT_LEADER_RUN: usize = 4;

/// 1 行が見出しの頭かどうか。当たったら (階層の深さ, 題) を返す。純関数。
pub fn heading_of_line(
    line: &TextLine,
    body_height: f32,
    page_height: f32,
) -> Option<(usize, String)> {
    if body_height <= 0.0 || line.height <= body_height * HEADING_HEIGHT_RATIO {
        return None;
    }
    if in_margin_band(line, page_height) {
        return None;
    }
    if line.text.chars().count() >= HEADING_MAX_CHARS {
        return None;
    }
    let (number, tail) = line.text.split_once(' ')?;
    let depth = section_number_depth(number)?;
    if tail.trim().is_empty() {
        return None;
    }
    if looks_like_running_head(&line.text, line.trailing_gap) {
        return None;
    }
    Some((depth, line.text.clone()))
}

/// 折り返した見出しの続きか。見出しと同じ高さで、行間が詰まっていて、それ自身が
/// 節番号で始まらない行を続きとみなす。
///
/// 要るのは、2 行に渡る見出しが実在するからである(xhci の
/// `4.25 USB Virtualization Based Trusted IO Management (USB` が 1 行目で途切れ、
/// `-IO)` が次行に落ちる)。連結しないと題が壊れたまま索引に載る。
pub fn is_wrapped_continuation(heading: &TextLine, next: &TextLine) -> bool {
    if (heading.height - next.height).abs() > WRAP_HEIGHT_TOLERANCE_PT {
        return false;
    }
    let gap = next.y_min - heading.y_min;
    if gap <= 0.0 || gap > heading.height * WRAP_LINE_GAP_RATIO {
        return false;
    }
    match next.text.split_once(' ') {
        Some((number, _)) => section_number_depth(number).is_none(),
        None => section_number_depth(&next.text).is_none(),
    }
}

/// ページの列から見出しを拾う。本文の高さは文書全体から決める(1 ページだけ見ると、
/// 見出しばかりの扉ページで本文の高さを取り違える)。
pub fn headings_from_pages(pages: &[TextPage]) -> Vec<Heading> {
    let body_height = body_height_of(pages);
    let mut headings = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let page_number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let mut cursor = 0;
        while cursor < page.lines.len() {
            let line = &page.lines[cursor];
            let Some((depth, mut title)) = heading_of_line(line, body_height, page.height) else {
                cursor += 1;
                continue;
            };
            let mut last = cursor;
            while last + 1 < page.lines.len()
                && last - cursor < WRAP_MAX_CONTINUATIONS
                && is_wrapped_continuation(&page.lines[last], &page.lines[last + 1])
            {
                last += 1;
                title.push(' ');
                title.push_str(page.lines[last].text.trim());
            }
            headings.push(Heading { page: page_number, level: depth, title });
            cursor = last + 1;
        }
    }
    headings
}

// ---- ページへの割り当て(2 系統が共用する 1 箇所) ----

/// 見出しの列(読む順)をページごとの経路に畳む。
///
/// 割り当ての規則は 1 つだけである: そのページで始まる見出しがあれば最後のものを採り、
/// 無ければ前のページの経路をそのまま引き継ぐ。しおり系統には紙面の座標が無いので、
/// 「紙面を縦にどれだけ占めているか」といった座標に頼る規則は使えない。2 系統が同じ
/// 規則を通ることを優先した(should/0135)。
///
/// 深さは HEADING_PATH_DEPTH で丸める。丸めるのは外側からで、`4.20.1.3` の下にある
/// ページには `["4 ...", "4.20 ..."]` が付く。
pub fn attribute_headings_to_pages(
    headings: &[Heading],
    page_count: Option<u32>,
) -> Vec<PageHeadings> {
    // 階層は飛ぶ(4.20 の次に 5 が来る、1 段目の無い文書で 2 段目から始まる)ので、
    // 積みには段の番号も持たせ、同じか深い段を落としてから積む。添字を段の番号だと
    // みなすと、1 段目の無い文書で 2 件目以降が積み上がってしまう。
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut marks: Vec<PageHeadings> = Vec::new();
    for heading in headings {
        while stack.last().is_some_and(|(level, _)| *level >= heading.level) {
            stack.pop();
        }
        stack.push((heading.level, heading.title.clone()));
        let mut path: Vec<String> =
            stack.iter().map(|(_, title)| title.clone()).collect();
        path.truncate(HEADING_PATH_DEPTH);
        match marks.last_mut() {
            Some(last) if last.page == heading.page => last.path = path,
            _ => marks.push(PageHeadings { page: heading.page, path }),
        }
    }
    let Some(first) = marks.first().map(|entry| entry.page) else { return Vec::new() };
    let last = marks.last().map(|entry| entry.page).unwrap_or(first);
    let end = page_count.unwrap_or(last).max(last);
    let mut pages = Vec::with_capacity((end - first + 1) as usize);
    let mut cursor = 0;
    let mut current: Vec<String> = Vec::new();
    for page in first..=end {
        while cursor < marks.len() && marks[cursor].page == page {
            current = marks[cursor].path.clone();
            cursor += 1;
        }
        pages.push(PageHeadings { page, path: current.clone() });
    }
    pages
}

// ---- 外部コマンドの在り処 ----

/// pdftohtml の実行ファイル。
///
/// 「この機械で poppler がどこにあるか」は rendition::located_tool が既に解いている
/// (PATH には pdftotext だけを symlink し、一式は別の場所に展開してある置き方に耐える
/// ように書いてある)。その答えを使い回す(should/0135)。rendition::Tool にはまだ
/// pdftohtml が無いので、ここでは PATH の走査を自分で書かず、素の名前で 1 回起こして
/// OS(execvp)に引かせ、駄目なら located_tool が見つけた pdftotext の実体の隣を見る。
///
/// 取り込みへ配線するときに rendition::Tool へ Pdftohtml を足せば、この関数は
/// located_tool(Tool::Pdftohtml) の 1 行に畳める。
fn pdftohtml() -> Result<&'static Pdftohtml, String> {
    static FOUND: OnceLock<Pdftohtml> = OnceLock::new();
    if let Some(found) = FOUND.get() {
        return Ok(found);
    }
    let mut failures = Vec::new();
    let mut candidates = vec![PathBuf::from(PDFTOHTML)];
    if let Ok(pdftotext) = located_tool(Tool::Pdftotext) {
        // symlink の先の実体の隣を見たいので canonicalize してから親を取る
        // (rendition::candidate_commands の 3 番目の候補と同じ理由・同じ手順)。
        let real = std::fs::canonicalize(&pdftotext.command)
            .unwrap_or_else(|_| pdftotext.command.clone());
        if let Some(dir) = real.parent() {
            candidates.push(dir.join(PDFTOHTML));
        }
    }
    for candidate in candidates {
        match pdftohtml_version(&candidate) {
            Ok(version) => {
                let found = Pdftohtml { command: candidate, version };
                return Ok(FOUND.get_or_init(|| found));
            }
            Err(error) => failures.push(error),
        }
    }
    Err(format!(
        "しおりの読み出しには {PDFTOHTML} コマンドが必要({})。{POPPLER_INSTALL_HINT}",
        failures.join(" / ")
    ))
}

/// pdftohtml の実行ファイル名。探す側と誤りに出す側、そして unit の PATH に足す側
/// (install::DELEGATES)が同じ字句を見る(must/0023)。
pub const PDFTOHTML: &str = "pdftohtml";

/// 見つかった pdftohtml。rendition::LocatedTool を借りないのは、あれが tool:
/// rendition::Tool を持っており、その列挙に pdftohtml が無いからである。他の道具の
/// 名前を入れて辻褄を合わせると、読んだ人が別の道具だと信じる。
struct Pdftohtml {
    command: PathBuf,
    /// 例 "22.02.0"。どの版で読んだかは reason に載せる。
    version: String,
}

/// 起動確認と版の取得を 1 回の起動で兼ねる(`-v` が stderr へ出す先頭行
/// 「pdftohtml version <版>」を読む)。rendition::probe_version と同じ形だが、
/// あちらは private なので、ここでは pdftohtml の 1 行ぶんだけを書いてある。
fn pdftohtml_version(command: &Path) -> Result<String, String> {
    let probe = Command::new(command)
        .arg("-v")
        .output()
        .map_err(|error| format!("{}: {error}", command.display()))?;
    let stderr = String::from_utf8_lossy(&probe.stderr);
    let first_line = stderr.lines().next().unwrap_or("");
    first_line
        .strip_prefix(&format!("{PDFTOHTML} version "))
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!("{} -v の出力から版を読めない(先頭行: {first_line:?})", command.display())
        })
}

// ---- 作業ディレクトリ ----

/// pdftohtml は出力の接頭辞を要求し、しおり以外にも html と png を吐く。散らからない
/// よう、1 回ごとに専用のディレクトリを作って丸ごと消す。
fn work_dir() -> Result<PathBuf, String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("uniqnode-outline-{}-{serial}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| format!("{} を消せない: {error}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("{} を作れない: {error}", dir.display()))?;
    Ok(dir)
}

fn remove_work_dir(dir: &Path) -> Result<(), String> {
    std::fs::remove_dir_all(dir)
        .map_err(|error| format!("{} を消せない: {error}", dir.display()))
}

// ---- 単体テスト ----

#[cfg(test)]
mod tests {
    use super::*;

    /// 節番号として通る字句。PCIe の末尾ピリオドを通し、年号めいた裸の語や
    /// 版番号めいたものを落とす。
    #[test]
    fn section_numbers_allow_the_trailing_period_pcie_writes() {
        assert_eq!(section_number_depth("4"), Some(1));
        assert_eq!(section_number_depth("4.20"), Some(2));
        assert_eq!(section_number_depth("5.5.3.3."), Some(4));
        assert_eq!(section_number_depth("4.20."), Some(2));
        assert_eq!(section_number_depth("Chapter"), None);
        assert_eq!(section_number_depth("4.a"), None);
        assert_eq!(section_number_depth("4..2"), None);
        assert_eq!(section_number_depth("."), None);
        assert_eq!(section_number_depth(""), None);
    }

    fn line(text: &str, y_min: f32, height: f32) -> TextLine {
        TextLine {
            y_min,
            y_max: y_min + height,
            height,
            text: text.to_string(),
            trailing_gap: 3.0,
        }
    }

    /// 番号つきの見出しを、本文より背が高いという 1 点で拾う。xhci p.334 の実際の値
    /// (見出し 17.19pt / 本文 12.19pt / ページ高 792pt / y=97.65)をそのまま使う。
    #[test]
    fn numbered_line_taller_than_the_body_is_a_heading() {
        let heading = line("4.20 Scratchpad Buffers", 97.65, 17.19);
        assert_eq!(
            heading_of_line(&heading, 12.19, 792.0),
            Some((2, "4.20 Scratchpad Buffers".to_string()))
        );
        // 同じ紙面の本文は、番号で始まっていても高さが足りないので見出しにならない。
        let body = line("4.20 is the section that describes it", 120.19, 12.19);
        assert_eq!(heading_of_line(&body, 12.19, 792.0), None);
        // 背は高いが番号で始まらない行(図の題など)も見出しにしない。
        assert_eq!(heading_of_line(&line("Scratchpad Buffers", 97.65, 17.19), 12.19, 792.0), None);
        // 番号だけで題の無い行(ノンブルの類)も落とす。
        assert_eq!(heading_of_line(&line("4.20", 97.65, 17.19), 12.19, 792.0), None);
    }

    /// 柱の除外。ページ上下 8% の帯にあるものと、点リーダで開いた末尾の数字を落とす。
    /// 数字で終わるだけの本物の見出しは残す。
    #[test]
    fn running_heads_are_dropped_but_headings_ending_in_a_digit_survive() {
        // ACPI の柱: 帯の中にある。
        let top = line("6.2. Device Configuration Objects 376", 20.0, 14.0);
        assert!(in_margin_band(&top, 792.0));
        assert_eq!(heading_of_line(&top, 12.19, 792.0), None);
        // 帯の外に落ちた目次の行でも、末尾の数字の手前が大きく開いていれば落とす。
        let toc = TextLine {
            trailing_gap: 60.0,
            ..line("6.2. Device Configuration 376", 300.0, 14.0)
        };
        assert!(looks_like_running_head(&toc.text, toc.trailing_gap));
        assert_eq!(heading_of_line(&toc, 12.19, 792.0), None);
        // 目次の点リーダは、末尾の空きが無くても落とす(uefi_2_9 の目次 p.46-49)。
        let contents =
            line("24 Network Protocols — SNP, PXE and HTTP Boot ......... 1017", 300.0, 14.0);
        assert!(has_dot_leader(&contents.text));
        assert!(looks_like_running_head(&contents.text, contents.trailing_gap));
        assert_eq!(heading_of_line(&contents, 12.19, 792.0), None);
        // 節番号の連なるピリオドは点リーダではない。
        assert!(!has_dot_leader("5.5.3.3. L1.2.Exit"));
        // xhci p.434 の見出しは数字で終わるが、空きは語間ぶんしかないので残る。
        let real = line("5.7.2 VTIO Common Assignment Register 1", 100.0, 14.0);
        assert!(!looks_like_running_head(&real.text, real.trailing_gap));
        assert_eq!(
            heading_of_line(&real, 12.19, 792.0),
            Some((3, "5.7.2 VTIO Common Assignment Register 1".to_string()))
        );
    }

    /// 2 行に折り返した見出しを連結する。xhci 4.25 の実際の割れ方を使う。
    #[test]
    fn wrapped_headings_are_joined_with_the_next_line_of_the_same_height() {
        let first = line("4.25 USB Virtualization Based Trusted IO Management (USB", 100.0, 17.2);
        let second = line("-IO)", 118.0, 17.2);
        assert!(is_wrapped_continuation(&first, &second));
        // 本文の高さに戻った次行は続きではない。
        assert!(!is_wrapped_continuation(&first, &line("The USB-IO ...", 118.0, 12.2)));
        // 次の見出しは、同じ高さでも節番号で始まるので続きにしない。
        assert!(!is_wrapped_continuation(&first, &line("4.26 Next Section", 118.0, 17.2)));
        // 行が離れていれば続きにしない。
        assert!(!is_wrapped_continuation(&first, &line("-IO)", 300.0, 17.2)));

        let body = line("body", 140.0, 12.2);
        let page = TextPage { height: 792.0, lines: vec![first, second, body] };
        // 本文の高さを最頻値で決めるために、本文の行を厚くしたページを足す。
        let filler = TextPage {
            height: 792.0,
            lines: (0..20).map(|i| line("body text here", 200.0 + i as f32 * 13.0, 12.2)).collect(),
        };
        let headings = headings_from_pages(&[page, filler]);
        assert_eq!(headings.len(), 1);
        assert_eq!(
            headings[0].title,
            "4.25 USB Virtualization Based Trusted IO Management (USB -IO)"
        );
        assert_eq!(headings[0].level, 2);
        assert_eq!(headings[0].page, 1);
    }

    /// 本文の高さは「よく出る高さ」のうち最も背の高いもの。ich9 の実際の比
    /// (表の細字 7.7pt が 26566 行、地の文 8.6pt が 14915 行)を縮めて使う。単純な
    /// 最頻値だと 7.7pt を採り、地の文がまるごと見出し側に回る。
    #[test]
    fn body_height_is_the_tallest_of_the_common_line_heights() {
        let table: Vec<TextLine> =
            (0..266).map(|i| line("cell", 100.0 + i as f32, 7.7)).collect();
        let prose: Vec<TextLine> =
            (0..149).map(|i| line("prose text", 100.0 + i as f32, 8.6)).collect();
        let headings: Vec<TextLine> =
            (0..17).map(|i| line("1.1 Heading", 100.0 + i as f32, 11.7)).collect();
        let pages = [
            TextPage { height: 792.0, lines: table },
            TextPage { height: 792.0, lines: prose },
            TextPage { height: 792.0, lines: headings },
        ];
        assert!((body_height_of(&pages) - 8.6).abs() < 0.05, "{}", body_height_of(&pages));
        assert_eq!(body_height_of(&[]), 0.0);
    }

    /// 改行しない空白を普通の空白にそろえる。そろえないと pcie_40 のしおりの
    /// `4.\u{a0}Physical ...` から節番号を切り出せない。
    #[test]
    fn titles_normalise_non_breaking_space_so_the_number_can_be_split_off() {
        assert_eq!(normalize_title("4.\u{a0}Physical Layer"), "4. Physical Layer");
        assert_eq!(normalize_title("  a   b \n"), "a b");
        assert_eq!(normalize_title("4.20 Scratchpad Buffers"), "4.20 Scratchpad Buffers");
        let normalized = normalize_title("4.\u{a0}Physical Layer");
        let (number, _) = normalized.split_once(' ').unwrap();
        assert_eq!(section_number_depth(number), Some(1));
    }

    /// しおりの html を読む。入れ子の ul がそのまま階層になり、href の末尾が物理ページ
    /// 番号になる。実体参照も戻す。
    #[test]
    fn outline_html_nesting_becomes_the_heading_level() {
        let html = concat!(
            "<body>\n<a name=\"outline\"></a><h1>Document Outline</h1>\n<ul>\n",
            "<li><a href=\"x-76.html\">1 - Introduction</a>\n<ul>\n",
            "<li><a href=\"x-83.html\">1.6 UEFI &amp; Driver Model</a>\n<ul>\n",
            "<li><a href=\"x-84.html\">1.6.2 Legacy Option ROM Issues</a></li>\n",
            "</ul>\n</li>\n</ul>\n</li>\n",
            "<li><a href=\"x-89.html\">2 - Overview</a></li>\n</ul>\n</body>"
        );
        let headings = headings_from_outline_html(html);
        let seen: Vec<(u32, usize, &str)> =
            headings.iter().map(|h| (h.page, h.level, h.title.as_str())).collect();
        assert_eq!(
            seen,
            vec![
                (76, 1, "1 - Introduction"),
                (83, 2, "1.6 UEFI & Driver Model"),
                (84, 3, "1.6.2 Legacy Option ROM Issues"),
                (89, 1, "2 - Overview"),
            ]
        );
    }

    /// ページへの割り当て。見出しの無いページは前を引き継ぎ、深さは 2 段で丸まる。
    #[test]
    fn pages_between_headings_inherit_the_previous_path() {
        let headings = vec![
            Heading { page: 300, level: 1, title: "4 Operational Model".to_string() },
            Heading { page: 334, level: 2, title: "4.20 Scratchpad Buffers".to_string() },
            Heading { page: 336, level: 3, title: "4.20.1 Details".to_string() },
        ];
        let pages = attribute_headings_to_pages(&headings, Some(338));
        assert_eq!(pages.first().map(|p| p.page), Some(300));
        assert_eq!(pages.last().map(|p| p.page), Some(338));
        let path = |page: u32| {
            pages.iter().find(|entry| entry.page == page).map(|entry| entry.path.clone()).unwrap()
        };
        assert_eq!(path(300), vec!["4 Operational Model".to_string()]);
        assert_eq!(
            path(335),
            vec!["4 Operational Model".to_string(), "4.20 Scratchpad Buffers".to_string()]
        );
        // 3 段目は丸められ、2 段目までが残る。
        assert_eq!(
            path(338),
            vec!["4 Operational Model".to_string(), "4.20 Scratchpad Buffers".to_string()]
        );
        assert_eq!(attribute_headings_to_pages(&[], Some(10)), Vec::new());
    }

    /// 同じページに複数の見出しがあるときは最後のものを採る(その節が紙面の残りを
    /// 占めているため)。
    #[test]
    fn the_last_heading_on_a_page_wins() {
        let headings = vec![
            Heading { page: 10, level: 2, title: "1.1 First".to_string() },
            Heading { page: 10, level: 2, title: "1.2 Second".to_string() },
        ];
        let pages = attribute_headings_to_pages(&headings, None);
        assert_eq!(pages, vec![PageHeadings { page: 10, path: vec!["1.2 Second".to_string()] }]);
    }

    /// bbox の xml から行に畳む。xMin/yMin の実際の値(xhci p.334)を使う。節番号が
    /// ぶら下げインデントで x=76、題が x=141 に離れていても、yMin が同じなら 1 行に
    /// なる(素の pdftotext は `4.20` と `Scratchpad Buffers` を別の段落に割る)。
    #[test]
    fn words_on_the_same_baseline_join_across_a_hanging_indent() {
        let xml = concat!(
            "<doc>\n<page width=\"612.000000\" height=\"792.000000\">\n",
            "<word xMin=\"75.98\" yMin=\"97.65\" xMax=\"104.47\" yMax=\"114.83\">4.20</word>\n",
            "<word xMin=\"141.02\" yMin=\"97.65\" xMax=\"214.74\" yMax=\"114.83\">",
            "Scratchpad</word>\n",
            "<word xMin=\"218.16\" yMin=\"97.65\" xMax=\"265.23\" yMax=\"114.83\">Buffers</word>\n",
            "<word xMin=\"141.02\" yMin=\"120.18\" xMax=\"159.09\" yMax=\"132.38\">The</word>\n",
            "</page>\n</doc>\n"
        );
        let pages = pages_from_bbox_xml(xml);
        assert_eq!(pages.len(), 1);
        assert!((pages[0].height - 792.0).abs() < 0.01);
        assert_eq!(pages[0].lines.len(), 2);
        assert_eq!(pages[0].lines[0].text, "4.20 Scratchpad Buffers");
        assert!((pages[0].lines[0].height - 17.18).abs() < 0.01);
        assert_eq!(pages[0].lines[1].text, "The");
        assert!((pages[0].lines[1].height - 12.2).abs() < 0.01);
    }

    /// 実 PDF を通す。外部コマンドが無い環境では黙って飛ばさず、導入手順を示して失敗
    /// する(docs/design/TESTING.md の外部コマンドの規約。飛ばして緑にすると、検証した
    /// のか検証を諦めたのかが結果から区別できなくなる)。
    ///
    /// 使う PDF は 3 ページの試験用紙面で、しおりも節番号つきの見出しも持たない。
    /// つまりこれは「2 系統とも駄目だったときに、空と理由を返す」道の試験である。
    #[test]
    fn a_pdf_without_any_heading_reports_why_instead_of_guessing() {
        const THREE_PAGES: &[u8] = include_bytes!("../tests/assets/three_pages.pdf");
        let outline = match outline_of_pdf(THREE_PAGES, Some(3)) {
            Ok(outline) => outline,
            Err(error) => panic!("見出しの取り出しには poppler が必要: {error}"),
        };
        assert_eq!(outline.source, OutlineSource::Missing, "reason: {}", outline.reason);
        assert!(outline.pages.is_empty());
        assert!(outline.reason.contains("しおり"), "理由が空: {}", outline.reason);
        assert!(outline.reason.contains("高さ"), "理由が空: {}", outline.reason);
        assert_eq!(outline.path_for_page(1), Vec::<String>::new().as_slice());
    }
}
