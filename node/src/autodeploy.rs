//! 自動更新(docs/mop/SYSTEMD.md の「自動更新」)。
//!
//! - `uniqnode autodeploy <checkout> --bin <path> [--state-dir <dir>] [--publish-branch <name>]`
//!   は、常駐と同じ利用者で走る 1 回のループである。main を追い、組み直したバイナリへ
//!   `<path>` を置き換え、常駐(serve と viewer)が自己再起動(node/src/selfreload.rs)で新しい
//!   版に替わったことを確かめる。sudo を使わない。
//! - `uniqnode install-autodeploy <checkout> [--user <name>] [--bin <path>] [--publish-branch <name>]`
//!   は、そのループを 1 分ごとに起こす system 単位の service と timer をデプロイする(root で一回)。
//!
//! lamalium の crystal の自動デプロイ(lamalium の docs/design/CQ-DEPLOY.md)と同じ形で、日々の
//! 更新の経路に root を置かない(lamalium の policy/must/sudo-and-policy-authority)。

use crate::install::{self, Account, Scope};
use crate::selfreload::FileIdentity;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const SERVICE_UNIT: &str = "uniqnode-autodeploy.service";
pub const TIMER_UNIT: &str = "uniqnode-autodeploy.timer";

/// デプロイする unit(名, 中身)。中身は docs/mop/systemd/system/ の現物である(install と同じく
/// include_str!。should/0112)。
pub const UNITS: [(&str, &str); 2] = [
    (
        SERVICE_UNIT,
        include_str!("../../docs/mop/systemd/system/uniqnode-autodeploy.service"),
    ),
    (
        TIMER_UNIT,
        include_str!("../../docs/mop/systemd/system/uniqnode-autodeploy.timer"),
    ),
];

/// 状態の置き場の既定(利用者の home からの道)。
pub const STATE_SUBDIR: &str = ".local/state/uniqnode-autodeploy";
/// これがあれば何もしない(手で作業する間、自動更新を止める)。
pub const HOLD_NAME: &str = "hold";
/// デプロイした SHA と失敗した SHA の記録。
pub const RECORD_NAME: &str = "record";
/// 同時に 2 つの回が走らないためのロック。
pub const LOCK_NAME: &str = "lock";
/// 試した回のログの置き場(状態の置き場の下)。
pub const LOGS_NAME: &str = "logs";
/// 置き換える前のバージョンを残す名(`<bin>` の隣)。
pub const PREVIOUS_SUFFIX: &str = ".prev";
/// 確かめの対象の unit。
pub const WATCHED_UNITS: [&str; 2] = ["uniqnode-serve@*.service", "uniqnode-viewer@*.service"];
/// 置き換えから、全ての常駐が新しい版で落ち着くまでの上限。
pub const VERIFY_BOUND: Duration = Duration::from_secs(120);
/// 新しい版で起きた後、落ちずに居続けることを見る長さ。
pub const STABLE_FOR: Duration = Duration::from_secs(15);

/// 記録。1 行 1 項目の `<鍵> <SHA>`。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Record {
    /// 最後にデプロイした(常駐が新しい版に替わったと確かめた)SHA。
    pub deployed: Option<String>,
    /// 最後に publish branch へ push できた SHA。
    pub published: Option<String>,
    /// 最後に失敗した SHA(試し直さない。main が進めば試す)。
    pub failed: Option<String>,
    /// 失敗の後に前のバージョンへ戻した SHA。
    pub rolled_back: Option<String>,
}

const RECORD_KEYS: [&str; 4] = ["deployed", "published", "failed", "rolled_back"];

impl Record {
    pub fn parse(text: &str) -> Result<Record, String> {
        let mut record = Record::default();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let (key, value) = line
                .split_once(' ')
                .ok_or_else(|| format!("cannot read a record line: {line:?}"))?;
            let value = Some(value.trim().to_string());
            match key {
                "deployed" => record.deployed = value,
                "published" => record.published = value,
                "failed" => record.failed = value,
                "rolled_back" => record.rolled_back = value,
                _ => return Err(format!("unknown key in the record: {line:?} (the keys are {RECORD_KEYS:?})")),
            }
        }
        Ok(record)
    }

    pub fn render(&self) -> String {
        let mut text = String::new();
        for (key, value) in RECORD_KEYS.iter().zip([
            &self.deployed,
            &self.published,
            &self.failed,
            &self.rolled_back,
        ]) {
            if let Some(value) = value {
                text.push_str(&format!("{key} {value}\n"));
            }
        }
        text
    }
}

/// 1 回のループで何をするか。
#[derive(Debug, PartialEq)]
pub enum Next {
    /// デプロイ済みか、失敗した SHA のまま。
    Nothing,
    /// デプロイしたが publish branch への push が残っている。
    Publish,
    /// 新しい SHA をデプロイする。
    Deploy,
}

/// 判断はここ 1 箇所(should/0135)。publishing は publish branch の指定があるか。
pub fn decide(record: &Record, target: &str, publishing: bool) -> Next {
    let is = |value: &Option<String>| value.as_deref() == Some(target);
    if is(&record.deployed) {
        if publishing && !is(&record.published) {
            Next::Publish
        } else {
            Next::Nothing
        }
    } else if is(&record.failed) {
        Next::Nothing
    } else {
        Next::Deploy
    }
}

pub struct Options {
    pub repo: PathBuf,
    pub binary: PathBuf,
    pub state_dir: PathBuf,
    pub publish_branch: Option<String>,
}

/// 標準出力と、試した回だけ開くログの両方へ行を書く。
struct Journal {
    file: Option<std::fs::File>,
}

impl Journal {
    fn line(&mut self, text: &str) {
        println!("autodeploy: {text}");
        if let Some(file) = &mut self.file {
            let stamp = crate::clock::format_unix_time(crate::clock::unix_now());
            for line in text.lines() {
                // ログに書けなくても回は進める。標準出力(journal)には同じ行が出ている。
                let _ = writeln!(file, "{stamp} {line}");
            }
        }
    }
}

fn git(repo: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("cannot start git {}: {e}", arguments.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} ended with {}: {}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn identity(path: &Path) -> Result<FileIdentity, String> {
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn write_record(options: &Options, record: &Record) -> Result<(), String> {
    let path = options.state_dir.join(RECORD_NAME);
    let staging = options.state_dir.join(format!(".{RECORD_NAME}.new"));
    std::fs::write(&staging, record.render())
        .map_err(|e| format!("cannot write {}: {e}", staging.display()))?;
    std::fs::rename(&staging, &path).map_err(|e| format!("cannot put {} in place: {e}", path.display()))
}

/// 1 回のループ。Err は理由を言って 1 で終わる回(デプロイしなかった、または前のバージョンへ戻した)。
pub fn run(options: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&options.state_dir)
        .map_err(|e| format!("cannot create {}: {e}", options.state_dir.display()))?;
    let hold = options.state_dir.join(HOLD_NAME);
    if hold.exists() {
        println!("autodeploy: {} exists, so doing nothing", hold.display());
        return Ok(());
    }
    let lock_path = options.state_dir.join(LOCK_NAME);
    let lock = std::fs::File::create(&lock_path)
        .map_err(|e| format!("cannot open {}: {e}", lock_path.display()))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            println!("autodeploy: another run is in progress, so doing nothing ({})", lock_path.display());
            return Ok(());
        }
        Err(std::fs::TryLockError::Error(e)) => {
            return Err(format!("cannot take the lock on {}: {e}", lock_path.display()))
        }
    }
    let record_path = options.state_dir.join(RECORD_NAME);
    let mut record = match std::fs::read_to_string(&record_path) {
        Ok(text) => Record::parse(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Record::default(),
        Err(e) => return Err(format!("cannot read {}: {e}", record_path.display())),
    };
    // 取得の失敗は記録しない(一時の失敗でありうる。次の回にやり直す)。
    git(&options.repo, &["fetch", "--quiet", "origin", "main"])?;
    let target = git(&options.repo, &["rev-parse", "origin/main"])?;
    match decide(&record, &target, options.publish_branch.is_some()) {
        Next::Nothing => Ok(()),
        Next::Publish => {
            let mut journal = Journal { file: None };
            publish(options, &target, &mut record, &mut journal)
        }
        Next::Deploy => deploy(options, &target, &mut record),
    }
}

fn publish(options: &Options, target: &str, record: &mut Record, journal: &mut Journal) -> Result<(), String> {
    let Some(branch) = &options.publish_branch else {
        return Ok(());
    };
    // push が失敗してもデプロイした事実は変わらない。記録の published が進まないので次の回にやり直す。
    git(&options.repo, &["push", "--quiet", "origin", &format!("{target}:refs/heads/{branch}")])?;
    record.published = Some(target.to_string());
    write_record(options, record)?;
    journal.line(&format!("advanced {branch} to {target}"));
    Ok(())
}

fn deploy(options: &Options, target: &str, record: &mut Record) -> Result<(), String> {
    // checkout の形の誤りは手元の問題であって SHA の失敗ではない。記録せず、ログも開かずに言う。
    let branch = git(&options.repo, &["symbolic-ref", "--short", "HEAD"])?;
    if branch != "main" {
        return Err(format!("{} is on branch {branch} (only main is followed)", options.repo.display()));
    }
    let dirty = git(&options.repo, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.is_empty() {
        return Err(format!("{} has uncommitted changes: {dirty}", options.repo.display()));
    }
    git(&options.repo, &["merge-base", "--is-ancestor", "HEAD", &target])
        .map_err(|e| format!("main of {} is not an ancestor of origin/main: {e}", options.repo.display()))?;

    let logs = options.state_dir.join(LOGS_NAME);
    std::fs::create_dir_all(&logs).map_err(|e| format!("cannot create {}: {e}", logs.display()))?;
    let stamp = crate::clock::format_unix_time(crate::clock::unix_now()).replace([':', '-'], "");
    let short = &target[..target.len().min(12)];
    let log_path = logs.join(format!("{stamp}-{short}.log"));
    let file = std::fs::File::create(&log_path)
        .map_err(|e| format!("cannot create {}: {e}", log_path.display()))?;
    let mut journal = Journal { file: Some(file) };
    journal.line(&format!("deploying {target}; log: {}", log_path.display()));

    match build_and_replace(options, target, &mut journal) {
        Ok(()) => {
            record.deployed = Some(target.to_string());
            write_record(options, record)?;
            journal.line(&format!("deployed {target}"));
            if let Err(message) = publish(options, target, record, &mut journal) {
                journal.line(&format!("cannot advance the publish branch (retrying on the next run): {message}"));
            }
            Ok(())
        }
        Err(failure) if failure.of_sha => {
            record.failed = Some(target.to_string());
            if failure.rolled_back {
                record.rolled_back = Some(target.to_string());
            }
            write_record(options, record)?;
            journal.line(&format!(
                "cannot deploy {target}: {}; this SHA is not retried (to retry by hand, delete the failed line in {})",
                failure.message,
                options.state_dir.join(RECORD_NAME).display()
            ));
            Err(failure.message)
        }
        Err(failure) => {
            journal.line(&format!("cannot deploy {target} (not recorded; retrying on the next run): {}", failure.message));
            Err(failure.message)
        }
    }
}

struct Failure {
    message: String,
    /// その SHA の失敗(ビルドか確かめ)か。そうでない失敗(systemctl や写しの一時の失敗)は
    /// 記録せず、次の回にやり直す。
    of_sha: bool,
    rolled_back: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Failure {
        Failure { message, of_sha: false, rolled_back: false }
    }
}

fn sha_failure(message: String) -> Failure {
    Failure { message, of_sha: true, rolled_back: false }
}

fn build_and_replace(options: &Options, target: &str, journal: &mut Journal) -> Result<(), Failure> {
    git(&options.repo, &["merge", "--ff-only", "--quiet", target])?;
    // 組んだものの置き場を決めておく(利用者の cargo の設定が target-dir を変えていても読み違えない)。
    let target_dir = options.repo.join("target");
    journal.line("cargo build --release --locked -p uniqnode");
    let output = Command::new("cargo")
        .args(["build", "--release", "--locked", "-p", "uniqnode", "--target-dir"])
        .arg(&target_dir)
        .current_dir(&options.repo)
        .output()
        .map_err(|e| format!("cannot start cargo: {e}"))?;
    journal.line(String::from_utf8_lossy(&output.stderr).trim_end());
    if !output.status.success() {
        return Err(sha_failure(format!("cargo build ended with {}", output.status)));
    }
    let built = target_dir.join("release").join("uniqnode");
    let help = Command::new(&built)
        .arg("--help")
        .output()
        .map_err(|e| format!("cannot start {}: {e}", built.display()))?;
    if !help.status.success() {
        return Err(sha_failure(format!("{} --help ended with {}", built.display(), help.status)));
    }
    // 置き換える前に対象を挙げる。置き換えた後では古い版の /proc/<pid>/exe が " (deleted)" になる。
    // 対象は今の <bin> を走らせている unit だけ(別のバイナリで動く unit を待たない)。
    let units = units_running(&options.binary)?;
    journal.line(&format!("units to verify: {}", if units.is_empty() { "none".to_string() } else { units.join(" ") }));
    let new_bytes = std::fs::read(&built).map_err(|e| format!("cannot read {}: {e}", built.display()))?;
    if std::fs::read(&options.binary).is_ok_and(|old| old == new_bytes) {
        // 置き換えない(常駐を起こし直さない)。前の回が確かめの途中で切れていても、ここで確かめる。
        journal.line(&format!("{} has the same content, so not replacing it", options.binary.display()));
        let elapsed = verify(&units, &options.binary).map_err(sha_failure)?;
        journal.line(&format!("all units are settled on this version ({} s)", elapsed.as_secs()));
        return Ok(());
    }
    let previous = previous_path(&options.binary);
    if options.binary.exists() {
        journal.line(&install::place_binary(&options.binary, &previous, None)?);
    }
    journal.line(&install::place_binary(&built, &options.binary, None)?);
    let message = match verify(&units, &options.binary) {
        Ok(elapsed) => {
            journal.line(&format!("all units stayed settled on the new version for {} s ({} s)", STABLE_FOR.as_secs(), elapsed.as_secs()));
            return Ok(());
        }
        Err(message) => message,
    };
    if !previous.exists() {
        return Err(sha_failure(format!("{message}; there is no previous version, so not rolling back")));
    }
    match install::place_binary(&previous, &options.binary, None) {
        Ok(line) => {
            journal.line(&format!("rolled back to the previous version: {line}"));
            // 戻した版で落ち着いたかはログに残すだけにする(戻した事実は変わらない)。
            match verify(&units, &options.binary) {
                Ok(elapsed) => journal.line(&format!("all units settled on the previous version ({} s)", elapsed.as_secs())),
                Err(e) => journal.line(&format!("the previous version does not settle either: {e}")),
            }
            Err(Failure { message: format!("{message}; rolled back to the previous version"), of_sha: true, rolled_back: true })
        }
        Err(e) => Err(sha_failure(format!("{message}; cannot roll back to the previous version: {e}"))),
    }
}

pub fn previous_path(binary: &Path) -> PathBuf {
    let mut name = binary.as_os_str().to_os_string();
    name.push(PREVIOUS_SUFFIX);
    PathBuf::from(name)
}

/// 動いている serve と viewer の unit(機械の systemd の)のうち、MainPID が今の binary を
/// 走らせているもの。別のバイナリで動く unit や、既に名の外れた古い版で動く unit は含めない。
fn units_running(binary: &Path) -> Result<Vec<String>, String> {
    let Ok(current) = identity(binary) else {
        return Ok(Vec::new());
    };
    let mut arguments = vec!["list-units", "--plain", "--no-legend", "--state=active"];
    arguments.extend(WATCHED_UNITS);
    let listing = systemctl(&arguments)?;
    let mut units = Vec::new();
    for unit in listing.lines().filter_map(|line| line.split_whitespace().next()) {
        let shown = systemctl(&["show", "-p", "MainPID", unit])?;
        let running = show_value(&shown, "MainPID")
            .and_then(|pid| pid.parse::<u32>().ok())
            .filter(|pid| *pid > 0)
            .and_then(|pid| identity(Path::new(&format!("/proc/{pid}/exe"))).ok());
        if running == Some(current) {
            units.push(unit.to_string());
        }
    }
    Ok(units)
}

fn systemctl(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("systemctl")
        .args(arguments)
        .output()
        .map_err(|e| format!("cannot start systemctl {}: {e}", arguments.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "systemctl {} ended with {}: {}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// `systemctl show -p` の `鍵=値` の行から値を引く。
pub fn show_value<'a>(listing: &'a str, key: &str) -> Option<&'a str> {
    listing
        .lines()
        .find_map(|line| line.strip_prefix(key).and_then(|rest| rest.strip_prefix('=')))
}

/// Environment= の値の並びから、最後の `KEY=値` の値を引く(値に空白の無いものだけ)。
pub fn environment_value<'a>(environment: &'a str, key: &str) -> Option<&'a str> {
    environment
        .split_whitespace()
        .filter_map(|word| word.strip_prefix(key).and_then(|rest| rest.strip_prefix('=')))
        .last()
}

/// unit の状態の 1 回分: active で、MainPID のバイナリが新しい版か。返り値は MainPID。
fn unit_on_new_binary(unit: &str, new: FileIdentity) -> Result<u32, String> {
    let listing = systemctl(&["show", "-p", "ActiveState", "-p", "MainPID", "-p", "Environment", unit])?;
    let state = show_value(&listing, "ActiveState").unwrap_or("");
    if state != "active" {
        return Err(format!("{unit} is {state}"));
    }
    let pid: u32 = show_value(&listing, "MainPID")
        .and_then(|pid| pid.parse().ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| format!("{unit} has no MainPID"))?;
    if identity(Path::new(&format!("/proc/{pid}/exe")))? != new {
        return Err(format!("{unit} (pid {pid}) is still the previous version"));
    }
    // serve は主の口の /v1/status に答えるまで見る(同じ利用者なので主の口が通す)。
    if unit.starts_with("uniqnode-serve@") {
        let environment = show_value(&listing, "Environment").unwrap_or("");
        let listen = environment_value(environment, "UNIQNODE_LISTEN")
            .ok_or_else(|| format!("the Environment of {unit} has no UNIQNODE_LISTEN"))?;
        install::status_node_id(listen).map_err(|e| format!("/v1/status of {unit} ({listen}): {e}"))?;
    }
    Ok(pid)
}

/// 全ての unit が新しい版で active になり、STABLE_FOR の間 MainPID が変わらないことを見る。
fn verify(units: &[String], binary: &Path) -> Result<Duration, String> {
    const TICK: Duration = Duration::from_secs(1);
    let new = identity(binary)?;
    let started = Instant::now();
    let mut settled: Option<(Instant, Vec<u32>)> = None;
    loop {
        let observed: Result<Vec<u32>, String> =
            units.iter().map(|unit| unit_on_new_binary(unit, new)).collect();
        match (&observed, &settled) {
            (Ok(pids), Some((since, settled_pids))) if pids == settled_pids => {
                if since.elapsed() >= STABLE_FOR {
                    return Ok(started.elapsed());
                }
            }
            (Ok(pids), _) => settled = Some((Instant::now(), pids.clone())),
            (Err(_), _) => settled = None,
        }
        if started.elapsed() >= VERIFY_BOUND {
            return Err(format!(
                "not all units settled on the new version within {} s: {}",
                VERIFY_BOUND.as_secs(),
                match observed {
                    Ok(_) => "the MainPID changed before settling".to_string(),
                    Err(e) => e,
                }
            ));
        }
        std::thread::sleep(TICK);
    }
}

/// install-autodeploy の指定。
pub struct InstallOptions {
    pub repo: PathBuf,
    pub account: Account,
    pub binary: PathBuf,
    pub publish_branch: Option<String>,
    pub unit_dir: PathBuf,
    pub start: bool,
}

/// 値に空白があると、drop-in の UNIQNODE_AUTODEPLOY_OPTIONS を ExecStart= が空白で割るときに
/// 崩れるので断る。
fn plain_value(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(format!("{what} {value:?} must be a non-empty value without whitespace"));
    }
    Ok(())
}

/// service の drop-in の中身。
pub fn service_drop_in(options: &InstallOptions, path_env: &str) -> Result<String, String> {
    let repo = options.repo.to_string_lossy();
    let binary = options.binary.to_string_lossy();
    plain_value("checkout path", &repo)?;
    plain_value("--bin", &binary)?;
    let mut autodeploy_options = format!("--bin {binary}");
    if let Some(branch) = &options.publish_branch {
        plain_value("--publish-branch", branch)?;
        autodeploy_options.push_str(&format!(" --publish-branch {branch}"));
    }
    let unit = UNITS[0].1;
    Ok(format!(
        "# drop-in written by uniqnode install-autodeploy; rewritten on each run.\n\
         [Service]\n\
         User={}\n\
         Group={}\n\
         {}\n\
         {}\n\
         {}\n\
         ExecStart=\n\
         ExecStart={}\n",
        install::unit_word(&options.account.name)?,
        install::unit_word(&options.account.group)?,
        install::environment_line("UNIQNODE_AUTODEPLOY_REPO", &repo)?,
        install::environment_line("UNIQNODE_AUTODEPLOY_OPTIONS", &autodeploy_options)?,
        install::environment_line("PATH", path_env)?,
        install::exec_start_with_binary(unit, &install::unit_word(&binary)?)?,
    ))
}

/// root の側は git を走らせない(利用者の書ける .git/config を root で実行しない)。持ち主と
/// 形だけを見る。git の木として正しいかは、利用者で走る回が毎回確かめる。
pub fn check_checkout(repo: &Path, account: &Account) -> Result<(), String> {
    if !repo.is_absolute() {
        return Err(format!("checkout {} must be an absolute path", repo.display()));
    }
    let metadata =
        std::fs::metadata(repo).map_err(|e| format!("cannot read checkout {}: {e}", repo.display()))?;
    if metadata.uid() != account.uid {
        return Err(format!(
            "checkout {} is not owned by {} (uid {})",
            repo.display(),
            account.name,
            metadata.uid()
        ));
    }
    for name in [".git", "Cargo.toml"] {
        if !repo.join(name).exists() {
            return Err(format!("checkout {} has no {name}", repo.display()));
        }
    }
    Ok(())
}

/// unit の PATH: 利用者の ~/.cargo/bin を systemd の既定の前に足す。cargo がどちらにも
/// 無ければ断る。
pub fn path_with_cargo(account: &Account) -> Result<String, String> {
    let cargo_dir = account.home.join(".cargo").join("bin");
    let path = format!("{}:{}", cargo_dir.display(), install::SYSTEMD_DEFAULT_PATH);
    install::find_in_path("cargo", &path).ok_or_else(|| {
        format!("cargo is neither in {} nor on the default systemd PATH", cargo_dir.display())
    })?;
    Ok(path)
}

pub fn install_units(options: &InstallOptions, out: &mut dyn Write) -> Result<(), String> {
    let say = |out: &mut dyn Write, line: &str| -> Result<(), String> {
        writeln!(out, "install-autodeploy: {line}").map_err(|e| format!("cannot write to standard output: {e}"))
    };
    check_checkout(&options.repo, &options.account)?;
    if !options.binary.is_file() {
        return Err(format!(
            "the binary {} does not exist (deploy it first with uniqnode install)",
            options.binary.display()
        ));
    }
    let path_env = path_with_cargo(&options.account)?;
    let drop_in = service_drop_in(options, &path_env)?;
    for (name, text) in UNITS {
        let path = options.unit_dir.join(name);
        install::write_file(&path, text)?;
        say(out, &format!("unit {}", path.display()))?;
    }
    let drop_in_path = options
        .unit_dir
        .join(format!("{SERVICE_UNIT}.d"))
        .join(install::DROP_IN_NAME);
    install::write_file(&drop_in_path, &drop_in)?;
    say(out, &format!("drop-in {}", drop_in_path.display()))?;
    if !options.start {
        say(out, "--no-start given, so not running daemon-reload or starting the timer")?;
        return Ok(());
    }
    let scope = Scope::System(options.account.clone());
    install::systemctl_ok(&scope, &["daemon-reload"])?;
    install::systemctl_ok(&scope, &["enable", "--now", TIMER_UNIT])?;
    let state = install::systemctl(&scope, &["is-active", TIMER_UNIT])?;
    let state = String::from_utf8_lossy(&state.stdout).trim().to_string();
    if state != "active" {
        return Err(format!("{TIMER_UNIT} did not become active ({state})"));
    }
    say(
        out,
        &format!(
            "{TIMER_UNIT} is active; following main every minute as {} (logs in {STATE_SUBDIR}/{LOGS_NAME}/ under {}, \
             output of each run in journalctl -u {SERVICE_UNIT})",
            options.account.name,
            options.account.home.display()
        ),
    )?;
    Ok(())
}
