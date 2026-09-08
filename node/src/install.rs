//! `uniqnode install <data_dir> [options]`: serve・viewer・毎日の backup を systemd に据える
//! 1 命令。user 単位(既定。~/.config/systemd/user)と、`--system` で system 単位
//! (/etc/systemd/system。root で走らせ、常駐は `--user`/SUDO_USER の利用者で)の両方を扱う。
//! 手順は [docs/mop/SYSTEMD.md](uuid:7de68e4a-e6a6-4930-8cc7-a56f90f522e2) で、この命令は
//! 同じ文書の「手で同じことをするなら」を機械にしたもの(should/0117: 手作業で直したものは
//! 再現できる形にして置く)。
//!
//! 置く unit は docs/mop/systemd/user/(user 単位)と docs/mop/systemd/system/(system 単位)
//! の現物をコンパイル時に埋め込んだもの(include_str!。should/0112)。文書と実体を別々に
//! 持たないので、unit を直せばこの命令が置くものも同時に変わる。運用者ごとの値(ストアの
//! 道・待ち受け・写し先・バイナリの道・実行ユーザ・待つ unit・読み口)は unit を編集せず
//! drop-in `<unit>.d/override.conf` に書く。unit の ExecStart= も drop-in で差し替えるが、
//! 引数の並びは埋め込んだ unit の ExecStart= 行を読んでバイナリの道だけ替える(同じ並びを
//! 2 箇所に持たない。must/0023)。
//!
//! 1 手順 1 exec で、失敗は理由を言って止まる(黙って飛ばさない。must/0022)。設定は
//! 書いた時点ではなく効果を見た時点で完了なので(should/0116)、起こした後に serve と
//! viewer 経由の /v1/status が同じ node_id を返すこと、読み口(`--listen-agent`)があれば
//! その /v1/status が同じ node_id を返し POST /v1/admin/gc が 403 であること、書く口
//! (`--agent-writable`)があれば許したコレクションへの PUT が門を越え許していない
//! コレクションへの PUT が 403 であること(書かずに見る)、backup の unit を 1 回走らせた
//! 写し先が開けて fsck が緑であること、system 単位ならストアと写し先に実行ユーザ以外の
//! 所有のファイルが無いことまで見る。

use crate::http;
use crate::json::Json;
use crate::store::{self, FsckReport};
use crate::{fetch, outline, rendition};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// テンプレート unit の名。`@` の後にインスタンス名が入り、同じ機械に何組でも置ける
/// (`uniqnode-serve@default.service`、`uniqnode-serve@graph.service`)。1 台 1 ストアの
/// 形から移った理由は docs/mop/SYSTEMD.md の「2 つ目のストアを同じ機械で」(should/0118:
/// 増やすのは設定の 1 項目であって、ファイルの写しではない)。
pub const SERVE_TEMPLATE: &str = "uniqnode-serve@.service";
pub const VIEWER_TEMPLATE: &str = "uniqnode-viewer@.service";
pub const BACKUP_TEMPLATE: &str = "uniqnode-backup@.service";
pub const BACKUP_TIMER_TEMPLATE: &str = "uniqnode-backup@.timer";

/// テンプレート unit になる前の名。据え先に残っていると、同じ主の口と同じストアを 2 つの
/// unit が取り合うので、既定のインスタンスを据えるときだけ見て断る(run の (5b))。
pub const LEGACY_UNITS: [&str; 4] = [
    "uniqnode-serve.service",
    "uniqnode-viewer.service",
    "uniqnode-backup.service",
    "uniqnode-backup.timer",
];

/// `--instance` を省いたときのインスタンス名。
pub const DEFAULT_INSTANCE: &str = "default";
pub const INSTANCE_FLAG: &str = "--instance";
/// インスタンス名に許す長さ。unit の名・nft の表の名・drop-in の道に入るので短く保つ。
pub const INSTANCE_MAX_CHARS: usize = 32;

/// user 単位で置く unit(テンプレートの名, 中身)。中身は docs/mop/systemd/user/ の現物である。
pub const USER_UNITS: [(&str, &str); 4] = [
    (
        SERVE_TEMPLATE,
        include_str!("../../docs/mop/systemd/user/uniqnode-serve@.service"),
    ),
    (
        VIEWER_TEMPLATE,
        include_str!("../../docs/mop/systemd/user/uniqnode-viewer@.service"),
    ),
    (
        BACKUP_TEMPLATE,
        include_str!("../../docs/mop/systemd/user/uniqnode-backup@.service"),
    ),
    (
        BACKUP_TIMER_TEMPLATE,
        include_str!("../../docs/mop/systemd/user/uniqnode-backup@.timer"),
    ),
];

/// system 単位で置く unit(テンプレートの名, 中身)。中身は docs/mop/systemd/system/ の現物である。
pub const SYSTEM_UNITS: [(&str, &str); 4] = [
    (
        SERVE_TEMPLATE,
        include_str!("../../docs/mop/systemd/system/uniqnode-serve@.service"),
    ),
    (
        VIEWER_TEMPLATE,
        include_str!("../../docs/mop/systemd/system/uniqnode-viewer@.service"),
    ),
    (
        BACKUP_TEMPLATE,
        include_str!("../../docs/mop/systemd/system/uniqnode-backup@.service"),
    ),
    (
        BACKUP_TIMER_TEMPLATE,
        include_str!("../../docs/mop/systemd/system/uniqnode-backup@.timer"),
    ),
];

/// enable --now するもの(service は timer が起こすので backup は timer の方)。
pub const STARTED_TEMPLATES: [&str; 3] = [SERVE_TEMPLATE, VIEWER_TEMPLATE, BACKUP_TIMER_TEMPLATE];

/// テンプレート unit の名にインスタンス名を差し込む(`uniqnode-serve@.service` と `graph` で
/// `uniqnode-serve@graph.service`)。差し込みの規則はここ 1 箇所にある(should/0135)。
pub fn unit_for(template: &str, instance: &str) -> String {
    let (stem, suffix) = template
        .split_once('@')
        .expect("テンプレート unit の名には @ がある");
    format!("{stem}@{instance}{suffix}")
}

/// インスタンス名を検める。ASCII の小文字・数字・`_` だけにするのは、この名が 3 つの場所に
/// そのまま入るからである: systemd の unit の名(`/` は階層の意味を持つ)、nft の表の名
/// (識別子に `-` を使えない)、drop-in のディレクトリの道。3 つとも通る字種は狭い方に
/// 合わせる(should/0135)。
pub fn check_instance(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.chars().count() <= INSTANCE_MAX_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        return Ok(());
    }
    Err(format!(
        "{INSTANCE_FLAG} {name:?} の形が違う(ASCII の小文字と数字と _ の 1..={INSTANCE_MAX_CHARS} 字。         unit の名・nft の表の名・drop-in の道に同じ字が入るので、3 つとも通る字種に限る)"
    ))
}

/// nft の表の名(家族名 inet)。インスタンスごとに別の表にする。1 つの名を共有すると、
/// 規則ファイルが表ごと消して作り直す形なので、後から起きた serve が先の実体の規則を消し、
/// その実体の読み口が誰からでも届くようになる(2026-09-08 に据える前に見つけた)。
pub fn nft_table_for(instance: &str) -> String {
    format!("inet uniqnode_{instance}")
}

/// drop-in のファイル名(`<unit>.d/` の下)。
pub const DROP_IN_NAME: &str = "override.conf";

/// system 単位の unit と drop-in の置き場(`--system` の `--unit-dir` の既定)。
pub const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7440";
pub const DEFAULT_VIEWER_LISTEN: &str = "127.0.0.1:7450";

/// 読み口の待ち受けを serve に渡す環境変数と、serve の引数。drop-in は
/// `Environment=UNIQNODE_AGENT_LISTEN=<addr>` と ExecStart= 末尾の
/// `--listen-agent ${UNIQNODE_AGENT_LISTEN}` を書く(serve 側の引数の名と同じ字句)。
pub const AGENT_LISTEN_ENV: &str = "UNIQNODE_AGENT_LISTEN";
pub const AGENT_LISTEN_FLAG: &str = "--listen-agent";
/// 読み口から書けるコレクション(第 2 段。docs/design/AGENT_DOOR.md)。serve の引数は
/// `--agent-writable <c>` の繰り返しで、drop-in は集合を空白で分けた
/// `Environment=UNIQNODE_AGENT_WRITABLE="<c1> <c2>"` に写し、ExecStart= の末尾に
/// `--agent-writable <c>` を集合の数だけそのまま並べる(環境変数の展開に頼らない。値を
/// そのまま書くのが一番単純で、読み手が unit だけで集合を読める)。
pub const AGENT_WRITABLE_ENV: &str = "UNIQNODE_AGENT_WRITABLE";
pub const AGENT_WRITABLE_FLAG: &str = "--agent-writable";
/// 読み口から読めるコレクション(docs/design/AGENT_DOOR.md の「読める集合」)。書く口と
/// 全く同じ流儀で、drop-in は `Environment="UNIQNODE_AGENT_COLLECTIONS=<c1> <c2>"` と
/// ExecStart= 末尾の `--agent-collections <c>` の並びに写す。指定が無ければ読み口は全
/// コレクションを読める(既定は変えない)。
pub const AGENT_COLLECTIONS_ENV: &str = "UNIQNODE_AGENT_COLLECTIONS";
pub const AGENT_COLLECTIONS_FLAG: &str = "--agent-collections";
/// 読み口から読めるグラフ(docs/design/GRAPH.md)。コレクションの 2 つと全く同じ流儀で、
/// drop-in は `Environment="UNIQNODE_AGENT_GRAPH=<g1> <g2>"` と ExecStart= 末尾の
/// `--agent-graph <g>` の並びに写す。指定が無ければ読み口からグラフ層は見えない
/// (コレクションの既定と逆にしてあるのは、グラフを後から足したからである。既定を開く側に
/// すると、本番の読み口が据え直しただけでグラフを晒す)。
pub const AGENT_GRAPH_ENV: &str = "UNIQNODE_AGENT_GRAPH";
pub const AGENT_GRAPH_FLAG: &str = "--agent-graph";
/// 読み口から書けるグラフ。書ける名は読めもする。
pub const AGENT_GRAPH_WRITABLE_ENV: &str = "UNIQNODE_AGENT_GRAPH_WRITABLE";
pub const AGENT_GRAPH_WRITABLE_FLAG: &str = "--agent-graph-writable";
/// user 単位の常駐を install 自身が止めて外す指定(`--system` のときだけ)。
pub const TAKE_OVER_FLAG: &str = "--take-over-user-units";
/// 読み口へ届いてよい相手を firewall(ufw)に入れる指定(`--system` と `--listen-agent` の
/// ときだけ)。値は相手のアドレス。
pub const FIREWALL_ALLOW_FLAG: &str = "--firewall-allow";
/// user 単位を止めた後、ストアのロックが外れるのを待つ上限。
pub const TAKE_OVER_WAIT: Duration = Duration::from_secs(10);
/// nft に置く表の名は nft_table_for が組む(インスタンスごとに 1 つ)。iptables-nft や
/// docker が持つ表には触らず、自分の表だけを持つ(base chain は表ごとに独立に評価され、
/// どれかの drop が勝つ)。
/// nft の規則ファイルの名(serve の drop-in の隣に置き、ExecStartPre= が読む)。
pub const NFT_RULES_NAME: &str = "agent-door.nft";

/// 起こした後の確認で待つ上限(serve と viewer の /v1/status が揃うまで、読み口が答えるまで)。
/// 読み口は wg1 のような後から上がるインターフェースのアドレスに束縛されることがあり、
/// serve は bind に失敗しても主の口を殺さず再試行するので、その分をここで待つ。上限は安全の
/// 網で、条件が立った瞬間に進む(should/0104)。起動直後の serve は索引を裏で温め、その間
/// ストアのロックを持つので /v1/status も待たされる(本番 55,452 オブジェクトで冷えた初回が
/// 22 秒。docs/design/SEARCH.md の索引の構築の節)。30 秒ではその余裕が無いので 90 秒。
pub const WAIT_BOUND: Duration = Duration::from_secs(90);

/// PrivateTmp=yes の unit からは見えない置き場。ここにストアや写し先を指されたら断る
/// (unit は自分だけの空の /tmp を見るので、起こしても「ストアが無い」で落ちる)。
pub const PRIVATE_TMP_ROOTS: [&str; 2] = ["/tmp", "/var/tmp"];

/// ProtectHome= が隠す置き場。system 単位の unit は ProtectHome=yes(空で見えない)なので、
/// バイナリ・ストア・写し先のどれかがこの下にあるときは drop-in で read-only に緩める
/// (user 単位の unit と同じ値。ReadWritePaths= は read-only の下では効くが、yes の下では
/// 隠された道ごと捨てられる)。
pub const PROTECTED_HOME_ROOTS: [&str; 3] = ["/home", "/root", "/run/user"];

/// system 単位のとき、install 自身の PATH に加えて実行ユーザの home の下で外部の道具を
/// 探すディレクトリ。sudo は PATH を secure_path に置き換えるので、利用者が ~/.local/bin に
/// 置いた pdftotext は root の PATH からは見えない。ログインシェルの既定(Ubuntu の
/// ~/.profile)が PATH に足すのはこの 2 つで、install は同じ場所を後ろに足して探す。
pub const SERVICE_USER_TOOL_DIRS: [&str; 2] = [".local/bin", "bin"];

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

/// system 単位で unit を走らせる利用者。`getent passwd` と `getent group` で引いた実物。
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    /// 主グループの名(drop-in の Group=)。
    pub group: String,
    /// バイナリ・写し先の既定と、外部の道具を探す場所(SERVICE_USER_TOOL_DIRS)の根。
    pub home: PathBuf,
}

/// どの systemd に据えるか。user 単位は自分のマネージャ(`systemctl --user`)に、system 単位は
/// 機械のマネージャに unit を置き、Account の利用者で走らせる。
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    User,
    System(Account),
}

impl Scope {
    pub fn is_system(&self) -> bool {
        matches!(self, Scope::System(_))
    }

    /// system 単位の実行ユーザ。
    pub fn account(&self) -> Option<&Account> {
        match self {
            Scope::User => None,
            Scope::System(account) => Some(account),
        }
    }

    /// systemctl・journalctl に付ける引数(user 単位なら `--user`)。
    pub fn manager_flags(&self) -> &'static [&'static str] {
        match self {
            Scope::User => &["--user"],
            Scope::System(_) => &[],
        }
    }

    /// 報告と文言に書く命令の頭(`systemctl --user ` か `systemctl `)。
    pub fn systemctl_prefix(&self) -> String {
        self.command_prefix("systemctl")
    }

    pub fn journalctl_prefix(&self) -> String {
        self.command_prefix("journalctl")
    }

    fn command_prefix(&self, command: &str) -> String {
        let mut prefix = command.to_string();
        for flag in self.manager_flags() {
            prefix.push(' ');
            prefix.push_str(flag);
        }
        prefix.push(' ');
        prefix
    }

    /// unit と drop-in の置き場の既定。user 単位は home の下、system 単位は /etc/systemd/system。
    pub fn default_unit_dir(&self, home: &Path) -> PathBuf {
        match self {
            Scope::User => home.join(".config").join("systemd").join("user"),
            Scope::System(_) => PathBuf::from(SYSTEM_UNIT_DIR),
        }
    }
}

/// 自分の実効 uid(/proc/self/status の Uid: 行の 2 つ目。Linux の systemd に据える命令なので
/// procfs に頼ってよい)。読めなければ Err(root かどうかを推し量らない。must/0022)。
pub fn effective_uid() -> Result<u32, String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|e| format!("/proc/self/status を読めない: {e}"))?;
    let line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .ok_or_else(|| "/proc/self/status に Uid: 行が無い".to_string())?;
    line.split_whitespace()
        .nth(2)
        .and_then(|euid| euid.parse().ok())
        .ok_or_else(|| format!("/proc/self/status の Uid: 行を読めない: {line}"))
}

/// `getent <database> <key>` の 1 行を `:` で割って返す。無い鍵は Err。
fn getent(database: &str, key: &str) -> Result<Vec<String>, String> {
    let output = Command::new("getent")
        .args([database, key])
        .output()
        .map_err(|e| format!("getent {database} {key} を起こせない: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "getent {database} {key}: 見つからない({})",
            output.status
        ));
    }
    let line = String::from_utf8_lossy(&output.stdout);
    Ok(line.trim_end().split(':').map(str::to_string).collect())
}

/// 利用者の名から uid・gid・主グループの名・home を引く。NSS を通す(/etc/passwd を直に読むと
/// LDAP などの利用者を取りこぼす)。
pub fn lookup_account(name: &str) -> Result<Account, String> {
    // passwd: name:x:uid:gid:gecos:home:shell
    let passwd = getent("passwd", name)?;
    let field = |at: usize, what: &str| -> Result<&str, String> {
        passwd
            .get(at)
            .map(String::as_str)
            .ok_or_else(|| format!("getent passwd {name} の {what} の欄が無い: {passwd:?}"))
    };
    let uid: u32 = field(2, "uid")?
        .parse()
        .map_err(|e| format!("getent passwd {name} の uid を読めない: {e}"))?;
    let gid: u32 = field(3, "gid")?
        .parse()
        .map_err(|e| format!("getent passwd {name} の gid を読めない: {e}"))?;
    let home = PathBuf::from(field(5, "home")?);
    if !home.is_absolute() {
        return Err(format!(
            "利用者 {name} の home {} が絶対の道ではない",
            home.display()
        ));
    }
    // group: name:x:gid:members
    let group = getent("group", &gid.to_string())?
        .first()
        .cloned()
        .ok_or_else(|| format!("getent group {gid} の名の欄が無い"))?;
    Ok(Account {
        name: name.to_string(),
        uid,
        gid,
        group,
        home,
    })
}

/// `--system` と `--user` から据え先を決める。判断はここ 1 箇所(should/0135):
/// - `--user` は `--system` のときだけ(user 単位は自分で走る)。
/// - system 単位の実行ユーザは `--user`、無ければ環境変数 SUDO_USER、それも無ければ断る。
///   root では常駐させない。
/// - system 単位は root で走っていなければ断る(/etc/systemd/system に書き、systemctl を
///   機械のマネージャに掛けるため)。文言に sudo で走らせる形を含める。
///
/// euid と SUDO_USER を引数で受けるのは、環境を読むのを呼び手の 1 箇所にし、ここを純関数に
/// するため。実行ユーザの実物(uid・home)は最後に NSS で引く。
pub fn scope(
    system: bool,
    user: Option<String>,
    sudo_user: Option<String>,
    euid: u32,
) -> Result<Scope, String> {
    if !system {
        return match user {
            Some(name) => Err(format!(
                "--user {name} は --system のときだけ受け付ける(user 単位は自分の systemd に\
                 自分で載る)"
            )),
            None => Ok(Scope::User),
        };
    }
    let name = user
        .or_else(|| sudo_user.filter(|name| !name.is_empty()))
        .ok_or_else(|| {
            "--system は常駐させる利用者が要る: --user <name> で指すか、その利用者から sudo で\
             走らせる(SUDO_USER を読む)。root では常駐させない"
                .to_string()
        })?;
    if name == "root" {
        return Err("--system の実行ユーザに root は使えない(root で常駐させない。--user で\
                    ストアの所有者を指す)"
            .to_string());
    }
    if euid != 0 {
        return Err(format!(
            "--system は root で走らせる({SYSTEM_UNIT_DIR} に書き、機械の systemd に掛ける)。\
             同じ引数を sudo で、出力をファイルに残す形で: \
             sudo <bin>/uniqnode install <dir> --system … 2>&1 | tee /tmp/uniqnode-install-system.log\
             (常駐は {name} で走る)"
        ));
    }
    lookup_account(&name).map(Scope::System)
}

pub struct Options {
    pub data_dir: PathBuf,
    /// インスタンス名(`--instance`)。unit の名・nft の表の名・既定の置き場がここから出る。
    pub instance: String,
    /// serve の待ち受け(UNIQNODE_LISTEN)。viewer の転送先もここから導く。
    pub listen: String,
    pub viewer_listen: String,
    /// serve の追加の引数(UNIQNODE_SERVE_OPTIONS)。空白で分けた 1 本の文字列。
    pub serve_options: String,
    pub backup_dir: PathBuf,
    /// 実行ファイルを置く道。走っている自分自身を写す。
    pub binary: PathBuf,
    /// unit と drop-in を置くディレクトリ(既定は Scope::default_unit_dir)。
    pub unit_dir: PathBuf,
    /// false なら daemon-reload まで行い、enable/start と確認を省く。
    pub start: bool,
    pub scope: Scope,
    /// drop-in の [Unit] に After= と Wants= で書く unit(`--after`。system 単位だけ)。
    pub after: Vec<String>,
    /// 読み口の待ち受け(`--listen-agent`)。serve の第 2 の TcpListener。
    pub agent_listen: Option<String>,
    /// 読み口から書けるコレクション(`--agent-writable`。与えられた順)。読み口があるとき
    /// だけ。空なら読み口は読むだけ。
    pub agent_writable: Vec<String>,
    /// 読み口から読めるコレクション(`--agent-collections`。与えられた順)。読み口がある
    /// ときだけ。空なら読み口は全コレクションを読める(既定)。
    pub agent_collections: Vec<String>,
    /// 読み口から読めるグラフ(`--agent-graph`。与えられた順)。空ならグラフ層は見えない。
    pub agent_graph: Vec<String>,
    /// 読み口から書けるグラフ(`--agent-graph-writable`。与えられた順)。
    pub agent_graph_writable: Vec<String>,
    /// true なら、据える前に実行ユーザの user 単位の常駐(STARTED_TEMPLATES)を止めて外す
    /// (`--take-over-user-units`。system 単位だけ)。
    pub take_over_user_units: bool,
    /// 読み口へ届いてよい相手のアドレス(`--firewall-allow`)。ufw が active ならその規則を
    /// 入れ、載ったことを見る。system 単位で読み口があるときだけ。
    pub firewall_allow: Option<String>,
    /// 規則をどこに入れるか(run が ufw status を読んで決める。drop-in を描く前に要る:
    /// nft なら serve の ExecStartPre= が規則ファイルを読む)。
    pub firewall_backend: Option<FirewallBackend>,
}

/// firewall の実物。ufw が active ならその規則、そうでなければ nft の自分の表。
#[derive(Debug, Clone, PartialEq)]
pub enum FirewallBackend {
    Ufw,
    /// nft の実行ファイルの絶対パス(ExecStartPre= は絶対パスを要する)。
    Nft(PathBuf),
}

impl Options {
    /// `--instance` を決める。写し先の既定は実体ごとに分かれる(unit のテンプレートの
    /// `%h/uniqnode-backup/%i` と同じ形)ので、既定を使っているあいだは名を変えると写し先も
    /// 付け替わる。`--backup-dir` で明示した道はここを通らないので動かない
    /// (node/src/main.rs は --backup-dir をこの後に上書きする)。
    ///
    /// 分けるのは、2 つの実体が同じ写し先を取り合うと backup が毎日「別のノードの写し
    /// (node_key が写し元と違う)」で失敗するからである(2026-09-08、2 つ目を据える前の
    /// 試しで踏んだ。写しは壊れない: backup は取り合いに気づいて何も書かずに終わる)。
    pub fn set_instance(&mut self, instance: String) {
        if self.backup_dir.file_name() == Some(std::ffi::OsStr::new(&self.instance)) {
            self.backup_dir.set_file_name(&instance);
        }
        self.instance = instance;
    }

    /// home の下の既定で組む(system 単位なら home は実行ユーザのもの)。
    pub fn defaults(data_dir: PathBuf, home: &Path, scope: Scope) -> Options {
        Options {
            data_dir,
            instance: DEFAULT_INSTANCE.to_string(),
            listen: DEFAULT_LISTEN.to_string(),
            viewer_listen: DEFAULT_VIEWER_LISTEN.to_string(),
            serve_options: String::new(),
            backup_dir: home.join("uniqnode-backup").join(DEFAULT_INSTANCE),
            binary: home.join(".local").join("bin").join("uniqnode"),
            unit_dir: scope.default_unit_dir(home),
            start: true,
            scope,
            after: Vec::new(),
            agent_listen: None,
            agent_writable: Vec::new(),
            agent_collections: Vec::new(),
            agent_graph: Vec::new(),
            agent_graph_writable: Vec::new(),
            take_over_user_units: false,
            firewall_allow: None,
            firewall_backend: None,
        }
    }

    /// nft の規則ファイルの置き場(serve の drop-in の隣。systemd は *.conf しか読まない)。
    pub fn nft_rules_path(&self) -> PathBuf {
        self.unit_dir
            .join(format!("{}.d", self.serve_unit()))
            .join(NFT_RULES_NAME)
    }

    /// このインスタンスの unit の名。テンプレートから組む(unit_for が唯一の差し込み口)。
    pub fn unit(&self, template: &str) -> String {
        unit_for(template, &self.instance)
    }

    pub fn serve_unit(&self) -> String {
        self.unit(SERVE_TEMPLATE)
    }

    pub fn viewer_unit(&self) -> String {
        self.unit(VIEWER_TEMPLATE)
    }

    pub fn backup_unit(&self) -> String {
        self.unit(BACKUP_TEMPLATE)
    }

    pub fn backup_timer(&self) -> String {
        self.unit(BACKUP_TIMER_TEMPLATE)
    }

    /// enable --now するもの(テンプレートの並びと同じ順)。
    pub fn started_units(&self) -> Vec<String> {
        STARTED_TEMPLATES
            .iter()
            .map(|template| self.unit(template))
            .collect()
    }

    /// このインスタンスの nft の表の名。
    pub fn nft_table(&self) -> String {
        nft_table_for(&self.instance)
    }

    /// 置く unit(テンプレートの名, 中身)。据え先で user/system を切り替える。
    pub fn units(&self) -> &'static [(&'static str, &'static str); 4] {
        match self.scope {
            Scope::User => &USER_UNITS,
            Scope::System(_) => &SYSTEM_UNITS,
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
fn unit_text(units: &[(&str, &'static str)], name: &str) -> &'static str {
    units
        .iter()
        .find(|(unit, _)| *unit == name)
        .map(|(_, text)| *text)
        .expect("UNITS に載っている名前")
}

/// ProtectHome= が隠す下にある道(バイナリ・ストア・写し先)。1 つでもあれば system 単位の
/// drop-in は ProtectHome=read-only を書く(判断はここ 1 箇所。should/0135)。
pub fn paths_under_protected_home(options: &Options) -> Vec<&Path> {
    [&options.binary, &options.data_dir, &options.backup_dir]
        .into_iter()
        .filter(|path| PROTECTED_HOME_ROOTS.iter().any(|root| path.starts_with(root)))
        .map(PathBuf::as_path)
        .collect()
}

/// 3 つの service の drop-in を描く: (unit 名, override.conf の中身)。ExecStart= と
/// ReadWritePaths= は追記型なので、空の行で unit の値を一度消してから書く。tool_path は
/// tool_path() の value(Environment=PATH= に書く値)。
///
/// system 単位では [Service] の先頭に User=/Group=(unit の専用ユーザー uniqnode を実行ユーザに
/// 替える)と StateDirectory=(空で打ち消す。データディレクトリは明示なので %S は要らず、
/// 残すと /var/lib/uniqnode を無駄に作る)を書く。`--after` は [Unit] の After= と Wants= に
/// 3 つとも書く(待つ相手が wg1 でもストアのマウントでも、どの unit が待つかを unit ごとに
/// 追わせない)。読み口(`--listen-agent`)は serve だけ。
pub fn drop_ins(
    options: &Options,
    tool_path: &str,
) -> Result<Vec<(String, String)>, String> {
    let data_dir = options.data_dir.to_string_lossy();
    let backup_dir = options.backup_dir.to_string_lossy();
    let binary = unit_word(&options.binary.to_string_lossy())?;
    let mut header = String::from(
        "# uniqnode install が書いた drop-in。再実行で書き直されるので、手で変えるなら\n\
         # 別の名前の *.conf を隣に置く。\n",
    );
    if !options.after.is_empty() {
        let mut units = Vec::new();
        for unit in &options.after {
            units.push(unit_word(unit)?);
        }
        let units = units.join(" ");
        header.push_str(&format!("[Unit]\nAfter={units}\nWants={units}\n"));
    }
    header.push_str("[Service]\n");
    let mut service_head = Vec::new();
    if let Some(account) = options.scope.account() {
        service_head.push(format!("User={}", unit_word(&account.name)?));
        service_head.push(format!("Group={}", unit_word(&account.group)?));
        service_head.push("StateDirectory=".to_string());
        let under_home = paths_under_protected_home(options);
        if !under_home.is_empty() {
            service_head.push(format!(
                "# {} は ProtectHome= が隠す下にあるので、unit の yes を read-only に緩める\n\
                 ProtectHome=read-only",
                under_home
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" と ")
            ));
        }
    }
    let mut rendered = Vec::new();
    for template in [SERVE_TEMPLATE, VIEWER_TEMPLATE, BACKUP_TEMPLATE] {
        // PATH は 3 つとも同じ値。外部の道具(DELEGATES)を起こすのは serve だけだが、unit ごとに
        // 違う PATH を書くと「どの unit がどの道具を見るか」を読み手が unit ごとに追うことになる。
        // 同じ値なら drop-in を 1 つ読めば全部が分かる。
        let mut lines = service_head.clone();
        lines.push(environment_line("PATH", tool_path)?);
        lines.push(environment_line("UNIQNODE_DATA_DIR", &data_dir)?);
        let mut writable: Vec<&str> = Vec::new();
        // ExecStart= の末尾に足す引数(読み口)。
        let mut exec_tail = String::new();
        match template {
            SERVE_TEMPLATE => {
                lines.push(environment_line("UNIQNODE_LISTEN", &options.listen)?);
                lines.push(environment_line(
                    "UNIQNODE_SERVE_OPTIONS",
                    &options.serve_options,
                )?);
                if let Some(agent_listen) = &options.agent_listen {
                    lines.push(environment_line(AGENT_LISTEN_ENV, agent_listen)?);
                    exec_tail = format!(" {AGENT_LISTEN_FLAG} ${{{AGENT_LISTEN_ENV}}}");
                    if !options.agent_writable.is_empty() {
                        // 集合は環境変数に 1 本で写し(読み手のため)、ExecStart= には値を
                        // そのまま並べる(1 語ずつの展開に頼らない)。
                        lines.push(environment_line(
                            AGENT_WRITABLE_ENV,
                            &options.agent_writable.join(" "),
                        )?);
                        for collection in &options.agent_writable {
                            exec_tail.push_str(&format!(
                                " {AGENT_WRITABLE_FLAG} {}",
                                unit_word(collection)?
                            ));
                        }
                    }
                    if !options.agent_collections.is_empty() {
                        // 読める集合も書く集合と全く同じ流儀(環境変数は読み手のための
                        // 写し、ExecStart= には値をそのまま並べる)。
                        lines.push(environment_line(
                            AGENT_COLLECTIONS_ENV,
                            &options.agent_collections.join(" "),
                        )?);
                        for collection in &options.agent_collections {
                            exec_tail.push_str(&format!(
                                " {AGENT_COLLECTIONS_FLAG} {}",
                                unit_word(collection)?
                            ));
                        }
                    }
                    // グラフの 2 つも全く同じ流儀(環境変数は読み手のための写し、
                    // ExecStart= には値をそのまま並べる)。
                    for (env, flag, names) in [
                        (AGENT_GRAPH_ENV, AGENT_GRAPH_FLAG, &options.agent_graph),
                        (
                            AGENT_GRAPH_WRITABLE_ENV,
                            AGENT_GRAPH_WRITABLE_FLAG,
                            &options.agent_graph_writable,
                        ),
                    ] {
                        if names.is_empty() {
                            continue;
                        }
                        lines.push(environment_line(env, &names.join(" "))?);
                        for graph in names {
                            exec_tail.push_str(&format!(" {flag} {}", unit_word(graph)?));
                        }
                    }
                }
                if let Some(FirewallBackend::Nft(nft)) = &options.firewall_backend {
                    // 読み口の firewall は serve を起こすたびに入れ直す(nftables.service が
                    // 無効な機械でも再起動で消えない)。先頭の + は User= に関わらず root で
                    // 走らせる印。
                    lines.push(format!(
                        "# 読み口へ届いてよい相手を nft の表 {} で限る(隣の {NFT_RULES_NAME})\n\
                         ExecStartPre=+{} -f {}",
                        options.nft_table(),
                        unit_word(&nft.to_string_lossy())?,
                        unit_word(&options.nft_rules_path().to_string_lossy())?
                    ));
                }
                writable.push(&data_dir);
            }
            VIEWER_TEMPLATE => {
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
            "ExecStart={}{exec_tail}",
            exec_start_with_binary(unit_text(options.units(), template), &binary)?
        ));
        rendered.push((options.unit(template), format!("{header}{}\n", lines.join("\n"))));
    }
    Ok(rendered)
}

/// `--after` の unit 名を検める: 空でなく、空白を含まず、種類の拡張子(.service や .mount)
/// を持つこと。拡張子の無い名は systemd が .service と読むので、wg-quick@wg1 のつもりが
/// 通ってしまう前に断る。
pub fn check_after_unit(unit: &str) -> Result<(), String> {
    if unit.is_empty() || unit.chars().any(char::is_whitespace) {
        return Err(format!("--after {unit:?}: unit 名は空白を含まない 1 語"));
    }
    let known_suffix = [
        ".service", ".mount", ".target", ".socket", ".device", ".path", ".timer", ".slice",
        ".scope", ".swap", ".automount",
    ];
    if !known_suffix.iter().any(|suffix| unit.ends_with(suffix)) {
        return Err(format!(
            "--after {unit}: unit の種類の拡張子(.service や .mount)まで書く"
        ));
    }
    Ok(())
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
    check_instance(&options.instance)?;
    check_listen(&options.listen, "--listen")?;
    check_listen(&options.viewer_listen, "--viewer-listen")?;
    if let Some(agent_listen) = &options.agent_listen {
        check_listen(agent_listen, AGENT_LISTEN_FLAG)?;
        if agent_listen == &options.listen {
            return Err(format!(
                "{AGENT_LISTEN_FLAG} {agent_listen} が --listen と同じ(読み口は主の口と別の\
                 アドレスに束縛する)"
            ));
        }
    }
    for collection in &options.agent_writable {
        check_agent_writable(collection)?;
    }
    if !options.agent_writable.is_empty() && options.agent_listen.is_none() {
        return Err(format!(
            "{AGENT_WRITABLE_FLAG} {} は {AGENT_LISTEN_FLAG} があるときだけ受け付ける(書く\
             許可は読み口に掛かるもので、読み口が無ければ効かせる先が無い)",
            options.agent_writable.join(" ")
        ));
    }
    for collection in &options.agent_collections {
        check_agent_collection(collection)?;
    }
    if !options.agent_collections.is_empty() && options.agent_listen.is_none() {
        return Err(format!(
            "{AGENT_COLLECTIONS_FLAG} {} は {AGENT_LISTEN_FLAG} があるときだけ受け付ける(読む\
             許可は読み口に掛かるもので、読み口が無ければ効かせる先が無い)",
            options.agent_collections.join(" ")
        ));
    }
    for (flag, names) in [
        (AGENT_GRAPH_FLAG, &options.agent_graph),
        (AGENT_GRAPH_WRITABLE_FLAG, &options.agent_graph_writable),
    ] {
        for graph in names {
            check_agent_graph(flag, graph)?;
        }
        if !names.is_empty() && options.agent_listen.is_none() {
            return Err(format!(
                "{flag} {} は {AGENT_LISTEN_FLAG} があるときだけ受け付ける(グラフの許可は\
                 読み口に掛かるもので、読み口が無ければ効かせる先が無い)",
                names.join(" ")
            ));
        }
    }
    if !options.after.is_empty() && !options.scope.is_system() {
        return Err(format!(
            "--after {} は --system のときだけ受け付ける(user unit は system unit を待てない: \
             user 単位のマネージャは機械の unit を知らない)",
            options.after.join(" ")
        ));
    }
    for unit in &options.after {
        check_after_unit(unit)?;
    }
    if options.take_over_user_units && !options.scope.is_system() {
        return Err(format!(
            "{TAKE_OVER_FLAG} は --system のときだけ受け付ける(user 単位から system 単位へ移る\
             ときに、止めて外す段を install に含める指定)"
        ));
    }
    if let Some(allow) = &options.firewall_allow {
        check_firewall_allow(allow)?;
        if !options.scope.is_system() || options.agent_listen.is_none() {
            return Err(format!(
                "{FIREWALL_ALLOW_FLAG} {allow} は --system で {AGENT_LISTEN_FLAG} があるときだけ\
                 受け付ける(規則は読み口のポートに向けて入れ、root で ufw を打つ)"
            ));
        }
    }
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
        instance: options.instance,
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
        scope: options.scope,
        after: options.after,
        agent_listen: options.agent_listen,
        agent_writable: options.agent_writable,
        agent_collections: options.agent_collections,
        agent_graph: options.agent_graph,
        agent_graph_writable: options.agent_graph_writable,
        take_over_user_units: options.take_over_user_units,
        firewall_allow: options.firewall_allow,
        firewall_backend: options.firewall_backend,
    })
}

/// `--agent-graph` と `--agent-graph-writable` の値はグラフ名 1 つ。形の判断は
/// crate::graph の 1 箇所から借りる(should/0135)。serve の引数の読み手と install の
/// normalize が同じ検査を使う。
pub fn check_agent_graph(flag: &str, graph: &str) -> Result<(), String> {
    match crate::graph::is_valid_name(graph) {
        true => Ok(()),
        false => Err(format!(
            "{flag} の値が{}",
            crate::graph::invalid_name_refusal("グラフ名", graph)
        )),
    }
}

/// `--agent-writable` の値はコレクション名 1 つ: 空でなく、/ と空白を含まない 1 語(空白を
/// 断るのは、drop-in が集合を空白で分けて 1 つの環境変数に写すため)。serve の引数の読み手
/// (node/src/main.rs)と install の normalize が同じ検査を使う(should/0135)。
pub fn check_agent_writable(collection: &str) -> Result<(), String> {
    check_collection_flag(AGENT_WRITABLE_FLAG, collection)
}

/// `--agent-collections` の値も同じ形のコレクション名 1 つ。断りの文言は指定の名前だけが
/// 違う(検査そのものは 1 箇所。should/0135)。
pub fn check_agent_collection(collection: &str) -> Result<(), String> {
    check_collection_flag(AGENT_COLLECTIONS_FLAG, collection)
}

fn check_collection_flag(flag: &str, collection: &str) -> Result<(), String> {
    if collection.is_empty() || collection.contains('/') || collection.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "{flag} はコレクション名(空でなく / と空白を含まない 1 語): {collection:?}"
        ));
    }
    Ok(())
}

/// `--firewall-allow` の値は IPv4 アドレス 1 つ(範囲や名前は受けない。ufw と nft に渡す
/// 字句をここで固定する。読み口の側も `ip daddr` で書くので IPv4 に限る)。
pub fn check_firewall_allow(allow: &str) -> Result<(), String> {
    match allow.parse::<std::net::Ipv4Addr>() {
        Ok(_) => Ok(()),
        Err(_) => Err(format!(
            "{FIREWALL_ALLOW_FLAG} {allow} は IPv4 アドレスでない(例 10.10.128.4)"
        )),
    }
}

/// path を account の所有にする(root で走る system 単位の据え付けが作ったものを、実行ユーザが
/// 書ける形で残すため)。
fn chown_to(path: &Path, account: &Account) -> Result<(), String> {
    std::os::unix::fs::chown(path, Some(account.uid), Some(account.gid)).map_err(|e| {
        format!(
            "{} を {}:{} の所有にできない: {e}",
            path.display(),
            account.name,
            account.group
        )
    })
}

/// dir の下(dir 自身を含む。symlink は辿らない)で uid の所有でないものを集める。
/// system 単位の据え付けの最後に、root が触ったストアと写し先に root 所有のものが残って
/// いないことを見るための観測。
pub fn paths_not_owned_by(dir: &Path, uid: u32) -> Result<Vec<PathBuf>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("{} の所有者を読めない: {e}", path.display()))?;
        if metadata.uid() != uid {
            found.push(path.clone());
        }
        if metadata.is_dir() {
            let entries = std::fs::read_dir(&path)
                .map_err(|e| format!("{} を読めない: {e}", path.display()))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("{} の項を読めない: {e}", path.display()))?;
                pending.push(entry.path());
            }
        }
    }
    found.sort();
    Ok(found)
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
/// その名前で残らない。destination が自分自身なら写さない。owner があれば(system 単位。
/// root が写す)据える前にその所有にする。返り値は報告の 1 行。
pub fn install_binary(destination: &Path, owner: Option<&Account>) -> Result<String, String> {
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
    if let Some(account) = owner {
        chown_to(&staging, account)?;
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

/// systemctl(user 単位なら --user 付き)を 1 命令 1 exec で呼ぶ。見つからない・呼べないは
/// Err。終了コードと出力は呼び手が読む(is-active のように非 0 が答えである命令があるため)。
fn systemctl(scope: &Scope, arguments: &[&str]) -> Result<std::process::Output, String> {
    Command::new("systemctl")
        .args(scope.manager_flags())
        .args(arguments)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "systemctl が PATH に無い(systemd の無い機械では install は使えない)".to_string()
            }
            _ => format!(
                "{}{} を起こせない: {e}",
                scope.systemctl_prefix(),
                arguments.join(" ")
            ),
        })
}

/// 成功(終了 0)を要する systemctl。失敗は標準エラーを添えて言う。
fn systemctl_ok(scope: &Scope, arguments: &[&str]) -> Result<String, String> {
    let output = systemctl(scope, arguments)?;
    if !output.status.success() {
        return Err(format!(
            "{}{} が {} で終わった: {}",
            scope.systemctl_prefix(),
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// `systemctl [--user] is-active <unit>` の答え(active / inactive / failed / …)。
fn unit_state(scope: &Scope, unit: &str) -> Result<String, String> {
    let output = systemctl(scope, &["is-active", unit])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `<unit> が failed になった` の言い方(journal の読み方を添える)。
fn failed_unit_message(scope: &Scope, unit: &str) -> String {
    format!(
        "{unit} が failed になった。理由は {}-u {unit} -n 20",
        scope.journalctl_prefix()
    )
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
    scope: &Scope,
    listen: &str,
    viewer_listen: &str,
    serve_unit: &str,
    viewer_unit: &str,
) -> Result<(String, Duration), String> {
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
        for unit in [serve_unit, viewer_unit] {
            if unit_state(scope, unit)? == "failed" {
                return Err(failed_unit_message(scope, unit));
            }
        }
        if started.elapsed() >= WAIT_BOUND {
            let describe = |r: &Result<String, String>| match r {
                Ok(id) => format!("node_id {id}"),
                Err(e) => e.clone(),
            };
            return Err(format!(
                "{} 秒待っても serve と viewer の /v1/status が揃わない。serve({listen}): {}。\
                 viewer({viewer_listen}): {}",
                WAIT_BOUND.as_secs(),
                describe(&serve),
                describe(&viewer)
            ));
        }
        std::thread::sleep(TICK);
    }
}

/// 読み口(agent_listen)が主の口と同じ node_id を /v1/status で返し、許可表の外の
/// POST /v1/admin/gc を 403 で断るまで待つ。返り値は掛かった時間。
///
/// 待つ理由: 読み口は wg1 のような後から上がるインターフェースのアドレスに束縛されることが
/// あり、serve は読み口の bind に失敗しても主の口を殺さず再試行する。上限は WAIT_BOUND で、
/// serve の unit が failed に落ちたら待たずに言う。node_id が違う・403 以外で答える、は
/// 待っても直らないのでその場で断る(別のものが同じアドレスで答えている、許可表が壊れている)。
/// gc に送る本文は dry_run: true にする: 許可表が壊れていて通ってしまっても、確認の 1 本が
/// 本番のストアの pack を回収してしまわないように。
fn wait_for_agent_door(
    scope: &Scope,
    serve_unit: &str,
    node_id: &str,
    agent_listen: &str,
) -> Result<Duration, String> {
    const TICK: Duration = Duration::from_millis(200);
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
    const GC_PATH: &str = "/v1/admin/gc";
    let started = Instant::now();
    loop {
        match status_node_id(agent_listen) {
            Ok(seen) if seen == node_id => {
                let response = http::post_json(
                    agent_listen,
                    GC_PATH,
                    br#"{"dry_run":true}"#,
                    REQUEST_TIMEOUT,
                )?;
                if response.status != 403 {
                    return Err(format!(
                        "読み口 {agent_listen} が POST {GC_PATH} を 403 で断らず {} を返した: {}",
                        response.status,
                        http::body_head(&response.body)
                    ));
                }
                return Ok(started.elapsed());
            }
            Ok(seen) => {
                return Err(format!(
                    "読み口 {agent_listen} の /v1/status の node_id {seen} が主の口の {node_id} と\
                     違う(別のものが同じアドレスで答えている)"
                ));
            }
            Err(reason) => {
                if unit_state(scope, serve_unit)? == "failed" {
                    return Err(failed_unit_message(scope, serve_unit));
                }
                if started.elapsed() >= WAIT_BOUND {
                    return Err(format!(
                        "{} 秒待っても読み口 {agent_listen} が /v1/status に答えない: {reason}。\
                         束縛先のアドレスを持つインターフェースが上がっているか(--after で\
                         待つ unit の状態)と、{}-u {serve_unit} の bind の行を見る",
                        WAIT_BOUND.as_secs(),
                        scope.journalctl_prefix()
                    ));
                }
            }
        }
        std::thread::sleep(TICK);
    }
}

/// 確認の探りに使う名。書く口では文書名として使い、拡張子が無いので api.rs の put_document が
/// 400 で断り、ストアには何も書かれない(試し書きはしない)。読み口の門を越えた証拠は
/// 「403 でなく、本文が門の断りでない」ことで、api.rs まで届かなければこの 400 は出ない。
/// 読める集合の確認では、集合に無いコレクション名としても使う(本番のコレクションの名前を
/// 確認のために書かないため。集合にあれば -x を足してずらす)。
pub const WRITABLE_PROBE_NAME: &str = "uniqnode-install-probe";

/// 集合に無い名を作る(確認が「許していない名」を要るときに使う。本番のコレクション名を
/// 探りに使わないための 1 箇所)。
fn name_outside(allowed: &[String]) -> String {
    let mut name = WRITABLE_PROBE_NAME.to_string();
    while allowed.iter().any(|entry| *entry == name) {
        name.push_str("-x");
    }
    name
}

/// 読み口の書く口(`--agent-writable`)を、書かずに確かめる。許した各コレクションへの PUT
/// (拡張子の無い文書名、空の本文)が門を越えて api.rs の 400 で止まること、許していない
/// コレクションへの PUT が門の 403 の文言で断られること。返り値は報告の右側。
fn verify_agent_writable(agent_listen: &str, writable: &[String]) -> Result<String, String> {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
    let put = |collection: &str| -> Result<http::ClientResponse, String> {
        http::request(
            agent_listen,
            "PUT",
            &format!("/v1/collections/{collection}/documents/{WRITABLE_PROBE_NAME}"),
            Some(("application/octet-stream", b"")),
            REQUEST_TIMEOUT,
        )
    };
    for collection in writable {
        let response = put(collection)?;
        let body = String::from_utf8_lossy(&response.body).to_string();
        if response.status == 403 || body.contains(crate::agent_door::ERROR_PREFIX) {
            return Err(format!(
                "読み口 {agent_listen} が {AGENT_WRITABLE_FLAG} {collection} への PUT を門で断った\
                 ({}: {})",
                response.status,
                http::body_head(&response.body)
            ));
        }
        if response.status != 400 {
            return Err(format!(
                "読み口 {agent_listen} が {collection} への拡張子の無い PUT を 400 で断らず {} を\
                 返した(何かを書いたかもしれない): {}",
                response.status,
                http::body_head(&response.body)
            ));
        }
    }
    // 許していない名前: 集合に無いことを確かめてから使う。
    let unallowed = name_outside(writable);
    let response = put(&unallowed)?;
    let body = String::from_utf8_lossy(&response.body).to_string();
    let refusal_mark = format!("は書けない({AGENT_WRITABLE_FLAG} で許したのは");
    if response.status != 403 || !body.contains(&refusal_mark) {
        return Err(format!(
            "読み口 {agent_listen} が許していないコレクション {unallowed} への PUT を 403 の文言で\
             断らず {} を返した: {}",
            response.status,
            http::body_head(&response.body)
        ));
    }
    Ok(format!(
        "{} への PUT が門を越えて 400 で止まり(何も書かない)、{unallowed} への PUT は 403 で\
         断られた",
        writable.join(", ")
    ))
}

/// 読み口の読める集合(`--agent-collections`)を、本番のデータに触らずに確かめる。集合に
/// 無い名前を collection に指した検索が、門の 403 の文言で断られること。読むだけの確認で、
/// 何も書かない(許した側のコレクションを引かないのは、本番の索引の温めを確認のために
/// 走らせないため。集合が効いていることは「外を指すと断られる」だけで見える)。
/// 返り値は報告の右側。
fn verify_agent_collections(agent_listen: &str, readable: &[String]) -> Result<String, String> {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
    let unallowed = name_outside(readable);
    let body = format!("{{\"query\":\"probe\",\"collection\":\"{unallowed}\",\"top_k\":1}}");
    let response =
        http::post_json(agent_listen, "/v1/search", body.as_bytes(), REQUEST_TIMEOUT)?;
    // 期待する本文は読み口が組むものと同じ字句(must/0023)。
    let refusal = crate::agent_door::unreadable_refusal(&unallowed, readable);
    let seen = String::from_utf8_lossy(&response.body).to_string();
    if response.status != 403 || !seen.contains(&refusal) {
        return Err(format!(
            "読み口 {agent_listen} が許していないコレクション {unallowed} を指した検索を 403 の\
             文言で断らず {} を返した: {}",
            response.status,
            http::body_head(&response.body)
        ));
    }
    Ok(format!(
        "{} だけが読め、{unallowed} を指した検索は 403 で断られた",
        readable.join(", ")
    ))
}

/// 読み口のグラフの許可を、書かずに確かめる。読める側は許した名の `GET /v1/graphs/{g}` が
/// 門を越えること、許していない名が 403 の文言で断られること。書ける側は、許していない名への
/// PUT が 403 で断られること、そして許した名への PUT が門を越えて 400(attrs の形)で止まる
/// ことである。400 で止まるので何も書かない(コレクションの確認と同じ流儀)。
fn verify_agent_graph(agent_listen: &str, allowed: &crate::agent_door::Allowed) -> Result<String, String> {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
    // attrs がオブジェクトでない本文。門を越えても api が 400 で止めるので、何も書かない。
    const MALFORMED_ATTRS: &[u8] = b"[]";
    let readable = allowed.readable_graphs();
    for graph in &readable {
        let response = http::request(agent_listen, "GET", &format!("/v1/graphs/{graph}"), None, REQUEST_TIMEOUT)?;
        if response.status != 200 {
            return Err(format!(
                "読み口 {agent_listen} が {AGENT_GRAPH_FLAG} {graph} の GET に {} を返した: {}",
                response.status,
                http::body_head(&response.body)
            ));
        }
    }
    let names: Vec<String> = readable.iter().map(|name| name.to_string()).collect();
    let unallowed = name_outside(&names).replace('-', "_");
    let response = http::request(agent_listen, "GET", &format!("/v1/graphs/{unallowed}"), None, REQUEST_TIMEOUT)?;
    let refusal = crate::agent_door::ungraphed_refusal(&unallowed, allowed);
    let seen = String::from_utf8_lossy(&response.body).to_string();
    if response.status != 403 || !seen.contains(&refusal) {
        return Err(format!(
            "読み口 {agent_listen} が許していないグラフ {unallowed} の GET を 403 の文言で断らず \
             {} を返した: {}",
            response.status,
            http::body_head(&response.body)
        ));
    }
    if allowed.graphs_writable.is_empty() {
        return Ok(format!(
            "{} が読め、{unallowed} は 403 で断られた(書ける名は無い)",
            readable.join(", ")
        ));
    }
    let put = |graph: &str| -> Result<http::ClientResponse, String> {
        http::request(
            agent_listen,
            "PUT",
            &format!("/v1/graphs/{graph}/nodes/{WRITABLE_PROBE_NAME}"),
            Some(("application/json", MALFORMED_ATTRS)),
            REQUEST_TIMEOUT,
        )
    };
    for graph in &allowed.graphs_writable {
        let response = put(graph)?;
        let body = String::from_utf8_lossy(&response.body).to_string();
        if response.status == 403 || body.contains(crate::agent_door::ERROR_PREFIX) {
            return Err(format!(
                "読み口 {agent_listen} が {AGENT_GRAPH_WRITABLE_FLAG} {graph} への PUT を門で断った\
                 ({}: {})",
                response.status,
                http::body_head(&response.body)
            ));
        }
        if response.status != 400 {
            return Err(format!(
                "読み口 {agent_listen} が {graph} への形の違う PUT を 400 で断らず {} を返した\
                 (何かを書いたかもしれない): {}",
                response.status,
                http::body_head(&response.body)
            ));
        }
    }
    let response = put(&unallowed)?;
    let refusal = crate::agent_door::unwritable_graph_refusal(&unallowed, allowed);
    let seen = String::from_utf8_lossy(&response.body).to_string();
    if response.status != 403 || !seen.contains(&refusal) {
        return Err(format!(
            "読み口 {agent_listen} が許していないグラフ {unallowed} への PUT を 403 の文言で断らず \
             {} を返した: {}",
            response.status,
            http::body_head(&response.body)
        ));
    }
    Ok(format!(
        "{} が読め、{} への PUT が門を越えて 400 で止まり(何も書かない)、{unallowed} は読みも\
         書きも 403 で断られた",
        readable.join(", "),
        allowed.graphs_writable.join(", ")
    ))
}

fn fsck_summary(report: &FsckReport) -> String {
    format!(
        "objects {} refs {} errors {}",
        report.objects_checked,
        report.refs_checked,
        report.errors.len()
    )
}

/// 写し先を開いて fsck し、緑なら報告の 1 行の右側を返す。判断(写しが健全か)は
/// backup::verify_copy の 1 箇所だが、写しを開くと未封印の尻尾の切り詰めで写し先に書くので、
/// system 単位では root のこのプロセスで開かず、実行ユーザで `<bin> fsck <dir>` を起こす
/// (root 所有のファイルを写し先に残さないため)。user 単位は自分のプロセスで開く。
fn verify_backup(options: &Options) -> Result<String, String> {
    let backup_dir = &options.backup_dir;
    let Some(account) = options.scope.account() else {
        let verification = crate::backup::verify_copy(backup_dir)
            .map_err(|e| format!("写し先 {} を開けない: {e}", backup_dir.display()))?;
        if !verification.errors.is_empty() {
            return Err(format!(
                "写し先 {} の fsck が赤: {}",
                backup_dir.display(),
                verification.errors.join("; ")
            ));
        }
        return Ok(fsck_summary(&verification));
    };
    let mut command = Command::new("runuser");
    command
        .arg("-u")
        .arg(&account.name)
        .arg("--")
        .arg(&options.binary)
        .arg("fsck")
        .arg(backup_dir);
    let output = command
        .output()
        .map_err(|e| format!("runuser -u {} -- {} fsck を起こせない: {e}", account.name, options.binary.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "写し先 {} の fsck({} で走らせた)が {} で終わった: {} {}",
            backup_dir.display(),
            account.name,
            output.status,
            stdout.trim(),
            stderr.trim()
        ));
    }
    Ok(format!("{}({} で走らせた)", stdout.trim(), account.name))
}

/// 外部の道具を探す PATH。user 単位は install 自身の PATH。system 単位はそれに実行ユーザの
/// home の下(SERVICE_USER_TOOL_DIRS)を後ろに足す(sudo の secure_path には利用者の
/// ~/.local/bin が無い)。
pub fn tool_search_path(path_env: &str, scope: &Scope) -> String {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path_env).collect();
    if let Some(account) = scope.account() {
        for dir in SERVICE_USER_TOOL_DIRS {
            let dir = account.home.join(dir);
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    std::env::join_paths(&dirs)
        .expect("PATH の項は join_paths で繋げる")
        .to_string_lossy()
        .into_owned()
}

/// install の本体。手順ごとに 1 行を out へ書き、失敗はその場で Err(呼び手が理由を出して
/// exit 1)。
/// root から実行ユーザの user マネージャに触る systemctl の引数(`--user -M <name>@`。
/// systemd 249 で使える)。文言と実行が同じ字句を使う(must/0023)。
pub fn user_manager_flags(account_name: &str) -> Vec<String> {
    vec!["--user".to_string(), "-M".to_string(), format!("{account_name}@")]
}

/// 実行ユーザの user 単位の常駐を止めて外す命令(報告に写すもの)。
pub fn take_over_command(account_name: &str, instance: &str) -> String {
    let units: Vec<String> = STARTED_TEMPLATES
        .iter()
        .map(|template| unit_for(template, instance))
        .collect();
    format!(
        "systemctl {} disable --now {}",
        user_manager_flags(account_name).join(" "),
        units.join(" ")
    )
}

/// `systemctl is-active` の答えのうち「まだ走っている」と読むもの。
pub fn unit_is_running(state: &str) -> bool {
    matches!(state.trim(), "active" | "activating" | "reloading" | "deactivating")
}

/// user 単位の常駐を止めて外す(`--take-over-user-units`)。disable --now の終了コードは
/// 見ない(unit が無かったときも 0 でない終わり方をする)。効果で判定する: 3 つの unit の
/// is-active が走っていない答えであること、そしてストアのロックが外れること。ただし
/// system 単位の serve が既に走っていてロックを持つ(再実行 = 更新)なら、それは restart が
/// 引き継ぐので待たない。
fn take_over_user_units(
    scope: &Scope,
    account: &Account,
    instance: &str,
    data_dir: &Path,
    out: &mut dyn Write,
) -> Result<String, String> {
    let units: Vec<String> = STARTED_TEMPLATES
        .iter()
        .map(|template| unit_for(template, instance))
        .collect();
    let flags = user_manager_flags(&account.name);
    let mut arguments: Vec<&str> = flags.iter().map(String::as_str).collect();
    arguments.extend(["disable", "--now"]);
    arguments.extend(units.iter().map(String::as_str));
    let disable = Command::new("systemctl")
        .args(&arguments)
        .output()
        .map_err(|e| {
            format!(
                "{} を起こせない: {e}",
                take_over_command(&account.name, instance)
            )
        })?;
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&disable.stdout).trim(),
        String::from_utf8_lossy(&disable.stderr).trim()
    );
    let mut still_running = Vec::new();
    for unit in &units {
        let output = Command::new("systemctl")
            .args(&flags)
            .args(["is-active", unit])
            .output()
            .map_err(|e| format!("systemctl {} is-active {unit} を起こせない: {e}", flags.join(" ")))?;
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        writeln!(out, "install: user 単位の {unit}: {state}")
            .map_err(|e| format!("標準出力に書けない: {e}"))?;
        if unit_is_running(&state) {
            still_running.push(format!("{unit}={state}"));
        }
    }
    if !still_running.is_empty() {
        return Err(format!(
            "{} を打ったが user 単位がまだ走っている({})。出力: {transcript}",
            take_over_command(&account.name, instance),
            still_running.join(", ")
        ));
    }
    let started = Instant::now();
    loop {
        if !store::opened_by_another_process(data_dir).map_err(|e| e.to_string())? {
            return Ok(format!(
                "{}: 済み(ストアのロックは {} ms で外れた)",
                take_over_command(&account.name, instance),
                started.elapsed().as_millis()
            ));
        }
        let serve_unit = unit_for(SERVE_TEMPLATE, instance);
        if unit_state(scope, &serve_unit)? == "active" {
            return Ok(format!(
                "{}: 済み(ストアのロックは system 単位の {serve_unit} が持っている。restart が引き継ぐ)",
                take_over_command(&account.name, instance)
            ));
        }
        if started.elapsed() >= TAKE_OVER_WAIT {
            return Err(format!(
                "user 単位は止まったが {} のロックが {} 秒経っても外れない(unit 以外のプロセスが\
                 開いている)",
                data_dir.display(),
                TAKE_OVER_WAIT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `ufw status` の 1 行目の読み。
#[derive(Debug, PartialEq)]
pub enum UfwStatus {
    Active,
    Inactive,
}

pub fn ufw_status_of(stdout: &str) -> Result<UfwStatus, String> {
    for line in stdout.lines() {
        match line.trim() {
            "Status: active" => return Ok(UfwStatus::Active),
            "Status: inactive" => return Ok(UfwStatus::Inactive),
            _ => {}
        }
    }
    Err(format!("ufw status の答えに Status: の行が無い: {}", stdout.trim()))
}

/// 読み口の待ち受けから、規則に書く IP とポートを取り出す。
pub fn agent_ip_and_port(agent_listen: &str) -> Result<(String, String), String> {
    match agent_listen.rsplit_once(':') {
        Some((ip, port)) if !ip.is_empty() && !port.is_empty() => {
            Ok((ip.trim_matches(|c| c == '[' || c == ']').to_string(), port.to_string()))
        }
        _ => Err(format!("{AGENT_LISTEN_FLAG} {agent_listen} から IP とポートを分けられない")),
    }
}

/// `ufw allow …` の引数(from の相手から読み口の IP:port への TCP だけ)。
pub fn ufw_allow_arguments(from: &str, agent_listen: &str) -> Result<Vec<String>, String> {
    let (ip, port) = agent_ip_and_port(agent_listen)?;
    Ok(["allow", "from", from, "to", &ip, "port", &port, "proto", "tcp"]
        .iter()
        .map(|s| s.to_string())
        .collect())
}

/// `ufw status` の表に、その規則が載っているか(`10.10.128.1 7441/tcp  ALLOW IN  10.10.128.4`
/// の形。列の幅は環境で変わるので、字句が同じ行にあることで見る)。
pub fn ufw_rule_listed(status: &str, from: &str, agent_listen: &str) -> Result<bool, String> {
    let (ip, port) = agent_ip_and_port(agent_listen)?;
    let target = format!("{port}/tcp");
    Ok(status.lines().any(|line| {
        let cells: Vec<&str> = line.split_whitespace().collect();
        cells.contains(&ip.as_str())
            && cells.contains(&target.as_str())
            && cells.contains(&"ALLOW")
            && cells.contains(&from)
    }))
}

fn command_output(program: &str, arguments: &[String]) -> Result<String, String> {
    let output = Command::new(program).args(arguments).output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{program} が PATH に無い"),
        _ => format!("{program} {} を起こせない: {e}", arguments.join(" ")),
    })?;
    if !output.status.success() {
        return Err(format!(
            "{program} {} が {} で終わった: {}{}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// nft の規則ファイル。表を空で作ってから消して作り直す形なので、何度読んでも同じ 1 表に
/// なる(nft は無い表の delete を断るので、先に空で作る)。
pub fn nft_rules_text(table: &str, from: &str, agent_listen: &str) -> Result<String, String> {
    let (ip, port) = agent_ip_and_port(agent_listen)?;
    Ok(format!(
        "# uniqnode install が書いた。読み口 {agent_listen} へ届いてよいのは {from} だけ。\n\
         # serve の unit の ExecStartPre= が起動のたびに読む(nft -f)。手で入れるなら同じ命令。\n\
         # 表はインスタンスごとに別である(1 つの名を共有すると、後から起きた serve が\n\
         # 先の実体の規則を消してしまう)。\n\
         table {table} {{}}\n\
         delete table {table}\n\
         table {table} {{\n\
         \tchain agent_door {{\n\
         \t\ttype filter hook input priority filter; policy accept;\n\
         \t\tip daddr {ip} tcp dport {port} ip saddr != {{ {from}, {ip} }} counter drop\n\
         \t}}\n\
         }}\n"
    ))
}

/// `nft list table inet uniqnode` の答えに、その規則が載っているか(字句が同じ行にあること
/// で見る。counter の数は行ごとに変わるので照合しない)。許す相手は from と読み口自身の IP
/// の 2 つ(install の確認は同じ機械から 10.10.128.1 を源に届くので、自分を締め出さない)。
pub fn nft_rule_listed(listing: &str, from: &str, agent_listen: &str) -> Result<bool, String> {
    let (ip, port) = agent_ip_and_port(agent_listen)?;
    let pieces = [
        format!("ip daddr {ip} "),
        format!("tcp dport {port} "),
        "ip saddr != {".to_string(),
        " drop".to_string(),
    ];
    // 集合の要素は nft が並べ替えることがあるので、各要素は「, か } が続く」形で個別に見る。
    let element = |line: &str, address: &str| {
        line.contains(&format!(" {address},")) || line.contains(&format!(" {address} }}"))
    };
    Ok(listing.lines().any(|line| {
        pieces.iter().all(|piece| line.contains(piece.as_str()))
            && element(line, from)
            && element(line, &ip)
    }))
}

/// PATH から実行ファイルの絶対パスを引く(ExecStartPre= に書くため)。
pub fn find_in_path(program: &str, path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| candidate.is_file())
}

/// firewall の実物を決める: ufw が active ならそれ、inactive(または ufw が無い)なら nft。
/// どちらも無ければ Err。
fn firewall_backend(path_env: &str) -> Result<FirewallBackend, String> {
    let ufw_active = match command_output("ufw", &["status".to_string()]) {
        Ok(status) => ufw_status_of(&status)? == UfwStatus::Active,
        Err(reason) if reason.contains("PATH に無い") => false,
        Err(reason) => return Err(reason),
    };
    if ufw_active {
        return Ok(FirewallBackend::Ufw);
    }
    match find_in_path("nft", path_env) {
        Some(nft) => Ok(FirewallBackend::Nft(nft)),
        None => Err(format!(
            "{FIREWALL_ALLOW_FLAG}: ufw は active でなく、nft も PATH({path_env})に無い。規則を\
             入れる先が無い"
        )),
    }
}

/// firewall の規則(`--firewall-allow`)の効果を見る。ufw なら規則を入れて表に載ったことを、
/// nft なら serve の ExecStartPre= が入れた表に規則が載っていることを見る(nft の道では
/// ここで入れ直さない: 入れるのは unit の起動そのもので、それが再起動のたびに同じ道で入る
/// ことの証拠になる)。
fn apply_firewall(options: &Options, out: &mut dyn Write) -> Result<String, String> {
    let say = |out: &mut dyn Write, line: &str| -> Result<(), String> {
        writeln!(out, "install: {line}").map_err(|e| format!("標準出力に書けない: {e}"))
    };
    let (from, agent_listen) = match (&options.firewall_allow, &options.agent_listen) {
        (Some(from), Some(agent_listen)) => (from.as_str(), agent_listen.as_str()),
        _ => return Err("firewall の確認には --firewall-allow と読み口が要る".to_string()),
    };
    match &options.firewall_backend {
        Some(FirewallBackend::Ufw) => {
            let arguments = ufw_allow_arguments(from, agent_listen)?;
            let added = command_output("ufw", &arguments)?;
            say(out, &format!("ufw {}: {}", arguments.join(" "), added.trim()))?;
            let after = command_output("ufw", &["status".to_string()])?;
            if !ufw_rule_listed(&after, from, agent_listen)? {
                return Err(format!(
                    "ufw allow を打ったが ufw status の表に載っていない: {}",
                    after.trim()
                ));
            }
            Ok(format!(
                "ufw が {from} から読み口 {agent_listen} への TCP を許す規則を持つ(ufw status に載った)"
            ))
        }
        Some(FirewallBackend::Nft(nft)) => {
            let nft_table = options.nft_table();
            let (family, table) = nft_table
                .split_once(' ')
                .expect("nft_table_for は「家族名 表の名」の 2 語を返す");
            let listing = command_output(
                &nft.to_string_lossy(),
                &["list".to_string(), "table".to_string(), family.to_string(), table.to_string()],
            )?;
            if !nft_rule_listed(&listing, from, agent_listen)? {
                return Err(format!(
                    "serve の ExecStartPre= が nft の表 {nft_table} を入れたはずだが、規則が載って\
                     いない: {}",
                    listing.trim()
                ));
            }
            Ok(format!(
                "nft の表 {nft_table} が {from} 以外から読み口 {agent_listen} への TCP を落とす\
                 (規則は {} にあり、serve の起動のたびに入る)",
                options.nft_rules_path().display()
            ))
        }
        None => Err("firewall の実物が決まっていない(run の順序の誤り)".to_string()),
    }
}

pub fn run(options: Options, out: &mut dyn Write) -> Result<(), String> {
    let mut options = normalize(options)?;
    let scope = options.scope.clone();
    let scope = &scope;
    let say = |out: &mut dyn Write, line: &str| -> Result<(), String> {
        writeln!(out, "install: {line}").map_err(|e| format!("標準出力に書けない: {e}"))
    };
    if let Some(account) = scope.account() {
        say(
            out,
            &format!(
                "system 単位。unit は {} に置き、{}:{}(uid {})で走らせる。ストアと写し先に \
                 root 所有のものは残さない(最後に所有者を見て確かめる)",
                options.unit_dir.display(),
                account.name,
                account.group,
                account.uid
            ),
        )?;
    }

    say(
        out,
        &format!(
            "インスタンス {}(unit は {}、nft の表は {})",
            options.instance,
            options.serve_unit(),
            options.nft_table()
        ),
    )?;

    // (0) テンプレート unit になる前の名が据え先に残っていないか。残ったまま既定の
    // インスタンスを据えると、同じ主の口と同じストアを 2 つの unit が取り合う。黙って
    // 外しはしない(操作者のものを止めるのは操作者の判断。must/0022)。別の名の
    // インスタンスなら取り合わないので、そのまま進む。
    let legacy = legacy_units_present(&options.unit_dir);
    if !legacy.is_empty() {
        if options.instance == DEFAULT_INSTANCE {
            return Err(format!(
                "据え先 {} にテンプレートになる前の名の unit が残っている({})。既定の\
                 インスタンス {DEFAULT_INSTANCE} を据えると、同じ主の口と同じストアを 2 つの \
                 unit が取り合う。先に外す: {}。nft を使っているなら古い表も消す\
                 (nft delete table inet uniqnode)。別の実体を足すだけなら \
                 {INSTANCE_FLAG} <名> で名前を分ける",
                options.unit_dir.display(),
                legacy.join(" "),
                legacy_units_removal_command(scope, &options.unit_dir, &legacy)
            ));
        }
        say(
            out,
            &format!(
                "据え先に古い名の unit がある({})が、据えるのは {} なので取り合わない\
                 (そのまま残す)",
                legacy.join(" "),
                options.instance
            ),
        )?;
    }

    // (1) バイナリ。
    let binary_line = install_binary(&options.binary, scope.account())?;
    say(out, &binary_line)?;

    // (2) unit。docs/mop/systemd/{user,system}/ の現物をそのまま。テンプレートなので
    // 名前にインスタンスは入らない(`uniqnode-serve@.service` のまま置く)。
    for (name, text) in options.units() {
        let path = options.unit_dir.join(name);
        write_file(&path, text)?;
        say(out, &format!("unit {}", path.display()))?;
    }

    // (3) drop-in と、ReadWritePaths= が要求する在るディレクトリ。unit の PATH は systemd の
    // 既定なので、自分の PATH で見つけた道具の場所を前に足す。無い道具は 1 行ずつ言う
    // (黙って進めない。must/0022)が、止めない: 道具が無くても serve は起き、無い機能だけが
    // 503 で答える(should/0114 と同じ扱い)。環境から PATH を読むのはここだけ。
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let search_path = tool_search_path(&path_env.to_string_lossy(), scope);
    // firewall の実物は drop-in を描く前に決める(nft なら serve の ExecStartPre= に載る)。
    if let (Some(from), Some(agent_listen)) = (&options.firewall_allow, &options.agent_listen) {
        let backend = firewall_backend(&search_path)?;
        match &backend {
            FirewallBackend::Ufw => say(out, "firewall: ufw が active。規則は ufw に入れる")?,
            FirewallBackend::Nft(nft) => {
                let path = options.nft_rules_path();
                let table = options.nft_table();
                write_file(&path, &nft_rules_text(&table, from, agent_listen)?)?;
                say(
                    out,
                    &format!(
                        "firewall: ufw は active でない。nft の表 {table} を {} に書き、serve の \
                         ExecStartPre=+{} -f が起動のたびに入れる",
                        path.display(),
                        nft.display()
                    ),
                )?;
            }
        }
        options.firewall_backend = Some(backend);
    }
    let options = &options;
    let tools = tool_path(&search_path);
    say(
        out,
        &format!(
            "PATH={}(serve が起こす外部の道具の探し先。探したのは {search_path})",
            tools.value
        ),
    )?;
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
        let existed = dir.exists();
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れない: {e}", dir.display()))?;
        // root が作ったディレクトリは実行ユーザの所有にする。在ったものの所有者は変えない
        // (最後の確認が見る)。
        let owner_note = match scope.account() {
            Some(account) if !existed => {
                chown_to(dir, account)?;
                format!("。作ったので {} の所有にした", account.name)
            }
            _ => String::new(),
        };
        say(
            out,
            &format!(
                "{what} {}(ReadWritePaths= は無い道を作らないので先に作る{owner_note})",
                dir.display()
            ),
        )?;
    }

    // (4) daemon-reload。
    systemctl_ok(scope, &["daemon-reload"])?;
    say(out, &format!("{}daemon-reload: 済み", scope.systemctl_prefix()))?;

    if !options.start {
        say(
            out,
            &format!(
                "--no-start なので起こしていない。起こすには {}enable --now {}",
                scope.systemctl_prefix(),
                options.started_units().join(" ")
            ),
        )?;
        return Ok(());
    }

    // (5) user 単位からの移行なら、先に止めて外す(--take-over-user-units。指定が無ければ
    // 下の探針が止めて外す命令を添えて断る)。
    if options.take_over_user_units {
        let account = scope.account().expect("normalize が system 単位に限る");
        let line = take_over_user_units(scope, account, &options.instance, &options.data_dir, out)?;
        say(out, &line)?;
    }

    // 起こす前にロックを探る。unit は exit 1 で起こし直さない設計なので、落ちてから journal
    // を読ませるより先に言う。system 単位への移行では、持ち主は user 単位の serve であることが
    // 多い: 止めて外す命令を添えるが、黙って止めはしない。
    if store::opened_by_another_process(&options.data_dir).map_err(|e| e.to_string())? {
        let serve_unit = options.serve_unit();
        let serve_state = unit_state(scope, &serve_unit)?;
        if serve_state != "active" {
            let migration_hint = match scope {
                Scope::User => String::new(),
                Scope::System(account) => format!(
                    "。user 単位で常駐させていたなら、{} で {} を打って止めて外してから再実行するか、\
                     {TAKE_OVER_FLAG} を足して install に止めさせる",
                    account.name,
                    user_units_disable_command(&options.instance)
                ),
            };
            return Err(format!(
                "{} は別プロセスが開いている(unit の serve ではない。{serve_unit} は {serve_state})。\
                 そのプロセスを止めてから再実行するか、--no-start で unit だけ置く{migration_hint}",
                options.data_dir.display()
            ));
        }
    }

    // (6) enable と restart(restart は止まっている unit も起こすので、初回と更新で同じ手順)。
    for unit in options.started_units() {
        let unit = unit.as_str();
        let before = unit_state(scope, unit)?;
        systemctl_ok(scope, &["enable", unit])?;
        systemctl_ok(scope, &["restart", unit])?;
        let verb = if before == "active" {
            "起こし直した"
        } else {
            "起こした"
        };
        say(out, &format!("{unit}: enable、{verb}(直前は {before})"))?;
    }

    // (7) linger(user 単位だけ)。無ければログアウトで user 単位のマネージャごと止まる。
    // 取れないのは警告にとどめる(polkit の設定次第で対話が要る)。system 単位は機械と共に
    // 起きるので要らない。
    if !scope.is_system() {
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
    }

    // (8) 確認。効果を見るまでは完了ではない(should/0116)。
    let (node_id, waited) = wait_for_matching_status(
        scope,
        &options.listen,
        &options.viewer_listen,
        &options.serve_unit(),
        &options.viewer_unit(),
    )?;
    say(
        out,
        &format!(
            "確認: {}/v1/status と viewer {} 経由が同じ node_id {node_id} を返した({} ms)",
            serve_url_of(&options.listen),
            serve_url_of(&options.viewer_listen),
            waited.as_millis()
        ),
    )?;
    if let Some(agent_listen) = &options.agent_listen {
        let waited = wait_for_agent_door(scope, &options.serve_unit(), &node_id, agent_listen)?;
        say(
            out,
            &format!(
                "確認: 読み口 {}/v1/status が同じ node_id {node_id} を返し、POST /v1/admin/gc を \
                 403 で断った({} ms)",
                serve_url_of(agent_listen),
                waited.as_millis()
            ),
        )?;
        if !options.agent_writable.is_empty() {
            let seen = verify_agent_writable(agent_listen, &options.agent_writable)?;
            say(out, &format!("確認: 読み口の書く口: {seen}"))?;
        }
        if !options.agent_collections.is_empty() {
            let seen = verify_agent_collections(agent_listen, &options.agent_collections)?;
            say(out, &format!("確認: 読み口の読める集合: {seen}"))?;
        }
        let graph_allowed = crate::agent_door::Allowed {
            graphs: options.agent_graph.clone(),
            graphs_writable: options.agent_graph_writable.clone(),
            ..Default::default()
        };
        if !graph_allowed.readable_graphs().is_empty() {
            let seen = verify_agent_graph(agent_listen, &graph_allowed)?;
            say(out, &format!("確認: 読み口のグラフ: {seen}"))?;
        }
    }
    let backup_unit = options.backup_unit();
    systemctl_ok(scope, &["start", &backup_unit]).map_err(|e| {
        format!(
            "{e}。理由は {}-u {backup_unit} -n 20",
            scope.journalctl_prefix()
        )
    })?;
    let verification = verify_backup(&options)?;
    say(
        out,
        &format!(
            "確認: {backup_unit} を 1 回走らせ、写し先 {} を開いて fsck: {verification}",
            options.backup_dir.display()
        ),
    )?;
    if let Some(account) = scope.account() {
        for (dir, what) in [
            (&options.data_dir, "ストア"),
            (&options.backup_dir, "写し先"),
        ] {
            let foreign = paths_not_owned_by(dir, account.uid)?;
            if !foreign.is_empty() {
                return Err(format!(
                    "{what} {} に {} 以外の所有のものが {} 件ある(root で触った跡。unit は {} で\
                     走るので書けなくなる): {}",
                    dir.display(),
                    account.name,
                    foreign.len(),
                    account.name,
                    foreign
                        .iter()
                        .take(5)
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            say(
                out,
                &format!(
                    "確認: {what} {} の下に {} 以外の所有のものは無い",
                    dir.display(),
                    account.name
                ),
            )?;
        }
    }

    // (8b) firewall。読み口が答えるのを見た後に、届いてよい相手の規則を入れる。
    if options.firewall_backend.is_some() {
        let line = apply_firewall(options, out)?;
        say(out, &format!("確認: {line}"))?;
    }

    // (9) 次の刻み。systemctl の表は見出しと行の後に空行と件数の脚注が付くので、表だけを載せる。
    let timers = systemctl_ok(scope, &["list-timers", &options.backup_timer(), "--no-pager"])?;
    for line in timers.lines().take_while(|line| !line.trim().is_empty()) {
        say(out, &format!("次の刻み: {line}"))?;
    }
    Ok(())
}

/// テンプレート unit になる前の名の unit で、据え先に残っているもの。
pub fn legacy_units_present(unit_dir: &Path) -> Vec<&'static str> {
    LEGACY_UNITS
        .into_iter()
        .filter(|unit| unit_dir.join(unit).exists())
        .collect()
}

/// 古い名の unit を止めて外す命令(操作者が打つもの。文書と install の断りが同じ字句を
/// 使う。must/0023)。drop-in のディレクトリも一緒に消す。
pub fn legacy_units_removal_command(scope: &Scope, unit_dir: &Path, units: &[&str]) -> String {
    let started: Vec<&str> = units
        .iter()
        .copied()
        .filter(|unit| *unit != "uniqnode-backup.service")
        .collect();
    let paths: Vec<String> = units
        .iter()
        .flat_map(|unit| {
            let path = unit_dir.join(unit);
            [
                path.to_string_lossy().to_string(),
                format!("{}.d", path.to_string_lossy()),
            ]
        })
        .collect();
    format!(
        "{}disable --now {} ; rm -rf {} ; {}daemon-reload",
        scope.systemctl_prefix(),
        started.join(" "),
        paths.join(" "),
        scope.systemctl_prefix()
    )
}

/// user 単位の常駐を止めて外す命令(system 単位へ移るときに操作者が打つもの。文書と文言が
/// 同じ字句を使う。must/0023)。
pub fn user_units_disable_command(instance: &str) -> String {
    let units: Vec<String> = STARTED_TEMPLATES
        .iter()
        .map(|template| unit_for(template, instance))
        .collect();
    format!("systemctl --user disable --now {}", units.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_options() -> Options {
        Options {
            data_dir: PathBuf::from("/home/op/store"),
            instance: DEFAULT_INSTANCE.to_string(),
            listen: "127.0.0.1:7443".to_string(),
            viewer_listen: "127.0.0.1:7453".to_string(),
            serve_options: "--embed http://127.0.0.1:8083/v1/embeddings".to_string(),
            backup_dir: PathBuf::from("/home/op/uniqnode-backup"),
            binary: PathBuf::from("/home/op/.local/bin/uniqnode"),
            unit_dir: PathBuf::from("/home/op/.config/systemd/user"),
            start: true,
            scope: Scope::User,
            after: Vec::new(),
            agent_listen: None,
            agent_writable: Vec::new(),
            agent_collections: Vec::new(),
            agent_graph: Vec::new(),
            agent_graph_writable: Vec::new(),
            take_over_user_units: false,
            firewall_allow: None,
            firewall_backend: None,
        }
    }

    fn sample_account() -> Account {
        Account {
            name: "op".to_string(),
            uid: 1000,
            gid: 1000,
            group: "op".to_string(),
            home: PathBuf::from("/home/op"),
        }
    }

    /// テンプレートの名で drop-in を引く(書き出す名はインスタンスが入ったもの)。
    fn drop_in_for(template: &str, options: &Options) -> String {
        let unit = options.unit(template);
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
        let text = drop_in_for(SERVE_TEMPLATE, &sample_options());
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
        let serve = drop_in_for(SERVE_TEMPLATE, &options);
        assert!(
            serve.contains("\nEnvironment=UNIQNODE_SERVE_OPTIONS=\n"),
            "{serve}"
        );
        let viewer = drop_in_for(VIEWER_TEMPLATE, &options);
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
        let backup = drop_in_for(BACKUP_TEMPLATE, &options);
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

    /// 据え先の判断: user 単位に --user は付かず、system 単位は --user → SUDO_USER の順で
    /// 実行ユーザを決め、root は断り、root で走っていなければ断る(順序は「利用者が要る」が
    /// 先。root でなくても文言を観測できるように)。実在する利用者の引き当ては最後なので、
    /// ここまでの判断は実在しない名でも見られる。
    #[test]
    fn the_scope_decides_the_service_user_before_asking_for_root() {
        let system = |user: Option<&str>, sudo_user: Option<&str>, euid: u32| {
            scope(
                true,
                user.map(str::to_string),
                sudo_user.map(str::to_string),
                euid,
            )
        };
        assert_eq!(scope(false, None, None, 1000).expect("user"), Scope::User);
        assert_eq!(
            scope(false, None, Some("op".to_string()), 1000).expect("user"),
            Scope::User,
            "user 単位は SUDO_USER を見ない"
        );
        let user_with_user = scope(false, Some("op".to_string()), None, 1000)
            .err()
            .expect("断る");
        assert!(user_with_user.contains("--system のときだけ"), "{user_with_user}");
        let no_user = system(None, None, 0).err().expect("断る");
        assert!(no_user.contains("--user <name>"), "{no_user}");
        let empty_sudo_user = system(None, Some(""), 0).err().expect("断る");
        assert!(empty_sudo_user.contains("--user <name>"), "{empty_sudo_user}");
        let root_user = system(Some("root"), None, 0).err().expect("断る");
        assert!(root_user.contains("root は使えない"), "{root_user}");
        let root_sudo = system(None, Some("root"), 0).err().expect("断る");
        assert!(root_sudo.contains("root は使えない"), "{root_sudo}");
        let not_root = system(Some("no-such-user"), None, 1000).err().expect("断る");
        assert!(
            not_root.contains("sudo ") && not_root.contains("tee /tmp/uniqnode-install-system.log"),
            "{not_root}"
        );
        let sudo_user_not_root = system(None, Some("no-such-user"), 1000).err().expect("断る");
        assert!(sudo_user_not_root.contains("no-such-user"), "{sudo_user_not_root}");
    }

    /// 所有者の観測: 自分が作った木は自分の uid で「他人のものは無い」、別の uid で見れば
    /// 全項目が挙がる(ディレクトリ自身を含む)。
    #[test]
    fn foreign_owners_are_listed_with_the_directory_itself() {
        use std::os::unix::fs::MetadataExt;
        let dir = std::env::temp_dir().join(format!(
            "uniqnode-install-unit-{}-owners",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
        std::fs::write(dir.join("sub").join("file"), "x").expect("write");
        let me = std::fs::metadata(&dir).expect("metadata").uid();
        assert!(paths_not_owned_by(&dir, me).expect("walk").is_empty());
        let foreign = paths_not_owned_by(&dir, me.wrapping_add(1)).expect("walk");
        assert_eq!(
            foreign,
            vec![dir.clone(), dir.join("sub"), dir.join("sub").join("file")]
        );
        assert!(paths_not_owned_by(&dir.join("missing"), me).is_err());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// system 単位で家の下に何かがあれば ProtectHome=read-only に緩め、無ければ unit のまま。
    #[test]
    fn protected_home_paths_are_named() {
        let mut options = sample_options();
        options.scope = Scope::System(sample_account());
        options.data_dir = PathBuf::from("/srv/store");
        let under = paths_under_protected_home(&options);
        assert_eq!(
            under,
            vec![
                Path::new("/home/op/.local/bin/uniqnode"),
                Path::new("/home/op/uniqnode-backup")
            ]
        );
        options.binary = PathBuf::from("/usr/local/bin/uniqnode");
        options.backup_dir = PathBuf::from("/var/backups/uniqnode");
        assert!(paths_under_protected_home(&options).is_empty());
        options.data_dir = PathBuf::from("/root/store");
        assert_eq!(
            paths_under_protected_home(&options),
            vec![Path::new("/root/store")]
        );
    }

    /// ExecStart= の差し替えは先頭の道だけを替え、残りの並びを unit から写す。
    #[test]
    fn exec_start_rewrite_replaces_only_the_binary() {
        let rewritten =
            exec_start_with_binary(unit_text(&USER_UNITS, SERVE_TEMPLATE), "/opt/u/uniqnode")
                .expect("rewrite");
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
