//! serve の主の口の信頼の境界(API_AUTH (uuid:abde9b3c-75f8-453b-988e-bfb1e178c771) の 1)。
//!
//! 主の口は認証を持たず、次の 3 つで境界を置く:
//! - 束縛はループバックの IP リテラルだけ(check_listen。serve と install が同じ判定を使う)。
//! - 接続の相手のソケットの uid が許す集合に入ること(MainDoor::check。要求を 1 バイトも読む
//!   前に、NETLINK_SOCK_DIAG へ 1 件照会する。node/src/sock_diag.rs)。
//! - ブラウザからの要求を断る門(Host・Origin・Content-Type。http::BrowserGate と、道ごとの
//!   型の決まり screen_content_type)。
//!
//! 主の口に届くプロセスは全部、操作者とみなす。網越しに届く口(読み口)はここを通らず、
//! 自分の許可表(node/src/agent_door.rs)を持つ。

use crate::http::{self, Request, Response};
use crate::sock_diag;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

/// 許す uid の集合を置き換える serve の引数。
pub const ALLOW_UID_FLAG: &str = "--main-allow-uid";
/// 許す Host(authority)を足す serve の引数。
pub const ALLOW_HOST_FLAG: &str = "--main-allow-host";

/// テスト用の口の鍵: 下のテスト用の口(UNIQNODE_MAIN_ で始まる 4 つ)は、cfg(debug_assertions) のビルドで、かつこの
/// 環境変数が `1` のときだけ読む(debug ビルドを常駐に置いても、環境変数 1 つの取り違えで
/// 判定が変わらないように、2 つ揃って初めて効く)。release ビルドには入らない。使っている
/// テスト用の口は起動時にログへ言う。
pub const TEST_HOOKS_ENV: &str = "UNIQNODE_MAIN_TEST_HOOKS";

/// テスト用の口: 値はミリ秒で、判定の前にその間待つ。相手が送ってすぐ閉じた接続を、相手の
/// ソケットが FIN_WAIT2・TIME_WAIT の形になってから判定させ、判定の枠が埋まった形を作るため
/// (node/tests/main_door.rs)。起動時の自己試験には効かない。
pub const CHECK_DELAY_ENV: &str = "UNIQNODE_MAIN_CHECK_DELAY_MS";

/// テスト用の口: 値はファイルの道で、判定のときにそのファイルがあれば、中身の uid を相手の
/// ソケットの uid として判定する(無ければ実際の uid のまま)。別の uid の接続は root 無しには
/// 作れないので、断られる相手と許される相手を同じ serve に当てる試験(node/tests/main_door.rs)が
/// これで前者を作る。起動時の自己試験には効かない。
pub const PEER_UID_FILE_ENV: &str = "UNIQNODE_MAIN_PEER_UID_FILE";

/// テスト用の口: 値が `1` なら、sock_diag の照会ソケットを作れない(AF_NETLINK を塞いだ unit で
/// socket が EAFNOSUPPORT を返す)形を真似る。起動時の自己試験が理由を言って 2 で終わることを
/// 試験で固めるため。
pub const NETLINK_UNAVAILABLE_ENV: &str = "UNIQNODE_MAIN_NETLINK_UNAVAILABLE";

/// テスト用の口: 値はミリ秒で、断りの読み捨ての期限(http::REFUSAL_DRAIN_TIME、既定 1 秒)を
/// 置き換える。断りの枠を試験の間ずっと埋めたままにして、枠の数を決まった形で見るため。
pub const REFUSAL_DRAIN_ENV: &str = "UNIQNODE_MAIN_REFUSAL_DRAIN_MS";

/// テスト用の口の値(鍵が揃わなければ None)。
fn test_hook(name: &str) -> Option<String> {
    #[cfg(debug_assertions)]
    {
        if std::env::var(TEST_HOOKS_ENV).is_ok_and(|value| value == "1") {
            return std::env::var(name).ok();
        }
        None
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = name;
        None
    }
}

/// 起動時の自己試験の期限(自分への接続と、それを accept するまで)。
pub const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(2);
/// 自己試験で、自分の接続の端が分かる前に accept して持っておく接続の上限。越えた分は
/// その場で閉じる(listening on の前に繋いでくる他人の接続で、記憶と fd を際限なく使わない)。
pub const SELF_TEST_PENDING_LIMIT: usize = 256;

/// overflowuid の置き場(写せない uid がこの値で表示される)。
pub const OVERFLOW_UID_PATH: &str = "/proc/sys/kernel/overflowuid";

/// 主の口の束縛先を検める: ループバックの IP リテラル(`127.0.0.0/8` と `[::1]`)だけを通す。
/// `localhost` のような名前、unspecified(`0.0.0.0`・`[::]`)、IPv4 射影(`[::ffff:127.0.0.1]`)、
/// ループバックでないアドレスは断る。例外のフラグは置かない: 認証の無い主の口を網へ出す道を
/// 作らない。ポートは問わない(0 の自動割当を断るかは呼び手が決める)。
pub fn check_listen(listen: &str, what: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = listen.parse().map_err(|_| {
        format!(
            "{what} {listen}: 主の口はループバックの IP リテラル(127.0.0.1:7440 や [::1]:7440)に\
             だけ束縛する(名前は解決の結果が変わりうるので受けない)"
        )
    })?;
    let loopback = match address.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        // Ipv6Addr::is_loopback は ::1 だけを真とする(IPv4 射影は偽)。
        IpAddr::V6(v6) => v6.is_loopback(),
    };
    if !loopback {
        return Err(format!(
            "{what} {listen}: 主の口はループバック(127.0.0.0/8 か [::1])にだけ束縛する。\
             主の口は認証を持たないので網へ出さない(網越しに要るものは読み口で受ける。\
             docs/plan/API_AUTH.md)"
        ));
    }
    Ok(address)
}

/// 主の口の既定の許す Host: 束縛したループバックの字面と `localhost:<port>`。ポート 0 で
/// 束縛したときは、実際に割り当てられたポート(bound)で作る。
pub fn default_hosts(bound: SocketAddr) -> Vec<String> {
    vec![bound.to_string(), format!("localhost:{}", bound.port())]
}

/// `--main-allow-uid` の値(`1000,0` のような , 区切り)を読む。
pub fn parse_uid_list(text: &str) -> Result<Vec<u32>, String> {
    let mut uids = Vec::new();
    for part in text.split(',') {
        let uid: u32 = part
            .trim()
            .parse()
            .map_err(|_| format!("{ALLOW_UID_FLAG} {text}: uid は , で区切った非負整数"))?;
        if !uids.contains(&uid) {
            uids.push(uid);
        }
    }
    Ok(uids)
}

/// `--main-allow-host` の値を検める(`host:port` の 1 語)。
pub fn check_allow_host(text: &str) -> Result<(), String> {
    let well_formed = match text.rsplit_once(':') {
        Some((host, port)) => {
            !host.is_empty()
                && port.parse::<u16>().is_ok()
                && !text.chars().any(|c| c.is_whitespace() || c == '/' || c == ',')
        }
        None => false,
    };
    match well_formed {
        true => Ok(()),
        false => Err(format!(
            "{ALLOW_HOST_FLAG} {text}: Host の値を host:port の形で書く(例 localhost:17440)"
        )),
    }
}

/// /proc/sys/kernel/overflowuid を読む。
pub fn read_overflow_uid() -> Result<u32, String> {
    let text = std::fs::read_to_string(OVERFLOW_UID_PATH)
        .map_err(|e| format!("{OVERFLOW_UID_PATH} を読めない: {e}"))?;
    text.trim()
        .parse()
        .map_err(|_| format!("{OVERFLOW_UID_PATH} の値を読めない: {text:?}"))
}

/// 主の口の uid の判定。
pub struct MainDoor {
    allowed_uids: Vec<u32>,
    euid: u32,
    overflow_uid: u32,
    check_delay: Option<Duration>,
    peer_uid_file: Option<std::path::PathBuf>,
    netlink_unavailable: bool,
    refusal_drain: Option<Duration>,
}

impl MainDoor {
    /// 起動時に組む。allowed は `--main-allow-uid` の集合(None なら serve 自身の euid だけ)。
    /// sock_diag の無い OS、overflowuid を読めないとき、許す集合に overflowuid が入るときは、
    /// 理由を言って断る(写せない uid は本当にその uid のソケットと区別できない)。
    pub fn new(allowed: Option<Vec<u32>>, euid: u32, overflow_uid: u32) -> Result<MainDoor, String> {
        if !sock_diag::SUPPORTED {
            return Err(
                "主の口の uid の判定は Linux の sock_diag に依る。この OS では serve を起こさない"
                    .to_string(),
            );
        }
        let explicit = allowed.is_some();
        let allowed_uids = allowed.unwrap_or_else(|| vec![euid]);
        if allowed_uids.contains(&overflow_uid) {
            return Err(format!(
                "許す uid の集合 {} に overflowuid {overflow_uid} が入っている。写せない uid は\
                 この値で表示されるので、認可に使えない({})",
                sock_diag::uid_list(&allowed_uids),
                match explicit {
                    true => format!("{ALLOW_UID_FLAG} で与えた"),
                    false => format!(
                        "既定の集合は serve 自身の euid で、serve が uid を写せない user namespace で\
                         走っている。{ALLOW_UID_FLAG} で集合を与える"
                    ),
                }
            ));
        }
        Ok(MainDoor {
            allowed_uids,
            euid,
            overflow_uid,
            check_delay: test_hook(CHECK_DELAY_ENV)
                .and_then(|text| text.parse::<u64>().ok())
                .map(Duration::from_millis),
            peer_uid_file: test_hook(PEER_UID_FILE_ENV).map(std::path::PathBuf::from),
            netlink_unavailable: test_hook(NETLINK_UNAVAILABLE_ENV).is_some_and(|v| v == "1"),
            refusal_drain: test_hook(REFUSAL_DRAIN_ENV)
                .and_then(|text| text.parse::<u64>().ok())
                .map(Duration::from_millis),
        })
    }

    /// 断りの読み捨ての期限(ふつうは http::REFUSAL_DRAIN_TIME。テスト用の口で置き換えられる)。
    pub fn refusal_drain(&self) -> Duration {
        self.refusal_drain.unwrap_or(http::REFUSAL_DRAIN_TIME)
    }

    /// 効いているテスト用の口の名前(起動時のログに載せる。release ビルドでは常に空)。
    pub fn test_hooks_in_use(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.check_delay.is_some() {
            names.push(CHECK_DELAY_ENV);
        }
        if self.peer_uid_file.is_some() {
            names.push(PEER_UID_FILE_ENV);
        }
        if self.netlink_unavailable {
            names.push(NETLINK_UNAVAILABLE_ENV);
        }
        if self.refusal_drain.is_some() {
            names.push(REFUSAL_DRAIN_ENV);
        }
        names
    }

    /// sock_diag への照会(テスト用の口 NETLINK_UNAVAILABLE_ENV が効いていれば、照会ソケットを
    /// 作れない形の Err を返す)。
    fn lookup(&self, expected: sock_diag::Endpoints) -> Result<Vec<sock_diag::Answer>, String> {
        if self.netlink_unavailable {
            const EAFNOSUPPORT: i32 = 97;
            return Err(sock_diag::socket_unavailable(&std::io::Error::from_raw_os_error(
                EAFNOSUPPORT,
            )));
        }
        sock_diag::lookup(expected)
    }

    /// 起動時の自己試験(束縛の後、ストアを開く前・listening on の前に 1 回)。束縛した主の口へ
    /// ループバックで自分から 1 本繋ぎ、その接続を accept して、相手(自分)の uid が serve の
    /// euid と判定されることを確かめる。照会ソケットを作れない(unit が AF_NETLINK を塞いでいる)
    /// か照会が壊れていると、serve は起動しても全部の接続を 403 で断り、unit は active のまま
    /// Restart= も効かず、/v1/admin/shutdown も届かない。その形で起き上がらないために、ここで
    /// 理由を言って終わる。許す集合(--main-allow-uid)は問わない(集合から euid を外した
    /// serve も起きてよい)。テスト用の口のうち判定の遅れと uid の差し替えは効かない。
    ///
    /// listening on の前なので、この間に届いた他人の接続は読まずに閉じる(待つ側は listening on
    /// を見てから繋ぐ取り決め)。
    ///
    /// 失敗は 2 種類に分ける(SelfTestError::exit_code)。設定が原因で起こし直しても直らないもの
    /// (照会ソケットの socket() が EAFNOSUPPORT・EPERM・EACCES で失敗した、自分の uid が
    /// overflowuid に見える)は 2 で、unit の RestartPreventExitStatus= が起こし直さない。それ
    /// 以外(期限切れ、照会の一時の失敗など)は 3 で、unit の Restart=on-failure が起こし直す。
    pub fn self_test(&self, listener: &std::net::TcpListener) -> Result<(), SelfTestError> {
        let transient = |reason: String| SelfTestError { config: false, reason };
        match self.probe_socket() {
            Ok(()) => {}
            Err(error) => {
                const EPERM: i32 = 1;
                const EACCES: i32 = 13;
                const EAFNOSUPPORT: i32 = 97;
                let config = matches!(error.raw_os_error(), Some(EPERM | EACCES | EAFNOSUPPORT));
                return Err(SelfTestError { config, reason: sock_diag::socket_unavailable(&error) });
            }
        }
        let bound = listener.local_addr().map_err(|e| transient(format!("束縛先を読めない: {e}")))?;
        let (client, accepted) = connect_to_self(listener, bound).map_err(transient)?;
        let peer =
            accepted.peer_addr().map_err(|e| transient(format!("相手のアドレスを読めない: {e}")))?;
        let local =
            accepted.local_addr().map_err(|e| transient(format!("自分のアドレスを読めない: {e}")))?;
        let expected = sock_diag::Endpoints { local: peer, remote: local };
        let answers = self.lookup(expected).map_err(transient)?;
        let judged = sock_diag::judge(&answers, expected, self.overflow_uid, &[self.euid]);
        // 自分の接続が判定を終えるまで、自分の側の端を開けておく(閉じると ESTABLISHED でなくなる)。
        drop(client);
        judged.map(|_| ()).map_err(|reason| SelfTestError {
            // 自分の uid が overflowuid に見えるのは user namespace の写しの設定で、一時の失敗ではない。
            config: answers.iter().any(|answer| answer.uid == self.overflow_uid),
            reason,
        })
    }

    /// 照会ソケットを作れるか(テスト用の口 NETLINK_UNAVAILABLE_ENV が効いていれば EAFNOSUPPORT)。
    fn probe_socket(&self) -> std::io::Result<()> {
        if self.netlink_unavailable {
            const EAFNOSUPPORT: i32 = 97;
            return Err(std::io::Error::from_raw_os_error(EAFNOSUPPORT));
        }
        sock_diag::probe_socket()
    }


    pub fn allowed_uids(&self) -> &[u32] {
        &self.allowed_uids
    }

    /// 接続 1 本の判定(要求を読む前)。許すなら Ok。
    pub fn check(&self, stream: &TcpStream) -> Result<(), String> {
        if let Some(delay) = self.check_delay {
            std::thread::sleep(delay);
        }
        let peer = stream.peer_addr().map_err(|e| format!("相手のアドレスを読めない: {e}"))?;
        let local = stream.local_addr().map_err(|e| format!("自分のアドレスを読めない: {e}"))?;
        // 相手のソケットから見た向き: local が相手、remote が自分の束縛先。
        let expected = sock_diag::Endpoints { local: peer, remote: local };
        let mut answers = self.lookup(expected)?;
        if let Some(path) = &self.peer_uid_file {
            if let Some(uid) = std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse().ok()) {
                for answer in &mut answers {
                    answer.uid = uid;
                }
            }
        }
        sock_diag::judge(&answers, expected, self.overflow_uid, &self.allowed_uids).map(|_| ())
    }
}

/// 起動時の自己試験の失敗。
#[derive(Debug)]
pub struct SelfTestError {
    /// 設定が原因で、起こし直しても直らない失敗か。
    pub config: bool,
    pub reason: String,
}

impl SelfTestError {
    /// serve の終了コード: 設定の誤りは 2(unit は起こし直さない)、それ以外は 3(起こし直す)。
    pub fn exit_code(&self) -> i32 {
        match self.config {
            true => 2,
            false => 3,
        }
    }
}

impl std::fmt::Display for SelfTestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = &self.reason;
        match self.config {
            true => write!(
                f,
                "起動時の自己試験: 主の口への自分の接続の uid を判定できない: {reason}。このまま\
                 起こすと主の口はすべての接続を 403 で断るので起動しない。設定の誤りなので 2 で\
                 終わる(systemd の unit なら RestrictAddressFamilies= に AF_NETLINK が要る)"
            ),
            false => write!(
                f,
                "起動時の自己試験: 主の口への自分の接続の uid を判定できない: {reason}。このまま\
                 起こすと主の口はすべての接続を 403 で断るので起動しない。一時の失敗でありうるので\
                 3 で終わる(systemd の unit は起こし直す)"
            ),
        }
    }
}

/// 自己試験の接続を作る: 自分の主の口への接続を裏のスレッドで始め、その完了を待つ間も accept
/// を回して待ち行列を空ける(先に繋ぎ切ってから accept すると、待ち行列が他人の接続で埋まって
/// いれば自分の SYN が落とされうる)。自分の接続の端が分かったら、相手がそれである接続を返し、
/// 他人の接続は閉じる。期限は SELF_TEST_TIMEOUT。listener の非ブロックは戻してから返す。
/// 返すのは (自分の側の端, accept した側の端)。
fn connect_to_self(
    listener: &std::net::TcpListener,
    bound: SocketAddr,
) -> Result<(TcpStream, TcpStream), String> {
    let deadline = std::time::Instant::now() + SELF_TEST_TIMEOUT;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("main-door-self-test".to_string())
        .spawn(move || {
            let _ = sender.send(TcpStream::connect_timeout(&bound, SELF_TEST_TIMEOUT));
        })
        .map_err(|e| format!("自己試験のスレッドを作れない: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| format!("束縛を非ブロックにできない: {e}"))?;
    let mut client: Option<(TcpStream, SocketAddr)> = None;
    // 自分の接続の端が分かる前に accept した接続(自分のものが混ざりうる)。
    let mut pending: Vec<(TcpStream, SocketAddr)> = Vec::new();
    let result = loop {
        // 期限は毎周に見る(他人の接続が途切れず届き続けても、WouldBlock を待たずに終わる)。
        if std::time::Instant::now() >= deadline {
            break Err(format!("自分の接続を {SELF_TEST_TIMEOUT:?} の内に accept できない"));
        }
        if client.is_none() {
            match receiver.try_recv() {
                Ok(Ok(stream)) => match stream.local_addr() {
                    Ok(address) => {
                        if let Some(at) = pending.iter().position(|(_, peer)| *peer == address) {
                            break Ok((stream, pending.swap_remove(at).0));
                        }
                        pending.clear();
                        client = Some((stream, address));
                    }
                    Err(e) => break Err(format!("自分の接続の端を読めない: {e}")),
                },
                Ok(Err(e)) => break Err(format!("{bound} へ繋げない: {e}")),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    break Err("自己試験の接続のスレッドが答えずに終わった".to_string())
                }
            }
        }
        match listener.accept() {
            Ok((stream, peer)) => match &client {
                Some((_, address)) if peer == *address => {
                    let (own, _) = client.take().expect("client");
                    break Ok((own, stream));
                }
                Some(_) => drop(stream),
                // 上限を越えた分はその場で閉じる(自分の接続がそれだったなら期限切れの 3 になる)。
                None if pending.len() >= SELF_TEST_PENDING_LIMIT => drop(stream),
                None => pending.push((stream, peer)),
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => break Err(format!("自分の接続を accept できない: {e}")),
        }
    };
    drop(pending);
    let restored = listener.set_nonblocking(false).map_err(|e| format!("束縛をブロックに戻せない: {e}"));
    let (own, accepted) = result?;
    restored?;
    accepted.set_nonblocking(false).map_err(|e| format!("接続をブロックに戻せない: {e}"))?;
    Ok((own, accepted))
}

/// 道ごとの本文の型の決まり(主の口だけ。読み口と feed の口には広げない)。
#[derive(Debug, PartialEq)]
pub enum BodyRule {
    /// JSON を読む道: application/json を要る。
    Json,
    /// 本文を省いてよい JSON の道(gc、グラフの PUT): 本文があるときだけ application/json を要る。
    JsonIfPresent,
    /// 生のバイト列を読む道: application/octet-stream か、文書の種別を言う型を要る。
    Raw,
    /// 本文を読まない道: 型を問わない。
    Any,
}

/// 要求の道の本文の決まり。道の表は api::handle と同じ字面で、api.rs の道を足したらここにも
/// 足す(足し忘れると Any になり、単純な 3 つの型の 415 だけが残る)。足し忘れは試験
/// every_route_in_api_rs_has_a_body_rule が api.rs の道の字面を拾って赤にする。
pub fn body_rule(method: &str, path: &str) -> BodyRule {
    let path = path.split('?').next().unwrap_or("");
    match (method, path) {
        ("POST", "/v1/query")
        | ("POST", "/v1/search")
        | ("POST", crate::distributed_search::PEER_QUERY_PATH)
        | ("POST", "/v1/pins")
        | ("POST", "/v1/sync") => BodyRule::Json,
        ("POST", "/v1/admin/gc") => BodyRule::JsonIfPresent,
        ("POST", "/v1/objects") => BodyRule::Raw,
        ("PUT", p) if p.starts_with("/v1/refs/") => BodyRule::Json,
        ("PUT", p) if p.starts_with("/v1/graphs/") => BodyRule::JsonIfPresent,
        ("POST", p) if p.starts_with("/v1/collections/") && p.ends_with("/fetch") => BodyRule::Json,
        ("PUT", p) if p.starts_with("/v1/collections/") => BodyRule::Raw,
        _ => BodyRule::Any,
    }
}

/// 道ごとの Content-Type の決まりを当てる。断るなら 415 の応答を返す。単純な 3 つの型の 415 は
/// http::BrowserGate が先に言う。
pub fn screen_content_type(request: &Request) -> Option<Response> {
    let media = request.header("content-type").map(http::media_type);
    let refuse = |wanted: &str| {
        Some(Response::text(
            415,
            &format!(
                "main door: {} {} の本文は {wanted} で送る(Content-Type: {})\n",
                request.method,
                request.path,
                media.as_deref().unwrap_or("無し")
            ),
        ))
    };
    match body_rule(&request.method, &request.path) {
        BodyRule::Json => match media.as_deref() {
            Some("application/json") => None,
            _ => refuse("application/json"),
        },
        BodyRule::JsonIfPresent if request.body.is_empty() => None,
        BodyRule::JsonIfPresent => match media.as_deref() {
            Some("application/json") => None,
            _ => refuse("application/json"),
        },
        BodyRule::Raw => match media.as_deref() {
            Some(m) if !m.is_empty() && !http::SIMPLE_CONTENT_TYPES.contains(&m) => None,
            _ => refuse("application/octet-stream か文書の種別を言う型"),
        },
        BodyRule::Any => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自己試験の失敗の終了コード: 設定の誤りは 2(起こし直さない)、それ以外は 3(起こし直す)。
    #[test]
    fn a_transient_self_test_failure_exits_3_and_a_config_one_2() {
        let error = |config| SelfTestError { config, reason: String::new() };
        assert_eq!(error(false).exit_code(), 3);
        assert_eq!(error(true).exit_code(), 2);
    }

    #[test]
    fn only_loopback_ip_literals_are_accepted() {
        for listen in ["127.0.0.1:7440", "127.0.0.2:7440", "[::1]:7440", "127.0.0.1:0"] {
            assert!(check_listen(listen, "--listen").is_ok(), "{listen}");
        }
        for listen in [
            "0.0.0.0:7440",
            "[::]:7440",
            "[::ffff:127.0.0.1]:7440",
            "10.10.128.1:7440",
            "localhost:7440",
        ] {
            let error = check_listen(listen, "--listen").expect_err(listen);
            assert!(error.contains(listen), "{error}");
            assert!(error.contains("ループバック"), "{error}");
        }
    }

    #[test]
    fn the_default_hosts_are_the_bound_literal_and_localhost() {
        assert_eq!(
            default_hosts("127.0.0.1:7440".parse().unwrap()),
            vec!["127.0.0.1:7440".to_string(), "localhost:7440".to_string()]
        );
        assert_eq!(
            default_hosts("[::1]:7440".parse().unwrap()),
            vec!["[::1]:7440".to_string(), "localhost:7440".to_string()]
        );
    }

    #[test]
    fn the_uid_list_reads_commas_and_refuses_words() {
        assert_eq!(parse_uid_list("1000,0"), Ok(vec![1000, 0]));
        assert_eq!(parse_uid_list("1000,1000"), Ok(vec![1000]));
        assert!(parse_uid_list("root").is_err());
        assert!(parse_uid_list("").is_err());
        assert!(check_allow_host("localhost:17440").is_ok());
        assert!(check_allow_host("localhost").is_err());
        assert!(check_allow_host("a b:1").is_err());
    }

    /// 許す集合に overflowuid が入れば、明示でも既定でも起動を断る。
    #[test]
    fn the_overflow_uid_in_the_set_refuses_to_start() {
        assert!(MainDoor::new(Some(vec![1000, 65534]), 1000, 65534).is_err());
        assert!(MainDoor::new(None, 65534, 65534).is_err());
        assert!(MainDoor::new(None, 1000, 65534).is_ok());
    }

    fn request(method: &str, path: &str, content_type: Option<&str>, body: &[u8]) -> Request {
        let mut headers = vec![("host".to_string(), "127.0.0.1:7440".to_string())];
        if let Some(value) = content_type {
            headers.push(("content-type".to_string(), value.to_string()));
        }
        Request { method: method.into(), path: path.into(), headers, body: body.to_vec() }
    }

    #[test]
    fn each_route_takes_its_own_content_type() {
        let status = |r: &Request| screen_content_type(r).map(|response| response.status);
        assert_eq!(status(&request("POST", "/v1/search", Some("application/json"), b"{}")), None);
        assert_eq!(
            status(&request("POST", "/v1/search", Some("application/json; charset=utf-8"), b"{}")),
            None
        );
        assert_eq!(status(&request("POST", "/v1/search", None, b"{}")), Some(415));
        assert_eq!(status(&request("PUT", "/v1/refs/a", None, b"{}")), Some(415));
        assert_eq!(status(&request("POST", "/v1/collections/c/fetch", None, b"{}")), Some(415));
        assert_eq!(status(&request("POST", "/v1/objects", Some("application/octet-stream"), b"x")), None);
        assert_eq!(
            status(&request("PUT", "/v1/collections/c/documents/a.md?meta.k=v", Some("text/markdown"), b"x")),
            None
        );
        assert_eq!(status(&request("POST", "/v1/objects", None, b"x")), Some(415));
        // 本文を持たずに動く道は型を問わない。
        assert_eq!(status(&request("POST", "/v1/admin/shutdown", None, b"")), None);
        assert_eq!(status(&request("POST", "/v1/admin/gc", None, b"")), None);
        assert_eq!(status(&request("POST", "/v1/admin/gc", None, b"{}")), Some(415));
        assert_eq!(status(&request("DELETE", "/v1/graphs/g/edges/t/a/b", None, b"")), None);
        assert_eq!(status(&request("PUT", "/v1/graphs/g/edges/t/a/b", None, b"")), None);
        assert_eq!(status(&request("PUT", "/v1/graphs/g/nodes/n", None, b"{}")), Some(415));
        assert_eq!(status(&request("GET", "/v1/status", None, b"")), None);
    }

    #[test]
    fn the_browser_gate_checks_host_origin_and_simple_types() {
        let gate = http::BrowserGate::new(&default_hosts("127.0.0.1:7440".parse().unwrap()));
        let status = |r: &Request| gate.screen(r).map(|response| response.status);
        assert_eq!(status(&request("GET", "/v1/status", None, b"")), None);
        let mut upper = request("GET", "/v1/status", None, b"");
        upper.headers[0].1 = "LOCALHOST:7440".into();
        assert_eq!(status(&upper), None, "大文字と小文字を区別しない");
        let mut other = request("GET", "/v1/status", None, b"");
        other.headers[0].1 = "attacker.example:7440".into();
        assert_eq!(status(&other), Some(421));
        let mut no_host = request("GET", "/v1/status", None, b"");
        no_host.headers.clear();
        assert_eq!(status(&no_host), Some(421));
        let mut origin = request("POST", "/v1/admin/shutdown", None, b"");
        origin.headers.push(("origin".into(), "http://127.0.0.1:7440".into()));
        assert_eq!(status(&origin), Some(403));
        let mut cross = request("GET", "/v1/status", None, b"");
        cross.headers.push(("sec-fetch-site".into(), "cross-site".into()));
        assert_eq!(status(&cross), Some(403));
        let mut same = request("GET", "/v1/status", None, b"");
        same.headers.push(("sec-fetch-site".into(), "none".into()));
        assert_eq!(status(&same), None);
        for simple in http::SIMPLE_CONTENT_TYPES {
            assert_eq!(status(&request("POST", "/v1/admin/shutdown", Some(simple), b"")), Some(415));
        }
    }

    /// api.rs の道の字面(一致の腕 `("POST", "/v1/…")` と、`path.strip_prefix("/v1/…")` の接頭辞)を
    /// 全部拾い、本文の決まりの付け忘れを赤にする(Claude 低 4)。本文を読む道(POST・PUT)は
    /// Any でないか、本文を読まない道の一覧にあること。接頭辞の道は下の表に代表の道と決まりを
    /// 置き、表に無い接頭辞が api.rs に現れたら赤にする(新しい道の決まりを決めさせる)。入れ子の
    /// 腕の字面(`("GET", "history")` など)と接尾辞(`strip_suffix("/fetch")`)も表と突き合わせ、
    /// 表に無いものは黙って飛ばさずに赤にする(再レビューの Claude 低 3)。
    #[test]
    fn every_route_in_api_rs_has_a_body_rule() {
        let source = include_str!("api.rs");
        // 本文を読まない POST(型を問わない)。
        const NO_BODY: [(&str, &str); 1] = [("POST", "/v1/admin/shutdown")];
        // 入れ子の腕: (親の接頭辞, method, api.rs の字面, 代表の道)。代表の道の決まりを、全体の道の
        // 腕と同じ規則(POST・PUT は Any でない、他は Any)で見る。
        const NESTED: [(&str, &str, &str, &str); 5] = [
            ("/v1/graphs/", "GET", "", "/v1/graphs/g/nodes/n"),
            ("/v1/graphs/", "PUT", "", "/v1/graphs/g/nodes/n"),
            ("/v1/graphs/", "DELETE", "", "/v1/graphs/g/nodes/n"),
            ("/v1/graphs/", "GET", "history", "/v1/graphs/g/nodes/n/history"),
            ("/v1/graphs/", "GET", "neighbors", "/v1/graphs/g/nodes/n/neighbors"),
        ];
        // 親の接頭辞から入れ子の腕のある関数までの呼び出しの鎖。親の対応は、api.rs で
        // `strip_prefix(親)` の枝が鎖の先頭を呼び、各関数が次を呼び、腕が鎖の末尾の関数の中に
        // あることで確かめる(表の親を書き違えると赤になる)。
        const CHAINS: [(&str, &[&str]); 1] = [("/v1/graphs/", &["handle_graph", "handle_graph_node"])];
        let fn_body = |name: &str| -> &str {
            let start = source.find(&format!("\nfn {name}(")).unwrap_or_else(|| panic!("fn {name} が無い"));
            let body = &source[start + 1..];
            let end = ["\nfn ", "\npub fn "].iter().filter_map(|next| body.find(next)).min();
            &body[..end.unwrap_or(body.len())]
        };
        let enclosing_fn = |at: usize| -> &str {
            let start = source[..at].rfind("\nfn ").expect("腕を囲む関数") + "\nfn ".len();
            let rest = &source[start..];
            &rest[..rest.find('(').expect("関数名の終わり")]
        };
        for (parent, chain) in CHAINS {
            let branch_at = source
                .find(&format!("strip_prefix(\"{parent}\")"))
                .unwrap_or_else(|| panic!("strip_prefix({parent}) が無い"));
            let branch = &source[branch_at..];
            let branch = &branch[..branch.find("\n    }").expect("枝の終わり")];
            assert!(branch.contains(&format!("{}(", chain[0])), "{parent} の枝が {} を呼ばない", chain[0]);
            for pair in chain.windows(2) {
                assert!(fn_body(pair[0]).contains(&format!("{}(", pair[1])), "{} が {} を呼ばない", pair[0], pair[1]);
            }
        }
        let mut nested_seen: Vec<(&str, &str, &str)> = Vec::new();
        let mut arms = Vec::new();
        for method in ["GET", "POST", "PUT", "DELETE"] {
            let opener = format!("(\"{method}\", ");
            for (at, _) in source.match_indices(&opener) {
                let rest = &source[at + opener.len()..];
                let Some(end) = rest.find(')') else { continue };
                let path_token = &rest[..end];
                let path = match path_token {
                    "crate::distributed_search::PEER_QUERY_PATH" => {
                        crate::distributed_search::PEER_QUERY_PATH.to_string()
                    }
                    token if token.starts_with("\"/") && token.ends_with('"') => {
                        token.trim_matches('"').to_string()
                    }
                    // 入れ子の腕(`("PUT", "")` など)は、下の NESTED の表に無ければ赤にする。
                    token if token.starts_with('"') && token.ends_with('"') && token.len() >= 2 => {
                        let literal = &token[1..token.len() - 1];
                        // 腕を囲む関数から、鎖を逆にたどって親の接頭辞を決める。
                        let function = enclosing_fn(at);
                        let Some((parent, _)) =
                            CHAINS.iter().find(|(_, chain)| chain.last() == Some(&function))
                        else {
                            panic!("api.rs の入れ子の腕 ({method}, {token}) を囲む {function} が CHAINS に無い");
                        };
                        let known = NESTED
                            .iter()
                            .find(|(p, m, l, _)| p == parent && *m == method && *l == literal);
                        let Some((_, _, _, sample)) = known else {
                            panic!(
                                "api.rs の入れ子の腕 ({parent}, {method}, {token}) の本文の決まりが NESTED の表に無い"
                            );
                        };
                        assert!(sample.starts_with(parent), "{sample} は {parent} の下でない");
                        nested_seen.push((parent, method, literal));
                        sample.to_string()
                    }
                    other => panic!("api.rs の腕 ({method}, {other}) の道を読めない(表に足す)"),
                };
                arms.push((method, path));
            }
        }
        for (parent, method, literal, _) in NESTED {
            assert!(
                nested_seen.contains(&(parent, method, literal)),
                "NESTED の ({parent}, {method}, {literal:?}) は api.rs にもう無い(表から消す)"
            );
        }
        assert!(arms.len() >= 15, "api.rs の道の腕を拾えていない: {arms:?}");
        for (method, path) in &arms {
            let rule = body_rule(method, path);
            match *method {
                "POST" | "PUT" if !NO_BODY.contains(&(method, path.as_str())) => {
                    assert_ne!(rule, BodyRule::Any, "{method} {path} に本文の決まりが無い")
                }
                _ => assert_eq!(rule, BodyRule::Any, "{method} {path}"),
            }
        }
        // 接頭辞の道: 接頭辞 → (method, 代表の道, 決まり)。
        let prefixes: [(&str, &[(&str, &str, BodyRule)]); 7] = [
            ("/v1/queries/", &[("GET", "/v1/queries/q1", BodyRule::Any)]),
            ("/v1/replication/refs?", &[("GET", "/v1/replication/refs?signer=a", BodyRule::Any)]),
            ("/v1/objects/", &[("GET", "/v1/objects/s256:0/citation", BodyRule::Any)]),
            ("/v1/closure/", &[("GET", "/v1/closure/s256:0", BodyRule::Any)]),
            (
                "/v1/refs/",
                &[("GET", "/v1/refs/a", BodyRule::Any), ("PUT", "/v1/refs/a", BodyRule::Json)],
            ),
            (
                "/v1/graphs/",
                &[
                    ("PUT", "/v1/graphs/g/nodes/n", BodyRule::JsonIfPresent),
                    ("PUT", "/v1/graphs/g/edges/t/a/b", BodyRule::JsonIfPresent),
                    ("DELETE", "/v1/graphs/g/edges/t/a/b", BodyRule::Any),
                ],
            ),
            (
                "/v1/collections/",
                &[
                    ("POST", "/v1/collections/c/fetch", BodyRule::Json),
                    ("PUT", "/v1/collections/c/documents/a.md?meta.k=v", BodyRule::Raw),
                ],
            ),
        ];
        let opener = "strip_prefix(\"";
        for (at, _) in source.match_indices(opener) {
            let rest = &source[at + opener.len()..];
            let prefix = &rest[..rest.find('"').expect("閉じの引用符")];
            if !prefix.starts_with("/v1/") {
                continue;
            }
            assert!(
                prefixes.iter().any(|(known, _)| *known == prefix),
                "api.rs の接頭辞の道 {prefix} の本文の決まりが表に無い"
            );
        }
        for (_, samples) in &prefixes {
            for (method, path, rule) in samples.iter() {
                assert_eq!(&body_rule(method, path), rule, "{method} {path}");
            }
        }
        // 接尾辞で分ける道(`rest.strip_suffix("/fetch")` など): 接尾辞 → (親の接頭辞、method、
        // 代表の道、決まり)。表に無い接尾辞が api.rs に現れたら赤にする。
        let suffixes: [(&str, &str, &str, &str, BodyRule); 4] = [
            ("/citation", "/v1/objects/", "GET", "/v1/objects/s256:0/citation", BodyRule::Any),
            ("/referrers", "/v1/objects/", "GET", "/v1/objects/s256:0/referrers", BodyRule::Any),
            ("/rendition", "/v1/objects/", "GET", "/v1/objects/s256:0/rendition", BodyRule::Any),
            ("/fetch", "/v1/collections/", "POST", "/v1/collections/c/fetch", BodyRule::Json),
        ];
        let opener = "strip_suffix(\"";
        for (at, _) in source.match_indices(opener) {
            let rest = &source[at + opener.len()..];
            let suffix = &rest[..rest.find('"').expect("閉じの引用符")];
            if !suffix.starts_with('/') {
                continue;
            }
            assert!(
                suffixes.iter().any(|(known, ..)| *known == suffix),
                "api.rs の接尾辞の道 {suffix} の本文の決まりが表に無い"
            );
        }
        for (suffix, parent, method, sample, rule) in &suffixes {
            assert!(sample.starts_with(parent) && sample.ends_with(suffix), "{sample}");
            assert_eq!(&body_rule(method, sample), rule, "{method} {sample}");
        }
        // method だけで分ける腕(`"PUT" => {` と `method != "POST"`)の数。本文を読む method の腕が
        // 増えたら、その道を上の表に足してからここの数を直す(黙って Any に落とさない)。
        let count = |needle: &str| source.matches(needle).count();
        let method_arms = [
            // /v1/refs/{name} の PUT と、/v1/graphs/{g}/edges/… の PUT。
            ("\"PUT\" =>", 2),
            ("\"POST\" =>", 0),
            // /v1/collections/{c}/fetch。
            ("method != \"POST\"", 1),
            // /v1/collections/{c}/documents/{name}。
            ("method != \"PUT\"", 1),
        ];
        for (needle, expected) in method_arms {
            assert_eq!(count(needle), expected, "api.rs の {needle} の数が変わった(道の表を見直す)");
        }
    }
}
