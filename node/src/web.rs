//! 取ってきた HTML を「外部へ 1 つも取りに行かない自足した 1 枚」に書き換える層(依存ゼロ)。
//!
//! URL からの取り込み(docs/plan/RAG.md 項目 13)は、取りに行く側(curl の呼び出し・API・CLI)と、
//! 取れた HTML を書き換える純関数のここに分かれる。ここは入力の文字列と転送後の最終 URL だけ
//! を受け取り、ブラウザが外部へ要求を出しうるものを落とした HTML と、落とした数を返す。
//! 判断に迷う場面の規則は 1 つ: ブラウザが外部へ要求を出しうるものは残さない。
//!
//! 落とすもの: script(中身ごと)、href を持つ link、iframe/frame/embed/object(中身ごと)、
//! 外部を指す img/source/track(と svg の image・外部を指す use)、`on*` 属性、base、
//! `meta http-equiv=refresh`、srcset/poster/background/ping/formaction/manifest 属性、
//! 外部を指す src 属性(video・audio・input など)、CSS の `url(...)` と `@import`。
//! 残すもの: 本文・見出し・a href(相対は絶対にする。リンクは読み込まないので依存ではない)、
//! `data:` を指す img、インラインの style(url と @import を除く)、noscript の中身、
//! DOCTYPE・コメント・pre の中身。
//!
//! 先頭には出所の meta(uniqnode-source)と CSP の meta を入れる。CSP は落とし損ねた参照が
//! あってもブラウザが取りに行かない二重の守りで、原本を返すときの `sandbox allow-scripts`
//! (docs/design/RENDITION.md)とは別の層である。
//!
//! 札の読み方は html.rs の Tag::parse と skip_element をそのまま使う(同じ判断を二重に
//! 実装しない。should/0135)。壊れた HTML でもパニックしない。読めない `<` は本文として残る。

use crate::html::{self, Tag};

/// 書き換えの結果。dropped は落としたものの数(黙って捨てない。must/0022 の同型)。
#[derive(Debug)]
pub struct SelfContained {
    pub html: String,
    pub dropped: Dropped,
}

/// 落としたものの種類ごとの数。
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Dropped {
    /// script 要素。
    pub scripts: usize,
    /// rel に stylesheet を含む link。
    pub stylesheets: usize,
    /// 画像と媒体: img・source・track・svg の image、src/srcset/poster/background 属性。
    pub images: usize,
    /// iframe・frame・embed・object。
    pub frames: usize,
    /// CSS の url(...) と @import、`as="font"` の link。
    pub fonts: usize,
    /// `on*` 属性。
    pub handlers: usize,
    /// それ以外: base、meta refresh、stylesheet でも font でもない link、ping/formaction/
    /// manifest 属性、外部を指す svg の use。
    pub others: usize,
}

/// 出所を記す meta の name。取りに行く側が読むときはこの名前で探す。
pub const SOURCE_META_NAME: &str = "uniqnode-source";
/// 紙面に埋める CSP。外部は何も許さず、インラインの style と data: の画像だけ許す。
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:";

/// html を、base_url(転送後の最終 URL)を基準に自足した 1 枚へ書き換える。
pub fn self_contain(html: &str, base_url: &str) -> SelfContained {
    let mut writer = Writer {
        out: String::with_capacity(html.len()),
        dropped: Dropped::default(),
        base: base_url,
        head_open_end: None,
        html_open_end: None,
        doctype_end: None,
    };
    let mut rest = html;
    while let Some(position) = rest.find('<') {
        let (text, tail) = rest.split_at(position);
        writer.out.push_str(text);
        let Some(tag) = Tag::parse(tail) else {
            // '<' で始まるが札として読めない(比較演算子など)。本文の文字として残す。
            writer.out.push('<');
            rest = &tail[1..];
            continue;
        };
        let span = &tail[..tail.len() - tag.rest.len()];
        rest = writer.tag(span, &tag, tag.rest);
    }
    writer.out.push_str(rest);
    writer.insert_markers();
    SelfContained { html: writer.out, dropped: writer.dropped }
}

/// 書き出し先と、途中で覚えておく位置。
struct Writer<'a> {
    out: String,
    dropped: Dropped,
    base: &'a str,
    /// 最初の `<head>` の直後(出力側の位置)。ここに出所と CSP の meta を入れる。
    head_open_end: Option<usize>,
    /// 最初の `<html>` の直後。head が無いときはここに head を作る。
    html_open_end: Option<usize>,
    /// 先頭の `<!DOCTYPE>` の直後。html も無いときは DOCTYPE の後ろに head を作る
    /// (DOCTYPE より前に置くと quirks mode になる)。
    doctype_end: Option<usize>,
}

/// 読んだ属性 1 つ。value は原文のまま(実体参照を戻していない)。
struct Attr<'a> {
    name: &'a str,
    value: Option<String>,
    /// 原文の引用符。None は引用符無し。書き換えた値は必ず二重引用符で出す。
    quote: Option<char>,
}

impl<'a> Attr<'a> {
    fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    /// 実体参照を戻し、前後の空白を落とした値。
    fn decoded(&self) -> Option<String> {
        self.value.as_deref().map(|v| html::decode_entities(v).trim().to_string())
    }
}

impl<'a> Writer<'a> {
    /// 札 1 つを処理し、続きを返す。span は札の原文、rest は札の直後。
    fn tag<'h>(&mut self, span: &'h str, tag: &Tag<'h>, rest: &'h str) -> &'h str {
        if tag.name.is_empty() {
            // コメント・DOCTYPE・処理命令。そのまま。
            if self.doctype_end.is_none() && span.starts_with("<!") && !span.starts_with("<!--") {
                self.doctype_end = Some(self.out.len() + span.len());
            }
            self.out.push_str(span);
            return rest;
        }
        if tag.closing {
            self.out.push_str(span);
            return rest;
        }
        let name = tag.name.as_str();
        // 中身ごと落とす要素。閉じ札まで読み飛ばす(自己閉じなら中身は無い。html.rs と同じ読み)。
        let skip = |rest| if tag.self_closing { rest } else { html::skip_element(rest, name) };
        match name {
            "script" => {
                self.dropped.scripts += 1;
                return skip(rest);
            }
            "iframe" | "object" => {
                self.dropped.frames += 1;
                return skip(rest);
            }
            "frame" | "embed" => {
                self.dropped.frames += 1;
                return rest;
            }
            "base" => {
                self.dropped.others += 1;
                return rest;
            }
            _ => {}
        }

        let raw_name = &span[1..1 + name.len()];
        let mut inside = span[1 + name.len()..].strip_suffix('>').unwrap_or(&span[1 + name.len()..]);
        if tag.self_closing {
            inside = inside.trim_end().strip_suffix('/').unwrap_or(inside);
        }
        let mut attrs = parse_attributes(inside);

        // 属性を見て要素ごと落とすもの。
        match name {
            "link" if attrs.iter().any(|a| a.is("href")) => {
                let rel = attribute(&attrs, "rel").unwrap_or_default();
                let as_font = attribute(&attrs, "as").is_some_and(|v| v.eq_ignore_ascii_case("font"));
                if rel.split_whitespace().any(|r| r.eq_ignore_ascii_case("stylesheet")) {
                    self.dropped.stylesheets += 1;
                } else if as_font {
                    self.dropped.fonts += 1;
                } else {
                    self.dropped.others += 1;
                }
                return rest;
            }
            "meta" => {
                let http_equiv = attribute(&attrs, "http-equiv").unwrap_or_default();
                if http_equiv.eq_ignore_ascii_case("refresh") {
                    self.dropped.others += 1;
                    return rest;
                }
                // 自分が入れる 2 つの meta は、入力にあっても先頭で作り直す(置き換えであって
                // 落とすのではないので数えない。再度通しても変わらないための規則)。
                if http_equiv.eq_ignore_ascii_case("content-security-policy")
                    || attribute(&attrs, "name").is_some_and(|n| n.eq_ignore_ascii_case(SOURCE_META_NAME))
                {
                    return rest;
                }
            }
            "img" | "image" if fetches(&attrs, &["src", "href", "xlink:href"]) => {
                self.dropped.images += 1;
                if let Some(alt) = attribute(&attrs, "alt") {
                    // alt の文字は本文として残す。属性値の '<' は本文では札になるので逃がす。
                    self.out.push_str(&alt.replace('<', "&lt;"));
                }
                return rest;
            }
            "source" | "track" if fetches(&attrs, &["src", "srcset"]) => {
                self.dropped.images += 1;
                return rest;
            }
            "use" if fetches(&attrs, &["href", "xlink:href"]) => {
                self.dropped.others += 1;
                return rest;
            }
            _ => {}
        }

        // 残す要素の属性を篩う。
        let mut changed = false;
        attrs.retain(|a| {
            let lower = a.name.to_ascii_lowercase();
            let drop = if lower.starts_with("on") {
                self.dropped.handlers += 1;
                true
            } else if ["srcset", "poster", "background"].contains(&lower.as_str()) {
                self.dropped.images += 1;
                true
            } else if ["ping", "formaction", "manifest"].contains(&lower.as_str()) {
                self.dropped.others += 1;
                true
            } else if lower == "src" && is_external(a.decoded().as_deref()) {
                self.dropped.images += 1;
                true
            } else {
                false
            };
            changed |= drop;
            !drop
        });
        for attr in attrs.iter_mut() {
            let Some(decoded) = attr.decoded() else { continue };
            let rewritten = if attr.is("style") {
                let (css, fonts) = strip_css(&decoded);
                self.dropped.fonts += fonts;
                (fonts > 0).then_some(css)
            } else if (name == "a" || name == "area") && attr.is("href") {
                // ページ内の # だけの参照は残す(絶対にすると保存した紙面から離れてしまう)。
                if decoded.is_empty() || decoded.starts_with('#') {
                    None
                } else {
                    Some(resolve_url(self.base, &decoded)).filter(|r| *r != decoded)
                }
            } else if name == "form" && attr.is("action") {
                Some(resolve_url(self.base, &decoded)).filter(|r| *r != decoded)
            } else {
                None
            };
            if let Some(value) = rewritten {
                attr.value = Some(encode_attribute(&value));
                attr.quote = Some('"');
                changed = true;
            }
        }
        if changed {
            self.out.push_str(&serialize(raw_name, &attrs, tag.self_closing));
        } else {
            self.out.push_str(span);
        }
        match name {
            "head" if self.head_open_end.is_none() => self.head_open_end = Some(self.out.len()),
            "html" if self.html_open_end.is_none() => self.html_open_end = Some(self.out.len()),
            "style" if !tag.self_closing => {
                // 中身は CSS。url(...) と @import を落として残す。
                let after = html::skip_element(rest, "style");
                let consumed = &rest[..rest.len() - after.len()];
                let (css, closing) = split_closing(consumed, "style");
                let (stripped, fonts) = strip_css(css);
                self.dropped.fonts += fonts;
                self.out.push_str(&stripped);
                self.out.push_str(closing);
                return after;
            }
            _ => {}
        }
        rest
    }

    /// 出所と CSP の meta を head の先頭に入れる。head が無ければ html の直後に head を作り、
    /// それも無ければ先頭(DOCTYPE があればその後ろ)に作る。
    fn insert_markers(&mut self) {
        let markers = format!(
            "<meta name=\"{SOURCE_META_NAME}\" content=\"{}\">\
             <meta http-equiv=\"Content-Security-Policy\" content=\"{CONTENT_SECURITY_POLICY}\">",
            encode_attribute(self.base)
        );
        match (self.head_open_end, self.html_open_end) {
            (Some(at), _) => self.out.insert_str(at, &markers),
            (None, Some(at)) => self.out.insert_str(at, &format!("<head>{markers}</head>")),
            (None, None) => {
                self.out.insert_str(self.doctype_end.unwrap_or(0), &format!("<head>{markers}</head>"))
            }
        }
    }
}

/// 名前の属性の値(実体参照を戻し、前後の空白を落としたもの)。
fn attribute(attrs: &[Attr], name: &str) -> Option<String> {
    attrs.iter().find(|a| a.is(name)).and_then(Attr::decoded)
}

/// 名前のどれかの属性が外部を指しているか。
fn fetches(attrs: &[Attr], names: &[&str]) -> bool {
    names.iter().any(|name| attrs.iter().any(|a| a.is(name) && is_external(a.decoded().as_deref())))
}

/// 値が外部を指すか。空・data:・ページ内の # は取りに行かない。
fn is_external(value: Option<&str>) -> bool {
    match value {
        None | Some("") => false,
        Some(v) => !v.starts_with('#') && !v.get(..5).is_some_and(|head| head.eq_ignore_ascii_case("data:")),
    }
}

/// 札の名前の後ろ('>' と自己閉じの '/' を除く)を属性の列として読む。
/// 引用符付き・無し・値無しを受け、読めない断片は 1 文字ずつ飛ばす(パニックしない)。
fn parse_attributes(mut text: &str) -> Vec<Attr<'_>> {
    let mut attrs = Vec::new();
    loop {
        text = text.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if text.is_empty() {
            return attrs;
        }
        let name_len =
            text.find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>').unwrap_or(text.len());
        if name_len == 0 {
            // '=' や '>' が名前の位置にある。1 文字飛ばす。
            let width = text.chars().next().map_or(1, char::len_utf8);
            text = &text[width..];
            continue;
        }
        let (name, tail) = text.split_at(name_len);
        let after_name = tail.trim_start();
        let Some(after_eq) = after_name.strip_prefix('=') else {
            attrs.push(Attr { name, value: None, quote: None });
            text = tail;
            continue;
        };
        let value_text = after_eq.trim_start();
        let (value, quote, remaining) = match value_text.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let body = &value_text[1..];
                match body.find(q) {
                    Some(end) => (&body[..end], Some(q), &body[end + 1..]),
                    None => (body, Some(q), ""),
                }
            }
            _ => {
                let end = value_text.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(value_text.len());
                (&value_text[..end], None, &value_text[end..])
            }
        };
        attrs.push(Attr { name, value: Some(value.to_string()), quote });
        text = remaining;
    }
}

/// 属性を書き換えた札を組み立て直す。名前と引用符は原文のまま、書き換えた値は二重引用符。
fn serialize(raw_name: &str, attrs: &[Attr], self_closing: bool) -> String {
    let mut out = String::from("<");
    out.push_str(raw_name);
    for attr in attrs {
        out.push(' ');
        out.push_str(attr.name);
        if let Some(value) = &attr.value {
            out.push('=');
            match attr.quote {
                Some(q) => {
                    out.push(q);
                    out.push_str(value);
                    out.push(q);
                }
                None => out.push_str(value),
            }
        }
    }
    if self_closing {
        out.push_str(" /");
    }
    out.push('>');
    out
}

/// 二重引用符の属性値として安全な形にする。
fn encode_attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// skip_element が読み飛ばした範囲を、中身と閉じ札に分ける。閉じ札が無ければ全部が中身。
fn split_closing<'a>(consumed: &'a str, name: &str) -> (&'a str, &'a str) {
    let is_closing = |text: &str| {
        Tag::parse(text).is_some_and(|tag| tag.closing && tag.name == name && tag.rest.is_empty())
    };
    match consumed.rfind("</") {
        Some(at) if is_closing(&consumed[at..]) => consumed.split_at(at),
        _ => (consumed, ""),
    }
}

/// CSS から `@import …;` と外部を指す `url(...)` を落とす。url は `none` に置き換える
/// (background や list-style では有効な値、@font-face の src では無効な宣言として捨てられる。
/// 空の url() はブラウザによって紙面自身を取りに行くので使わない)。`url(data:…)` は残す。
/// 返す数は落とした数。
fn strip_css(css: &str) -> (String, usize) {
    // ASCII の小文字化は長さを変えないので、位置は原文と共有できる。
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut dropped = 0;
    let mut at = 0;
    while at < css.len() {
        let import = lower[at..].find("@import").map(|p| p + at);
        let url = lower[at..].find("url(").map(|p| p + at);
        let Some(next) = [import, url].into_iter().flatten().min() else {
            out.push_str(&css[at..]);
            break;
        };
        out.push_str(&css[at..next]);
        if Some(next) == import {
            at = lower[next..].find(';').map_or(css.len(), |p| next + p + 1);
            dropped += 1;
            continue;
        }
        let end = lower[next..].find(')').map_or(css.len(), |p| next + p + 1);
        let inner = css[next + 4..end].trim_end_matches(')').trim().trim_matches(['"', '\'']).trim();
        if !is_external(Some(inner)) {
            out.push_str(&css[next..end]);
        } else {
            out.push_str("none");
            dropped += 1;
        }
        at = end;
    }
    (out, dropped)
}

/// 相対 URL を base に対して絶対にする(RFC 3986 §5.2 の範囲で、スキーム相対・絶対パス・
/// 相対パス・?・# を扱う)。reference がスキームを持てばそのまま(点の段だけ畳む)。
pub fn resolve_url(base: &str, reference: &str) -> String {
    let r = Uri::split(reference);
    let b = Uri::split(base);
    let (scheme, authority, path, query) = if r.scheme.is_some() {
        (r.scheme, r.authority, remove_dot_segments(r.path), r.query)
    } else if r.authority.is_some() {
        (b.scheme, r.authority, remove_dot_segments(r.path), r.query)
    } else if r.path.is_empty() {
        (b.scheme, b.authority, b.path.to_string(), r.query.or(b.query))
    } else if r.path.starts_with('/') {
        (b.scheme, b.authority, remove_dot_segments(r.path), r.query)
    } else {
        (b.scheme, b.authority, remove_dot_segments(&merge_paths(&b, r.path)), r.query)
    };
    let mut out = String::new();
    if let Some(scheme) = scheme {
        out.push_str(scheme);
        out.push(':');
    }
    if let Some(authority) = authority {
        out.push_str("//");
        out.push_str(authority);
    }
    out.push_str(&path);
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    if let Some(fragment) = r.fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// URI の 5 つの部品(RFC 3986 §3)。
struct Uri<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

impl<'a> Uri<'a> {
    fn split(text: &'a str) -> Uri<'a> {
        let (text, fragment) = match text.find('#') {
            Some(at) => (&text[..at], Some(&text[at + 1..])),
            None => (text, None),
        };
        let (text, query) = match text.find('?') {
            Some(at) => (&text[..at], Some(&text[at + 1..])),
            None => (text, None),
        };
        let scheme_len = text.find(':').filter(|&at| {
            let candidate = &text[..at];
            let mut chars = candidate.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        });
        let (scheme, text) = match scheme_len {
            Some(len) => (Some(&text[..len]), &text[len + 1..]),
            None => (None, text),
        };
        let (authority, path) = match text.strip_prefix("//") {
            Some(after) => {
                let end = after.find('/').unwrap_or(after.len());
                (Some(&after[..end]), &after[end..])
            }
            None => (None, text),
        };
        Uri { scheme, authority, path, query, fragment }
    }
}

/// RFC 3986 §5.2.3。base の最後の段を reference の道で置き換える。
fn merge_paths(base: &Uri, reference: &str) -> String {
    if base.authority.is_some() && base.path.is_empty() {
        return format!("/{reference}");
    }
    match base.path.rfind('/') {
        Some(at) => format!("{}{reference}", &base.path[..=at]),
        None => reference.to_string(),
    }
}

/// RFC 3986 §5.2.4。"." と ".." の段を畳む。
fn remove_dot_segments(path: &str) -> String {
    let mut input = path;
    let mut output = String::with_capacity(path.len());
    let pop = |output: &mut String| match output.rfind('/') {
        Some(at) => output.truncate(at),
        None => output.clear(),
    };
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../").or_else(|| input.strip_prefix("./")) {
            input = rest;
        } else if input.starts_with("/./") {
            // "/./" を "/" に置き換える: 先頭の "/." を落とす。
            input = &input[2..];
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") {
            input = &input[3..];
            pop(&mut output);
        } else if input == "/.." {
            input = "/";
            pop(&mut output);
        } else if input == "." || input == ".." {
            input = "";
        } else {
            let start = usize::from(input.starts_with('/'));
            let end = input[start..].find('/').map_or(input.len(), |at| at + start);
            output.push_str(&input[..end]);
            input = &input[end..];
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.org/dir/page.html";

    /// 出所と CSP の meta(head の先頭に入るもの)。
    fn markers() -> String {
        format!(
            "<meta name=\"uniqnode-source\" content=\"{BASE}\">\
             <meta http-equiv=\"Content-Security-Policy\" content=\"{CONTENT_SECURITY_POLICY}\">"
        )
    }

    /// 出力から meta 2 つを除いたもの(本文側の規則を見るテスト用)。
    fn body_of(html: &str) -> String {
        self_contain(html, BASE).html.replacen(&markers(), "", 1)
    }

    /// (a) script・外部 link・iframe・img は落ち、Dropped がそれぞれを 1 つずつ数える。
    #[test]
    fn scripts_external_links_frames_and_images_are_dropped_and_counted() {
        let html = "<html><head><link rel=\"stylesheet\" href=\"/a.css\">\
                    <SCRIPT src=\"/a.js\"></SCRIPT><script>var a = 1 < 2;</script></head>\
                    <body><iframe src=\"https://x/\"><p>中</p></iframe>\
                    <img src=\"/a.png\"><embed src=\"/a.swf\"><object data=\"/a\">代替</object>\
                    <link rel=\"icon\" href=\"/i.ico\"><p>本文</p></body></html>";
        let result = self_contain(html, BASE);
        assert_eq!(
            result.html,
            format!("<html><head>{}</head><body><p>本文</p></body></html>", markers())
        );
        assert_eq!(
            result.dropped,
            Dropped { scripts: 2, stylesheets: 1, images: 1, frames: 3, fonts: 0, handlers: 0, others: 1 }
        );
    }

    /// (b) 外部を指す img は消えるが、alt の文字は本文として残る。
    #[test]
    fn the_alt_text_of_a_dropped_image_stays_as_text() {
        let result = self_contain("<p><img src=\"/a.png\" alt=\"図 1: a &lt; b\">の説明</p>", BASE);
        assert!(result.html.ends_with("<p>図 1: a &lt; bの説明</p>"), "{}", result.html);
        assert_eq!(result.dropped.images, 1);
    }

    /// (c) on* 属性だけが落ち、他の属性は原文のまま残る。
    #[test]
    fn only_event_handler_attributes_are_removed() {
        let result = self_contain(
            "<div class=\"k\" onclick=\"f()\" id=x ONMOUSEOVER='g()' data-one=\"1\">t</div>",
            BASE,
        );
        assert!(result.html.ends_with("<div class=\"k\" id=x data-one=\"1\">t</div>"), "{}", result.html);
        assert_eq!(result.dropped.handlers, 2);
    }

    /// (d) style 要素と style 属性の url() と @import は落ち、色などの宣言は残る。
    #[test]
    fn css_urls_and_imports_are_removed_but_declarations_stay() {
        let html = "<style>@import url(\"/x.css\");\n\
                    body { color: red; background: URL(/bg.png) no-repeat }\n\
                    @font-face { src: url('/f.woff2') format('woff2') }</style>\
                    <p style=\"color: blue; background-image: url(a.png)\">t</p>";
        let result = self_contain(html, BASE);
        assert!(
            result.html.contains(
                "<style>\nbody { color: red; background: none no-repeat }\n\
                 @font-face { src: none format('woff2') }</style>"
            ),
            "{}",
            result.html
        );
        assert!(result.html.contains("<p style=\"color: blue; background-image: none\">t</p>"), "{}", result.html);
        assert_eq!(result.dropped.fonts, 4);
    }

    /// (e) 相対な a href と form action は絶対になり、絶対なものとページ内の # は触らない。
    #[test]
    fn relative_links_and_form_actions_become_absolute() {
        let html = "<a href=\"../up.html\">u</a><a href=\"#s\">s</a>\
                    <a href=\"https://o/?a=1&amp;b=2\">o</a><area href=\"g?y\">\
                    <form action=\"post\"><input name=q></form>";
        let result = self_contain(html, BASE);
        assert!(
            result.html.ends_with(
                "<a href=\"https://example.org/up.html\">u</a><a href=\"#s\">s</a>\
                 <a href=\"https://o/?a=1&amp;b=2\">o</a><area href=\"https://example.org/dir/g?y\">\
                 <form action=\"https://example.org/dir/post\"><input name=q></form>"
            ),
            "{}",
            result.html
        );
        assert_eq!(result.dropped, Dropped::default());
    }

    /// (e) resolve_url は RFC 3986 §5.4.1 の正常系の例どおりに解く。
    #[test]
    fn resolve_url_follows_the_rfc_3986_examples() {
        let base = "http://a/b/c/d;p?q";
        for (reference, expected) in [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("../g", "http://a/b/g"),
            ("../../g", "http://a/g"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g?y#s", "http://a/b/c/g?y#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../..", "http://a/"),
            ("https://h/x", "https://h/x"),
        ] {
            assert_eq!(resolve_url(base, reference), expected, "reference {reference:?}");
        }
        // 末尾スラッシュの有無で最後の段の扱いが変わる。
        assert_eq!(resolve_url("http://a/b/c/", "g"), "http://a/b/c/g");
        assert_eq!(resolve_url("http://a/b/c", "g"), "http://a/b/g");
        assert_eq!(resolve_url("http://a", "g"), "http://a/g");
    }

    /// (f) 出所と CSP の meta は head の先頭に入る。head が無ければ html の直後に head を作り、
    /// それも無ければ先頭(DOCTYPE の後ろ)に作る。
    #[test]
    fn the_source_and_csp_metas_go_first_in_head_creating_it_if_needed() {
        let m = markers();
        assert_eq!(
            self_contain("<html><head><title>t</title></head><body></body></html>", BASE).html,
            format!("<html><head>{m}<title>t</title></head><body></body></html>")
        );
        assert_eq!(
            self_contain("<html lang=ja><body>b</body></html>", BASE).html,
            format!("<html lang=ja><head>{m}</head><body>b</body></html>")
        );
        assert_eq!(self_contain("<p>b</p>", BASE).html, format!("<head>{m}</head><p>b</p>"));
        assert_eq!(
            self_contain("<!DOCTYPE html>\n<p>b</p>", BASE).html,
            format!("<!DOCTYPE html><head>{m}</head>\n<p>b</p>")
        );
    }

    /// (g) data: URI の img は残る(ブラウザは取りに行かない)。base と meta refresh は落ちる。
    #[test]
    fn data_uri_images_stay_while_base_and_refresh_go() {
        let html = "<head><base href=\"/\"><meta http-equiv=\"Refresh\" content=\"0; url=/x\">\
                    <meta charset=utf-8></head><img src=\"data:image/png;base64,AAAA\" alt=a>";
        let result = self_contain(html, BASE);
        assert!(
            result.html.ends_with("<meta charset=utf-8></head><img src=\"data:image/png;base64,AAAA\" alt=a>"),
            "{}",
            result.html
        );
        assert_eq!(result.dropped, Dropped { others: 2, ..Dropped::default() });
    }

    /// 媒体の外部参照は属性ごとに落ち、noscript と pre の中身は残る。
    #[test]
    fn media_sources_are_stripped_and_noscript_and_pre_are_kept() {
        let html = "<video src=\"/v.mp4\" poster=\"/p.jpg\" controls><source src=\"/v.webm\">\
                    <track src=\"/t.vtt\"></video><noscript><p>無効</p></noscript>\
                    <pre>  a &lt; b\n  <img src=x alt=y></pre>\
                    <a href=\"/x\" ping=\"/p\">l</a><body background=\"/b.png\">";
        let result = self_contain(html, BASE);
        assert!(
            result.html.ends_with(
                "<video controls></video><noscript><p>無効</p></noscript>\
                 <pre>  a &lt; b\n  y</pre><a href=\"https://example.org/x\">l</a><body>"
            ),
            "{}",
            result.html
        );
        assert_eq!(result.dropped, Dropped { images: 6, others: 1, ..Dropped::default() });
    }

    /// (h) 出力を再度 self_contain しても変わらない(冪等)。
    #[test]
    fn the_output_is_a_fixed_point() {
        let html = "<!DOCTYPE html><html><head><meta charset=utf-8><style>a{background:url(x)}</style>\
                    <script>x()</script></head><body onload=f()><img src=a alt=\"図\">\
                    <a href=\"../r?a=1&b=2\">r</a><p style=\"color:red;cursor:url(c),auto\">1 < 2</p>\
                    </body></html>";
        let once = self_contain(html, BASE);
        let twice = self_contain(&once.html, BASE);
        assert_eq!(twice.html, once.html);
        assert_eq!(twice.dropped, Dropped::default());
        assert_eq!(body_of(&once.html), body_of(html));
    }

    /// (i) 壊れた入力(閉じない札、'<' だけ、空、閉じない属性値)でパニックしない。
    #[test]
    fn broken_input_does_not_panic() {
        for html in [
            "",
            "<",
            "<<<>>>",
            "<p",
            "<a href=\"/x",
            "<img src=/x alt",
            "<script>never closed",
            "<style>body{url(",
            "<iframe><p>x",
            "<!-- open",
            "<p>1 < 2</p>",
            "< p>",
            "<a =\"x\" href=/y>l</a>",
            "</head></html>",
            "<style>@import",
        ] {
            let result = self_contain(html, BASE);
            assert!(result.html.contains(SOURCE_META_NAME), "{html:?} -> {}", result.html);
            let again = self_contain(&result.html, BASE);
            assert_eq!(again.html, result.html, "{html:?}");
        }
        // 読めない '<' は本文の文字として残る。
        assert!(self_contain("<p>1 < 2</p>", BASE).html.ends_with("<p>1 < 2</p>"));
        // base も壊れていてよい。
        assert_eq!(resolve_url("", "g"), "g");
        assert_eq!(resolve_url("nonsense", "../g"), "g");
    }
}
