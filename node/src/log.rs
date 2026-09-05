//! 運用ログの出口(should/0135: どこへ、どんな形で出すかの判断はこの module にしかない)。
//!
//! serve・mcp・viewer は常駐し、そこに出る診断の行は後から読むためのデバッグ資料である。
//! 標準エラーだけに出すと、前景を離れた瞬間に失われる。mcp の標準エラーは登録した
//! LLM クライアント(Claude Code など)が吸うので、利用者の目には最初から触れない。
//! そこで、この 3 つの命令は既定でファイルにも残す。シェルのリダイレクトを忘れたら
//! 失われる、という置き方をしない。
//!
//! 出す側は log_line! だけを使う。行の前に立つ時刻と pid も、標準エラーとファイルの
//! どちらへ出すかも、回転の時機も、ここでしか決めない。
//!
//! 使い分け: 常駐する serve・mcp・viewer の診断は log_line! を通す。usage の説明文と、
//! 一回きりの命令が返す結果の行(fsck の異常一覧、backup の写した一覧と検証、sync の集計、
//! cert-verify の判定)は
//! その命令の出力であってログではないので、これまで通り println!/eprintln! で出す。

use crate::clock;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// ログを置くディレクトリ(データディレクトリの直下)。
pub const DIRECTORY_NAME: &str = "logs";

/// serve のログの役割名(そのままファイル名になる)。
pub const SERVE_ROLE: &str = "serve";

/// mcp のログの役割名。同じデータディレクトリに対して serve・mcp・viewer が同時に走るので、
/// 役割ごとに別のファイルへ書く。
pub const MCP_ROLE: &str = "mcp";

/// viewer のログの役割名。ビューワも serve と同時に走る(転送する形なので錠を取らない)。
pub const VIEWER_ROLE: &str = "viewer";

/// 1 世代の上限(既定)。実測(2026-08-17、25443 オブジェクトの実コーパス)では、
/// 待つだけの serve は 60 秒で 1 行も出さず、検索 1 件あたり 189 バイト、埋め込みサーバに
/// 届かない最悪の状態でも 355 バイトである。時刻と pid の前置を足しても 1 件 430 バイトを
/// 越えないので、8 MiB は検索およそ 2 万件分にあたる。保持する世代と合わせて、
/// ディスクの上限は 8 MiB × (1 + 4) = 40 MiB で頭打ちになる。
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 保持する古い世代の数。serve.log.1 から serve.log.4 までが残り、それより古いものは
/// 回転のたびに消える。
pub const RETAINED_GENERATIONS: u32 = 4;

/// 既定の保存先(<data_dir>/logs/<役割>.log)。
pub fn default_path(data_dir: &Path, role: &str) -> PathBuf {
    data_dir.join(DIRECTORY_NAME).join(format!("{role}.log"))
}

/// 世代のファイル名(serve.log の 1 世代前は serve.log.1)。拡張子を置き換えず後ろに
/// 足すのは、serve.log.1 が「serve.log の 1 つ前」と読めるためである。
pub fn generation_path(path: &Path, number: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{number}"));
    PathBuf::from(name)
}

/// 開いているログファイル。プロセスに 1 つだけ持つ。
static DESTINATION: Mutex<Option<Destination>> = Mutex::new(None);

/// 明示された道(--log)にログの保存を始める。開けなければ理由を返す(呼び手が、黙って
/// 落とさずに知らせる。must/0022)。明示された道は別の場所へ倒さない: 利用者が指した道を
/// 黙って変えると、指した所を読みに行った者が何も見つけられない。倒すのは既定の道のとき
/// だけである(open_default)。
pub fn open(path: &Path, max_bytes: u64) -> Result<(), String> {
    let destination = Destination::open(path.to_path_buf(), max_bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    *lock() = Some(destination);
    Ok(())
}

/// 既定の道(<data_dir>/logs/<役割>.log)にログの保存を始め、実際に開いた道を返す。
///
/// 既定の道が開けないとき(ディレクトリを作れない・書けない)は、利用者の書ける場所
/// (fallback_path)へ倒す。system 単位のストア(0750、所有者 uniqnode)に人間の権限で mcp を
/// 起こすと既定の道には書けず、標準エラーだけにするとそれは LLM クライアントが吸うので誰も
/// 読めない、という穴を塞ぐ。倒したときはその事実と両方の道を、この走行の最初の 1 行として
/// 標準エラーとファイルの双方に出す(どこに残したかが、どちらの記録からも読める)。
///
/// どちらにも開けなければ理由を返す(両方の道とそれぞれの失敗)。倒す先を決めるのも、
/// 倒すか否かの判断も、ここにしかない(should/0135。serve・mcp・viewer が同じ関数を呼ぶ)。
pub fn open_default(data_dir: &Path, role: &str, max_bytes: u64) -> Result<PathBuf, String> {
    let preferred = default_path(data_dir, role);
    let refused = match Destination::open(preferred.clone(), max_bytes) {
        Ok(destination) => {
            *lock() = Some(destination);
            return Ok(preferred);
        }
        Err(error) => error,
    };
    let Some(fallback) = fallback_path(role) else {
        return Err(format!(
            "{}: {refused}。倒す先も無い(XDG_STATE_HOME も HOME も取れない)",
            preferred.display()
        ));
    };
    let destination = Destination::open(fallback.clone(), max_bytes).map_err(|error| {
        format!("{}: {refused}。倒す先 {}: {error}", preferred.display(), fallback.display())
    })?;
    *lock() = Some(destination);
    write_line(format_args!(
        "uniqnode: {role}: 既定のログ先 {} に書けない({refused})ので {} に残す",
        preferred.display(),
        fallback.display()
    ));
    Ok(fallback)
}

/// 既定の道が開けないときに倒す先。$XDG_STATE_HOME/uniqnode/logs/<役割>.log、無ければ
/// ~/.local/state/uniqnode/logs/<役割>.log(systemd の user 単位の %S と同じ決め方。
/// 状態の置き場として利用者が既に持っている場所であり、新しい規約を増やさない)。
/// HOME も取れなければ None(呼び手は従来どおり標準エラーだけで続ける)。
pub fn fallback_path(role: &str) -> Option<PathBuf> {
    fallback_path_from(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"), role)
}

/// fallback_path の判断そのもの(環境を引数にした純関数。テストから環境を差し替えずに
/// 呼べる)。空の値は無いものとして扱う(XDG の規約)。
fn fallback_path_from(
    xdg_state_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    role: &str,
) -> Option<PathBuf> {
    let state_home = match xdg_state_home.filter(|value| !value.is_empty()) {
        Some(state_home) => PathBuf::from(state_home),
        None => PathBuf::from(home.filter(|value| !value.is_empty())?)
            .join(".local")
            .join("state"),
    };
    Some(state_home.join("uniqnode").join(DIRECTORY_NAME).join(format!("{role}.log")))
}

/// 1 行出す(log_line! の実体)。標準エラーへは常に出し、ログファイルが開いていれば
/// 同じ文字列をそこへも書く。両者の内容が同一なのは意図で、前景で見た行がそのまま
/// ファイルに残っていなければ、後から読む者は 2 つの記録を突き合わせられない。
pub fn write_line(arguments: std::fmt::Arguments) {
    let line = format!("{} {arguments}", prefix());
    eprintln!("{line}");
    let mut destination = lock();
    let Some(opened) = destination.as_mut() else { return };
    if let Err(error) = opened.append(&line) {
        // ここで log_line! を呼ぶと自分を呼び戻す(かつ錠を握ったままになる)ので、
        // 直接標準エラーへ書く。書けなくなった事実を黙って飲まず、以後は標準エラー
        // だけに出す(毎行同じ苦情を繰り返さない。must/0022)。
        eprintln!(
            "{} uniqnode: ログを {} に書けない: {error}。以後は標準エラーだけに出す",
            prefix(),
            opened.path.display()
        );
        *destination = None;
    }
}

/// 行頭に立つ時刻と pid。時刻は UTC(機械をまたいで突き合わせられる)。pid を入れるのは、
/// 同じデータディレクトリに対して mcp が同時に何本も走る(LLM クライアントの会話ごとに
/// 1 本)ので、どの会話の行かを後から分けられるようにするためである。
fn prefix() -> String {
    format!("{} [pid {}]", clock::format_unix_time(clock::unix_now()), std::process::id())
}

/// 他のスレッドが panic した後もログは出し続ける。錠が毒されたことを理由に記録を
/// 止めると、panic の前後という最も読みたい部分が残らない。
fn lock() -> MutexGuard<'static, Option<Destination>> {
    DESTINATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 開いているログファイルと、その世代の大きさ。
struct Destination {
    path: PathBuf,
    file: std::fs::File,
    /// 今の世代に書いた量。回転の時機を毎行の metadata 取得なしに判断するために持つ。
    written_bytes: u64,
    max_bytes: u64,
}

impl Destination {
    fn open(path: PathBuf, max_bytes: u64) -> std::io::Result<Destination> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 追記で開く。前回の走行の記録は消さない(消してよいのは回転で溢れた世代だけ)。
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        let written_bytes = file.metadata()?.len();
        Ok(Destination { path, file, written_bytes, max_bytes })
    }

    fn append(&mut self, line: &str) -> std::io::Result<()> {
        let bytes = line.len() as u64 + 1;
        // 1 行だけで上限を越える場合も、その世代に必ず 1 行は書く(空の世代を作りながら
        // 回り続けない)。
        if self.written_bytes > 0 && self.written_bytes + bytes > self.max_bytes {
            self.rotate()?;
        }
        // std::fs::File は buffer を持たないので、1 行ごとにそのまま OS へ渡る。
        // kill -9 で裂かれても直前の行までは残る(機械ごと落ちた場合の末尾は保証しない)。
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.written_bytes += bytes;
        Ok(())
    }

    /// 世代をずらして新しいログを開く。古い方から順に押し出し、溢れた 1 世代を捨てる。
    fn rotate(&mut self) -> std::io::Result<()> {
        remove_if_present(&generation_path(&self.path, RETAINED_GENERATIONS))?;
        for number in (1..RETAINED_GENERATIONS).rev() {
            rename_if_present(
                &generation_path(&self.path, number),
                &generation_path(&self.path, number + 1),
            )?;
        }
        rename_if_present(&self.path, &generation_path(&self.path, 1))?;
        self.file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.written_bytes = 0;
        Ok(())
    }
}

/// 無いものを消そうとしたのは失敗ではない(まだ 4 世代溜まっていない段階)。
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// 同じ理由で、無い世代の改名も失敗ではない。
fn rename_if_present(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// 運用の記録を 1 行出す(標準エラーと、開いていればログファイルの両方へ)。常駐する
/// serve・mcp・viewer の診断は eprintln! ではなくこれを使う。
#[macro_export]
macro_rules! log_line {
    ($($argument:tt)*) => {
        $crate::log::write_line(::std::format_args!($($argument)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-log-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        dir
    }

    /// 上限を越えたら回転し、古い世代は 4 つまで残って、それより古いものは消える。
    /// 期待値はリテラルで書く(検査対象の式から導出しない。should/0137)。
    #[test]
    fn rotation_keeps_four_generations_and_drops_the_oldest() {
        let dir = scratch("rotation");
        let path = default_path(&dir, SERVE_ROLE);
        // 1 行 21 バイト(20 文字 + 改行)、上限 40 バイトなので 2 行で 1 世代。
        let mut destination = Destination::open(path.clone(), 40).expect("open");
        for number in 0..20 {
            destination.append(&format!("line {number:015}")).expect("append");
        }
        assert!(path.exists(), "現行のログが無い: {}", path.display());
        for generation in 1..=RETAINED_GENERATIONS {
            let old = generation_path(&path, generation);
            assert!(old.exists(), "{} 世代前が無い: {}", generation, old.display());
        }
        let overflowed = generation_path(&path, RETAINED_GENERATIONS + 1);
        assert!(
            !overflowed.exists(),
            "保持する世代を越えたファイルが残っている: {}",
            overflowed.display()
        );
        // どの世代も上限のまわりに収まっている(1 行分の超過まで許す)。
        for generation in 1..=RETAINED_GENERATIONS {
            let size = std::fs::metadata(generation_path(&path, generation))
                .expect("metadata")
                .len();
            assert!(size <= 40, "世代 {generation} が上限 40 バイトを越えた: {size}");
        }
        // 最新の行は現行のファイルに居る。
        let current = std::fs::read_to_string(&path).expect("read");
        assert!(current.contains("line 000000000000019"), "最後の行が現行に無い: {current}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 1 行が上限より大きくても、その行は捨てずに書く(空の世代を作りながら回り続けない)。
    #[test]
    fn a_line_longer_than_the_limit_is_still_written() {
        let dir = scratch("longline");
        let path = default_path(&dir, SERVE_ROLE);
        let mut destination = Destination::open(path.clone(), 8).expect("open");
        destination.append("これは上限 8 バイトより長い 1 行である").expect("append");
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(
            written.contains("これは上限 8 バイトより長い 1 行である"),
            "上限より長い行が落ちた: {written}"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 追記で開くので、前回の走行の記録は残る(回転だけが古い行を捨てる)。
    #[test]
    fn reopening_appends_instead_of_truncating() {
        let dir = scratch("append");
        let path = default_path(&dir, SERVE_ROLE);
        let mut first = Destination::open(path.clone(), DEFAULT_MAX_BYTES).expect("open");
        first.append("前回の走行").expect("append");
        drop(first);
        let mut second = Destination::open(path.clone(), DEFAULT_MAX_BYTES).expect("reopen");
        second.append("今回の走行").expect("append");
        let written = std::fs::read_to_string(&path).expect("read");
        assert_eq!(written, "前回の走行\n今回の走行\n", "追記になっていない: {written}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 倒す先の決め方: XDG_STATE_HOME があればその下、無ければ(空も無いとみなして)
    /// HOME/.local/state、どちらも無ければ倒す先は無い。期待値はリテラルで書く。
    #[test]
    fn the_fallback_follows_xdg_state_home_then_home_then_nothing() {
        let os = |text: &str| Some(std::ffi::OsString::from(text));
        assert_eq!(
            fallback_path_from(os("/state"), os("/home/op"), MCP_ROLE),
            Some(PathBuf::from("/state/uniqnode/logs/mcp.log")),
            "XDG_STATE_HOME があるのにそこへ倒していない"
        );
        assert_eq!(
            fallback_path_from(None, os("/home/op"), MCP_ROLE),
            Some(PathBuf::from("/home/op/.local/state/uniqnode/logs/mcp.log")),
            "XDG_STATE_HOME が無いとき HOME/.local/state へ倒していない"
        );
        assert_eq!(
            fallback_path_from(os(""), os("/home/op"), SERVE_ROLE),
            Some(PathBuf::from("/home/op/.local/state/uniqnode/logs/serve.log")),
            "空の XDG_STATE_HOME を「無い」と扱っていない"
        );
        assert_eq!(fallback_path_from(None, None, MCP_ROLE), None, "HOME が無いのに倒す先がある");
        assert_eq!(fallback_path_from(None, os(""), MCP_ROLE), None, "空の HOME を道にしている");
    }

    /// 開けない道は理由つきで断る(呼び手がそれを知らせる。must/0022)。
    #[test]
    fn an_unopenable_path_is_refused_with_the_reason() {
        let dir = scratch("unopenable");
        std::fs::create_dir_all(&dir).expect("mkdir");
        // 通常ファイルの下にはディレクトリを作れない。
        let blocking_file = dir.join("blocked");
        std::fs::write(&blocking_file, b"file, not a directory").expect("write");
        let error = open(&blocking_file.join("logs").join("serve.log"), DEFAULT_MAX_BYTES)
            .expect_err("開けないはず");
        assert!(
            error.contains("blocked"),
            "断りの文がどの道で失敗したかを言っていない: {error}"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
