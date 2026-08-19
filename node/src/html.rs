//! HTML の紙面から本文テキストを取り出す層(依存ゼロ)。
//!
//! 取り込み(node/src/ingest.rs)は原本の blob と検索用のチャンクを分けて持つ。PDF では
//! 抽出を pdftotext に委ねたが、HTML には標準で前提にできる委譲先が無い(lynx も pandoc も
//! 入っているとは限らない)ので、ここで自前に落とす。狙いは忠実な再現ではなく、検索と
//! 引用に耐える本文である。引き受けるのは次の 3 つだけである。
//!
//! - 人が読まない部分を落とす(script・style・svg・noscript・template の中身、コメント)
//! - 見出しと箇条書きの構造を Markdown の記法へ写す。こうするとチャンカー
//!   (ingest::chunk_markdown)がそのまま使えて、見出しの入れ子が breadcrumbs になる
//! - 実体参照を文字へ戻す
//!
//! 落ちるものも書いておく(黙って捨てない。must/0022 の同型)。リンク先の URL、画像
//! (alt も含む)、表の罫線と列の対応、装飾(強調・色)は残らない。表は行を改行、
//! セルを空白 1 つに落とすだけである。原本の HTML は blob として無傷で残るので、
//! 落ちたものが要るときはそちらを読む。
//!
//! ナビゲーションや脚のような、どの紙面にも出る定型もそのまま本文に入る。紙面ごとに
//! 「ここからが本文」を当てる規則は書けない(生成器ごとに違う)ので、当てずに全部入れる。
//! 定型は短く、チャンクの大半は本文になる。

/// 中身を丸ごと落とす要素。閉じ札まで読み飛ばす。
const OPAQUE: [&str; 5] = ["script", "style", "svg", "noscript", "template"];

/// 前後で段落を切る要素(開き札でも閉じ札でも切る)。
const BLOCK: [&str; 19] = [
    "p", "div", "section", "article", "header", "footer", "main", "nav", "aside", "ul", "ol",
    "dl", "table", "tbody", "thead", "tr", "blockquote", "form", "figure",
];

/// HTML の本文を Markdown 寄りの素文にする。
pub fn to_text(html: &str) -> String {
    let mut out = Out::new();
    let mut rest = html;
    // 開いている見出しの階数(1..=6)。閉じ札で行を切るために覚えておく。
    let mut heading = None;
    let mut in_pre = false;
    while let Some(position) = rest.find('<') {
        let (text, tail) = rest.split_at(position);
        out.push_text(text, in_pre);
        let Some(tag) = Tag::parse(tail) else {
            // '<' で始まるが札として読めない(比較演算子など)。本文の文字として扱う。
            out.push_text("<", in_pre);
            rest = &tail[1..];
            continue;
        };
        rest = tag.rest;
        if OPAQUE.contains(&tag.name.as_str()) && !tag.closing {
            // 自己閉じの opaque 要素(<svg/> など)は中身を持たない。
            if !tag.self_closing {
                rest = skip_element(rest, &tag.name);
            }
            continue;
        }
        match tag.name.as_str() {
            "title" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = match tag.name.as_str() {
                    "title" => 1,
                    name => name[1..].parse().unwrap_or(1),
                };
                if tag.closing {
                    heading = None;
                    out.break_block();
                } else {
                    out.break_block();
                    out.push_raw(&"#".repeat(level));
                    out.push_raw(" ");
                    heading = Some(level);
                }
            }
            // フェンスは前を 1 行だけ空ける。閉じ札の側で段落を空けると、コードの
            // 末尾と閉じフェンスのあいだに空行が入ってしまう。
            "pre" if tag.closing => {
                in_pre = false;
                out.break_line();
                out.push_raw("```");
                out.break_block();
            }
            "pre" => {
                out.break_block();
                out.push_raw("```");
                out.break_line();
                in_pre = true;
            }
            "li" if !tag.closing => {
                out.break_line();
                out.push_raw("- ");
            }
            "br" => out.break_line(),
            "td" | "th" if tag.closing => out.push_text(" ", false),
            name if BLOCK.contains(&name) => out.break_block(),
            _ => {}
        }
    }
    out.push_text(rest, in_pre);
    let _ = heading;
    out.finish()
}

/// 閉じ札 </name> まで読み飛ばす。閉じ札が無ければ末尾まで(壊れた HTML でも落ちない)。
fn skip_element<'a>(mut rest: &'a str, name: &str) -> &'a str {
    loop {
        let Some(position) = rest.find('<') else { return "" };
        let tail = &rest[position..];
        match Tag::parse(tail) {
            Some(tag) if tag.closing && tag.name == name => return tag.rest,
            Some(tag) => rest = tag.rest,
            None => rest = &tail[1..],
        }
    }
}

/// 読めた札。rest は札の直後。
struct Tag<'a> {
    name: String,
    closing: bool,
    self_closing: bool,
    rest: &'a str,
}

impl<'a> Tag<'a> {
    /// '<' から始まる文字列を札として読む。読めなければ None(本文の '<' として扱う)。
    fn parse(text: &'a str) -> Option<Tag<'a>> {
        let body = text.strip_prefix('<')?;
        if let Some(comment) = body.strip_prefix("!--") {
            let rest = match comment.find("-->") {
                Some(end) => &comment[end + 3..],
                None => "",
            };
            return Some(Tag { name: String::new(), closing: false, self_closing: true, rest });
        }
        if body.starts_with('!') || body.starts_with('?') {
            // <!DOCTYPE …> や処理命令。名前は持たない。
            let rest = match body.find('>') {
                Some(end) => &body[end + 1..],
                None => "",
            };
            return Some(Tag { name: String::new(), closing: false, self_closing: true, rest });
        }
        let (closing, body) = match body.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, body),
        };
        let name: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            return None;
        }
        // 属性値の中の '>' で切らないよう、引用符の内側を数える。
        let after_name = &body[name.len()..];
        let mut quote = None;
        for (offset, c) in after_name.char_indices() {
            match (quote, c) {
                (None, '"') | (None, '\'') => quote = Some(c),
                (Some(open), c) if c == open => quote = None,
                (None, '>') => {
                    let self_closing = after_name[..offset].trim_end().ends_with('/');
                    let rest = &after_name[offset + 1..];
                    return Some(Tag { name, closing, self_closing, rest });
                }
                _ => {}
            }
        }
        // 閉じない札。末尾まで札とみなす(本文として出すと壊れた属性が混ざる)。
        Some(Tag { name, closing, self_closing: false, rest: "" })
    }
}

/// 素文の組み立て。空白の潰しと改行の重複除去をここ 1 箇所で持つ(should/0135)。
struct Out {
    text: String,
    /// まだ出していない改行の数(0・1・2)。本文が続いたときにだけ実際に出す。
    pending_newlines: u8,
    /// 直前が空白で、次の本文の前に空白 1 つを出す必要がある。
    pending_space: bool,
}

impl Out {
    fn new() -> Out {
        Out { text: String::new(), pending_newlines: 0, pending_space: false }
    }

    /// 本文。pre の中では空白と改行をそのまま保つ。
    fn push_text(&mut self, text: &str, verbatim: bool) {
        if text.is_empty() {
            return;
        }
        let decoded = decode_entities(text);
        if verbatim {
            self.flush();
            self.text.push_str(&decoded);
            return;
        }
        for c in decoded.chars() {
            if c.is_whitespace() {
                if !self.text.is_empty() {
                    self.pending_space = true;
                }
                continue;
            }
            self.flush();
            self.text.push(c);
        }
    }

    /// 札から出す記号(実体参照の復号も空白の潰しもしない)。
    fn push_raw(&mut self, text: &str) {
        self.flush();
        self.text.push_str(text);
    }

    fn break_line(&mut self) {
        self.pending_space = false;
        self.pending_newlines = self.pending_newlines.max(1);
    }

    fn break_block(&mut self) {
        self.pending_space = false;
        self.pending_newlines = 2;
    }

    /// 溜めた空白・改行を実際に書く。先頭では何も書かない(頭の空行を作らないため)。
    fn flush(&mut self) {
        if self.text.is_empty() {
            self.pending_newlines = 0;
            self.pending_space = false;
            return;
        }
        for _ in 0..self.pending_newlines {
            self.text.push('\n');
        }
        if self.pending_newlines == 0 && self.pending_space {
            self.text.push(' ');
        }
        self.pending_newlines = 0;
        self.pending_space = false;
    }

    fn finish(mut self) -> String {
        while self.text.ends_with(['\n', ' ']) {
            self.text.pop();
        }
        self.text.push('\n');
        self.text
    }
}

/// 実体参照を文字へ戻す。名前つきは実際に紙面へ出るものだけを持ち、知らない名前は
/// そのまま残す(勝手に消さない)。数値参照は 10 進と 16 進の両方を受ける。
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('&') {
        out.push_str(&rest[..position]);
        let tail = &rest[position..];
        // 参照の名前になれるのは英数字と '#' だけである。そこで切らずに次の ';' まで
        // 探すと、素の '&' が後ろの本物の参照を飲み込む(「& ラベル&amp;」が 1 つの
        // 名前に見える)。長さの上限も置く(実在の名前はここまで長くない)。
        let name: String =
            tail[1..].chars().take(12).take_while(|c| c.is_ascii_alphanumeric() || *c == '#').collect();
        let terminated = tail[1 + name.len()..].starts_with(';');
        match decode_one(&name).filter(|_| terminated) {
            Some(c) => {
                out.push(c);
                rest = &tail[name.len() + 2..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_one(name: &str) -> Option<char> {
    if let Some(digits) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        return u32::from_str_radix(digits, 16).ok().and_then(char::from_u32);
    }
    if let Some(digits) = name.strip_prefix('#') {
        return digits.parse().ok().and_then(char::from_u32);
    }
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "hellip" => Some('…'),
        "lsquo" => Some('\u{2018}'),
        "rsquo" => Some('\u{2019}'),
        "ldquo" => Some('\u{201c}'),
        "rdquo" => Some('\u{201d}'),
        "times" => Some('×'),
        "middot" => Some('·'),
        "copy" => Some('©'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 人が読まない部分は本文に出ない(script・style・コメント・DOCTYPE)。
    #[test]
    fn opaque_elements_and_comments_do_not_reach_the_text() {
        let text = to_text(
            "<!DOCTYPE html><html><head><style>body { color: red }</style>\
             <script>var a = 1 < 2;</script></head><body><!-- 覚え書き -->\
             <p>本文</p></body></html>",
        );
        assert_eq!(text, "本文\n");
    }

    /// 見出しは Markdown の記法に写り、チャンカーが入れ子を breadcrumbs にできる。
    #[test]
    fn headings_become_markdown_headings_in_order() {
        let text = to_text("<h1>大</h1><p>あ</p><h2>中</h2><p>い</p>");
        assert_eq!(text, "# 大\n\nあ\n\n## 中\n\nい\n");
        let chunks = crate::ingest::chunk_markdown(&text);
        assert_eq!(chunks[0].breadcrumbs, vec!["大".to_string()]);
        assert_eq!(chunks[1].breadcrumbs, vec!["大".to_string(), "中".to_string()]);
    }

    /// title は紙面の題として先頭の見出しになる(head の中にあっても拾う)。
    #[test]
    fn the_title_element_becomes_the_first_heading() {
        let text = to_text("<html><head><title>題</title></head><body><p>本文</p></body></html>");
        assert_eq!(text, "# 題\n\n本文\n");
    }

    /// 箇条書きは行頭の「- 」になる。
    #[test]
    fn list_items_become_dashes() {
        assert_eq!(to_text("<ul><li>あ</li><li>い</li></ul>"), "- あ\n- い\n");
    }

    /// pre の中は空白と改行を保ち、コードフェンスで囲う(チャンカーがフェンス内で
    /// 段落を切らないため)。
    #[test]
    fn preformatted_text_keeps_its_shape_inside_a_fence() {
        let text = to_text("<pre>fn main() {\n    let a = 1;\n}</pre>");
        assert_eq!(text, "```\nfn main() {\n    let a = 1;\n}\n```\n");
    }

    /// 属性値の中の '>' で札を切らない。
    #[test]
    fn a_greater_than_inside_an_attribute_does_not_end_the_tag() {
        assert_eq!(to_text("<a title=\"a > b\" href=\"/x\">見出し</a>"), "見出し\n");
    }

    /// 札として読めない '<' は本文の文字として残る。
    #[test]
    fn a_bare_less_than_stays_in_the_text() {
        assert_eq!(to_text("<p>1 < 2 なので</p>"), "1 < 2 なので\n");
    }

    /// 閉じ札の無い script は末尾まで落とす(壊れた HTML でも本文に漏らさない)。
    #[test]
    fn an_unclosed_opaque_element_swallows_the_rest() {
        assert_eq!(to_text("<p>前</p><script>var a = 1;"), "前\n");
    }

    /// 実体参照は文字へ戻る。知らない名前はそのまま残す。
    #[test]
    fn entities_are_decoded_and_unknown_names_are_kept() {
        assert_eq!(decode_entities("a &amp; b &#39;c&#39; &#x27;d&#x27;"), "a & b 'c' 'd'");
        assert_eq!(decode_entities("&nbsp;x&hellip;"), " x…");
        assert_eq!(decode_entities("&unknown; &amp"), "&unknown; &amp");
        // '&' の直後が多バイト文字でも、先読みの窓が文字の途中で切れない。
        assert_eq!(decode_entities("& ラベル&amp;付き"), "& ラベル&付き");
    }

    /// 空白は 1 つに潰れ、段落の切れ目は空行 1 つになる(頭と尻に空行を作らない)。
    #[test]
    fn whitespace_collapses_and_blank_lines_do_not_pile_up() {
        let text = to_text("\n\n  <div>  あ   い  </div>\n\n<div><div><p>う</p></div></div>  ");
        assert_eq!(text, "あ い\n\nう\n");
    }

    /// 表はセルを空白で継ぎ、行で切る(罫線と列の対応は残らない)。
    #[test]
    fn table_cells_are_joined_by_spaces_and_rows_break() {
        let text = to_text("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>");
        assert_eq!(text, "a b\n\nc\n");
    }
}
