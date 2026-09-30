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

/// テスト用の口: cfg(debug_assertions) のビルドだけが読む環境変数。値はミリ秒で、判定の前に
/// その間待つ。相手が送ってすぐ閉じた接続を、相手のソケットが FIN_WAIT2・TIME_WAIT の形に
/// なってから判定させ、判定の枠が埋まった形を作るため(node/tests/main_door.rs)。release
/// ビルドには入らない。
pub const CHECK_DELAY_ENV: &str = "UNIQNODE_MAIN_CHECK_DELAY_MS";

/// テスト用の口: cfg(debug_assertions) のビルドだけが読む環境変数。値はファイルの道で、判定の
/// ときにそのファイルがあれば、中身の uid を相手のソケットの uid として判定する(無ければ実際の
/// uid のまま)。別の uid の接続は root 無しには作れないので、断られる相手と許される相手を
/// 同じ serve に当てる試験(node/tests/main_door.rs)がこれで前者を作る。release ビルドには
/// 入らない。
pub const PEER_UID_FILE_ENV: &str = "UNIQNODE_MAIN_PEER_UID_FILE";

/// 相手の uid の差し替え(テスト用。上の PEER_UID_FILE_ENV)。
fn peer_uid_file() -> Option<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os(PEER_UID_FILE_ENV).map(std::path::PathBuf::from)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// 判定の前に待つ時間(テスト用。上の CHECK_DELAY_ENV)。
fn check_delay() -> Option<Duration> {
    #[cfg(debug_assertions)]
    {
        std::env::var(CHECK_DELAY_ENV)
            .ok()
            .and_then(|text| text.parse::<u64>().ok())
            .map(Duration::from_millis)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

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
    overflow_uid: u32,
    check_delay: Option<Duration>,
    peer_uid_file: Option<std::path::PathBuf>,
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
            overflow_uid,
            check_delay: check_delay(),
            peer_uid_file: peer_uid_file(),
        })
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
        let mut answers = sock_diag::lookup(expected)?;
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
/// 足す(足し忘れると Any になり、単純な 3 つの型の 415 だけが残る)。
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
}
