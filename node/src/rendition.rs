//! PDF の「ページの写し」(rendition)。取り込み済みの原本 blob から、そのページの
//! 画像や単ページ PDF を作り、ストアの普通のオブジェクトとして足す層である。
//!
//! 判断(何が作れるか・作れないなら理由は何か)はここに集める。HTTP の被せ物
//! (node/src/api.rs)は、返った誤りの status() をそのまま status code にし、
//! content_type と bytes をそのまま返せばよい。
//!
//! 写しは導出データだが、作り直しに 0.4 秒かかるものを毎回作り直さないため永続する。
//! 作り方は「レシピ」(順序の固定された字句列)で表し、ref の名前へ埋める。この文字列が
//! 再導出に必要な全部であり、同時に Content-Type も決める:
//!
//! ```text
//! renditions/<原本 blob のオブジェクト ID>/<ページ番号>/<実レシピ名> → 写しのID
//! ```
//!
//! 鍵は文書名ではなく原本 blob の ID である。同じ PDF を別名で取り込んでも写しは共有
//! され、文書名を変えても無効にならない。
//!
//! 写しをストアへ入れる道は ensure 一本だけにしてある(レシピから名前を組んで
//! put_object し set_ref する)。名前を付けずに入れる口が無いので、「レシピの欠けた
//! 画像がストアに残る」ことは規律ではなく不可能である。
//!
//! 写しのオブジェクトは PNG/JPEG/PDF の生バイト列そのものである(c1 で包まない。
//! 新しい kind を作らず、base64 で膨らませない)。

use crate::c1::{self, Value};
use crate::search::terms_of;
use crate::store::{Store, StoreError};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

// ---- レシピ表 ----

/// 使う外部の道具(poppler の実行ファイル)。道具の呼び方(標準出力への書かせ方・版の
/// 読み取り)はここに集め、レシピ表は道具の種別だけを持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pdftoppm,
    Pdftocairo,
    /// 写しは作らないが、ページ番号の照合(page_agreement)に使う。
    Pdftotext,
}

impl Tool {
    /// 実行ファイル名。const fn なのは、install が unit の PATH に足す道具の表
    /// (install::DELEGATES)がこの名前をコンパイル時に参照するため(must/0023)。
    pub const fn binary(self) -> &'static str {
        match self {
            Tool::Pdftoppm => "pdftoppm",
            Tool::Pdftocairo => "pdftocairo",
            Tool::Pdftotext => "pdftotext",
        }
    }

    /// 標準出力へ書かせるために引数列の末尾へ足すもの。pdftoppm は出力接頭辞を省くと
    /// 標準出力へ画像を書く(実測)ので何も足さない。pdftocairo と pdftotext は出力先の
    /// 位置に `-` を要求する。どちらも一時ファイルを 1 つ減らせる。
    fn stdout_args(self) -> &'static [&'static str] {
        match self {
            Tool::Pdftoppm => &[],
            Tool::Pdftocairo | Tool::Pdftotext => &["-"],
        }
    }
}

/// レシピ 1 件。レシピを知る家はこの表の欄だけである(名前の組み立て・名前の
/// 読み戻し・poppler の引数列・Content-Type・読み手への注)。生成側と判定側が同じ表を
/// 見る(must/0023)。
#[derive(Debug, PartialEq, Eq)]
struct RecipeSpec {
    /// URL で受ける短い別名。
    alias: &'static str,
    /// None は恒等レシピ(何も描かず原本 blob をそのまま返す)。
    tool: Option<Tool>,
    content_type: &'static str,
    /// 実レシピ名の後半。実レシピ名は `<道具>-<版>-<この文字列>` である。
    parameters: &'static str,
    /// poppler へ渡す引数(ページ指定・入力・標準出力は command_args が足す)。
    /// parameters の字句と 1 対 1 に対応する(対応は単体テストが縛る)。
    args: &'static [&'static str],
    /// 写しを人に見せる側へ渡す但し書き(カタログがそのまま載せる)。写しが何であって
    /// 何でないかはレシピの性質なので、表の欄にしてある(被せ物側に書くと、レシピを
    /// 足したときに但し書きだけが取り残される)。
    note: Option<&'static str>,
}

/// 恒等レシピの実レシピ名。別名と同じ字句にしてある(道具も版も要らないため)。
const IDENTITY_RECIPE: &str = "source";

/// 許可表。ここに無いレシピは受け付けない。任意の寸法を受けないのは、ストアが永久で
/// ある以上、口の広さがそのまま容量の広さになるからである。増やすときは、実測した
/// 費用(時間と大きさ)をコメントに残してから足すこと。
const RECIPES: &[RecipeSpec] = &[
    // 原本そのもの。オブジェクトは増えない。
    RecipeSpec {
        alias: "source",
        tool: None,
        content_type: "application/pdf",
        parameters: "",
        args: &[],
        note: None,
    },
    // 一覧用サムネ。実測(仕様書 PDF 6MB の 100 ページ目): 0.05 秒 / 64KB。
    // 注: 22.02.0 の pdftoppm は jpeg 出力に対して -gray を無視する(実測: -gray の
    // 有無で出力バイト列が同一)。それでも引数として渡すのは、レシピ名が再導出の指示で
    // あり、名前と指示を食い違わせないためである。効きが変わる版では名前に入っている
    // 版が変わるので、別の鍵の別の写しになる。
    RecipeSpec {
        alias: "thumb",
        tool: Some(Tool::Pdftoppm),
        content_type: "image/jpeg",
        parameters: "jpeg-q60-w700-gray",
        args: &[
            "-jpeg",
            "-jpegopt",
            "quality=60",
            "-scale-to-x",
            "700",
            "-scale-to-y",
            "-1",
            "-gray",
        ],
        note: None,
    },
    // 拡大表示用。実測: 0.40 秒 / 371KB。
    RecipeSpec {
        alias: "page",
        tool: Some(Tool::Pdftoppm),
        content_type: "image/png",
        parameters: "png-r150-rgb",
        args: &["-png", "-r", "150"],
        note: None,
    },
    // そのページだけの PDF。実測: 0.06 秒 / 65KB。
    RecipeSpec {
        alias: "pagepdf",
        tool: Some(Tool::Pdftocairo),
        content_type: "application/pdf",
        parameters: "pdf-native-color",
        args: &["-pdf"],
        note: Some(
            "再描画であり、字形の写しは完全ではない。正典の本文は取り込み済みチャンクである",
        ),
    },
];

/// 許可表の 1 件への手。中身(RecipeSpec)は表の外へ出さない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipe(&'static RecipeSpec);

impl Recipe {
    /// 名前の読み戻し(1): URL の短い別名から。許可表に無ければ None。
    pub fn from_alias(alias: &str) -> Option<Recipe> {
        RECIPES.iter().find(|spec| spec.alias == alias).map(Recipe)
    }

    /// 名前の読み戻し(2): 実レシピ名から(種別と、その名前が言っている poppler の版)。
    /// 許可表の引数列と道具に一致しない名前は受け付けない。恒等レシピの版は空である。
    pub fn from_recipe_name(name: &str) -> Option<(Recipe, String)> {
        for spec in RECIPES {
            let Some(tool) = spec.tool else {
                if name == IDENTITY_RECIPE {
                    return Some((Recipe(spec), String::new()));
                }
                continue;
            };
            let Some(rest) = name.strip_prefix(&format!("{}-", tool.binary())) else {
                continue;
            };
            let Some(version) = rest.strip_suffix(&format!("-{}", spec.parameters)) else {
                continue;
            };
            if is_version_text(version) {
                return Some((Recipe(spec), version.to_string()));
            }
        }
        None
    }

    /// 名前の組み立て: `<道具>-<版>-<引数の記述>`。恒等レシピだけは道具も版も持たない。
    pub fn recipe_name(self, version: &str) -> String {
        match self.0.tool {
            None => IDENTITY_RECIPE.to_string(),
            Some(tool) => format!("{}-{version}-{}", tool.binary(), self.0.parameters),
        }
    }

    pub fn alias(self) -> &'static str {
        self.0.alias
    }

    /// Content-Type の家。恒等レシピだけは中身で決まる(identity_content_type)。
    pub fn content_type(self) -> &'static str {
        self.0.content_type
    }

    /// 写しを人に見せる側へ渡す但し書き(無ければ None)。
    pub fn note(self) -> Option<&'static str> {
        self.0.note
    }

    pub fn tool(self) -> Option<Tool> {
        self.0.tool
    }

    /// 恒等レシピ(原本 blob をそのまま返す)か。
    pub fn is_identity(self) -> bool {
        self.0.tool.is_none()
    }

    /// poppler の引数列の家。ページは `-f`/`-l` の物理ページ番号で与える(pdftotext の
    /// 改ページ数えと一致するかは page_agreement が照合する)。
    /// 恒等レシピは道具を起こさないので None。
    pub fn command_args(self, page: u32, input: &Path) -> Option<Vec<OsString>> {
        let tool = self.0.tool?;
        let mut args: Vec<OsString> = self.0.args.iter().map(OsString::from).collect();
        args.push(OsString::from("-f"));
        args.push(OsString::from(page.to_string()));
        args.push(OsString::from("-l"));
        args.push(OsString::from(page.to_string()));
        args.push(input.as_os_str().to_os_string());
        args.extend(tool.stdout_args().iter().map(OsString::from));
        Some(args)
    }

    /// 受け付ける別名の一覧(誤りの本文に載せる)。
    pub fn aliases() -> Vec<&'static str> {
        RECIPES.iter().map(|spec| spec.alias).collect()
    }
}

/// 版として受け入れる字句(`22.02.0` の類)。実レシピ名を読み戻すときに、道具名と引数の
/// 記述に挟まれた部分がほんとうに版かを確かめる。
fn is_version_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

// ---- ref の名前 ----

/// 写しの ref パス(署名者の前置きは Store::own_ref_name が付ける)。
///
/// 原本 blob の ID(`s256:` + 16進64桁)をコロンごとそのまま入れている。store の ref 名の
/// 検査は「空でない・`/` で始まらない」だけで、名前は c1 の Text として署名され reflog に
/// 載るだけなのでコロンを通す(単体テスト ref_names_carry_object_ids_with_colons が、
/// 実際に set_ref して再オープン後も同じ名前で引けることを確かめている)。URL でも
/// コロンは path segment に置ける文字(RFC 3986 の pchar)なので、被せ物側も逃がさずに
/// 済む。
pub fn ref_path(blob_id: &str, page: u32, recipe_name: &str) -> String {
    format!("renditions/{blob_id}/{page}/{recipe_name}")
}

// ---- poppler の探し方 ----

/// poppler が見つからないときに示す導入手順(sudo なし)。ingest.rs の
/// PDFTOTEXT_INSTALL_HINT が先例。写しは pdftotext と同じ poppler 一式から作るので、
/// 手順も同じである。
pub const POPPLER_INSTALL_HINT: &str =
    "導入例(sudo なし): apt-get download poppler-utils と dpkg -x で ~/opt/poppler/ へ\
     展開し、その中の実行ファイルを PATH の通ったディレクトリへ symlink するか、\
     pdftotext と同じディレクトリに揃えて置く(pdftotext の隣も探索する)。\
     libpoppler の無い機械ではライブラリ側も同じ手順で展開して LD_LIBRARY_PATH を通す";

/// 見つかった道具。起動確認と版の取得は locate が 1 回の起動で兼ねる。
#[derive(Debug)]
pub struct LocatedTool {
    pub tool: Tool,
    pub command: PathBuf,
    /// 例 "22.02.0"。実レシピ名に入る。
    pub version: String,
}

/// PATH(または与えられた path_env)から実行できるファイルを探す。pub なのは、systemctl の
/// 有無を前提として見るテスト(node/tests/install.rs)が同じ探し方を使うため(should/0135)。
pub fn find_in_path(binary: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join(binary);
        let Ok(metadata) = std::fs::metadata(&candidate) else { continue };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// 探す順に候補を並べる: 明示指定 → PATH → 既に見つかっている pdftotext と同じ
/// ディレクトリ。3 番目が要るのは、PATH には pdftotext だけを symlink してあり poppler
/// 一式は別の場所に展開してある置き方が実在するためである(symlink 先の実体の隣を見る
/// ので canonicalize してから親を取る)。
///
/// 明示指定があるときは、それだけを候補にする(操作者が指した実行ファイルが動かない
/// ときに黙って別のものへ落ちると、どの版で写したのかが分からなくなる。ingest.rs の
/// PdfExtractor::locate と同じ扱い)。
///
/// PATH をプロセスの環境からではなく引数で受け取るのは、探索の順序を環境を触らずに
/// 試せるようにするためである(単体テスト poppler_is_found_next_to_pdftotext)。pub なのは、
/// install が「unit の serve はこの道具を見つけられるか」を同じ探し方で判じるため
/// (install::tool_path。should/0135)。
pub fn candidate_commands(
    binary: &str,
    explicit: Option<&Path>,
    path_env: Option<&OsStr>,
) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path.to_path_buf()];
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(found) = find_in_path(binary, path_env) {
        candidates.push(found);
    }
    if let Some(pdftotext) = find_in_path(Tool::Pdftotext.binary(), path_env) {
        let real = std::fs::canonicalize(&pdftotext).unwrap_or(pdftotext);
        if let Some(sibling) = real.parent().map(|dir| dir.join(binary)) {
            if sibling.is_file() && !candidates.contains(&sibling) {
                candidates.push(sibling);
            }
        }
    }
    candidates
}

impl LocatedTool {
    /// 実行ファイルを見つけて版を確かめる。見つからないときは、必要なバイナリ名と
    /// 導入手順を添えて明示的に失敗する(黙って写しを諦めない。must/0022 の同型)。
    /// 起動時には呼ばないこと(should/0114: 起動を外部プロセスの都合で止めない)。
    pub fn locate(
        tool: Tool,
        explicit: Option<&Path>,
    ) -> std::result::Result<LocatedTool, String> {
        let path_env = std::env::var_os("PATH");
        let candidates = candidate_commands(tool.binary(), explicit, path_env.as_deref());
        let mut failures = Vec::new();
        for command in &candidates {
            match probe_version(tool, command) {
                Ok(version) => {
                    return Ok(LocatedTool { tool, command: command.clone(), version })
                }
                Err(error) => failures.push(error),
            }
        }
        let detail = if failures.is_empty() {
            "PATH にも pdftotext の隣にも無い".to_string()
        } else {
            failures.join(" / ")
        };
        Err(format!(
            "PDF のページの写しには {} コマンドが必要({detail})。{POPPLER_INSTALL_HINT}",
            tool.binary()
        ))
    }
}

/// 起動確認と版の取得を 1 回の起動で兼ねる(`-v` が stderr へ出す先頭行
/// 「<道具> version <版>」を読む)。ingest.rs の PdfExtractor::locate は pdftotext に
/// 固定されていた接頭辞を、ここでは道具名から組む。
fn probe_version(tool: Tool, command: &Path) -> std::result::Result<String, String> {
    let probe = Command::new(command)
        .arg("-v")
        .output()
        .map_err(|error| format!("{}: {error}", command.display()))?;
    let stderr = String::from_utf8_lossy(&probe.stderr);
    let first_line = stderr.lines().next().unwrap_or("");
    first_line
        .strip_prefix(&format!("{} version ", tool.binary()))
        .map(str::trim)
        .filter(|version| is_version_text(version))
        .map(str::to_string)
        .ok_or_else(|| {
            format!("{} -v の出力から版を読めない(先頭行: {first_line:?})", command.display())
        })
}

/// プロセスにつき一度だけ探して使い回す。失敗は覚えない(serve の実行中に poppler が
/// 導入されれば、以後の要求は通る。api.rs の pdf_extractor と同じ扱い)。
pub fn located_tool(tool: Tool) -> std::result::Result<&'static LocatedTool, String> {
    static PDFTOPPM: OnceLock<LocatedTool> = OnceLock::new();
    static PDFTOCAIRO: OnceLock<LocatedTool> = OnceLock::new();
    static PDFTOTEXT: OnceLock<LocatedTool> = OnceLock::new();
    let cell = match tool {
        Tool::Pdftoppm => &PDFTOPPM,
        Tool::Pdftocairo => &PDFTOCAIRO,
        Tool::Pdftotext => &PDFTOTEXT,
    };
    if let Some(found) = cell.get() {
        return Ok(found);
    }
    let located = LocatedTool::locate(tool, None)?;
    Ok(cell.get_or_init(|| located))
}

// ---- 誤り ----

/// この層の判断。被せ物は status() を HTTP の status code にし、本文に Display を出す。
#[derive(Debug)]
pub enum RenditionError {
    /// 許可表に無いレシピ。
    UnknownRecipe(String),
    /// 要求そのものが成り立たない(ページ 0、ID の形でない、PDF でない原本)。
    InvalidRequest(String),
    /// このDBノードが持っていない(原本が無い・その紙面が無い)。ネットワークに対する
    /// 不存在の言明ではない(SPEC §7.2/§10)。
    NotFound(String),
    /// poppler が無い。導入手順を添えてある。
    ToolMissing(String),
    /// ページ番号の照合が食い違った。生成物は返さない。
    PageMismatch(String),
    /// poppler は動いたが写しを作れなかった。
    Failed(String),
    Store(StoreError),
}

impl RenditionError {
    /// HTTP の status code。判断はここにあり、被せ物は写すだけ。
    pub fn status(&self) -> u16 {
        match self {
            RenditionError::UnknownRecipe(_) | RenditionError::InvalidRequest(_) => 400,
            RenditionError::NotFound(_) => 404,
            // 道具が無いのは要求の誤りではなく、この機械の設備が足りていない状態で
            // ある。導入すれば同じ要求が通るので 503 で返す。
            RenditionError::ToolMissing(_) => 503,
            RenditionError::PageMismatch(_) | RenditionError::Failed(_) => 500,
            RenditionError::Store(StoreError::Invalid(_)) => 400,
            RenditionError::Store(_) => 500,
        }
    }
}

impl std::fmt::Display for RenditionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenditionError::UnknownRecipe(m)
            | RenditionError::InvalidRequest(m)
            | RenditionError::NotFound(m)
            | RenditionError::ToolMissing(m)
            | RenditionError::PageMismatch(m)
            | RenditionError::Failed(m) => write!(f, "{m}"),
            RenditionError::Store(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RenditionError {}

impl From<StoreError> for RenditionError {
    fn from(error: StoreError) -> Self {
        RenditionError::Store(error)
    }
}

impl From<std::io::Error> for RenditionError {
    fn from(error: std::io::Error) -> Self {
        RenditionError::Store(StoreError::Io(error))
    }
}

pub type Result<T> = std::result::Result<T, RenditionError>;

// ---- 生成 ----

/// 写しを作る側の設定。
#[derive(Clone, Debug)]
pub struct RenditionOptions {
    /// 中間ファイル(原本 PDF の写し)の置き場。純粋な作業ファイルであり、消えても
    /// 次の要求で作り直される。
    pub work_dir: PathBuf,
    /// ページ番号の照合を回すか。既定は回す(黙って壊れるより遅い方がまし)。
    pub verify_page: bool,
}

impl RenditionOptions {
    /// serve の設定から作る。作業ファイルは `<data_dir>/derived/sources/` に残す。
    pub fn in_data_dir(data_dir: &Path) -> RenditionOptions {
        RenditionOptions {
            work_dir: data_dir.join("derived").join("sources"),
            verify_page: true,
        }
    }
}

impl Default for RenditionOptions {
    /// data_dir を知らない呼び手のための既定(一時ディレクトリ)。
    fn default() -> RenditionOptions {
        RenditionOptions {
            work_dir: std::env::temp_dir().join("uniqnode-derived-sources"),
            verify_page: true,
        }
    }
}

/// 写し 1 枚の鍵。
#[derive(Clone, Copy, Debug)]
pub struct RenditionRequest<'a> {
    /// 原本 blob のオブジェクト ID。
    pub blob_id: &'a str,
    /// 物理ページ番号(1 始まり)。恒等レシピ(source)では使わない。
    pub page: u32,
    /// URL で受けた短い別名。
    pub alias: &'a str,
}

/// その写しがどこから来たか。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenditionOrigin {
    /// 恒等レシピ。原本 blob そのもの。
    Source,
    /// 自分の ref に既にあった。
    Existing,
    /// 他署名者の同名 ref から(複製で降ってきた写し)。値は署名者。
    Replicated(String),
    /// 今作った。
    Generated,
}

/// 返す写し。
#[derive(Debug)]
pub struct Rendition {
    pub object_id: String,
    /// 実レシピ名(恒等レシピは "source")。
    pub recipe: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
    pub origin: RenditionOrigin,
    /// ページ番号の照合結果(回さなかったとき・作らなかったときは None)。
    pub page_check: Option<PageAgreement>,
}

/// 範囲外のページを渡したときに poppler が出す拒否(実測: 終了状態 99 で
/// 「Wrong page range given: the first page (9) can not be after the last page (3).」)。
/// 生成側と判定側が同じ定数を見る(must/0023)。
const PAGE_RANGE_REFUSAL: &str = "Wrong page range given";

/// 恒等レシピ(source)が名乗る Content-Type。許可表の application/pdf をそのまま名乗ると、
/// PDF でない原本(markdown・text の原文)を PDF だと嘘をつくことになるので、中身の先頭で
/// 決める。判定はここ 1 箇所で、カタログ(inspect)も実体(prepare)も同じ答えを見る
/// (should/0135)。
pub fn identity_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if looks_like_html(text) => "text/html; charset=utf-8",
        Ok(_) => "text/plain; charset=utf-8",
        Err(_) => "application/octet-stream",
    }
}

/// HTML の原本かどうか。text/plain で返すと紙面が札のまま出るので、HTML と名乗れるもの
/// だけ text/html にする。見るのは頭だけである(本文の途中に出る "<html" という文字列
/// ではなく、紙面そのものの始まりを見たい)。
fn looks_like_html(text: &str) -> bool {
    let mut head = text.trim_start_matches('\u{feff}').trim_start();
    // 頭のコメント(取得の記録やライセンスの断り書き)は読み飛ばす。飛ばさないと、
    // 断り書きを 1 行付けただけの紙面が素文だと名乗ってしまう。
    while let Some(rest) = head.strip_prefix("<!--") {
        let Some(end) = rest.find("-->") else { return false };
        head = rest[end + 3..].trim_start();
    }
    let head: String = head.chars().take(64).collect::<String>().to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

// ---- 三段(ロックの規律) ----
//
// 写しの生成は poppler との往復で、実測 0.4 秒かかる(page レシピ)。そのあいだストアの
// ロックを握り続けると API 全体が止まるので、外部との往復のあいだはロックを持たない
// (node/src/embed.rs のクエリ埋め込み、node/src/sync.rs のピアとの往復と同じ規律)。
// そのために、もとは 1 本だった ensure を三段に割ってある:
//
//   1. prepare  ロックを持つ。既に在る写しを探し、無ければ poppler に渡す材料を持ち出す。
//   2. render   ロックを持たない。poppler を回し、ページ番号を照合する。
//   3. commit   ロックを取り直す。put_object して set_ref する。
//
// ensure は三段を続けて呼ぶ薄い包みとして残してある(ロックを持ったまま一気に済ませてよい
// 呼び手 — CLI と単体テスト — のため)。ロックを放して回す呼び手(node/src/api.rs)は三段を
// 自分で並べる。

/// 第 1 段の答え。
pub enum Prepared {
    /// もう在る(あるいは恒等レシピで、原本そのもの)。ロックを放したあとの仕事は無い。
    Ready(Rendition),
    /// 作らなければならない。
    Work(RenditionWork),
}

/// ロックを放してから回す仕事。ストアから読む必要のあるものは第 1 段で全部持ち出してある
/// (原本のバイト列と、照合の材料である取り込み済みチャンクの本文)。ここに Store への
/// 参照が無いことが、「この段はロックを要らない」ということの証拠である。
pub struct RenditionWork {
    recipe: Recipe,
    recipe_name: String,
    tool: &'static LocatedTool,
    blob_id: String,
    page: u32,
    /// 写しを載せる ref パス(署名者の前置き無し)。
    path: String,
    source: Vec<u8>,
    /// そのページの取り込み済みチャンクの本文(空なら照合の材料が無い)。
    chunk_texts: Vec<String>,
    verify_page: bool,
    work_dir: PathBuf,
}

/// 第 2 段の産物。まだストアに入っていない。
pub struct Rendered {
    bytes: Vec<u8>,
    page_check: Option<PageAgreement>,
}

/// 写しを 1 枚用意する(三段を続けて呼ぶ包み)。ストアのロックを持つ側で動く。
///
/// 1. 鍵から ref 名を組み、自分の署名者の ref を引く。有ればその target を返す。
/// 2. 無ければ他署名者の同名 ref を探す。有ればその target を返す。
/// 3. 無ければ生成し、照合してから put_object して set_ref する。
///
/// 同じ鍵について書き込みは一度きりで、既にあるものは上書きしない。
pub fn ensure(
    store: &mut Store,
    options: &RenditionOptions,
    request: &RenditionRequest,
) -> Result<Rendition> {
    match prepare(store, options, request, PageEvidence::ScanStore)? {
        Prepared::Ready(rendition) => Ok(rendition),
        Prepared::Work(work) => {
            let rendered = render(&work)?;
            commit(store, work, rendered)
        }
    }
}

/// ページ番号の照合に使う本文を、誰が集めるか。
///
/// 照合そのもの(何と何を比べ、どこで食い違いとするか)は page_agreement の一箇所に
/// あり、この選択が変えるのは材料の集め方だけである(should/0135)。
pub enum PageEvidence<'a> {
    /// 呼び手が既に持っている本文を使う。検索の索引はチャンクごとに原本 ID とページ
    /// 番号を持っているので、serve はそこから 1〜2 件を引くだけで済む。
    Given(&'a [String]),
    /// 呼び手が持っていないので、見えを走査して集める(索引を持たない CLI の道)。
    ScanStore,
}

/// 第 1 段(ロックを持つ): 既に在る写しを返すか、作るための材料を持ち出す。要求そのものの
/// 誤り(知らないレシピ・ページ 0・PDF でない原本・道具が無い)はここで断る。
pub fn prepare(
    store: &Store,
    options: &RenditionOptions,
    request: &RenditionRequest,
    evidence: PageEvidence<'_>,
) -> Result<Prepared> {
    let recipe = recipe_of(request)?;
    let Some(source) = store.get_object(request.blob_id)? else {
        return Err(RenditionError::NotFound(format!(
            "原本 {} はこのDBノードに無い",
            request.blob_id
        )));
    };

    // 恒等レシピは道具を要らない。poppler が無い機械でも原本は配れる。
    if recipe.is_identity() {
        return Ok(Prepared::Ready(Rendition {
            object_id: request.blob_id.to_string(),
            recipe: recipe.recipe_name(""),
            content_type: identity_content_type(&source),
            bytes: source,
            origin: RenditionOrigin::Source,
            page_check: None,
        }));
    }

    check_page_and_pdf(request, &source)?;
    let tool = located_tool(recipe.tool().expect("恒等でないレシピは道具を持つ"))
        .map_err(RenditionError::ToolMissing)?;
    let recipe_name = recipe.recipe_name(&tool.version);
    let path = ref_path(request.blob_id, request.page, &recipe_name);

    // 1) 自分の ref、2) 他署名者の同名 ref(複製で降ってきた写し)。
    if let Some((object_id, origin)) = lookup_existing(store, &path) {
        let Some(bytes) = store.get_object(&object_id)? else {
            return Err(RenditionError::Store(StoreError::Corruption(format!(
                "ref {path} は {object_id} を指すが実体が読めない"
            ))));
        };
        return Ok(Prepared::Ready(Rendition {
            object_id,
            recipe: recipe_name,
            content_type: recipe.content_type(),
            bytes,
            origin,
            page_check: None,
        }));
    }

    // 3) 作る。照合の材料もロックを持っているこの段で集める(第 2 段はストアを見ない)。
    //
    // 材料の集め方は呼び手が選ぶ。検索の索引を持っている呼び手(serve)は、そこから
    // 「この原本のこのページ」のチャンクを直接引ける: 索引はチャンクごとに原本 ID と
    // ページ番号を RAM に持っているからである(node/src/search.rs)。持っていない呼び手
    // (CLI)は見えを走査する。走査は文書のチャンクを全部開くので、大きな仕様書では
    // 桁が違う: sdm_vol2(4,188 チャンク)で実測 5 秒に対し、索引から引けば道具の費用
    // (pdftotext 0.05 秒 + pdftoppm 0.06 秒)だけになる。
    let chunk_texts = match (options.verify_page, evidence) {
        (false, _) => Vec::new(),
        (true, PageEvidence::Given(texts)) => texts.to_vec(),
        (true, PageEvidence::ScanStore) => {
            visible_page_chunk_texts(store, request.blob_id, request.page)?
        }
    };
    Ok(Prepared::Work(RenditionWork {
        recipe,
        recipe_name,
        tool,
        blob_id: request.blob_id.to_string(),
        page: request.page,
        path,
        source,
        chunk_texts,
        verify_page: options.verify_page,
        work_dir: options.work_dir.clone(),
    }))
}

/// 第 2 段(ロックを持たない): poppler を回し、ページ番号を照合する。ここが実測 0.4 秒の
/// 部分であり、そのあいだ他の要求はストアを使える。
pub fn render(work: &RenditionWork) -> Result<Rendered> {
    let request =
        RenditionRequest { blob_id: &work.blob_id, page: work.page, alias: work.recipe.alias() };
    let source_path = materialize_source(&work.work_dir, &work.blob_id, &work.source)?;
    let args = work
        .recipe
        .command_args(work.page, &source_path)
        .expect("道具のあるレシピは引数列を持つ");
    let bytes = run_tool(work.tool, &args, &request)?;
    if bytes.is_empty() {
        return Err(RenditionError::Failed(format!(
            "{} が {} の p.{} に対して空の出力を返した",
            work.tool.command.display(),
            work.blob_id,
            work.page
        )));
    }

    // 照合は保存の前に行う。食い違ったものはストアに残さない(永久に残る層に、
    // 間違っているのに正しく見えるものを入れない)。
    let page_check = if work.verify_page {
        Some(verify_page_number(work, &request, &source_path, &bytes)?)
    } else {
        None
    };
    Ok(Rendered { bytes, page_check })
}

/// 第 3 段(ロックを取り直す): ストアへ足して名前を付ける。
///
/// ロックを放していたあいだに、別のスレッドが同じ鍵の写しを入れているかもしれない。入って
/// いればそちらを使い、自分の産物は捨てる(同じ鍵の写しは一度きり)。content-addressed
/// なので普通は同じ ID になり、捨てても何も失われない。捨てたことは黙らない
/// (must/0019)。
pub fn commit(store: &mut Store, work: RenditionWork, rendered: Rendered) -> Result<Rendition> {
    if let Some((object_id, origin)) = lookup_existing(store, &work.path) {
        if let Some(bytes) = store.get_object(&object_id)? {
            crate::log_line!(
                "uniqnode: rendition: {} p.{} の {} は待っているあいだに他所から入った\
                 ({object_id}{})。今作ったぶんは捨てる",
                work.blob_id,
                work.page,
                work.recipe_name,
                if bytes == rendered.bytes { "、同じバイト列" } else { "、違うバイト列" },
            );
            return Ok(Rendition {
                object_id,
                recipe: work.recipe_name,
                content_type: work.recipe.content_type(),
                bytes,
                origin,
                page_check: None,
            });
        }
        // ref だけ在って実体が無い(複製が途中)。今作ったものを返すが、ref は動かさない
        // (同じ鍵への書き込みは一度きり。名前が版を含むので、版が変われば別の鍵になる)。
        let (object_id, _is_new) = store.put_object(&rendered.bytes)?;
        return Ok(Rendition {
            object_id,
            recipe: work.recipe_name,
            content_type: work.recipe.content_type(),
            bytes: rendered.bytes,
            origin: RenditionOrigin::Generated,
            page_check: rendered.page_check,
        });
    }
    let (object_id, _is_new) = store.put_object(&rendered.bytes)?;
    store.set_ref(&work.path, Some(&object_id))?;
    Ok(Rendition {
        object_id,
        recipe: work.recipe_name,
        content_type: work.recipe.content_type(),
        bytes: rendered.bytes,
        origin: RenditionOrigin::Generated,
        page_check: rendered.page_check,
    })
}

/// 別名から許可表のレシピを引く(知らない別名の断り方の家)。被せ物(node/src/api.rs)も、
/// 鍵を組む前に別名だけを検めるためにこれを呼ぶ。断りの文言を二箇所に書かないための
/// 一箇所である(should/0135)。
pub fn recipe_for_alias(alias: &str) -> Result<Recipe> {
    Recipe::from_alias(alias).ok_or_else(|| {
        RenditionError::UnknownRecipe(format!(
            "レシピ {alias} は許可表にない(受け付けるのは {})",
            Recipe::aliases().join(", ")
        ))
    })
}

fn recipe_of(request: &RenditionRequest) -> Result<Recipe> {
    let recipe = recipe_for_alias(request.alias)?;
    if !c1::is_object_id(request.blob_id) {
        return Err(RenditionError::InvalidRequest(format!(
            "原本 {} はオブジェクト ID(s256: + 16進64桁)の形ではない",
            request.blob_id
        )));
    }
    Ok(recipe)
}

/// 道具の要るレシピの前提(ページ番号と、原本がほんとうに PDF であること)。
fn check_page_and_pdf(request: &RenditionRequest, source: &[u8]) -> Result<()> {
    if request.page == 0 {
        return Err(RenditionError::InvalidRequest(
            "ページ番号は 1 から始まる物理ページ番号である".to_string(),
        ));
    }
    if !source.starts_with(b"%PDF-") {
        return Err(RenditionError::InvalidRequest(format!(
            "原本 {} は PDF ではない(先頭が %PDF- で始まらない)",
            request.blob_id
        )));
    }
    Ok(())
}

/// 既に在る写しの ID を探す(生成しない)。自分の ref が先で、無ければ他署名者の同名 ref
/// (複製で降ってきた写し)。ref だけ来ていて実体がまだ無いことがあるので、実体を持って
/// いるものだけを答える。
///
/// ページ番号の照合は作った側で済んでいる前提である(ここで測り直すのは、他署名者の言明を
/// 自分のチャンクで検算するという別の判断になる。写しの内容は鍵とレシピが決めるので、
/// 同じ鍵なら同じ紙面のはずである)。
///
/// 実体の有無は has_object(RAM 上の索引)で見る。カタログ(inspect)は状態を言うだけで
/// バイト列を要らないので、ここで読んでしまうと 371KB の PNG を毎回無駄に読むことになる。
fn lookup_existing(store: &Store, path: &str) -> Option<(String, RenditionOrigin)> {
    let own_name = store.own_ref_name(path);
    if let Some(target) = store.get_ref(&own_name).and_then(|state| state.target.clone()) {
        if store.has_object(&target) {
            return Some((target, RenditionOrigin::Existing));
        }
    }
    let suffix = format!("/{path}");
    store
        .list_refs()
        .filter(|(name, _)| !name.starts_with(store.node_id_hex()))
        .find_map(|(name, state)| {
            let signer = name.strip_suffix(&suffix)?;
            let target = state.target.clone()?;
            store
                .has_object(&target)
                .then(|| (target, RenditionOrigin::Replicated(signer.to_string())))
        })
}

// ---- カタログ(生成せずに状態を言う) ----

/// 写しの席が今どうなっているか。カタログ(GET /v1/objects/{id}/rendition)はこれを並べる
/// だけで、生成はしない。
pub const STATE_STORED: &str = "stored";
pub const STATE_ABSENT: &str = "absent";
pub const STATE_UNAVAILABLE: &str = "unavailable";

/// カタログの 1 席。被せ物はこの欄をそのまま JSON にする(判断はこちらにある)。
pub struct ViewStatus {
    pub alias: &'static str,
    pub content_type: &'static str,
    /// STATE_STORED / STATE_ABSENT / STATE_UNAVAILABLE のどれか。
    pub state: &'static str,
    /// unavailable のときは必ず在る(作れない理由。poppler の不在なら導入手順つき)。
    pub reason: Option<String>,
    pub note: Option<&'static str>,
}

/// 席 1 つを調べる(ストアを読むだけで、何も作らない)。ロックを持つ側で呼ぶ。
///
/// 恒等レシピだけは原本のバイト列を読む。名乗る Content-Type が中身で決まる
/// (identity_content_type)からで、ストアは部分読みの口を持たないため先頭だけを見ることは
/// できない。カタログは利用者の 1 回の選択につき 1 回なので、この読みは許す。
///
/// 席を作ってよいかどうか(原本が PDF か・ページ番号があるか)は呼び手が決める。ここは
/// 席がある前提で、その席の今の状態だけを言う。
pub fn inspect(store: &Store, request: &RenditionRequest) -> Result<ViewStatus> {
    let recipe = recipe_of(request)?;
    let unavailable = |content_type: &'static str, reason: String| ViewStatus {
        alias: recipe.alias(),
        content_type,
        state: STATE_UNAVAILABLE,
        reason: Some(reason),
        note: recipe.note(),
    };
    if recipe.is_identity() {
        return Ok(match store.get_object(request.blob_id)? {
            Some(bytes) => ViewStatus {
                alias: recipe.alias(),
                content_type: identity_content_type(&bytes),
                state: STATE_STORED,
                reason: None,
                note: recipe.note(),
            },
            None => unavailable(
                recipe.content_type(),
                format!("原本 {} はこのDBノードに無い", request.blob_id),
            ),
        });
    }
    let tool = match located_tool(recipe.tool().expect("恒等でないレシピは道具を持つ")) {
        Ok(tool) => tool,
        Err(reason) => return Ok(unavailable(recipe.content_type(), reason)),
    };
    let path = ref_path(request.blob_id, request.page, &recipe.recipe_name(&tool.version));
    let state = match lookup_existing(store, &path) {
        Some(_) => STATE_STORED,
        None => STATE_ABSENT,
    };
    Ok(ViewStatus {
        alias: recipe.alias(),
        content_type: recipe.content_type(),
        state,
        reason: None,
        note: recipe.note(),
    })
}

/// 作業ファイルの名前に付ける通し番号(プロセス番号 + プロセス内の連番)。同じ写しを
/// 別のスレッドが同時に頼んでも作業ファイルがぶつからないようにする(ingest.rs の
/// PdfExtractor::extract と同じ仕掛け)。
fn work_serial() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// 原本 blob を作業ファイルへ落とす。poppler は入力にファイルパスを要求する
/// (ingest.rs の PdfExtractor::extract と同じ事情)。名前が内容そのもの(blob ID の
/// 16進)なので、既に同じ大きさで在れば書き直さない。
fn materialize_source(work_dir: &Path, blob_id: &str, bytes: &[u8]) -> Result<PathBuf> {
    let hex = blob_id.strip_prefix("s256:").unwrap_or(blob_id);
    std::fs::create_dir_all(work_dir)?;
    let path = work_dir.join(format!("{hex}.pdf"));
    let usable = std::fs::metadata(&path)
        .map(|meta| meta.len() == bytes.len() as u64)
        .unwrap_or(false);
    if !usable {
        // 書きかけを他のスレッドに読ませないため、別名で書いてから改名する。
        let temp = work_dir.join(format!("{hex}.pdf.tmp-{}", work_serial()));
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, &path)?;
    }
    Ok(path)
}

/// poppler を起こして標準出力を受け取る。シェルは経由しない(must/0009 と同じ理由)。
fn run_tool(tool: &LocatedTool, args: &[OsString], request: &RenditionRequest) -> Result<Vec<u8>> {
    let output = Command::new(&tool.command).args(args).output().map_err(|error| {
        RenditionError::Failed(format!("{} を起こせない: {error}", tool.command.display()))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // 範囲外のページは「このDBノードにその紙面が無い」というローカルな事実である。
        if stderr.contains(PAGE_RANGE_REFUSAL) {
            return Err(RenditionError::NotFound(format!(
                "{} に p.{} は無い({stderr})",
                request.blob_id, request.page
            )));
        }
        return Err(RenditionError::Failed(format!(
            "{} が失敗した({}): {stderr}",
            tool.command.display(),
            output.status
        )));
    }
    Ok(output.stdout)
}

// ---- ページ番号の照合 ----

/// 照合に使う語の連なりの長さ。チャンクの本文はそのページの本文の一部を切り出したもので
/// あり、語の並びごと現れるはずである。1 語ずつの重なりだと、隣の紙面とも普通の語
/// (英文の the / of など)が重なってしまい、ずれを見抜けない。
const SHINGLE_LENGTH: usize = 5;

/// 整合とみなす重なりの下限。写しが正しければ 1.0 近くに出る(チャンクの本文はページの
/// 本文の部分列だから)。ずれていれば 0 付近に落ちる。境目の値は観測されないので、
/// 真ん中に置いてある。
const PAGE_AGREEMENT_RATIO: f64 = 0.5;

/// 誤りの本文に載せる抜粋の長さ(文字)。
const EXCERPT_CHARS: usize = 60;

/// 照合の結果。
#[derive(Clone, Debug, PartialEq)]
pub enum PageAgreement {
    /// 照合する材料が無い(そのページの取り込み済みチャンクが無い)。図版だけの紙面や、
    /// どのコレクションの見えにも入っていない原本がここに来る。材料が無いことを
    /// 食い違いとは呼ばない(呼ぶと、そういう紙面の写しが一切作れなくなる)。
    NoEvidence,
    /// 照合できた。best_ratio がこのページの証拠の強さである。
    Measured {
        chunks: usize,
        best_ratio: f64,
        /// best_ratio を出したチャンクの添字。
        best_chunk: usize,
        /// 最も重ならなかったチャンクの重なり(誤りの本文に添える)。
        worst_ratio: f64,
    },
}

impl PageAgreement {
    pub fn agrees(&self) -> bool {
        match self {
            PageAgreement::NoEvidence => true,
            PageAgreement::Measured { best_ratio, .. } => *best_ratio >= PAGE_AGREEMENT_RATIO,
        }
    }
}

/// ページの本文と、取り込み済みチャンクの本文の重なりを測る純関数(照合の家はここだけ。
/// should/0135)。
///
/// チャンクの `meta.page` は pdftotext の改ページ数え、poppler の `-f`/`-l` は物理ページ
/// 番号である。整合するはずだが、ずれると「間違っているのに正しく見える」最悪の壊れ方に
/// なるので、生成のたびに確かめる。
///
/// 測り方: 語(検索と同じ terms_of で切る。should/0135)の SHINGLE_LENGTH 語の連なりの
/// うち、ページの本文にも同じ並びで現れるものの割合。語が連なりに足りない短いチャンク
/// (柱・ページ番号だけ)は語そのものの含まれ具合で測る。
///
/// 判定は最良のチャンクで行う。証拠は 1 本あれば「この紙面はこのページのものである」と
/// 言えるのに対し、最悪値で判定すると、柱だけのチャンクや pdftotext が拾い損ねた図版の
/// チャンクのせいで、正しい写しまで拒んでしまうためである。
pub fn page_agreement(page_text: &str, chunk_texts: &[String]) -> PageAgreement {
    let page_terms = terms_of(page_text);
    let page_term_set: BTreeSet<&str> = page_terms.iter().map(String::as_str).collect();
    let page_shingles = shingles(&page_terms);

    let mut chunks = 0usize;
    let mut best_ratio = 0.0f64;
    let mut best_chunk = 0usize;
    let mut worst_ratio = 1.0f64;
    for (index, text) in chunk_texts.iter().enumerate() {
        let terms = terms_of(text);
        if terms.is_empty() {
            continue;
        }
        let ratio = if terms.len() < SHINGLE_LENGTH {
            let found = terms.iter().filter(|term| page_term_set.contains(term.as_str())).count();
            found as f64 / terms.len() as f64
        } else {
            let chunk_shingles = shingles(&terms);
            let found =
                chunk_shingles.iter().filter(|s| page_shingles.contains(*s)).count();
            found as f64 / chunk_shingles.len() as f64
        };
        if chunks == 0 || ratio > best_ratio {
            best_ratio = ratio;
            best_chunk = index;
        }
        worst_ratio = worst_ratio.min(ratio);
        chunks += 1;
    }
    if chunks == 0 {
        return PageAgreement::NoEvidence;
    }
    PageAgreement::Measured { chunks, best_ratio, best_chunk, worst_ratio }
}

/// 語の連なりの集合。区切りには語に現れない制御文字を使う(語の境目を潰さないため)。
fn shingles(terms: &[String]) -> BTreeSet<String> {
    terms.windows(SHINGLE_LENGTH).map(|window| window.join("\u{1}")).collect()
}

/// 食い違いの言い分を組む純関数。何と何が食い違ったかを言う: 鍵(原本・ページ・
/// レシピ)、重なりの数値、そして突き合わせた両方の本文の抜粋である。数値だけでは
/// 「どちらがずれているのか」を人が確かめられない。
pub fn disagreement_message(
    agreement: &PageAgreement,
    request: &RenditionRequest,
    recipe_name: &str,
    page_text: &str,
    chunk_texts: &[String],
) -> String {
    let (chunks, best_ratio, worst_ratio, best_chunk) = match agreement {
        PageAgreement::NoEvidence => (0, 0.0, 0.0, 0),
        PageAgreement::Measured { chunks, best_ratio, worst_ratio, best_chunk } => {
            (*chunks, *best_ratio, *worst_ratio, *best_chunk)
        }
    };
    let chunk_excerpt = chunk_texts.get(best_chunk).map(|t| excerpt(t)).unwrap_or_default();
    format!(
        "ページ番号が食い違っている: 原本 {} の p.{} を {recipe_name} で写したが、\
         poppler が p.{} として出した本文と、取り込み済みの meta.page={} のチャンク\
         {chunks} 件との語の重なりが最良 {best_ratio:.2}(最悪 {worst_ratio:.2})しかない\
         (整合とみなす下限は {PAGE_AGREEMENT_RATIO:.2})。\
         poppler の本文「{}」に対し、チャンクは「{chunk_excerpt}」である。写しは返さない",
        request.blob_id,
        request.page,
        request.page,
        request.page,
        excerpt(page_text)
    )
}

/// 生成した写しのページ番号を照合する。
///
/// 照合の材料は 2 通り: 写しが PDF(pagepdf)なら、できた写しそのものに pdftotext を
/// 掛ける(道具が実際に取り出した紙面を見られる)。画像のレシピでは写しから本文を
/// 取れないので、原本に `-f`/`-l` を掛けて同じ物理ページの本文を取る。どちらも
/// 突き合わせ先は同じで、取り込み済みの meta.page がそのページだと言っているチャンクの
/// 本文である。費用は実測で 1 回あたり約 10 ミリ秒(6MB・数百ページの仕様書 PDF)。
///
/// 突き合わせ先のチャンク本文は第 1 段(prepare)がロックを持つあいだに集めてある。この関数が
/// Store を取らないことが、照合がロックを要らないということの証拠である。
fn verify_page_number(
    work: &RenditionWork,
    request: &RenditionRequest,
    source_path: &Path,
    artifact: &[u8],
) -> Result<PageAgreement> {
    let chunk_texts = &work.chunk_texts;
    if chunk_texts.is_empty() {
        return Ok(PageAgreement::NoEvidence);
    }
    let text = if work.recipe.content_type() == "application/pdf" {
        let temp = work.work_dir.join(format!("verify-{}.pdf", work_serial()));
        std::fs::write(&temp, artifact)?;
        let extracted = extract_text(&temp, None, request);
        // 作業ファイルの後始末の失敗も飲み込まない(must/0022)。
        let removed = std::fs::remove_file(&temp);
        let extracted = extracted?;
        removed?;
        extracted
    } else {
        extract_text(source_path, Some(request.page), request)?
    };
    let agreement = page_agreement(&text, chunk_texts);
    if !agreement.agrees() {
        return Err(RenditionError::PageMismatch(disagreement_message(
            &agreement,
            request,
            &work.recipe_name,
            &text,
            chunk_texts,
        )));
    }
    Ok(agreement)
}

/// pdftotext で本文を取る。page が None ならファイル全体(単ページ PDF に使う)。
fn extract_text(path: &Path, page: Option<u32>, request: &RenditionRequest) -> Result<String> {
    let tool = located_tool(Tool::Pdftotext).map_err(RenditionError::ToolMissing)?;
    let mut args: Vec<OsString> = Vec::new();
    if let Some(page) = page {
        args.push(OsString::from("-f"));
        args.push(OsString::from(page.to_string()));
        args.push(OsString::from("-l"));
        args.push(OsString::from(page.to_string()));
    }
    args.push(path.as_os_str().to_os_string());
    args.extend(Tool::Pdftotext.stdout_args().iter().map(OsString::from));
    let stdout = run_tool(tool, &args, request)?;
    String::from_utf8(stdout).map_err(|_| {
        RenditionError::Failed("pdftotext の出力が UTF-8 でない".to_string())
    })
}

/// 見え(collections/ 配下の ref が指す現行 doc_rev)のうち原本が blob_id のものから、
/// そのページのチャンク本文を集める。見えだけを見るのは検索の索引と同じ理由である
/// (ASSERTIONS の原理 5。旧版のチャンクは照合の相手にしない)。
pub fn visible_page_chunk_texts(
    store: &Store,
    blob_id: &str,
    page: u32,
) -> crate::store::Result<Vec<String>> {
    let mut texts = Vec::new();
    for (name, state) in store.list_refs() {
        let Some(target) = &state.target else { continue };
        let Some((_signer, path)) = name.split_once('/') else { continue };
        if !path.starts_with("collections/") {
            continue;
        }
        let Some(Value::Object(doc_rev)) = read_c1(store, target)? else { continue };
        if doc_rev.get("source") != Some(&Value::Text(blob_id.to_string())) {
            continue;
        }
        let Some(Value::Array(chunk_ids)) = doc_rev.get("chunks") else { continue };
        for chunk_ref in chunk_ids {
            let Value::Text(chunk_id) = chunk_ref else { continue };
            let Some(Value::Object(chunk)) = read_c1(store, chunk_id)? else { continue };
            let Some(Value::Text(text)) = chunk.get("text") else { continue };
            let Some(Value::Object(meta)) = chunk.get("meta") else { continue };
            if meta.get("page") == Some(&Value::Integer(i64::from(page))) {
                texts.push(text.clone());
            }
        }
    }
    Ok(texts)
}

/// ストア上のオブジェクトを c1 として読む。壊れた 1 個で照合全体を失敗させない
/// (search.rs の同名の補助と同じ扱い)。入出力の失敗だけは伝える。
fn read_c1(store: &Store, id: &str) -> crate::store::Result<Option<Value>> {
    let Some(bytes) = store.get_object(id)? else { return Ok(None) };
    let Ok(text) = String::from_utf8(bytes) else { return Ok(None) };
    Ok(c1::parse(&text).ok())
}

/// 本文の先頭を 1 行に均した抜粋(誤りの本文に添える)。
fn excerpt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(EXCERPT_CHARS) {
        Some((offset, _)) => format!("{}…", &flat[..offset]),
        None => flat,
    }
}

// ---- 単体テスト ----

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreConfig;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-rendition-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    fn options_at(dir: &Path) -> RenditionOptions {
        RenditionOptions { work_dir: dir.join("derived"), verify_page: true }
    }

    /// 外部コマンドの前提確認。poppler が無い環境では黙って飛ばさず、導入手順を示して
    /// 失敗する(docs/design/TESTING.md の外部コマンドの規約。tests/common/mod.rs の
    /// require_pdftotext と同じ流儀。飛ばして緑にすると、検証したのか検証を諦めたのかが
    /// 結果から区別できなくなる)。
    fn require_poppler(tool: Tool) -> &'static LocatedTool {
        match located_tool(tool) {
            Ok(located) => located,
            Err(error) => panic!("ページの写しのテストには poppler が必要: {error}"),
        }
    }

    // ---- レシピ表 ----

    #[test]
    fn recipe_names_round_trip_through_the_allow_list() {
        for alias in Recipe::aliases() {
            let recipe = Recipe::from_alias(alias).expect("別名は表にある");
            let name = recipe.recipe_name("22.02.0");
            let (back, version) = Recipe::from_recipe_name(&name).expect("読み戻せる");
            assert_eq!(back, recipe, "{name} が別のレシピに読み戻った");
            if recipe.is_identity() {
                assert_eq!(name, "source");
                assert_eq!(version, "");
            } else {
                assert_eq!(version, "22.02.0", "{name} の版を読めない");
            }
        }
        assert_eq!(
            Recipe::from_alias("thumb").expect("thumb").recipe_name("22.02.0"),
            "pdftoppm-22.02.0-jpeg-q60-w700-gray"
        );
        assert_eq!(
            Recipe::from_alias("page").expect("page").recipe_name("22.02.0"),
            "pdftoppm-22.02.0-png-r150-rgb"
        );
        assert_eq!(
            Recipe::from_alias("pagepdf").expect("pagepdf").recipe_name("22.02.0"),
            "pdftocairo-22.02.0-pdf-native-color"
        );
    }

    /// 許可表に無いものは受け付けない(任意の寸法を受けると、ストアが永久である以上、
    /// 口の広さがそのまま容量の広さになる)。
    #[test]
    fn recipes_outside_the_allow_list_are_refused() {
        assert!(Recipe::from_alias("w1200").is_none());
        assert!(Recipe::from_alias("").is_none());
        for name in [
            "pdftoppm-22.02.0-png-r300-rgb",   // 寸法違い
            "pdftoppm-22.02.0-jpeg-q90-w700-gray",
            "pdftocairo-22.02.0-png-r150-rgb", // 道具違い
            "pdftoppm--png-r150-rgb",          // 版が無い
            "pdftoppm-22.02.0/x-png-r150-rgb", // 版に ref 名を割る字が入っている
            "png-r150-rgb",
            "",
        ] {
            assert!(
                Recipe::from_recipe_name(name).is_none(),
                "{name} を受け付けてしまった"
            );
        }
    }

    /// 実レシピ名の後半は、実際に poppler へ渡す引数の記述でなければならない。名前が
    /// 再導出の指示である以上、名前と引数がずれたら名前は嘘になる。
    #[test]
    fn recipe_parameters_describe_the_arguments() {
        let args_of = |alias: &str| -> Vec<String> {
            let recipe = Recipe::from_alias(alias).expect("表にある");
            recipe
                .command_args(3, Path::new("/tmp/x.pdf"))
                .expect("道具のあるレシピ")
                .iter()
                .map(|a| a.to_string_lossy().to_string())
                .collect()
        };
        let thumb = args_of("thumb");
        assert!(thumb.contains(&"-jpeg".to_string()), "{thumb:?}");
        assert!(thumb.contains(&"quality=60".to_string()), "{thumb:?}"); // q60
        assert!(thumb.contains(&"-scale-to-x".to_string()), "{thumb:?}"); // w700
        assert!(thumb.contains(&"700".to_string()), "{thumb:?}");
        assert!(thumb.contains(&"-gray".to_string()), "{thumb:?}"); // gray
        let page = args_of("page");
        assert!(page.contains(&"-png".to_string()), "{page:?}");
        assert!(page.contains(&"-r".to_string()) && page.contains(&"150".to_string()));
        let pagepdf = args_of("pagepdf");
        assert!(pagepdf.contains(&"-pdf".to_string()), "{pagepdf:?}");
        // ページ指定は物理ページ番号で、-f と -l に同じ値が入る。
        for args in [&thumb, &page, &pagepdf] {
            let f = args.iter().position(|a| a == "-f").expect("-f がある");
            let l = args.iter().position(|a| a == "-l").expect("-l がある");
            assert_eq!(args[f + 1], "3");
            assert_eq!(args[l + 1], "3");
            assert_eq!(args[args.len() - if args.contains(&"-".to_string()) { 2 } else { 1 }],
                "/tmp/x.pdf");
        }
        // pdftocairo だけは出力先の位置に "-" を要求する(pdftoppm は省くと標準出力)。
        assert_eq!(pagepdf.last().map(String::as_str), Some("-"));
        assert_ne!(thumb.last().map(String::as_str), Some("-"));
        // Content-Type の家も同じ表から出る。
        assert_eq!(Recipe::from_alias("thumb").expect("t").content_type(), "image/jpeg");
        assert_eq!(Recipe::from_alias("page").expect("p").content_type(), "image/png");
        assert_eq!(
            Recipe::from_alias("pagepdf").expect("pp").content_type(),
            "application/pdf"
        );
        assert_eq!(
            Recipe::from_alias("source").expect("s").content_type(),
            "application/pdf"
        );
    }

    // ---- ref の名前 ----

    /// ref 名にオブジェクト ID をコロンごと入れられるかの実測。store の検査を通り、
    /// 署名され、reflog に載り、再オープン後も同じ名前で引けることを確かめる。
    #[test]
    fn ref_names_carry_object_ids_with_colons() {
        let dir = temp_dir("ref-colon");
        let blob_id;
        let path;
        {
            let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
            let (id, _) = store.put_object(b"%PDF-1.4 fake").expect("put");
            blob_id = id;
            path = ref_path(&blob_id, 2, "pdftoppm-22.02.0-png-r150-rgb");
            assert!(path.contains(':'), "オブジェクト ID のコロンが名前に入っている");
            store.set_ref(&path, Some(&blob_id)).expect("コロン入りの ref 名を受け付ける");
        }
        {
            let store = Store::open(StoreConfig::new(&dir)).expect("reopen");
            let name = store.own_ref_name(&path);
            let state = store.get_ref(&name).expect("再オープン後も同じ名前で引ける");
            assert_eq!(state.target.as_deref(), Some(blob_id.as_str()));
            let report = store.fsck().expect("fsck");
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn ref_paths_are_keyed_by_the_source_blob_not_the_document_name() {
        let blob = "s256:".to_string() + &"ab".repeat(32);
        assert_eq!(
            ref_path(&blob, 7, "pdftoppm-22.02.0-png-r150-rgb"),
            format!("renditions/{blob}/7/pdftoppm-22.02.0-png-r150-rgb")
        );
    }

    // ---- poppler の探し方 ----

    /// PATH には pdftotext だけを symlink してあり、poppler 一式は別の場所に展開して
    /// ある置き方(この機械の実際の姿)で、symlink 先の実体の隣から見つけられること。
    /// プロセスの環境は触らず、PATH を引数で与えて順序だけを試す。
    #[test]
    fn poppler_is_found_next_to_pdftotext() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("locate");
        let real = dir.join("opt/poppler/usr/bin");
        let linked = dir.join("bin");
        std::fs::create_dir_all(&real).expect("create");
        std::fs::create_dir_all(&linked).expect("create");
        for binary in ["pdftotext", "pdftoppm"] {
            let path = real.join(binary);
            std::fs::write(&path, "#!/bin/sh\n").expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        std::os::unix::fs::symlink(real.join("pdftotext"), linked.join("pdftotext"))
            .expect("symlink");

        let path_env = OsString::from(linked.as_os_str());
        // PATH に pdftoppm は無いので、pdftotext の実体の隣が唯一の候補になる。
        let found = candidate_commands("pdftoppm", None, Some(&path_env));
        assert_eq!(found, vec![real.join("pdftoppm")], "pdftotext の隣を見ていない");
        // 明示指定があれば、それだけを見る(黙って別の実行ファイルへ落ちない)。
        let explicit = dir.join("elsewhere/pdftoppm");
        let ordered = candidate_commands("pdftoppm", Some(&explicit), Some(&path_env));
        assert_eq!(ordered, vec![explicit]);
        // PATH にある道具は PATH のものが先(pdftotext 自身がその形)。
        let text = candidate_commands("pdftotext", None, Some(&path_env));
        assert_eq!(text.first(), Some(&linked.join("pdftotext")));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 版の読み取りは道具名から組んだ接頭辞で行う(ingest.rs の pdftotext 固定の
    /// 読み取りを、道具を受け取る形に一般化してある)。
    #[test]
    fn every_poppler_tool_reports_its_version() {
        for tool in [Tool::Pdftoppm, Tool::Pdftocairo, Tool::Pdftotext] {
            let located = require_poppler(tool);
            assert!(is_version_text(&located.version), "{:?}", located);
            assert_eq!(located.tool, tool);
        }
    }

    #[test]
    fn a_missing_tool_fails_with_the_install_procedure() {
        let error = LocatedTool::locate(Tool::Pdftoppm, Some(Path::new("/nonexistent/pdftoppm")))
            .expect_err("無い道具は見つからない");
        assert!(error.contains("pdftoppm"), "{error}");
        assert!(error.contains("apt-get download poppler-utils"), "{error}");
    }

    // ---- ページ番号の照合(純関数) ----

    const PAGE_ONE: &str = "The slot shall transition to the Default state due to the \
        successful completion of an Address Device Command with the Block Set Address \
        Request flag set to one.";
    const PAGE_TWO: &str = "A Transfer Ring is a circular queue of Transfer Descriptors \
        that the driver uses to schedule work items for a single endpoint of a device.";

    #[test]
    fn page_agreement_recognises_the_page_the_chunk_came_from() {
        let chunks = vec![PAGE_ONE.to_string()];
        let agreement = page_agreement(PAGE_ONE, &chunks);
        match agreement {
            PageAgreement::Measured { chunks, best_ratio, worst_ratio, .. } => {
                assert_eq!(chunks, 1);
                assert!(best_ratio > 0.99, "{best_ratio}");
                assert!(worst_ratio > 0.99, "{worst_ratio}");
            }
            other => panic!("{other:?}"),
        }
        assert!(page_agreement(PAGE_ONE, &chunks).agrees());
    }

    #[test]
    fn page_agreement_catches_an_off_by_one_page() {
        let chunks = vec![PAGE_ONE.to_string()];
        let agreement = page_agreement(PAGE_TWO, &chunks);
        assert!(!agreement.agrees(), "隣の紙面を掴んでいるのに整合と言った: {agreement:?}");
        match agreement {
            PageAgreement::Measured { best_ratio, .. } => assert!(best_ratio < 0.1),
            other => panic!("{other:?}"),
        }
    }

    /// 材料が無いのは食い違いではない(図版だけの紙面・見えに無い原本)。
    #[test]
    fn page_agreement_without_chunks_is_not_a_disagreement() {
        assert_eq!(page_agreement(PAGE_ONE, &[]), PageAgreement::NoEvidence);
        assert!(PageAgreement::NoEvidence.agrees());
        // 語を持たないチャンクだけでも材料にならない。
        assert_eq!(
            page_agreement(PAGE_ONE, &["...".to_string(), "  ".to_string()]),
            PageAgreement::NoEvidence
        );
    }

    /// 柱やページ番号だけの短いチャンクは、連なりを作れないので語で測る。正しい紙面
    /// なら含まれ、そうでなければ含まれない。
    ///
    /// ただし語 2, 3 個のチャンクは弱い証拠でしかない(ありふれた語が 1 つ当たるだけで
    /// 割合が閾値に届く)。ここは限界をそのまま書き残しておく: 短いチャンクしか無い
    /// 紙面では、この照合はずれを見抜けないことがある。
    #[test]
    fn short_chunks_are_matched_by_their_words() {
        let short = vec!["Block Set Address Request".to_string()];
        assert!(page_agreement(PAGE_ONE, &short).agrees());
        assert!(!page_agreement(PAGE_TWO, &short).agrees());
        let too_short = vec!["Address Device".to_string()];
        assert!(
            page_agreement(PAGE_TWO, &too_short).agrees(),
            "2 語のチャンクは device 1 語が当たるだけで通ってしまう(弱い証拠の限界)"
        );
    }

    /// 一致するチャンクが 1 本でもあれば整合とみなす(最悪値では拒みすぎる)。
    #[test]
    fn one_matching_chunk_is_evidence_enough() {
        let chunks = vec!["9 8 7 6 5 4 3 2 1 0".to_string(), PAGE_ONE.to_string()];
        let agreement = page_agreement(PAGE_ONE, &chunks);
        assert!(agreement.agrees(), "{agreement:?}");
        match agreement {
            PageAgreement::Measured { chunks, best_chunk, worst_ratio, .. } => {
                assert_eq!(chunks, 2);
                assert_eq!(best_chunk, 1);
                assert!(worst_ratio < 0.5, "{worst_ratio}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_disagreement_message_quotes_both_sides() {
        let chunks = vec![PAGE_ONE.to_string()];
        let agreement = page_agreement(PAGE_TWO, &chunks);
        let blob = "s256:".to_string() + &"cd".repeat(32);
        let request = RenditionRequest { blob_id: &blob, page: 4, alias: "thumb" };
        let message = disagreement_message(
            &agreement,
            &request,
            "pdftoppm-22.02.0-jpeg-q60-w700-gray",
            PAGE_TWO,
            &chunks,
        );
        assert!(message.contains(&blob), "{message}");
        assert!(message.contains("p.4"), "{message}");
        assert!(message.contains("pdftoppm-22.02.0-jpeg-q60-w700-gray"), "{message}");
        assert!(message.contains("A Transfer Ring"), "poppler 側の本文が無い: {message}");
        assert!(message.contains("The slot shall"), "チャンク側の本文が無い: {message}");
    }

    // ---- 生成(外部コマンドが要る) ----

    /// テスト用の PDF を組む。ページごとに違う語を置きたいので、既存の資産ではなく
    /// ここで組む(ページ番号の照合は「p.2 の写しに p.2 の語が出るか」を見る試験なので、
    /// 紙面が互いに紛れない必要がある)。非圧縮の Type1 テキストだけの最小構成。
    fn build_pdf(pages: &[&str]) -> Vec<u8> {
        let mut objects: Vec<Vec<u8>> = Vec::new();
        // 1: カタログ、2: ページ木、3..: ページと内容、最後: フォント。
        let font_number = 3 + pages.len() * 2;
        let kids: Vec<String> =
            (0..pages.len()).map(|i| format!("{} 0 R", 3 + i * 2)).collect();
        objects.push(b"<</Type/Catalog/Pages 2 0 R>>".to_vec());
        objects.push(
            format!("<</Type/Pages/Kids[{}]/Count {}>>", kids.join(" "), pages.len())
                .into_bytes(),
        );
        for (index, text) in pages.iter().enumerate() {
            let contents_number = 4 + index * 2;
            objects.push(
                format!(
                    "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]\
                     /Resources<</Font<</F1 {font_number} 0 R>>>>/Contents {contents_number} 0 R>>"
                )
                .into_bytes(),
            );
            let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET\n");
            let mut object =
                format!("<</Length {}>>stream\n", stream.len()).into_bytes();
            object.extend_from_slice(stream.as_bytes());
            object.extend_from_slice(b"endstream");
            objects.push(object);
        }
        objects.push(b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>".to_vec());

        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj", index + 1).as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"endobj\n");
        }
        let xref_offset = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer<</Size {}/Root 1 0 R>>\nstartxref\n{xref_offset}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    /// 紙面ごとに紛れない語を置いた 3 ページの PDF。
    fn three_pages() -> Vec<u8> {
        build_pdf(&[
            "alpha bravo charlie delta echo foxtrot golf hotel india juliett kilo lima",
            "mike november oscar papa quebec romeo sierra tango uniform victor whiskey",
            "xray yankee zulu tofu udon soba ramen curry sushi tempura miso natto",
        ])
    }

    /// 取り込み(pdftotext → chunk_pdf_text → ingest_document)まで本番の経路で行う。
    /// shift_pages が Some なら meta.page をずらして取り込む(照合を壊す試験用)。
    fn ingest_pdf(store: &mut Store, pdf: &[u8], shift: Option<u32>) -> String {
        let extractor = crate::ingest::PdfExtractor::locate(None).expect("pdftotext");
        let text = extractor.extract(pdf).expect("extract");
        let mut chunks = crate::ingest::chunk_pdf_text(&text);
        assert!(!chunks.is_empty(), "チャンクが 1 件も出ていない");
        if let Some(shift) = shift {
            let pages = 3u32;
            for chunk in &mut chunks {
                chunk.page = chunk.page.map(|p| (p + shift - 1) % pages + 1);
            }
        }
        let outcome = crate::ingest::ingest_document(
            store,
            &crate::ingest::DocumentInput {
                collection: "specs",
                name: "three_pages",
                source: pdf,
                media: "pdf",
                chunks: &chunks,
                extractor: Some(&extractor.extractor),
                extra_meta: &[],
            },
        )
        .expect("ingest");
        let doc_rev = store.get_object(&outcome.doc_rev_id).expect("get").expect("ある");
        let Ok(Value::Object(map)) = c1::parse(&String::from_utf8(doc_rev).expect("utf8"))
        else {
            panic!("doc_rev が c1 でない")
        };
        let Some(Value::Text(blob_id)) = map.get("source") else { panic!("source が無い") };
        blob_id.clone()
    }

    /// 生成 → 保存 → 再利用の一巡。写しはレシピの名前で ref に載り、2 度目は作り直さず、
    /// 書き込みも増えない。
    #[test]
    fn renditions_are_generated_once_and_then_reused() {
        require_poppler(Tool::Pdftoppm);
        let dir = temp_dir("generate");
        let options = options_at(&dir);
        let mut store = Store::open(StoreConfig::new(dir.join("store"))).expect("open");
        let blob_id = ingest_pdf(&mut store, &three_pages(), None);

        let objects_before = store.object_count();
        let seq_before = store.last_seq();
        let first = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 2, alias: "thumb" },
        )
        .expect("thumb");
        assert_eq!(first.origin, RenditionOrigin::Generated);
        assert_eq!(first.content_type, "image/jpeg");
        assert_eq!(&first.bytes[..2], &[0xff, 0xd8], "JPEG の先頭ではない");
        assert_eq!(first.recipe, "pdftoppm-22.02.0-jpeg-q60-w700-gray");
        assert!(first.page_check.as_ref().expect("照合した").agrees());
        assert_eq!(store.object_count(), objects_before + 1, "写しが 1 個増える");
        assert_eq!(store.last_seq(), seq_before + 1, "ref を 1 本張る");

        // 名前は鍵から組める(呼び手が同じ名前で引ける)。
        let name = store.own_ref_name(&ref_path(&blob_id, 2, &first.recipe));
        assert_eq!(
            store.get_ref(&name).and_then(|s| s.target.clone()),
            Some(first.object_id.clone())
        );

        let second = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 2, alias: "thumb" },
        )
        .expect("thumb 再訪");
        assert_eq!(second.origin, RenditionOrigin::Existing);
        assert_eq!(second.object_id, first.object_id);
        assert_eq!(second.bytes, first.bytes);
        assert_eq!(store.object_count(), objects_before + 1, "2 度目は増えない");
        assert_eq!(store.last_seq(), seq_before + 1, "2 度目は書かない");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 恒等レシピの Content-Type は中身で決まる。HTML の原本を text/plain で返すと
    /// 紙面が札のまま出てしまうので、頭を見て text/html と名乗る。
    #[test]
    fn the_identity_recipe_names_html_as_html() {
        assert_eq!(identity_content_type(b"%PDF-1.7\n..."), "application/pdf");
        assert_eq!(
            identity_content_type(b"<!DOCTYPE html><html><body>x</body></html>"),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            identity_content_type("\n  <html lang=\"ja\">…".as_bytes()),
            "text/html; charset=utf-8"
        );
        assert_eq!(identity_content_type("# 見出し\n\n本文".as_bytes()), "text/plain; charset=utf-8");
        assert_eq!(
            identity_content_type("本文に <html> と書いてあるだけの素文".as_bytes()),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            identity_content_type("<!-- 取得の記録 -->\n<!DOCTYPE html><html></html>".as_bytes()),
            "text/html; charset=utf-8",
            "頭の断り書きで素文に見えてはいけない"
        );
        assert_eq!(identity_content_type(&[0xff, 0xfe, 0x00]), "application/octet-stream");
    }

    /// 表の 3 つのレシピが、それぞれ名乗った型のバイト列を出すこと。単ページ PDF は
    /// 本文まで見て、そのページの語が入っていることを確かめる。
    #[test]
    fn every_recipe_produces_its_content_type() {
        require_poppler(Tool::Pdftocairo);
        let dir = temp_dir("recipes");
        let options = options_at(&dir);
        let mut store = Store::open(StoreConfig::new(dir.join("store"))).expect("open");
        let pdf = three_pages();
        let blob_id = ingest_pdf(&mut store, &pdf, None);

        let png = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 3, alias: "page" },
        )
        .expect("page");
        assert_eq!(png.content_type, "image/png");
        assert_eq!(&png.bytes[..4], b"\x89PNG");

        let pagepdf = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 3, alias: "pagepdf" },
        )
        .expect("pagepdf");
        assert_eq!(pagepdf.content_type, "application/pdf");
        assert!(pagepdf.bytes.starts_with(b"%PDF-"));
        let extractor = crate::ingest::PdfExtractor::locate(None).expect("pdftotext");
        let text = extractor.extract(&pagepdf.bytes).expect("extract");
        assert!(text.contains("tempura"), "3 ページ目の語が無い: {text:?}");
        assert!(!text.contains("bravo"), "別の紙面が混じっている: {text:?}");

        // 恒等レシピは何も作らず、原本をそのまま返す。
        let objects = store.object_count();
        let seq = store.last_seq();
        let source = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 1, alias: "source" },
        )
        .expect("source");
        assert_eq!(source.origin, RenditionOrigin::Source);
        assert_eq!(source.content_type, "application/pdf");
        assert_eq!(source.bytes, pdf);
        assert_eq!(source.object_id, blob_id);
        assert_eq!(store.object_count(), objects, "恒等レシピはオブジェクトを増やさない");
        assert_eq!(store.last_seq(), seq, "恒等レシピは ref を書かない");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 照合の本番経路: meta.page をずらして取り込むと、写しは返らず、何と何が食い違った
    /// かを言って失敗する。ストアには何も残らない。
    #[test]
    fn a_page_number_disagreement_refuses_to_store_the_rendition() {
        require_poppler(Tool::Pdftoppm);
        let dir = temp_dir("mismatch");
        let options = options_at(&dir);
        let mut store = Store::open(StoreConfig::new(dir.join("store"))).expect("open");
        // 1→2, 2→3, 3→1 とずらして取り込む(pdftotext の改ページ数えと poppler の
        // 物理ページ番号がずれている状態の再現)。
        let blob_id = ingest_pdf(&mut store, &three_pages(), Some(2));
        let objects = store.object_count();
        let seq = store.last_seq();

        let error = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 2, alias: "thumb" },
        )
        .expect_err("ずれているのに写しを返した");
        assert_eq!(error.status(), 500);
        let message = error.to_string();
        assert!(message.contains("ページ番号が食い違っている"), "{message}");
        assert!(message.contains(&blob_id), "{message}");
        assert!(message.contains("p.2"), "{message}");
        assert!(message.contains("pdftoppm-"), "{message}");
        assert_eq!(store.object_count(), objects, "食い違った写しを保存した");
        assert_eq!(store.last_seq(), seq, "食い違った写しに ref を張った");

        // 照合を止めれば同じ要求が通る(呼び手が選べる)。既定は回す側である。
        let mut without = options.clone();
        without.verify_page = false;
        let forced = ensure(
            &mut store,
            &without,
            &RenditionRequest { blob_id: &blob_id, page: 2, alias: "thumb" },
        )
        .expect("照合を切れば通る");
        assert_eq!(forced.origin, RenditionOrigin::Generated);
        assert!(forced.page_check.is_none());
        assert!(RenditionOptions::default().verify_page, "既定は照合する側");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 複製で降ってきた写しを使う道(手順 2)。他署名者の同名 ref と、その実体が
    /// ローカルに在れば、作り直さずそれを返し、自分の ref も書かない。
    #[test]
    fn a_replicated_rendition_is_used_without_regenerating() {
        require_poppler(Tool::Pdftoppm);
        let dir = temp_dir("replicated");
        let options = options_at(&dir);
        let pdf = three_pages();

        // 作った側。
        let mut origin = Store::open(StoreConfig::new(dir.join("origin"))).expect("open");
        let blob_id = ingest_pdf(&mut origin, &pdf, None);
        let made = ensure(
            &mut origin,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 1, alias: "thumb" },
        )
        .expect("thumb");
        assert_eq!(made.origin, RenditionOrigin::Generated);
        let origin_id = origin.node_id_hex().to_string();

        // 受け取った側: 署名済みレコードと実体だけを複製する(sync が運ぶもの)。
        let mut replica = Store::open(StoreConfig::new(dir.join("replica"))).expect("open");
        replica.put_object(&pdf).expect("put source");
        replica.put_object(&made.bytes).expect("put rendition");
        for (signer, _) in origin.signers() {
            for record in origin.export_ref_records(&signer, 0).expect("export") {
                replica.ingest_ref_record(&record).expect("ingest");
            }
        }
        let seq_before = replica.last_seq();
        let objects_before = replica.object_count();

        let taken = ensure(
            &mut replica,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 1, alias: "thumb" },
        )
        .expect("複製された写しを使える");
        assert_eq!(taken.origin, RenditionOrigin::Replicated(origin_id));
        assert_eq!(taken.object_id, made.object_id);
        assert_eq!(taken.bytes, made.bytes);
        assert_eq!(replica.last_seq(), seq_before, "自分の ref を書いてしまった");
        assert_eq!(replica.object_count(), objects_before, "作り直してしまった");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 材料の無い原本(どのコレクションの見えにも入っていない blob)でも写しは作れる。
    /// 照合は NoEvidence になる。
    #[test]
    fn a_blob_outside_any_collection_still_renders() {
        require_poppler(Tool::Pdftoppm);
        let dir = temp_dir("no-evidence");
        let options = options_at(&dir);
        let mut store = Store::open(StoreConfig::new(dir.join("store"))).expect("open");
        let (blob_id, _) = store.put_object(&three_pages()).expect("put");
        let rendition = ensure(
            &mut store,
            &options,
            &RenditionRequest { blob_id: &blob_id, page: 1, alias: "thumb" },
        )
        .expect("写せる");
        assert_eq!(rendition.page_check, Some(PageAgreement::NoEvidence));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 断りの型が要求ごとに分かれていること(被せ物はこの status をそのまま使う)。
    #[test]
    fn refusals_carry_the_status_the_caller_should_return() {
        require_poppler(Tool::Pdftoppm);
        let dir = temp_dir("refusals");
        let options = options_at(&dir);
        let mut store = Store::open(StoreConfig::new(dir.join("store"))).expect("open");
        let blob_id = ingest_pdf(&mut store, &three_pages(), None);
        let case = |store: &mut Store, blob: &str, page: u32, alias: &str| -> RenditionError {
            ensure(store, &options, &RenditionRequest { blob_id: blob, page, alias })
                .expect_err("断られるはず")
        };

        let unknown = case(&mut store, &blob_id, 1, "w1200");
        assert_eq!(unknown.status(), 400);
        assert!(unknown.to_string().contains("thumb"), "受け付ける別名を示す");

        assert_eq!(case(&mut store, &blob_id, 0, "thumb").status(), 400);
        assert_eq!(case(&mut store, "not-an-id", 1, "thumb").status(), 400);

        let missing = format!("s256:{}", "0".repeat(64));
        assert_eq!(case(&mut store, &missing, 1, "thumb").status(), 404);

        // 範囲外の紙面(poppler の拒否をこのDBノードの不存在として読む)。
        let out_of_range = case(&mut store, &blob_id, 9, "thumb");
        assert_eq!(out_of_range.status(), 404);
        assert!(out_of_range.to_string().contains("p.9"), "{out_of_range}");

        // PDF でない blob。
        let (text_blob, _) = store.put_object(b"not a pdf at all").expect("put");
        let not_pdf = case(&mut store, &text_blob, 1, "thumb");
        assert_eq!(not_pdf.status(), 400);
        assert!(not_pdf.to_string().contains("PDF ではない"), "{not_pdf}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
