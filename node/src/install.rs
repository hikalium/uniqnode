//! `uniqnode install <data_dir> [options]`: serve・viewer・毎日の backup を user 単位の systemd
//! に据える 1 命令。手順は [docs/mop/SYSTEMD.md](uuid:7de68e4a-e6a6-4930-8cc7-a56f90f522e2) の
//! 「user 単位で起こす」で、この命令は同じ節の「手で同じことをするなら」を機械にしたもの
//! (should/0117: 手作業で直したものは再現できる形にして置く)。
//!
//! 置く unit は docs/mop/systemd/user/ の現物をコンパイル時に埋め込んだもの
//! (include_str!。should/0112)。文書と実体を別々に持たないので、unit を直せばこの命令が
//! 置くものも同時に変わる。運用者ごとの値(ストアの道・待ち受け・写し先・バイナリの道)は
//! unit を編集せず drop-in `<unit>.d/override.conf` に書く。unit の ExecStart= も drop-in で
//! 差し替えるが、引数の並びは埋め込んだ unit の ExecStart= 行を読んでバイナリの道だけ替える
//! (同じ並びを 2 箇所に持たない。must/0023)。
//!
//! 1 手順 1 exec で、失敗は理由を言って止まる(黙って飛ばさない。must/0022)。設定は
//! 書いた時点ではなく効果を見た時点で完了なので(should/0116)、起こした後に serve と
//! viewer 経由の /v1/status が同じ node_id を返すことと、backup の unit を 1 回走らせた
//! 写し先が開けて fsck が緑であることまで見る。

use crate::http;
use crate::json::Json;
use crate::store::{self, FsckReport};
use crate::{fetch, outline, rendition};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const SERVE_UNIT: &str = "uniqnode-serve.service";
pub const VIEWER_UNIT: &str = "uniqnode-viewer.service";
pub const BACKUP_UNIT: &str = "uniqnode-backup.service";
pub const BACKUP_TIMER: &str = "uniqnode-backup.timer";

/// 置く unit(名前, 中身)。中身は docs/mop/systemd/user/ の現物である。
pub const UNITS: [(&str, &str); 4] = [
    (
        SERVE_UNIT,
        include_str!("../../docs/mop/systemd/user/uniqnode-serve.service"),
    ),
    (
        VIEWER_UNIT,
        include_str!("../../docs/mop/systemd/user/uniqnode-viewer.service"),
    ),
    (
        BACKUP_UNIT,
        include_str!("../../docs/mop/systemd/user/uniqnode-backup.service"),
    ),
    (
        BACKUP_TIMER,
        include_str!("../../docs/mop/systemd/user/uniqnode-backup.timer"),
    ),
];

/// enable --now する unit(service は timer が起こすので backup は timer の方)。
pub const STARTED_UNITS: [&str; 3] = [SERVE_UNIT, VIEWER_UNIT, BACKUP_TIMER];

/// drop-in のファイル名(`<unit>.d/` の下)。
pub const DROP_IN_NAME: &str = "override.conf";

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7440";
pub const DEFAULT_VIEWER_LISTEN: &str = "127.0.0.1:7450";

/// PrivateTmp=yes の unit からは見えない置き場。ここにストアや写し先を指されたら断る
/// (unit は自分だけの空の /tmp を見るので、起こしても「ストアが無い」で落ちる)。
pub const PRIVATE_TMP_ROOTS: [&str; 2] = ["/tmp", "/var/tmp"];

/// user 単位の service が Environment=PATH= を書かれないときに受け取る PATH。ログイン
/// シェルの PATH(~/.local/bin や ~/.cargo/bin を足したもの)ではなく、systemd が組み込みで
/// 持つこの値である(実測 2026-09-05、systemd 249: `systemctl --user show-environment` にも
/// 載らず、unit から `echo $PATH` させるとこの並びが出る)。`systemd-path` などで機械ごとに
/// 引かないのは、値が systemd のコンパイル時定数で、この並び以外を返す配布を見ていないため。
/// 万一違っても、この並びは /usr と / の標準の置き場を全て含むので、前に足す形なら害が無い。
pub const SYSTEMD_DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// serve が PATH から引く外部の道具。名前は探す側の定数を指す(must/0023: 同じ字句を 2 箇所に
/// 書かない)。無いときに起きることは、install が報告に書く文言である。
#[derive(Debug, PartialEq)]
pub struct Delegate {
    pub binary: &'static str,
    /// この道具が無い機械で何が起きるか(報告の 1 行に添える)。
    pub lost_without: &'static str,
    /// poppler の一式か。serve は poppler の道具を PATH の次に pdftotext の実体の隣でも探す
    /// (rendition::candidate_commands)ので、そこにあれば PATH に足さなくても届く。
    pub poppler: bool,
}

pub const DELEGATES: [Delegate; 4] = [
    Delegate {
        binary: rendition::Tool::Pdftotext.binary(),
        lost_without: "PDF の取り込みは 503 になる",
        poppler: true,
    },
    Delegate {
        binary: outline::PDFTOHTML,
        lost_without: "PDF のしおりから見出しを取れない(語の高さからの推定に落ちる)",
        poppler: true,
    },
    Delegate {
        binary: rendition::Tool::Pdftoppm.binary(),
        lost_without: "PDF のページの写しは 503 になる",
        poppler: true,
    },
    Delegate {
        binary: fetch::CURL,
        lost_without: "URL の取り込みは 503 になる",
        poppler: false,
    },
];

/// unit に書く PATH と、その判断の根拠。
#[derive(Debug, PartialEq)]
pub struct ToolPath {
    /// `Environment=PATH=` の値: 道具が見つかったディレクトリ(PATH の順、重複なし、systemd の
    /// 既定に含まれるものは除く)を SYSTEMD_DEFAULT_PATH の前に置いたもの。
    pub value: String,
    /// install の PATH でも pdftotext の隣でも見つからなかった道具。
    pub missing: Vec<&'static Delegate>,
    /// PATH には無いが pdftotext の実体の隣にあった道具と、その場所。serve はそこも探すので
    /// PATH には足さない。
    pub beside_pdftotext: Vec<(&'static Delegate, PathBuf)>,
}

/// install を走らせている自分の PATH(path_env)で DELEGATES を探し、unit の serve が同じ
/// 道具を見つけられる PATH を組む。
///
/// なぜ要るか: unit の PATH は SYSTEMD_DEFAULT_PATH であり、ログインシェルが足した
/// ~/.local/bin などを含まない。手で起こした serve が見つけていた pdftotext を unit の serve は
/// 見つけず、PDF の取り込みが 503 で失敗する(実測 2026-09-05)。install はそれを据える時点で
/// 判じ、見つかった場所を drop-in に書く。symlink の先ではなく PATH 上のディレクトリを書く
/// のは、操作者が置いた場所がそこであり、実体を置き直しても symlink を張り直せば済むため。
///
/// PATH の相対の項(`.` など)は見ない。unit は作業ディレクトリを持たず、相対の道は unit の
/// 中で別の場所を指すので、そこにしか無い道具は unit からは無いのと同じである。
///
/// PATH をプロセスの環境からではなく引数で受け取るのは純関数にするため(環境を読むのは
/// run の 1 箇所)。探し方は serve のもの(rendition::candidate_commands)を使う(should/0135)。
pub fn tool_path(path_env: &str) -> ToolPath {
    let searchable: Vec<PathBuf> = std::env::split_paths(path_env)
        .filter(|dir| dir.is_absolute())
        .collect();
    // 絶対の項だけを繋ぎ直す。split_paths が返す項は区切り文字を含まないので join は失敗しない。
    let searchable_env =
        std::env::join_paths(&searchable).expect("split_paths の項は join_paths で繋げる");
    let mut wanted: Vec<PathBuf> = Vec::new();
    let mut missing = Vec::new();
    let mut beside_pdftotext = Vec::new();
    for delegate in &DELEGATES {
        // candidate_commands は PATH の当たりを先頭に、poppler の道具なら pdftotext の実体の隣を
        // 続けて返す。curl は fetch.rs が PATH でしか探さないので隣は見ない。
        let candidates = rendition::candidate_commands(
            delegate.binary,
            None,
            Some(searchable_env.as_os_str()),
        );
        let parent_of = |command: &Path| command.parent().map(Path::to_path_buf);
        let in_path = candidates
            .iter()
            .find_map(|command| parent_of(command).filter(|dir| searchable.contains(dir)));
        match (in_path, candidates.first().and_then(|c| parent_of(c))) {
            (Some(dir), _) => wanted.push(dir),
            (None, Some(dir)) if delegate.poppler => beside_pdftotext.push((delegate, dir)),
            _ => missing.push(delegate),
        }
    }
    let default_dirs: Vec<PathBuf> = std::env::split_paths(SYSTEMD_DEFAULT_PATH).collect();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in &searchable {
        if wanted.contains(dir) && !default_dirs.contains(dir) && !dirs.contains(dir) {
            dirs.push(dir.clone());
        }
    }
    dirs.extend(default_dirs);
    let value = std::env::join_paths(&dirs)
        .expect("PATH の項は join_paths で繋げる")
        .to_string_lossy()
        .into_owned();
    ToolPath {
        value,
        missing,
        beside_pdftotext,
    }
}

pub struct Options {
    pub data_dir: PathBuf,
    /// serve の待ち受け(UNIQNODE_LISTEN)。viewer の転送先もここから導く。
    pub listen: String,
    pub viewer_listen: String,
    /// serve の追加の引数(UNIQNODE_SERVE_OPTIONS)。空白で分けた 1 本の文字列。
    pub serve_options: String,
    pub backup_dir: PathBuf,
    /// 実行ファイルを置く道。走っている自分自身を写す。
    pub binary: PathBuf,
    /// unit と drop-in を置くディレクトリ(既定 ~/.config/systemd/user)。
    pub unit_dir: PathBuf,
    /// false なら daemon-reload まで行い、enable/start と確認を省く。
    pub start: bool,
}

impl Options {
    /// home の下の既定で組む。
    pub fn defaults(data_dir: PathBuf, home: &Path) -> Options {
        Options {
            data_dir,
            listen: DEFAULT_LISTEN.to_string(),
            viewer_listen: DEFAULT_VIEWER_LISTEN.to_string(),
            serve_options: String::new(),
            backup_dir: home.join("uniqnode-backup"),
            binary: home.join(".local").join("bin").join("uniqnode"),
            unit_dir: home.join(".config").join("systemd").join("user"),
            start: true,
        }
    }
}

/// viewer が転送する先。UNIQNODE_LISTEN から導く(2 箇所で別々に決めない)。
pub fn serve_url_of(listen: &str) -> String {
    format!("http://{listen}")
}

/// 相対の道を絶対にする(unit は作業ディレクトリを持たないので、相対の道は unit の中で
/// 別の場所を指す)。在るものは symlink も解いておく(PrivateTmp の判定は実体で行う)。
pub fn absolute(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path)
        .map_err(|e| format!("{} を絶対の道にできない: {e}", path.display()))?;
    if absolute.exists() {
        std::fs::canonicalize(&absolute)
            .map_err(|e| format!("{} を正規化できない: {e}", absolute.display()))
    } else {
        Ok(absolute)
    }
}

/// PrivateTmp=yes の unit から見えない場所なら断る。
pub fn refuse_private_tmp(path: &Path, what: &str) -> Result<(), String> {
    for root in PRIVATE_TMP_ROOTS {
        if path.starts_with(root) {
            return Err(format!(
                "{what} {} は {root} の下にある。unit は PrivateTmp=yes で自分だけの空の {root} を\
                 見るので、そこに置いたストアや写しは unit から見えない。home の下など別の\
                 場所を指す",
                path.display()
            ));
        }
    }
    Ok(())
}

/// unit ファイルの 1 語として安全に書く。空白・引用符・`$`・`;`・`#` などを含む値は
/// 二重引用符で囲み(中の `\` と `"` は `\` で逃がす)、`%` は指定子として展開されないよう
/// `%%` にする。改行は unit の 1 行に収まらないので断る。
pub fn unit_word(text: &str) -> Result<String, String> {
    if text.contains(['\n', '\r']) {
        return Err(format!("{text:?}: 改行を含む値は unit に書けない"));
    }
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-:=+,@~".contains(c);
    let escaped_percent = text.replace('%', "%%");
    if !text.is_empty() && text.chars().all(plain) {
        return Ok(escaped_percent);
    }
    let mut quoted = String::from("\"");
    for c in escaped_percent.chars() {
        if c == '\\' || c == '"' {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    Ok(quoted)
}

/// `Environment=KEY=value` の 1 行。
fn environment_line(key: &str, value: &str) -> Result<String, String> {
    Ok(format!(
        "Environment={}",
        unit_word(&format!("{key}={value}"))?
    ))
}

/// unit の ExecStart= 行を読み、先頭のバイナリの道だけを差し替えた値を返す。引数の並びは
/// unit のもの(must/0023: 並びを drop-in 側でもう一度書かない)。
pub fn exec_start_with_binary(unit_text: &str, binary_word: &str) -> Result<String, String> {
    let value = unit_text
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .ok_or_else(|| "unit に ExecStart= が無い".to_string())?;
    let (_default_binary, tail) = value
        .split_once(' ')
        .ok_or_else(|| format!("ExecStart= にバイナリの道の後の引数が無い: {value}"))?;
    Ok(format!("{binary_word} {tail}"))
}

/// unit の中身を名前で引く。
fn unit_text(name: &str) -> &'static str {
    UNITS
        .iter()
        .find(|(unit, _)| *unit == name)
        .map(|(_, text)| *text)
        .expect("UNITS に載っている名前")
}

/// 3 つの service の drop-in を描く: (unit 名, override.conf の中身)。ExecStart= と
/// ReadWritePaths= は追記型なので、空の行で unit の値を一度消してから書く。tool_path は
/// tool_path() の value(Environment=PATH= に書く値)。
pub fn drop_ins(
    options: &Options,
    tool_path: &str,
) -> Result<Vec<(&'static str, String)>, String> {
    let data_dir = options.data_dir.to_string_lossy();
    let backup_dir = options.backup_dir.to_string_lossy();
    let binary = unit_word(&options.binary.to_string_lossy())?;
    let header = "# uniqnode install が書いた drop-in。再実行で書き直されるので、手で変えるなら\n\
                  # 別の名前の *.conf を隣に置く。\n\
                  [Service]\n";
    let mut rendered = Vec::new();
    for unit in [SERVE_UNIT, VIEWER_UNIT, BACKUP_UNIT] {
        // PATH は 3 つとも同じ値。外部の道具(DELEGATES)を起こすのは serve だけだが、unit ごとに
        // 違う PATH を書くと「どの unit がどの道具を見るか」を読み手が unit ごとに追うことになる。
        // 同じ値なら drop-in を 1 つ読めば全部が分かる。
        let mut lines = vec![
            environment_line("PATH", tool_path)?,
            environment_line("UNIQNODE_DATA_DIR", &data_dir)?,
        ];
        let mut writable: Vec<&str> = Vec::new();
        match unit {
            SERVE_UNIT => {
                lines.push(environment_line("UNIQNODE_LISTEN", &options.listen)?);
                lines.push(environment_line(
                    "UNIQNODE_SERVE_OPTIONS",
                    &options.serve_options,
                )?);
                writable.push(&data_dir);
            }
            VIEWER_UNIT => {
                lines.push(environment_line(
                    "UNIQNODE_VIEWER_LISTEN",
                    &options.viewer_listen,
                )?);
                lines.push(environment_line(
                    "UNIQNODE_SERVE_URL",
                    &serve_url_of(&options.listen),
                )?);
                // viewer が書くのは <dir>/logs/viewer.log だけだが、それはストアの下にある。
                writable.push(&data_dir);
            }
            _ => {
                lines.push(environment_line("UNIQNODE_BACKUP_DIR", &backup_dir)?);
                // backup は写し元を読むだけなので、書けるのは写し先だけにする。
                writable.push(&backup_dir);
            }
        }
        lines.push("ReadWritePaths=".to_string());
        for path in writable {
            lines.push(format!("ReadWritePaths={}", unit_word(path)?));
        }
        lines.push("ExecStart=".to_string());
        lines.push(format!(
            "ExecStart={}",
            exec_start_with_binary(unit_text(unit), &binary)?
        ));
        rendered.push((unit, format!("{header}{}\n", lines.join("\n"))));
    }
    Ok(rendered)
}

/// 待ち受けの指定を検める: 解決でき、ポートが固定されていること(`:0` の自動割当は、
/// viewer の転送先を導けないので install では受けない)。
pub fn check_listen(listen: &str, what: &str) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    let mut resolved = listen
        .to_socket_addrs()
        .map_err(|e| format!("{what} {listen} を解決できない: {e}"))?;
    match resolved.next() {
        None => Err(format!("{what} {listen} を解決できない")),
        Some(address) if address.port() == 0 => Err(format!(
            "{what} {listen}: ポートは固定する(0 の自動割当では viewer の転送先が決まらない)"
        )),
        Some(_) => Ok(()),
    }
}

/// 引数を検め、道を絶対にして整える。unit を書く前に、書いても効かない指定を断る。
pub fn normalize(options: Options) -> Result<Options, String> {
    check_listen(&options.listen, "--listen")?;
    check_listen(&options.viewer_listen, "--viewer-listen")?;
    let data_dir = absolute(&options.data_dir)?;
    let backup_dir = absolute(&options.backup_dir)?;
    refuse_private_tmp(&data_dir, "ストア")?;
    refuse_private_tmp(&backup_dir, "写し先")?;
    if data_dir == backup_dir {
        return Err(format!(
            "ストアと写し先が同じ場所を指している: {}",
            data_dir.display()
        ));
    }
    let binary = absolute(&options.binary)?;
    if binary.is_dir() {
        return Err(format!(
            "--bin {} はディレクトリ(実行ファイルの道を指す)",
            binary.display()
        ));
    }
    Ok(Options {
        data_dir,
        listen: options.listen,
        viewer_listen: options.viewer_listen,
        // 空白で分けて Environment= に載せる値なので、並びの空白を 1 つに揃える。
        serve_options: options
            .serve_options
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        backup_dir,
        binary,
        unit_dir: absolute(&options.unit_dir)?,
        start: options.start,
    })
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("{} を作れない: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("{} を書けない: {e}", path.display()))
}

/// 走っている自分自身を destination に写す。隣に書いてから rename で据えるので、走行中の
/// 実行ファイルを上書きせず(Text file busy にならず)、途中で止まっても半分のファイルが
/// その名前で残らない。destination が自分自身なら写さない。返り値は報告の 1 行。
pub fn install_binary(destination: &Path) -> Result<String, String> {
    let current = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| format!("自分自身の実行ファイルが分からない: {e}"))?;
    if destination.exists() {
        let existing = std::fs::canonicalize(destination)
            .map_err(|e| format!("{} を正規化できない: {e}", destination.display()))?;
        if existing == current {
            return Ok(format!(
                "バイナリ {} は走っている自分自身なので写さない",
                destination.display()
            ));
        }
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} に親ディレクトリが無い", destination.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{} を作れない: {e}", parent.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("{} にファイル名が無い", destination.display()))?
        .to_string_lossy();
    let staging = parent.join(format!(".{name}.install-{}", std::process::id()));
    let bytes = std::fs::copy(&current, &staging)
        .map_err(|e| format!("{} へ写せない: {e}", staging.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{} の許可ビット: {e}", staging.display()))?;
    }
    std::fs::rename(&staging, destination).map_err(|e| {
        format!(
            "{} → {} の rename: {e}",
            staging.display(),
            destination.display()
        )
    })?;
    Ok(format!(
        "バイナリ {} ← {}({bytes} bytes)",
        destination.display(),
        current.display()
    ))
}

/// systemctl --user を 1 命令 1 exec で呼ぶ。見つからない・呼べないは Err。終了コードと
/// 出力は呼び手が読む(is-active のように非 0 が答えである命令があるため)。
fn systemctl(arguments: &[&str]) -> Result<std::process::Output, String> {
    Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "systemctl が PATH に無い(systemd の無い機械では install は使えない)".to_string()
            }
            _ => format!("systemctl --user {} を起こせない: {e}", arguments.join(" ")),
        })
}

/// 成功(終了 0)を要する systemctl。失敗は標準エラーを添えて言う。
fn systemctl_ok(arguments: &[&str]) -> Result<String, String> {
    let output = systemctl(arguments)?;
    if !output.status.success() {
        return Err(format!(
            "systemctl --user {} が {} で終わった: {}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// `systemctl --user is-active <unit>` の答え(active / inactive / failed / …)。
fn unit_state(unit: &str) -> Result<String, String> {
    let output = systemctl(&["is-active", unit])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// /v1/status を 1 本引いて node_id を読む。
fn status_node_id(address: &str) -> Result<String, String> {
    // 1 本の期限。起動直後の serve は索引を組む前でも /v1/status には即答する。
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
    let response = http::get(address, "/v1/status", REQUEST_TIMEOUT)?;
    if response.status != 200 {
        return Err(format!(
            "HTTP {}: {}",
            response.status,
            http::body_head(&response.body)
        ));
    }
    let text = String::from_utf8_lossy(&response.body);
    let json = Json::parse(&text)?;
    json.field("node_id")
        .and_then(Json::text)
        .map(str::to_string)
        .ok_or_else(|| format!("応答に node_id が無い: {}", http::body_head(&response.body)))
}

/// serve と viewer 経由の /v1/status が同じ node_id を返すまで待つ。返り値は (node_id, 掛かった
/// 時間)。
///
/// 待つ理由: serve は Type=exec なので systemctl は exec した時点で戻り、ストアを開いて
/// 束縛するまでは待たない。条件(両方が同じ node_id を答える)を短い間隔で見る。上限は
/// 安全の網で、越えたら最後に見た結果を添えて失敗する。serve の unit が failed に落ちたら
/// 上限を待たずに言う(should/0104)。
fn wait_for_matching_status(
    listen: &str,
    viewer_listen: &str,
) -> Result<(String, Duration), String> {
    const BOUND: Duration = Duration::from_secs(30);
    const TICK: Duration = Duration::from_millis(200);
    let started = Instant::now();
    loop {
        let serve = status_node_id(listen);
        let viewer = status_node_id(viewer_listen);
        if let (Ok(a), Ok(b)) = (&serve, &viewer) {
            if a == b {
                return Ok((a.clone(), started.elapsed()));
            }
        }
        for unit in [SERVE_UNIT, VIEWER_UNIT] {
            if unit_state(unit)? == "failed" {
                return Err(format!(
                    "{unit} が failed になった。理由は journalctl --user -u {unit} -n 20"
                ));
            }
        }
        if started.elapsed() >= BOUND {
            let describe = |r: &Result<String, String>| match r {
                Ok(id) => format!("node_id {id}"),
                Err(e) => e.clone(),
            };
            return Err(format!(
                "{} 秒待っても serve と viewer の /v1/status が揃わない。serve({listen}): {}。\
                 viewer({viewer_listen}): {}",
                BOUND.as_secs(),
                describe(&serve),
                describe(&viewer)
            ));
        }
        std::thread::sleep(TICK);
    }
}

fn fsck_summary(report: &FsckReport) -> String {
    format!(
        "objects {} refs {} errors {}",
        report.objects_checked,
        report.refs_checked,
        report.errors.len()
    )
}

/// install の本体。手順ごとに 1 行を out へ書き、失敗はその場で Err(呼び手が理由を出して
/// exit 1)。
pub fn run(options: Options, out: &mut dyn Write) -> Result<(), String> {
    let options = normalize(options)?;
    let say = |out: &mut dyn Write, line: &str| -> Result<(), String> {
        writeln!(out, "install: {line}").map_err(|e| format!("標準出力に書けない: {e}"))
    };

    // (1) バイナリ。
    let binary_line = install_binary(&options.binary)?;
    say(out, &binary_line)?;

    // (2) unit。docs/mop/systemd/user/ の現物をそのまま。
    for (name, text) in UNITS {
        let path = options.unit_dir.join(name);
        write_file(&path, text)?;
        say(out, &format!("unit {}", path.display()))?;
    }

    // (3) drop-in と、ReadWritePaths= が要求する在るディレクトリ。unit の PATH は systemd の
    // 既定なので、自分の PATH で見つけた道具の場所を前に足す。無い道具は 1 行ずつ言う
    // (黙って進めない。must/0022)が、止めない: 道具が無くても serve は起き、無い機能だけが
    // 503 で答える(should/0114 と同じ扱い)。環境から PATH を読むのはここだけ。
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let tools = tool_path(&path_env.to_string_lossy());
    say(out, &format!("PATH={}(serve が起こす外部の道具の探し先)", tools.value))?;
    for (delegate, dir) in &tools.beside_pdftotext {
        say(
            out,
            &format!(
                "{} は PATH に無いが pdftotext の実体の隣({})にあり、serve はそこも探す",
                delegate.binary,
                dir.display()
            ),
        )?;
    }
    for delegate in &tools.missing {
        say(
            out,
            &format!(
                "{} は PATH に無い({})",
                delegate.binary, delegate.lost_without
            ),
        )?;
    }
    for (unit, content) in drop_ins(&options, &tools.value)? {
        let path = options
            .unit_dir
            .join(format!("{unit}.d"))
            .join(DROP_IN_NAME);
        write_file(&path, &content)?;
        say(out, &format!("drop-in {}", path.display()))?;
    }
    for (dir, what) in [
        (&options.data_dir, "ストア"),
        (&options.backup_dir, "写し先"),
    ] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れない: {e}", dir.display()))?;
        say(
            out,
            &format!(
                "{what} {}(ReadWritePaths= は無い道を作らないので先に作る)",
                dir.display()
            ),
        )?;
    }

    // (4) daemon-reload。
    systemctl_ok(&["daemon-reload"])?;
    say(out, "systemctl --user daemon-reload: 済み")?;

    if !options.start {
        say(
            out,
            &format!(
                "--no-start なので起こしていない。起こすには systemctl --user enable --now {}",
                STARTED_UNITS.join(" ")
            ),
        )?;
        return Ok(());
    }

    // (5) 起こす前にロックを探る。unit は exit 1 で起こし直さない設計なので、落ちてから journal
    // を読ませるより先に言う。
    if store::opened_by_another_process(&options.data_dir).map_err(|e| e.to_string())? {
        let serve_state = unit_state(SERVE_UNIT)?;
        if serve_state != "active" {
            return Err(format!(
                "{} は別プロセスが開いている(unit の serve ではない。{SERVE_UNIT} は {serve_state})。\
                 そのプロセスを止めてから再実行するか、--no-start で unit だけ置く",
                options.data_dir.display()
            ));
        }
    }

    // (6) enable と restart(restart は止まっている unit も起こすので、初回と更新で同じ手順)。
    for unit in STARTED_UNITS {
        let before = unit_state(unit)?;
        systemctl_ok(&["enable", unit])?;
        systemctl_ok(&["restart", unit])?;
        let verb = if before == "active" {
            "起こし直した"
        } else {
            "起こした"
        };
        say(out, &format!("{unit}: enable、{verb}(直前は {before})"))?;
    }

    // (7) linger。無ければログアウトで user 単位のマネージャごと止まる。取れないのは警告に
    // とどめる(polkit の設定次第で対話が要る)。
    match Command::new("loginctl").arg("enable-linger").output() {
        Ok(output) if output.status.success() => say(out, "loginctl enable-linger: 済み")?,
        Ok(output) => say(
            out,
            &format!(
                "警告: loginctl enable-linger が {} で終わった({})。ログアウトすると unit も\
                 止まるので、手で有効にする",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )?,
        Err(e) => say(
            out,
            &format!("警告: loginctl を起こせない({e})。linger は手で有効にする"),
        )?,
    }

    // (8) 確認。効果を見るまでは完了ではない(should/0116)。
    let (node_id, waited) = wait_for_matching_status(&options.listen, &options.viewer_listen)?;
    say(
        out,
        &format!(
            "確認: {}/v1/status と viewer {} 経由が同じ node_id {node_id} を返した({} ms)",
            serve_url_of(&options.listen),
            serve_url_of(&options.viewer_listen),
            waited.as_millis()
        ),
    )?;
    systemctl_ok(&["start", BACKUP_UNIT])
        .map_err(|e| format!("{e}。理由は journalctl --user -u {BACKUP_UNIT} -n 20"))?;
    let verification = crate::backup::verify_copy(&options.backup_dir)
        .map_err(|e| format!("写し先 {} を開けない: {e}", options.backup_dir.display()))?;
    if !verification.errors.is_empty() {
        return Err(format!(
            "写し先 {} の fsck が赤: {}",
            options.backup_dir.display(),
            verification.errors.join("; ")
        ));
    }
    say(
        out,
        &format!(
            "確認: {BACKUP_UNIT} を 1 回走らせ、写し先 {} を開いて fsck: {}",
            options.backup_dir.display(),
            fsck_summary(&verification)
        ),
    )?;

    // (9) 次の刻み。systemctl の表は見出しと行の後に空行と件数の脚注が付くので、表だけを載せる。
    let timers = systemctl_ok(&["list-timers", BACKUP_TIMER, "--no-pager"])?;
    for line in timers.lines().take_while(|line| !line.trim().is_empty()) {
        say(out, &format!("次の刻み: {line}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_options() -> Options {
        Options {
            data_dir: PathBuf::from("/home/op/store"),
            listen: "127.0.0.1:7443".to_string(),
            viewer_listen: "127.0.0.1:7453".to_string(),
            serve_options: "--embed http://127.0.0.1:8083/v1/embeddings".to_string(),
            backup_dir: PathBuf::from("/home/op/uniqnode-backup"),
            binary: PathBuf::from("/home/op/.local/bin/uniqnode"),
            unit_dir: PathBuf::from("/home/op/.config/systemd/user"),
            start: true,
        }
    }

    fn drop_in_for(unit: &str, options: &Options) -> String {
        drop_ins(options, SYSTEMD_DEFAULT_PATH)
            .expect("描ける")
            .into_iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, text)| text)
            .expect("3 つの service の 1 つ")
    }

    /// serve の drop-in: 空白を含む追加の引数は引用符で囲まれ、ExecStart= と ReadWritePaths=
    /// は空の行で消してから書かれ、ExecStart= の引数の並びは unit のまま。
    #[test]
    fn the_serve_drop_in_quotes_options_resets_exec_start_and_keeps_the_argument_order() {
        let text = drop_in_for(SERVE_UNIT, &sample_options());
        let expected = "[Service]\n\
                        Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n\
                        Environment=UNIQNODE_DATA_DIR=/home/op/store\n\
                        Environment=UNIQNODE_LISTEN=127.0.0.1:7443\n\
                        Environment=\"UNIQNODE_SERVE_OPTIONS=--embed http://127.0.0.1:8083/v1/embeddings\"\n\
                        ReadWritePaths=\n\
                        ReadWritePaths=/home/op/store\n\
                        ExecStart=\n\
                        ExecStart=/home/op/.local/bin/uniqnode serve ${UNIQNODE_DATA_DIR} ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS\n";
        assert!(text.ends_with(expected), "serve の drop-in:\n{text}");
        assert!(
            text.starts_with("# uniqnode install が書いた drop-in"),
            "{text}"
        );
    }

    /// viewer の drop-in: 転送先は --listen から導かれ、backup の drop-in: 書けるのは写し先
    /// だけで、空の追加引数は引用符なしの空の値になる。
    #[test]
    fn the_viewer_and_backup_drop_ins_derive_the_serve_url_and_open_only_what_each_writes() {
        let mut options = sample_options();
        options.serve_options.clear();
        let serve = drop_in_for(SERVE_UNIT, &options);
        assert!(
            serve.contains("\nEnvironment=UNIQNODE_SERVE_OPTIONS=\n"),
            "{serve}"
        );
        let viewer = drop_in_for(VIEWER_UNIT, &options);
        assert!(
            viewer.contains("\nEnvironment=UNIQNODE_VIEWER_LISTEN=127.0.0.1:7453\n"),
            "{viewer}"
        );
        assert!(
            viewer.contains("\nEnvironment=UNIQNODE_SERVE_URL=http://127.0.0.1:7443\n"),
            "{viewer}"
        );
        assert!(
            viewer.ends_with(
                "ExecStart=\nExecStart=/home/op/.local/bin/uniqnode viewer ${UNIQNODE_DATA_DIR} \
                 ${UNIQNODE_VIEWER_LISTEN} --serve-url ${UNIQNODE_SERVE_URL} $UNIQNODE_VIEWER_OPTIONS\n"
            ),
            "{viewer}"
        );
        let backup = drop_in_for(BACKUP_UNIT, &options);
        assert!(
            backup.contains("\nEnvironment=UNIQNODE_BACKUP_DIR=/home/op/uniqnode-backup\n"),
            "{backup}"
        );
        assert!(
            backup.contains(
                "\nReadWritePaths=\nReadWritePaths=/home/op/uniqnode-backup\nExecStart=\n"
            ),
            "{backup}"
        );
        assert!(
            !backup.contains("ReadWritePaths=/home/op/store"),
            "backup は写し元に書かない: {backup}"
        );
        assert!(
            backup.ends_with(
                "ExecStart=/home/op/.local/bin/uniqnode backup ${UNIQNODE_DATA_DIR} ${UNIQNODE_BACKUP_DIR}\n"
            ),
            "{backup}"
        );
    }

    /// 空白や % を含む道は引用符と %% で unit に書ける形になり、改行は断る。
    #[test]
    fn unit_words_are_quoted_and_percent_escaped_only_when_needed() {
        assert_eq!(
            unit_word("/home/op/store").expect("plain"),
            "/home/op/store"
        );
        assert_eq!(
            unit_word("/home/op/my store").expect("space"),
            "\"/home/op/my store\""
        );
        assert_eq!(unit_word("/x/100%").expect("percent"), "\"/x/100%%\"");
        assert_eq!(unit_word("a\"b\\c").expect("escapes"), "\"a\\\"b\\\\c\"");
        assert_eq!(unit_word("").expect("empty"), "\"\"");
        assert!(unit_word("a\nb").is_err());
    }

    /// /tmp と /var/tmp の下のストア・写し先は断る(PrivateTmp=yes の unit から見えない)。
    #[test]
    fn a_store_or_backup_under_tmp_is_refused_before_anything_is_written() {
        let mut options = sample_options();
        options.data_dir = PathBuf::from("/tmp/uniqnode-probe");
        let message = normalize(options).err().expect("断る");
        assert!(message.contains("PrivateTmp"), "{message}");
        assert!(message.contains("/tmp/uniqnode-probe"), "{message}");
        let mut options = sample_options();
        options.backup_dir = PathBuf::from("/var/tmp/copy");
        let message = normalize(options).err().expect("断る");
        assert!(message.contains("写し先 /var/tmp/copy"), "{message}");
        // home の下は通る(在れば正規化し、無ければそのまま)。
        assert!(normalize(sample_options()).is_ok());
    }

    /// 待ち受けの自動割当(ポート 0)は viewer の転送先が決まらないので断る。
    #[test]
    fn a_zero_port_listen_is_refused() {
        let mut options = sample_options();
        options.listen = "127.0.0.1:0".to_string();
        let message = normalize(options).err().expect("断る");
        assert!(message.contains("ポートは固定する"), "{message}");
    }

    /// 偽の道具(実行できる空のファイル)を置いた一時ディレクトリ。
    fn tool_dir(name: &str, binaries: &[&str]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "uniqnode-install-unit-{}-tools-{name}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        std::fs::create_dir_all(&dir).expect("mkdir");
        for binary in binaries {
            let path = dir.join(binary);
            std::fs::write(&path, "").expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        dir
    }

    fn names(delegates: &[&Delegate]) -> Vec<&'static str> {
        delegates.iter().map(|d| d.binary).collect()
    }

    /// (a) PATH の一時ディレクトリに pdftotext があれば、3 つの drop-in の PATH はその
    /// ディレクトリを systemd の既定の前に置く。(c) 同じディレクトリに 2 つ(pdftotext と curl)
    /// あっても、PATH にそのディレクトリが 2 度出ても、書くのは 1 回。既定に含まれる /usr/bin
    /// は、そこで何が見つかっても前には足さない(見つかった道具の名は機械の poppler の有無で
    /// 変わるので、ここでは pdftotext と curl が「無い」に挙がらないことだけを見る)。
    #[test]
    fn tools_found_in_path_put_their_directory_before_the_systemd_default_once() {
        let dir = tool_dir("found", &["pdftotext", "curl"]);
        let path_env = format!("{0}:/usr/bin:{0}", dir.display());
        let tools = tool_path(&path_env);
        assert_eq!(
            tools.value,
            format!(
                "{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                dir.display()
            )
        );
        let missing = names(&tools.missing);
        assert!(!missing.contains(&"pdftotext"), "{missing:?}");
        assert!(!missing.contains(&"curl"), "{missing:?}");
        let rendered = drop_ins(&sample_options(), &tools.value).expect("描ける");
        assert_eq!(rendered.len(), 3);
        for (unit, text) in &rendered {
            assert!(
                text.contains(&format!(
                    "\n[Service]\nEnvironment=PATH={}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n",
                    dir.display()
                )),
                "{unit} の drop-in にも同じ PATH:\n{text}"
            );
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// (b) 何も見つからない PATH では既定だけになり、4 つとも見つからなかった道具として名が
    /// 挙がる。相対の項(`.` や空の項)は unit からは別の場所を指すので見ない。
    #[test]
    fn a_path_without_any_tool_yields_the_default_alone_and_names_every_missing_tool() {
        let dir = tool_dir("empty", &[]);
        let tools = tool_path(&format!("{}:.:", dir.display()));
        assert_eq!(tools.value, SYSTEMD_DEFAULT_PATH);
        assert_eq!(
            names(&tools.missing),
            vec!["pdftotext", "pdftohtml", "pdftoppm", "curl"]
        );
        assert!(tools.beside_pdftotext.is_empty());
        assert_eq!(tool_path("").value, SYSTEMD_DEFAULT_PATH);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// PATH には pdftotext の symlink だけがあり、poppler の一式は別の場所に展開してある
    /// 置き方(この機械の姿)では、PATH に足すのは symlink のあるディレクトリで、隣にある
    /// pdftoppm・pdftohtml は「無い」ではなく「隣にある」と判じる。curl は隣を見ない。
    #[test]
    fn poppler_next_to_the_real_pdftotext_is_reachable_without_being_added_to_path() {
        let real = tool_dir("real", &["pdftotext", "pdftoppm", "pdftohtml", "curl"]);
        let linked = tool_dir("linked", &[]);
        std::os::unix::fs::symlink(real.join("pdftotext"), linked.join("pdftotext"))
            .expect("symlink");
        let tools = tool_path(&linked.display().to_string());
        assert!(
            tools.value.starts_with(&format!("{}:/usr/local/sbin:", linked.display())),
            "{}",
            tools.value
        );
        assert!(!tools.value.contains(&real.display().to_string()), "{}", tools.value);
        assert_eq!(
            tools
                .beside_pdftotext
                .iter()
                .map(|(d, dir)| (d.binary, dir.clone()))
                .collect::<Vec<_>>(),
            vec![("pdftohtml", real.clone()), ("pdftoppm", real.clone())]
        );
        assert_eq!(names(&tools.missing), vec!["curl"]);
        std::fs::remove_dir_all(&real).expect("cleanup");
        std::fs::remove_dir_all(&linked).expect("cleanup");
    }

    /// ExecStart= の差し替えは先頭の道だけを替え、残りの並びを unit から写す。
    #[test]
    fn exec_start_rewrite_replaces_only_the_binary() {
        let rewritten =
            exec_start_with_binary(unit_text(SERVE_UNIT), "/opt/u/uniqnode").expect("rewrite");
        assert_eq!(
            rewritten,
            "/opt/u/uniqnode serve ${UNIQNODE_DATA_DIR} ${UNIQNODE_LISTEN} $UNIQNODE_SERVE_OPTIONS"
        );
        assert!(exec_start_with_binary("[Service]\nType=exec\n", "/x").is_err());
    }

    /// ロックの探り: ストアを開いている間は「別プロセスが開いている」になり、閉じれば戻る。
    /// 無いディレクトリは誰も開けないので false。
    ///
    /// 「閉じれば false」は drop の直後ではなく、上限付きで false になるまで待って見る。
    /// 同じテストプロセスの別スレッドが子プロセスを spawn している最中だと、fork→exec の窓で
    /// ロックの FD の複製が子に渡り、drop したロックの名前をその複製が一瞬握り続けるので、
    /// 探りは本当に「持ち主が居る」を見る(docs/analysis/20260816-lock-inheritance-race.md。
    /// 複製の解放まで実測最悪 3.6ms)。探りの側は持ち主の正体を見分けられないので、待つのは
    /// テストの側である。上限は acquire_lock と同じ 250ms(実測最悪値の約 70 倍)で、条件が
    /// 立った瞬間に進み、上限まで塞がっていれば本物の取り残しとして落とす(should/0104)。
    #[test]
    fn the_lock_probe_sees_an_open_store_and_a_closed_one() {
        let dir = std::env::temp_dir().join(format!(
            "uniqnode-install-unit-{}-lock-probe",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        assert!(
            !store::opened_by_another_process(&dir).expect("probe"),
            "無いディレクトリ"
        );
        let held = store::Store::open(store::StoreConfig::new(&dir)).expect("open");
        assert!(
            store::opened_by_another_process(&dir).expect("probe"),
            "開いている間は true"
        );
        drop(held);
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            if !store::opened_by_another_process(&dir).expect("probe") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "閉じれば false(250ms 待っても別プロセスが開いていると見える)"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
