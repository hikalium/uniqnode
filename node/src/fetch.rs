//! URL からの取り込み(INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の「URL からの
//! 取り込み」節)。取りに行く道具は curl への外部プロセス委譲である: 依存クレートを持たない
//! この木では TLS を自前で話せないので、pdftotext と同じ枠(Command で直接起動、シェルは
//! 経由しない。must/0009)で curl に頼み、版を doc_rev.meta.fetcher に記録する。
//!
//! 取れたものは種別を見て既存の道へ流す。HTML は node/src/web.rs の self_contain で外部への
//! 依存を落とした 1 枚にしてから blob にし、PDF は pdftotext の道へ、素文はそのまま。
//! ここにあるのは「取る・見分ける・名前を決める・出所を組む」で、書き込みは
//! crate::ingest::ingest_document がファイルからの取り込みと共用する(should/0135)。
//!
//! ロックの規律は呼び手が守る: curl と pdftotext を待つあいだストアのロックを持たない
//! (node/src/sync.rs・node/src/rendition.rs と同じ)。この module はストアに触らない。

use crate::c1::Value;
use crate::ingest::{Chunk, DocumentInput, PdfExtractor};
use crate::web::Dropped;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// 取得の期限の既定(秒)。
pub const DEFAULT_MAX_SECONDS: u64 = 30;
/// 取得の大きさの上限の既定(バイト)。serve が受ける本文の上限(node/src/http.rs)と同じ。
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// 転送(3xx)を追う回数の上限。
const MAX_REDIRECTS: &str = "10";

/// 相手に名乗る User-Agent。
const USER_AGENT: &str = concat!("uniqnode/", env!("CARGO_PKG_VERSION"));

/// 文書名の長さの上限(文字)。超えたら切って末尾に短いハッシュを付ける。
const MAX_NAME_CHARS: usize = 100;
/// 切ったときに残す先頭の長さ(文字)。
const TRUNCATED_NAME_CHARS: usize = 80;

/// curl が見つからないときに示す導入手順。
const CURL_INSTALL_HINT: &str =
    "導入例: apt-get install curl(Debian/Ubuntu)。sudo なしなら apt-get download curl と \
     dpkg -x で ~/opt/curl/ へ展開し、PATH の通ったディレクトリへ symlink を置く";

/// 取得の上限。
#[derive(Clone, Copy, Debug)]
pub struct FetchLimits {
    pub max_seconds: u64,
    pub max_bytes: u64,
}

impl Default for FetchLimits {
    fn default() -> FetchLimits {
        FetchLimits {
            max_seconds: DEFAULT_MAX_SECONDS,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// 取り込めなかった理由。誰が直せるかで分け、HTTP の状態符号はここ 1 箇所が決める
/// (node/src/rendition.rs の RenditionError と同じ形。CLI は文言だけを使う)。
#[derive(Debug)]
pub enum FetchError {
    /// 400: 要求の誤り(URL が不正、名前が空)。
    BadRequest(String),
    /// 415: 取れたが取り込める種別ではない(画像など、UTF-8 でない素文)。
    UnsupportedMedia(String),
    /// 502: 向こうから取れない(繋がらない・期限切れ・2xx でない・上限超え)。
    Upstream(String),
    /// 503: 道具が無い(curl・pdftotext)。導入すれば同じ要求が通る。
    ToolMissing(String),
    /// 500: 取れたのにこちらで処理できない(pdftotext の失敗・一時ファイル)。
    Internal(String),
}

impl FetchError {
    pub fn status(&self) -> u16 {
        match self {
            FetchError::BadRequest(_) => 400,
            FetchError::UnsupportedMedia(_) => 415,
            FetchError::Upstream(_) => 502,
            FetchError::ToolMissing(_) => 503,
            FetchError::Internal(_) => 500,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::BadRequest(m)
            | FetchError::UnsupportedMedia(m)
            | FetchError::Upstream(m)
            | FetchError::ToolMissing(m)
            | FetchError::Internal(m) => f.write_str(m),
        }
    }
}

// ---- URL の検査 ----

/// 取りに行ける URL か。http と https だけを受け、file: や ftp: などは curl に渡す前に断る
/// (curl 側でも --proto =http,https で断る。二重なのは、こちらの断りが理由を言うため)。
pub fn validate_url(url: &str) -> Result<(), String> {
    if url
        .chars()
        .any(|c| c.is_ascii_control() || c.is_whitespace())
    {
        return Err(format!("{url:?}: URL に空白や制御文字が入っている"));
    }
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))
    else {
        return Err(format!(
            "{url}: 取りに行けるのは http と https だけ(file や ftp などは断る)"
        ));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.rsplit('@').next().unwrap_or("").is_empty() {
        return Err(format!("{url}: ホストが無い"));
    }
    Ok(())
}

// ---- curl への委譲 ----

/// curl への外部プロセス委譲。版は locate が起動確認を兼ねて一度だけ取得し、以後の取得で
/// 使い回す(crate::ingest::PdfExtractor と同型)。
pub struct Curl {
    command: PathBuf,
    /// doc_rev.meta.fetcher に書く文字列(例 "curl 7.81.0")。
    pub fetcher: String,
}

impl Curl {
    /// PATH の curl を見つけて版を確かめる。見つからないときは導入手順を示して失敗する
    /// (黙って空を取り込まない。must/0022)。
    pub fn locate() -> Result<Curl, FetchError> {
        let command = PathBuf::from("curl");
        let probe = Command::new(&command)
            .arg("--version")
            .output()
            .map_err(|error| {
                FetchError::ToolMissing(format!(
                    "URL の取り込みには curl コマンドが必要({}: {error})。{CURL_INSTALL_HINT}",
                    command.display()
                ))
            })?;
        // 版は --version の先頭行「curl 7.81.0 (x86_64-pc-linux-gnu) libcurl/…」の先頭 2 語。
        let stdout = String::from_utf8_lossy(&probe.stdout);
        let first_line = stdout.lines().next().unwrap_or("");
        let mut words = first_line.split_whitespace();
        let fetcher = match (words.next(), words.next()) {
            (Some("curl"), Some(version)) => format!("curl {version}"),
            _ => {
                return Err(FetchError::Internal(format!(
                    "{} --version の出力から版を読めない(先頭行: {first_line:?})",
                    command.display()
                )))
            }
        };
        Ok(Curl { command, fetcher })
    }

    /// URL を 1 つ取る。本文は一時ファイルへ落とし(標準出力は --write-out の 3 行に
    /// 使う)、読んだら消す。HTTP の状態が 2xx でなければ理由を言って失敗する。
    pub fn fetch(&self, url: &str, limits: &FetchLimits) -> Result<Fetched, FetchError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp =
            std::env::temp_dir().join(format!("uniqnode-fetch-{}-{serial}", std::process::id()));
        let output = Command::new(&self.command)
            .args([
                "--silent",
                "--show-error",
                "--location",
                "--max-redirs",
                MAX_REDIRECTS,
            ])
            // file: や他のスキームは、最初の URL も転送先も断る。
            .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
            .arg("--max-time")
            .arg(limits.max_seconds.to_string())
            .arg("--max-filesize")
            .arg(limits.max_bytes.to_string())
            .args(["--user-agent", USER_AGENT])
            .arg("--output")
            .arg(&temp)
            .args([
                "--write-out",
                "%{content_type}\n%{url_effective}\n%{http_code}\n",
            ])
            .arg("--")
            .arg(url)
            .output();
        // 本文は失敗のときも書かれていることがある(404 の紙面など)。読んでから消し、
        // 消せなかったことは黙らない(must/0022)。
        let body = std::fs::read(&temp);
        let removed = match std::fs::remove_file(&temp) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(FetchError::Internal(format!(
                "一時ファイル {} を消せない: {error}",
                temp.display()
            ))),
        };
        let output = output.map_err(|error| {
            FetchError::ToolMissing(format!(
                "curl を起動できない({}: {error})。{CURL_INSTALL_HINT}",
                self.command.display()
            ))
        })?;
        removed?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let reason = if stderr.is_empty() {
                format!("curl が {} で終わった", output.status)
            } else {
                stderr
            };
            return Err(FetchError::Upstream(format!("{url} を取れない: {reason}")));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let (Some(content_type), Some(final_url), Some(http_code)) =
            (lines.next(), lines.next(), lines.next())
        else {
            return Err(FetchError::Internal(format!(
                "curl の --write-out の出力を読めない: {stdout:?}"
            )));
        };
        let status: u16 = http_code.trim().parse().map_err(|_| {
            FetchError::Internal(format!("curl の http_code を読めない: {http_code:?}"))
        })?;
        if !(200..300).contains(&status) {
            return Err(FetchError::Upstream(format!(
                "{url} は HTTP {status} を返した(転送後の URL: {final_url})"
            )));
        }
        let bytes = body.map_err(|error| {
            FetchError::Internal(format!(
                "curl が落とした本文 {} を読めない: {error}",
                temp.display()
            ))
        })?;
        // 古い curl は転送の途中で上限を見ないことがあるので、こちらでも数える。
        if bytes.len() as u64 > limits.max_bytes {
            return Err(FetchError::Upstream(format!(
                "{url} の本文が上限 {} バイトを超えた({} バイト)",
                limits.max_bytes,
                bytes.len()
            )));
        }
        let content_type = content_type.trim();
        Ok(Fetched {
            bytes,
            content_type: (!content_type.is_empty()).then(|| content_type.to_string()),
            final_url: final_url.trim().to_string(),
            fetcher: self.fetcher.clone(),
        })
    }
}

/// プロセスで一度だけ curl を見つけて版を取る。見つからない失敗は覚えず、次の要求で
/// 引き直す(実行中に導入されれば以後の要求は通る。node/src/api.rs の pdf_extractor と
/// 同じ扱い)。
pub fn curl() -> Result<&'static Curl, FetchError> {
    static CURL: OnceLock<Curl> = OnceLock::new();
    if let Some(curl) = CURL.get() {
        return Ok(curl);
    }
    let located = Curl::locate()?;
    Ok(CURL.get_or_init(|| located))
}

/// 取れたもの。
#[derive(Debug)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    /// 相手が名乗った Content-Type(名乗らなければ None)。
    pub content_type: Option<String>,
    /// 転送(3xx)を追った後の URL。
    pub final_url: String,
    /// 道具の名前と版(例 "curl 7.81.0")。
    pub fetcher: String,
}

/// URL を検査してから curl で取る。
pub fn fetch_url(url: &str, limits: &FetchLimits) -> Result<Fetched, FetchError> {
    validate_url(url).map_err(FetchError::BadRequest)?;
    curl()?.fetch(url, limits)
}

// ---- 種別の判定 ----

/// Content-Type の主要部(";" より前を小文字化)。
fn primary_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Content-Type の主要部と media の対応(拡張子の表 crate::ingest::media_for_extension の
/// URL 側)。
pub fn media_for_content_type(primary: &str) -> Option<&'static str> {
    match primary {
        "text/html" | "application/xhtml+xml" => Some("html"),
        "application/pdf" => Some("pdf"),
        "text/plain" => Some("text"),
        "text/markdown" => Some("markdown"),
        _ => None,
    }
}

/// 取れたものの種別。中身が PDF なら PDF(ヘッダが text/plain と言っていても `%PDF-` は
/// 嘘をつかない)。それ以外は相手の名乗りに従い、名乗りが無い・octet-stream のときだけ
/// 中身の頭で決める。頭の見方は写しの恒等レシピと同じ 1 箇所
/// (crate::rendition::identity_content_type。should/0135)。取り込める種別でなければ理由を
/// 言って断る(画像などを黙って素文にしない)。
pub fn media_of(content_type: Option<&str>, bytes: &[u8]) -> Result<&'static str, String> {
    let sniffed = primary_type(crate::rendition::identity_content_type(bytes));
    if sniffed == "application/pdf" {
        return Ok("pdf");
    }
    let declared = content_type
        .map(primary_type)
        .filter(|declared| !declared.is_empty() && declared != "application/octet-stream");
    match declared {
        Some(declared) => media_for_content_type(&declared).ok_or_else(|| {
            format!(
                "Content-Type {declared} は取り込める種別ではない(text/html・application/pdf・\
                 text/plain・text/markdown だけ)"
            )
        }),
        None => media_for_content_type(&sniffed).ok_or_else(|| {
            "Content-Type が無く、中身も HTML・PDF・UTF-8 の素文のどれでもない".to_string()
        }),
    }
}

// ---- 文書名 ----

/// 末尾のパス区分の拡張子が取り込み対象のもの(.pdf/.html/.md など)なら落とす。判定は
/// ファイルからの取り込みと同じ表(crate::ingest::media_for_extension。should/0135)。
fn strip_known_extension(path: &str) -> &str {
    let last_slash = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let segment = &path[last_slash..];
    match segment.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty()
                && crate::ingest::media_for_extension(&extension.to_ascii_lowercase())
                    .is_some() =>
        {
            &path[..last_slash + stem.len()]
        }
        _ => path,
    }
}

/// URL から文書名を導く。同じ URL は同じ名前になる(再取得が上書きになる)。ホスト
/// (小文字)とパスと問い合わせ(? 以降)を、英数字と `.` `-` 以外を `_` に潰した 1 語に
/// する。スキームと断片(# 以降)は落とし、取り込み対象の拡張子は残さない。長すぎれば
/// 先頭を残して末尾に短いハッシュを付ける。
pub fn document_name_for_url(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let without_fragment = without_scheme.split('#').next().unwrap_or("");
    let (authority, rest) = match without_fragment.find(['/', '?']) {
        Some(cut) => (&without_fragment[..cut], &without_fragment[cut..]),
        None => (without_fragment, ""),
    };
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let mut raw = format!("{host}{}", strip_known_extension(path));
    if let Some(query) = query {
        raw.push('?');
        raw.push_str(query);
    }
    let mut safe = String::new();
    let mut last_was_separator = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() || character == '.' || character == '-' {
            safe.push(character);
            last_was_separator = false;
        } else if !last_was_separator {
            safe.push('_');
            last_was_separator = true;
        }
    }
    let safe = safe.trim_matches(|c| c == '_' || c == '.');
    let safe = if safe.is_empty() { "document" } else { safe };
    if safe.chars().count() <= MAX_NAME_CHARS {
        return safe.to_string();
    }
    let head: String = safe.chars().take(TRUNCATED_NAME_CHARS).collect();
    let digest = crate::sha2::hex(&crate::sha2::sha256(raw.as_bytes()));
    format!("{}-{}", head.trim_end_matches(['_', '.']), &digest[..8])
}

// ---- 取れたものを取り込みの形にする ----

/// 取り込みの要求。name が無ければ URL から導く。
pub struct FetchRequest<'a> {
    pub url: &'a str,
    pub name: Option<&'a str>,
    pub limits: FetchLimits,
}

/// PDF から取った本文と、取った道具の名前と版。
pub struct PdfText {
    pub text: String,
    pub extractor: String,
}

/// PDF のバイト列を本文にする(pdftotext を見つけた呼び手が、その抽出器で呼ぶ)。API と CLI
/// は pdftotext の見つけ方だけが違い、抽出と誤りの言い方はここで共用する(should/0135)。
pub fn pdf_text_with(extractor: &PdfExtractor, pdf: &[u8]) -> Result<PdfText, FetchError> {
    let text = extractor
        .extract(pdf)
        .map_err(|error| FetchError::Internal(format!("取れた PDF を読めない: {error}")))?;
    Ok(PdfText {
        text,
        extractor: extractor.extractor.clone(),
    })
}

/// 取れて、取り込みの形になった文書。ingest_document へ渡す DocumentInput は input() が組む。
pub struct FetchedDocument {
    pub name: String,
    pub media: &'static str,
    /// blob になるバイト列(HTML は自足化した後の紙面、PDF はそのまま、素文はそのまま)。
    pub source: Vec<u8>,
    pub chunks: Vec<Chunk>,
    pub extractor: Option<String>,
    /// doc_rev.meta に足す出所(source_url・final_url・fetched_at・fetcher・content_type・
    /// HTML なら dropped)。
    pub extra_meta: Vec<(String, Value)>,
    pub source_url: String,
    pub final_url: String,
    pub fetcher: String,
    pub content_type: Option<String>,
    /// HTML のとき、自足化で落としたものの数。
    pub dropped: Option<Dropped>,
    /// PDF の節見出しについて outline が言ったこと(呼び手が記録に残す)。
    pub outline_reason: Option<String>,
}

impl FetchedDocument {
    /// ingest_document への入力。
    pub fn input<'a>(&'a self, collection: &'a str) -> DocumentInput<'a> {
        DocumentInput {
            collection,
            name: &self.name,
            source: &self.source,
            media: self.media,
            chunks: &self.chunks,
            extractor: self.extractor.as_deref(),
            extra_meta: &self.extra_meta,
        }
    }
}

/// 落としたものの数の JSON(doc_rev.meta.dropped と応答の dropped が同じ形を見る)。
pub fn dropped_value(dropped: &Dropped) -> Value {
    let mut map = std::collections::BTreeMap::new();
    let mut put = |key: &str, count: usize| {
        map.insert(key.to_string(), Value::Integer(count as i64));
    };
    put("scripts", dropped.scripts);
    put("stylesheets", dropped.stylesheets);
    put("images", dropped.images);
    put("frames", dropped.frames);
    put("fonts", dropped.fonts);
    put("handlers", dropped.handlers);
    put("others", dropped.others);
    Value::Object(map)
}

/// 相手が text/html と名乗った紙面を、原本を返すときも HTML と名乗れる頭にする。`<html>` も
/// DOCTYPE も無い断片(self_contain の出力は `<head>` で始まる)は、写しの恒等レシピの
/// Content-Type 判定(頭が `<!doctype html` か `<html` のときだけ text/html。
/// crate::rendition::identity_content_type)で text/plain になり、ブラウザに札のまま出る。
/// 判定そのものは同じ 1 箇所に問い(should/0135)、HTML と名乗れなければ DOCTYPE を前置する。
fn with_html_head(html: String) -> String {
    if crate::rendition::identity_content_type(html.as_bytes()).starts_with("text/html") {
        return html;
    }
    format!("<!DOCTYPE html>\n{html}")
}

/// URL を取って取り込みの形にする(ストアには触らない。書き込みは呼び手がロックを取って
/// crate::ingest::ingest_document を呼ぶ)。extract_pdf は PDF だったときだけ呼ばれる
/// (pdftotext の見つけ方は API と CLI で違うので呼び手が渡す。pdf_text_with を使う)。
pub fn fetch_document(
    request: &FetchRequest,
    extract_pdf: &mut dyn FnMut(&[u8]) -> Result<PdfText, FetchError>,
) -> Result<FetchedDocument, FetchError> {
    let name = match request.name {
        None => document_name_for_url(request.url),
        Some(given) if !given.is_empty() && !given.starts_with('/') => given.to_string(),
        Some(_) => {
            return Err(FetchError::BadRequest(
                "name は空でなく / で始まらない文字列(省けば URL から導く)".to_string(),
            ))
        }
    };
    let fetched = fetch_url(request.url, &request.limits)?;
    let media = media_of(fetched.content_type.as_deref(), &fetched.bytes).map_err(|reason| {
        FetchError::UnsupportedMedia(format!("{}: {reason}", fetched.final_url))
    })?;
    let fetched_at = crate::clock::unix_now();
    let utf8 = |bytes: Vec<u8>| {
        String::from_utf8(bytes).map_err(|_| {
            FetchError::UnsupportedMedia(format!(
                "{}: UTF-8 でない({media} として取り込めない)",
                fetched.final_url
            ))
        })
    };
    let mut extractor = None;
    let mut dropped = None;
    let (source, text): (Vec<u8>, String) = match media {
        "pdf" => {
            let pdf = extract_pdf(&fetched.bytes)?;
            extractor = Some(pdf.extractor);
            (fetched.bytes, pdf.text)
        }
        "html" => {
            // 外部への依存を落として自足した 1 枚にしてから blob にする。相対リンクは
            // 転送後の URL を基準に絶対になる(node/src/web.rs)。
            let page = utf8(fetched.bytes)?;
            let contained = crate::web::self_contain(&page, &fetched.final_url);
            dropped = Some(contained.dropped);
            let html = with_html_head(contained.html);
            (html.clone().into_bytes(), html)
        }
        _ => {
            let text = utf8(fetched.bytes)?;
            (text.clone().into_bytes(), text)
        }
    };
    let (chunks, outline_reason) =
        crate::ingest::chunk_for_media_with_source(media, &text, &source);
    let mut extra_meta = vec![
        (
            "source_url".to_string(),
            Value::Text(request.url.to_string()),
        ),
        (
            "final_url".to_string(),
            Value::Text(fetched.final_url.clone()),
        ),
        ("fetched_at".to_string(), Value::Integer(fetched_at)),
        ("fetcher".to_string(), Value::Text(fetched.fetcher.clone())),
    ];
    if let Some(content_type) = &fetched.content_type {
        extra_meta.push((
            "content_type".to_string(),
            Value::Text(content_type.clone()),
        ));
    }
    if let Some(dropped) = &dropped {
        extra_meta.push(("dropped".to_string(), dropped_value(dropped)));
    }
    Ok(FetchedDocument {
        name,
        media,
        source,
        chunks,
        extractor,
        extra_meta,
        source_url: request.url.to_string(),
        final_url: fetched.final_url,
        fetcher: fetched.fetcher,
        content_type: fetched.content_type,
        dropped,
        outline_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// http と https だけを受ける。file: や ftp: は curl に渡す前に断る(期待値は
    /// リテラル。should/0137: 「取りに行けるのは」の文言を落とすと赤になる)。
    #[test]
    fn only_http_and_https_urls_are_accepted() {
        assert!(validate_url("http://example.test/a").is_ok());
        assert!(validate_url("HTTPS://Example.test").is_ok());
        let error = validate_url("file:///etc/hostname").expect_err("file は断る");
        assert!(error.contains("http と https だけ"), "{error}");
        assert!(validate_url("ftp://example.test/x").is_err());
        assert!(
            validate_url("example.test/x").is_err(),
            "スキームの無い URL は受けない"
        );
        let error = validate_url("http:///path").expect_err("ホストが無い");
        assert!(error.contains("ホストが無い"), "{error}");
        assert!(validate_url("http://user@/x").is_err());
        assert!(
            validate_url("http://example.test/a b").is_err(),
            "空白は受けない"
        );
    }

    /// 種別の判定: 名乗りが優先、名乗りが無ければ中身の頭、PDF の魔法数字は名乗りより強い。
    /// 画像などは断る。
    #[test]
    fn media_follows_the_declared_type_then_the_content_head() {
        assert_eq!(
            media_of(Some("text/html; charset=utf-8"), b"<p>x</p>"),
            Ok("html")
        );
        assert_eq!(
            media_of(Some("application/xhtml+xml"), b"<p>x</p>"),
            Ok("html")
        );
        assert_eq!(media_of(Some("Text/Plain"), b"hello"), Ok("text"));
        assert_eq!(media_of(Some("text/markdown"), b"# h"), Ok("markdown"));
        assert_eq!(media_of(Some("application/pdf"), b"%PDF-1.4"), Ok("pdf"));
        // 名乗りが嘘でも %PDF- は PDF。
        assert_eq!(media_of(Some("text/plain"), b"%PDF-1.4 ..."), Ok("pdf"));
        // 名乗りが無い・octet-stream は中身の頭で決める。
        assert_eq!(media_of(None, b"%PDF-1.7"), Ok("pdf"));
        assert_eq!(media_of(None, b"<!DOCTYPE html><html></html>"), Ok("html"));
        assert_eq!(
            media_of(Some("application/octet-stream"), b"<html lang=\"ja\">"),
            Ok("html")
        );
        assert_eq!(media_of(None, "素文。".as_bytes()), Ok("text"));
        let error = media_of(Some("image/png"), b"\x89PNG").expect_err("画像は断る");
        assert!(error.contains("image/png"), "{error}");
        assert!(
            media_of(None, &[0xff, 0xfe, 0x00]).is_err(),
            "名乗りが無く UTF-8 でもない"
        );
    }

    /// 文書名は URL から決まり、同じ URL は同じ名前になる(期待値はリテラル)。
    #[test]
    fn document_names_are_derived_from_the_url() {
        assert_eq!(
            document_name_for_url("https://arxiv.org/abs/2401.00001"),
            "arxiv.org_abs_2401.00001"
        );
        assert_eq!(
            document_name_for_url("https://arxiv.org/pdf/2401.00001"),
            "arxiv.org_pdf_2401.00001"
        );
        // 取り込み対象の拡張子は残さない。
        assert_eq!(
            document_name_for_url("https://Example.test/docs/Page.HTML"),
            "example.test_docs_Page"
        );
        assert_eq!(
            document_name_for_url("http://example.test/a/b.pdf"),
            "example.test_a_b"
        );
        // 対象でない拡張子は残す(名前の一部である)。
        assert_eq!(
            document_name_for_url("http://example.test/v1.2/notes.rs"),
            "example.test_v1.2_notes.rs"
        );
        // 末尾の / と断片は落ち、問い合わせは残る。
        assert_eq!(
            document_name_for_url("https://example.test/"),
            "example.test"
        );
        assert_eq!(
            document_name_for_url("https://example.test/a/#top"),
            "example.test_a"
        );
        assert_eq!(
            document_name_for_url("https://example.test/q?id=42&x=y"),
            "example.test_q_id_42_x_y"
        );
        assert_eq!(
            document_name_for_url("http://127.0.0.1:8080/page.html"),
            "127.0.0.1_8080_page"
        );
        // 同じ URL は同じ名前。
        assert_eq!(
            document_name_for_url("https://example.test/a/b"),
            document_name_for_url("https://example.test/a/b")
        );
        // 長い URL は先頭を残して短いハッシュを付け、上限に収まる。
        let long = format!("https://example.test/{}", "x".repeat(200));
        let name = document_name_for_url(&long);
        assert!(name.chars().count() <= MAX_NAME_CHARS, "{name}");
        assert!(name.starts_with("example.test_xxxx"), "{name}");
        assert_eq!(name.rsplit('-').next().map(str::len), Some(8), "{name}");
        assert_eq!(
            name,
            document_name_for_url(&long),
            "同じ長い URL は同じ名前"
        );
        assert_ne!(
            name,
            document_name_for_url(&format!("{long}y")),
            "違う URL は違う名前"
        );
    }

    /// `<html>` も DOCTYPE も無い断片には DOCTYPE を前置し、既に HTML と名乗れる頭は触らない
    /// (期待値はリテラル)。
    #[test]
    fn a_headless_fragment_gets_a_doctype_and_a_proper_page_is_left_alone() {
        assert_eq!(
            with_html_head("<head></head><p>b</p>".to_string()),
            "<!DOCTYPE html>\n<head></head><p>b</p>"
        );
        assert_eq!(
            with_html_head("<!DOCTYPE html><html></html>".to_string()),
            "<!DOCTYPE html><html></html>"
        );
        assert_eq!(
            with_html_head("\n<html lang=\"ja\"></html>".to_string()),
            "\n<html lang=\"ja\"></html>"
        );
        // 頭のコメントは判定が読み飛ばすので、前置しない。
        assert_eq!(
            with_html_head("<!-- 断り書き --><!DOCTYPE html><html></html>".to_string()),
            "<!-- 断り書き --><!DOCTYPE html><html></html>"
        );
    }

    /// 落としたものの数は 7 つの鍵すべてを書く(0 も書く: 数えた上で 0 だったことが読める)。
    #[test]
    fn dropped_counts_are_written_with_every_key() {
        let dropped = Dropped {
            scripts: 2,
            stylesheets: 1,
            ..Dropped::default()
        };
        let text = String::from_utf8(crate::c1::to_canonical_bytes(&dropped_value(&dropped)))
            .expect("utf-8");
        assert_eq!(
            text,
            "{\"fonts\":0,\"frames\":0,\"handlers\":0,\"images\":0,\"others\":0,\"scripts\":2,\
             \"stylesheets\":1}"
        );
    }
}
