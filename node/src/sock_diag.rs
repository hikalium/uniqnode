//! 接続の相手のソケットの持ち主(uid)を、カーネルの NETLINK_SOCK_DIAG(inet_diag)に 1 件
//! 照会して知る。主の口の uid の判定(node/src/main_door.rs。API_AUTH
//! (uuid:abde9b3c-75f8-453b-988e-bfb1e178c771)の 1)が使う。
//!
//! 照会は dump ではなく、相手のソケットの 4 つ組をそのまま指定する形にする。費用は表の大きさに
//! 依らない(/proc/net/tcp の全表の走査は、網越しに行数を増やされると主の口の全面の DoS に
//! なる)。root は要らない。依存を持たない方針(should/0101)なので、netlink のソケットは
//! extern "C" で libc の socket・sendto・recvfrom・setsockopt を直に呼ぶ。
//!
//! この module は 2 つに分かれる: カーネルへ問う部分(query。Linux だけ)と、答えを読む部分
//! (judge。純関数で、固定の答えを与える試験で固める)。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// TCP の状態のうち ESTABLISHED の番号(include/net/tcp_states.h の TCP_ESTABLISHED)。
pub const TCP_ESTABLISHED: u8 = 1;

/// 答えの 1 件(inet_diag_msg から読んだもの)。local と remote は、見つかったソケット
/// 自身から見た向きである(相手のソケットなら local が相手のアドレス)。
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// 答えが属した族(AF_INET か AF_INET6)。判断には使わず、診断に載せる。
    pub family: u8,
    pub state: u8,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    /// ソケットの cookie(inet_diag_sockid の idiag_cookie)。同じソケットが 2 つの族の照会の
    /// 両方から返るのをまとめる鍵の 1 つ。
    pub cookie: u64,
    /// serve の user namespace へ写した uid。写せない uid は overflowuid になる。
    pub uid: u32,
    pub inode: u32,
}

/// 接続の両端(4 つ組を名前つきで持つ。must/0018)。local と remote は、見たい側の
/// ソケットから見た向きである。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Endpoints {
    pub local: SocketAddr,
    pub remote: SocketAddr,
}

impl Endpoints {
    /// IPv4 射影(::ffff:a.b.c.d)を IPv4 に直し、flowinfo と scope を落とした形。
    /// 答えの 4 つ組と比べるときは、両方をこの形にしてから比べる。
    pub fn normalized(&self) -> Endpoints {
        Endpoints { local: normalize(self.local), remote: normalize(self.remote) }
    }
}

fn normalize(address: SocketAddr) -> SocketAddr {
    let ip = match address.ip() {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    };
    SocketAddr::new(ip, address.port())
}

/// 答えの読み方で許すときの持ち主の uid。断るときは理由の文。
///
/// 規則(API_AUTH の 1):
/// - 見たい 4 つ組(expected。相手のソケットから見た向き)に、正規化して完全に一致する
///   答えだけを見る。
/// - cookie・inode・正規化した 4 つ組が同じ答えは同じソケットとして 1 つにまとめる
///   (カーネルの 1 件の照会は、AF_INET と IPv4 射影の AF_INET6 の両方で同じソケットを
///   返しうる)。まとめた答えの間で uid か状態が違えば断る。
/// - まとめた後の異なるソケットがちょうど 1 つであること(0 も 2 以上も断る)。
/// - その状態が ESTABLISHED で、inode が 0 でないこと(切断の途中で TIME_WAIT の構造に移った
///   ソケットは uid と inode が 0 と表示され、root と取り違える)。
/// - uid が overflowuid でないこと(写せない uid は、本当にその uid のソケットと区別できない)。
/// - uid が許す集合に入ること。
pub fn judge(
    answers: &[Answer],
    expected: Endpoints,
    overflow_uid: u32,
    allowed: &[u32],
) -> Result<u32, String> {
    let expected = expected.normalized();
    let mut sockets: Vec<&Answer> = Vec::new();
    for answer in answers {
        let seen = Endpoints { local: answer.local, remote: answer.remote }.normalized();
        if seen != expected {
            continue;
        }
        let same = sockets.iter().find(|kept| {
            kept.cookie == answer.cookie
                && kept.inode == answer.inode
                && Endpoints { local: kept.local, remote: kept.remote }.normalized() == seen
        });
        match same {
            Some(kept) if kept.uid != answer.uid || kept.state != answer.state => {
                return Err(format!(
                    "同じソケットの答えが食い違う(uid {} と {}、状態 {} と {})",
                    kept.uid, answer.uid, kept.state, answer.state
                ));
            }
            Some(_) => {}
            None => sockets.push(answer),
        }
    }
    let socket = match sockets.as_slice() {
        [] => {
            return Err(format!(
                "相手のソケット(local {} remote {})が照会の答えに無い",
                expected.local, expected.remote
            ))
        }
        [socket] => *socket,
        many => {
            return Err(format!(
                "相手の 4 つ組に一致するソケットが {} 個ある(1 個でなければ断る)",
                many.len()
            ))
        }
    };
    if socket.state != TCP_ESTABLISHED {
        return Err(format!(
            "相手のソケットの状態が ESTABLISHED でない(状態 {}。閉じかけの接続は断る)",
            socket.state
        ));
    }
    if socket.inode == 0 {
        return Err("相手のソケットの inode が 0(持ち主を読めない形)".to_string());
    }
    if socket.uid == overflow_uid {
        return Err(format!(
            "相手の uid が overflowuid {overflow_uid}(この user namespace へ写せない uid)"
        ));
    }
    if !allowed.contains(&socket.uid) {
        return Err(format!(
            "相手の uid {} は許す集合 {} に無い",
            socket.uid,
            uid_list(allowed)
        ));
    }
    Ok(socket.uid)
}

/// uid の集合の書き方(`1000,0`)。起動のログ・断りの文・引数の読み方が同じ字面を使う。
pub fn uid_list(uids: &[u32]) -> String {
    uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

/// 照会の期限(lookup 1 回の全体。2 つの族への照会と、EINTR の再試行を含む)。ループバックの
/// 1 件の照会はカーネルの中で完結するので、ふつう 1 ms もかからない。期限は壊れたときの網である。
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

/// 相手のソケット(expected。相手から見た向き)を照会し、答えを集める。IPv4 の接続は AF_INET と、
/// IPv4 射影にした AF_INET6 の両方に問う(相手が AF_INET6 のソケットから ::ffff:127.0.0.1 で
/// 繋いだなら、ソケットは AF_INET6 の側にある)。IPv6 の接続は AF_INET6 だけに問う。
/// 見つからない(ENOENT)は答え 0 件で、照会そのものの失敗は Err。全体を 1 つの絶対の期限
/// (今から QUERY_TIMEOUT)の内に収める。
///
/// 照会は別のスレッドで行い、呼び手は絶対の期限まで答えを待つ。同期の sendto はカーネルの中
/// (sock_diag の mutex など)で待ちうる、ソケットの期限はそこへ届かないため。期限を過ぎたら
/// Err を返し、遅れて出た答えは捨てる。戻らない照会のスレッドが溜まらないように、走っている
/// 照会の数を判定の枠の数(http::CHECK_SLOTS)までに抑え、越えたら照会せずに Err を返す。
pub fn lookup(expected: Endpoints) -> Result<Vec<Answer>, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let deadline = std::time::Instant::now() + QUERY_TIMEOUT;
    if IN_FLIGHT.fetch_add(1, Ordering::AcqRel) >= crate::http::CHECK_SLOTS {
        drop(Release);
        return Err(format!(
            "走っている sock_diag の照会が上限({})に達している(期限を過ぎても戻らない照会がある)",
            crate::http::CHECK_SLOTS
        ));
    }
    let release = Release;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("sock-diag-query".to_string())
        .spawn(move || {
            let result = lookup_blocking(expected, deadline);
            // 数を戻してから答えを渡す(呼び手が答えを受けて枠を返し、次の接続が照会するときには
            // もう数えられていない。逆順だと枠が満ちているときに偽の上限超えが起きうる)。
            drop(release);
            // 呼び手が期限で去っていれば、送りは失敗して答えは捨てられる。
            let _ = sender.send(result);
        })
        .map_err(|e| format!("sock_diag の照会のスレッドを作れない: {e}"))?;
    receive_before(&receiver, deadline)
}

/// 照会のスレッドの答えを絶対の期限まで待つ。受けが成功した後にも期限を見る: recv_timeout は
/// 期限より先にキューを見るので、呼び手が止まって期限の後に再開したときや、照会のスレッドが
/// 最後の期限の確かめから送りまでに遅れたときに、期限の後の答えを返しうる。それも捨てる。
fn receive_before<T>(
    receiver: &std::sync::mpsc::Receiver<Result<T, String>>,
    deadline: std::time::Instant,
) -> Result<T, String> {
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    let expired = || Err(format!("sock_diag の照会が期限({QUERY_TIMEOUT:?})の内に終わらない"));
    match receiver.recv_timeout(left) {
        Ok(_) if std::time::Instant::now() >= deadline => expired(),
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => expired(),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("sock_diag の照会のスレッドが答えずに終わった".to_string())
        }
    }
}

/// lookup の本体(照会のスレッドで走る)。
fn lookup_blocking(expected: Endpoints, deadline: std::time::Instant) -> Result<Vec<Answer>, String> {
    let expected = expected.normalized();
    let mut answers = Vec::new();
    match (expected.local.ip(), expected.remote.ip()) {
        (IpAddr::V4(local), IpAddr::V4(remote)) => {
            answers.extend(query(
                platform::AF_INET,
                SocketAddr::new(IpAddr::V4(local), expected.local.port()),
                SocketAddr::new(IpAddr::V4(remote), expected.remote.port()),
                deadline,
            )?);
            answers.extend(query(
                platform::AF_INET6,
                SocketAddr::new(IpAddr::V6(local.to_ipv6_mapped()), expected.local.port()),
                SocketAddr::new(IpAddr::V6(remote.to_ipv6_mapped()), expected.remote.port()),
                deadline,
            )?);
        }
        (IpAddr::V6(_), IpAddr::V6(_)) => {
            answers.extend(query(platform::AF_INET6, expected.local, expected.remote, deadline)?);
        }
        _ => {
            return Err(format!(
                "4 つ組の族が揃わない(local {} remote {})",
                expected.local, expected.remote
            ))
        }
    }
    Ok(answers)
}

/// 照会の要求(nlmsghdr と inet_diag_req_v2)を組む。local と remote は探すソケットから見た
/// 向きで、inet_diag_sockid の src/sport と dst/dport に入る。
pub fn request_bytes(family: u8, local: SocketAddr, remote: SocketAddr, sequence: u32) -> Vec<u8> {
    const NLMSG_HEADER_LEN: usize = 16;
    const REQUEST_LEN: usize = 56;
    const SOCK_DIAG_BY_FAMILY: u16 = 20;
    const NLM_F_REQUEST: u16 = 1;
    const IPPROTO_TCP: u8 = 6;
    let mut bytes = Vec::with_capacity(NLMSG_HEADER_LEN + REQUEST_LEN);
    bytes.extend_from_slice(&((NLMSG_HEADER_LEN + REQUEST_LEN) as u32).to_ne_bytes());
    bytes.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    bytes.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
    bytes.extend_from_slice(&sequence.to_ne_bytes());
    bytes.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid: 宛先はカーネル
    // inet_diag_req_v2: family, protocol, ext(拡張の属性は要らない), pad, states(全部)
    bytes.extend_from_slice(&[family, IPPROTO_TCP, 0, 0]);
    bytes.extend_from_slice(&u32::MAX.to_ne_bytes());
    // inet_diag_sockid: sport, dport(網の順), src[4], dst[4], if, cookie[2](INET_DIAG_NOCOOKIE)
    bytes.extend_from_slice(&local.port().to_be_bytes());
    bytes.extend_from_slice(&remote.port().to_be_bytes());
    bytes.extend_from_slice(&address_field(local.ip()));
    bytes.extend_from_slice(&address_field(remote.ip()));
    bytes.extend_from_slice(&0u32.to_ne_bytes());
    bytes.extend_from_slice(&u32::MAX.to_ne_bytes());
    bytes.extend_from_slice(&u32::MAX.to_ne_bytes());
    bytes
}

/// inet_diag_sockid のアドレスの欄(16 バイト)。IPv4 は先頭の 4 バイトに網の順で入る。
fn address_field(ip: IpAddr) -> [u8; 16] {
    let mut field = [0u8; 16];
    match ip {
        IpAddr::V4(v4) => field[..4].copy_from_slice(&v4.octets()),
        IpAddr::V6(v6) => field.copy_from_slice(&v6.octets()),
    }
    field
}

/// 受け取った netlink の 1 データグラムを読む。1 件の照会(dump でない)の答えの形だけを通す:
/// sequence の合う通知がちょうど 1 つで、それが NLM_F_MULTI の付かない SOCK_DIAG_BY_FAMILY
/// (答えの inet_diag_msg)か NLMSG_ERROR(nlmsgerr の全長を持ち、ENOENT なら答え 0 件、
/// 誤りの値 0 の ACK と他の誤りは Err)であること。NLMSG_DONE は誤りの値が 0 のときだけ読み飛ばす(1 件の照会では来ないはずで、
/// 来ても答えを足さない)。sequence の違う通知は読み飛ばす。NLM_F_MULTI の付いた通知、
/// 誤りの値が 0 でない NLMSG_DONE、sequence の合う通知が 2 つ以上、末尾の端のバイト
/// (整列の詰め物を除く)は Err にする。
pub fn parse_reply(bytes: &[u8], sequence: u32) -> Result<Vec<Answer>, String> {
    const HEADER: usize = 16;
    const NLMSG_ERROR: u16 = 2;
    const NLMSG_DONE: u16 = 3;
    const SOCK_DIAG_BY_FAMILY: u16 = 20;
    const NLM_F_MULTI: u16 = 2;
    const ENOENT: i32 = 2;
    const NLMSGERR_LEN: usize = 4 + HEADER;
    let word = |payload: &[u8], what: &str| -> Result<i32, String> {
        payload
            .get(..4)
            .map(|b| i32::from_ne_bytes(b.try_into().expect("4")))
            .ok_or_else(|| format!("netlink の {what} が短い"))
    };
    let mut answers = Vec::new();
    let mut replies = 0usize;
    let mut offset = 0usize;
    while offset < bytes.len() {
        if offset + HEADER > bytes.len() {
            return Err(format!(
                "netlink の答えの末尾に半端なバイトが {} ある",
                bytes.len() - offset
            ));
        }
        let length = u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("4")) as usize;
        let kind = u16::from_ne_bytes(bytes[offset + 4..offset + 6].try_into().expect("2"));
        let flags = u16::from_ne_bytes(bytes[offset + 6..offset + 8].try_into().expect("2"));
        let seen_sequence =
            u32::from_ne_bytes(bytes[offset + 8..offset + 12].try_into().expect("4"));
        if length < HEADER || length > bytes.len() - offset {
            return Err(format!("netlink の答えの長さが不正({length})"));
        }
        let payload = &bytes[offset + HEADER..offset + length];
        if seen_sequence == sequence {
            if flags & NLM_F_MULTI != 0 {
                return Err(format!(
                    "netlink の答えに NLM_F_MULTI が付いている(種類 {kind}。1 件の照会の答えではない)"
                ));
            }
            match kind {
                NLMSG_DONE => match word(payload, "終わりの通知")? {
                    0 => {}
                    code => return Err(format!("netlink の終わりの通知が誤りの値 {code} を持つ")),
                },
                NLMSG_ERROR | SOCK_DIAG_BY_FAMILY => {
                    replies += 1;
                    if replies > 1 {
                        return Err("netlink の答えが 2 つ以上ある(1 件の照会の答えではない)".to_string());
                    }
                    if kind == SOCK_DIAG_BY_FAMILY {
                        answers.push(parse_message(payload)?);
                    } else {
                        // struct nlmsgerr は誤りの値(int)と、元の要求の nlmsghdr(16 バイト)を
                        // 持つ。それに満たない誤りの答えは断る。
                        if payload.len() < NLMSGERR_LEN {
                            return Err(format!(
                                "netlink の誤りの答えが短い({} バイト。nlmsgerr は {NLMSGERR_LEN})",
                                payload.len()
                            ));
                        }
                        let code = word(payload, "誤りの答え")?;
                        // i32::MIN の符号は返せない(checked_neg が None)。
                        let errno = code
                            .checked_neg()
                            .ok_or_else(|| format!("netlink の誤りの値が不正({code})"))?;
                        match errno {
                            // 見つからない、だけが答え 0 件である。
                            ENOENT => {}
                            // 誤りの値 0 は ACK で、NLM_F_ACK を頼んでいない 1 件の照会には来ない。
                            0 => {
                                return Err(
                                    "netlink の答えが想定外の ACK(誤りの値 0)である".to_string()
                                )
                            }
                            errno => {
                                return Err(format!(
                                    "sock_diag の照会が誤りを返した: {}",
                                    std::io::Error::from_raw_os_error(errno)
                                ))
                            }
                        }
                    }
                }
                other => return Err(format!("netlink の答えの種類が想定外({other})")),
            }
        }
        // NLMSG_ALIGN(4 バイト境界)。最後の通知の後の詰め物はデータグラムの外へはみ出してよい。
        offset = offset.saturating_add((length + 3) & !3);
    }
    if replies == 0 {
        return Err("netlink の答えに照会への返事が無い".to_string());
    }
    Ok(answers)
}

/// inet_diag_msg(72 バイト。後ろの属性は読まない)を読む。
fn parse_message(payload: &[u8]) -> Result<Answer, String> {
    if payload.len() < 72 {
        return Err(format!("inet_diag_msg が短い({} バイト)", payload.len()));
    }
    let family = payload[0];
    let state = payload[1];
    let id = &payload[4..52];
    let source_port = u16::from_be_bytes([id[0], id[1]]);
    let destination_port = u16::from_be_bytes([id[2], id[3]]);
    let ip = |field: &[u8]| -> Result<IpAddr, String> {
        match family {
            platform::AF_INET => {
                Ok(IpAddr::V4(Ipv4Addr::new(field[0], field[1], field[2], field[3])))
            }
            platform::AF_INET6 => {
                let octets: [u8; 16] = field.try_into().expect("16");
                Ok(IpAddr::V6(Ipv6Addr::from(octets)))
            }
            other => Err(format!("inet_diag_msg の族が想定外({other})")),
        }
    };
    let local = SocketAddr::new(ip(&id[4..20])?, source_port);
    let remote = SocketAddr::new(ip(&id[20..36])?, destination_port);
    let cookie_low = u32::from_ne_bytes(id[40..44].try_into().expect("4")) as u64;
    let cookie_high = u32::from_ne_bytes(id[44..48].try_into().expect("4")) as u64;
    let word = |at: usize| u32::from_ne_bytes(payload[at..at + 4].try_into().expect("4"));
    Ok(Answer {
        family,
        state,
        local,
        remote,
        cookie: cookie_low | (cookie_high << 32),
        uid: word(64),
        inode: word(68),
    })
}

/// 照会ソケット(AF_NETLINK)を作れないときの文。serve の起動時の自己試験
/// (node/src/main_door.rs の MainDoor::self_test)と接続ごとの判定が同じ文を使う。
pub fn socket_unavailable(error: &std::io::Error) -> String {
    format!(
        "sock_diag の照会ソケット(AF_NETLINK)を作れない: {error}(systemd の unit なら \
         RestrictAddressFamilies= に AF_NETLINK が要る)"
    )
}

/// 1 つの族に 1 件照会する(deadline は lookup 全体の絶対の期限)。
fn query(
    family: u8,
    local: SocketAddr,
    remote: SocketAddr,
    deadline: std::time::Instant,
) -> Result<Vec<Answer>, String> {
    platform::exchange(family, local, remote, deadline)
}

/// 照会ソケットを 1 つ作って閉じる(照会はしない)。serve の起動時の自己試験が、作れない理由
/// (errno)で設定の誤りか一時の失敗かを分けるために使う。
pub fn probe_socket() -> std::io::Result<()> {
    platform::open_socket().map(drop)
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{parse_reply, request_bytes, Answer, QUERY_TIMEOUT};
    use std::net::SocketAddr;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::raw::{c_int, c_long, c_void};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    pub const AF_INET: u8 = 2;
    pub const AF_INET6: u8 = 10;
    const AF_NETLINK: c_int = 16;
    const SOCK_DGRAM: c_int = 2;
    const SOCK_CLOEXEC: c_int = 0o2000000;
    const NETLINK_SOCK_DIAG: c_int = 4;
    const SOL_SOCKET: c_int = 1;
    const SO_RCVTIMEO: c_int = 20;
    const MSG_TRUNC: c_int = 0x20;

    extern "C" {
        fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
        fn sendto(
            fd: c_int,
            buffer: *const c_void,
            length: usize,
            flags: c_int,
            address: *const c_void,
            address_length: u32,
        ) -> isize;
        fn recvfrom(
            fd: c_int,
            buffer: *mut c_void,
            length: usize,
            flags: c_int,
            address: *mut c_void,
            address_length: *mut u32,
        ) -> isize;
        fn setsockopt(
            fd: c_int,
            level: c_int,
            name: c_int,
            value: *const c_void,
            length: u32,
        ) -> c_int;
    }

    /// struct timeval。
    #[repr(C)]
    struct Timeval {
        seconds: c_long,
        microseconds: c_long,
    }

    /// struct sockaddr_nl。宛先のカーネルは pid 0。
    #[repr(C)]
    struct SockaddrNetlink {
        family: u16,
        pad: u16,
        pid: u32,
        groups: u32,
    }

    static SEQUENCE: AtomicU32 = AtomicU32::new(1);

    /// 照会ソケットを作る。
    pub fn open_socket() -> std::io::Result<OwnedFd> {
        // SAFETY: 引数は定数で、返り値の fd は下で検めてから OwnedFd に渡す(閉じるのは OwnedFd)。
        let raw = unsafe { socket(AF_NETLINK, SOCK_DGRAM | SOCK_CLOEXEC, NETLINK_SOCK_DIAG) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: raw は socket が返した開いた fd で、他の誰も持っていない。
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    /// 期限までの残り。過ぎていれば Err。
    fn remaining(deadline: Instant, what: &str) -> Result<Duration, String> {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| format!("sock_diag の照会が期限({QUERY_TIMEOUT:?})の内に終わらない({what})"))
    }

    /// 受けの期限(SO_RCVTIMEO)を残りの時間に置く。0 は「待ち続ける」になるので 1 µs に丸める。
    fn set_receive_timeout(fd: &OwnedFd, left: Duration) -> Result<(), String> {
        let timeout = Timeval {
            seconds: left.as_secs() as c_long,
            microseconds: (left.subsec_micros() as c_long).max(if left.as_secs() == 0 { 1 } else { 0 }),
        };
        // SAFETY: timeout は生きている構造体で、長さはその大きさ。
        let set = unsafe {
            setsockopt(
                fd.as_raw_fd(),
                SOL_SOCKET,
                SO_RCVTIMEO,
                &timeout as *const Timeval as *const c_void,
                std::mem::size_of::<Timeval>() as u32,
            )
        };
        match set {
            0 => Ok(()),
            _ => Err(format!("照会ソケットの期限を置けない: {}", std::io::Error::last_os_error())),
        }
    }

    pub fn exchange(
        family: u8,
        local: SocketAddr,
        remote: SocketAddr,
        deadline: Instant,
    ) -> Result<Vec<Answer>, String> {
        let fd = open_socket().map_err(|error| super::socket_unavailable(&error))?;
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let request = request_bytes(family, local, remote, sequence);
        let kernel = SockaddrNetlink { family: AF_NETLINK as u16, pad: 0, pid: 0, groups: 0 };
        // 送りの EINTR は、期限の残りがある間だけ送り直す。
        loop {
            remaining(deadline, "送り")?;
            // SAFETY: request と kernel は呼び出しの間生きていて、長さはそれぞれの大きさ。
            let sent = unsafe {
                sendto(
                    fd.as_raw_fd(),
                    request.as_ptr() as *const c_void,
                    request.len(),
                    0,
                    &kernel as *const SockaddrNetlink as *const c_void,
                    std::mem::size_of::<SockaddrNetlink>() as u32,
                )
            };
            if sent < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("sock_diag の照会を送れない: {error}"));
            }
            if sent as usize != request.len() {
                return Err(format!("sock_diag の照会を送り切れない({sent} バイト)"));
            }
            break;
        }
        // 1 件の照会の答えは 1 データグラムに収まる(inet_diag_msg と少しの属性)。recvfrom に
        // MSG_TRUNC を渡すと、netlink は切り詰める前の長さを返す(buffer より長ければ切れた答え
        // なので断る)。送り手の sockaddr_nl も受け、カーネル(nl_pid 0)からでなければ断る。
        let mut buffer = vec![0u8; 8192];
        let (received, sender) = loop {
            // 受けの EINTR も、期限の残りを受けの期限に置き直してから受け直す。
            set_receive_timeout(&fd, remaining(deadline, "受け")?)?;
            let mut sender = SockaddrNetlink { family: 0, pad: 0, pid: u32::MAX, groups: 0 };
            let mut sender_length = std::mem::size_of::<SockaddrNetlink>() as u32;
            // SAFETY: buffer は書ける長さ buffer.len() の領域、sender と sender_length は呼び出しの
            // 間生きていて、sender_length は sender の大きさを言う。
            let received = unsafe {
                recvfrom(
                    fd.as_raw_fd(),
                    buffer.as_mut_ptr() as *mut c_void,
                    buffer.len(),
                    MSG_TRUNC,
                    &mut sender as *mut SockaddrNetlink as *mut c_void,
                    &mut sender_length,
                )
            };
            if received < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("sock_diag の答えを受けられない: {error}"));
            }
            if (sender_length as usize) < std::mem::size_of::<SockaddrNetlink>() {
                return Err(format!("sock_diag の答えの送り手の長さが不正({sender_length})"));
            }
            break (received as usize, sender);
        };
        // 受けが成功しても、期限を過ぎていれば答えを使わない。
        remaining(deadline, "受けの後")?;
        if sender.family != AF_NETLINK as u16 || sender.pid != 0 {
            return Err(format!(
                "sock_diag の答えの送り手がカーネルでない(族 {}、nl_pid {})",
                sender.family, sender.pid
            ));
        }
        if received > buffer.len() {
            return Err(format!(
                "sock_diag の答えが受け口({} バイト)より長い({received} バイト。切れた答えは読まない)",
                buffer.len()
            ));
        }
        parse_reply(&buffer[..received], sequence)
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::Answer;
    use std::net::SocketAddr;

    pub const AF_INET: u8 = 2;
    pub const AF_INET6: u8 = 10;

    pub fn exchange(
        _family: u8,
        _local: SocketAddr,
        _remote: SocketAddr,
        _deadline: std::time::Instant,
    ) -> Result<Vec<Answer>, String> {
        Err("sock_diag は Linux にしかない".to_string())
    }

    pub fn open_socket() -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

/// この OS で照会できるか(serve は起動時に問い、できなければ理由を言って断る)。
pub const SUPPORTED: bool = cfg!(target_os = "linux");

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> SocketAddr {
        text.parse().expect("socket address")
    }

    /// 期限の前に届いて待っていた答えでも、受け取りが期限の後なら捨てる(呼び手が止まって
    /// 期限の後に再開した場合)。期限の前の受け取りは答えを返す。
    #[test]
    fn an_answer_taken_after_the_deadline_is_discarded() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let past = std::time::Instant::now();
        sender.send(Ok(1000u32)).expect("send");
        std::thread::sleep(Duration::from_millis(5));
        let late = receive_before(&receiver, past);
        assert!(late.is_err(), "期限の後の答えを採った: {late:?}");

        sender.send(Ok(1000u32)).expect("send");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        assert_eq!(receive_before(&receiver, deadline), Ok(1000));
    }

    /// 相手のソケット(127.0.0.1:40000 から 127.0.0.1:7440 へ)から見た 4 つ組。
    fn expected() -> Endpoints {
        Endpoints { local: at("127.0.0.1:40000"), remote: at("127.0.0.1:7440") }
    }

    fn established(uid: u32) -> Answer {
        Answer {
            family: platform::AF_INET,
            state: TCP_ESTABLISHED,
            local: at("127.0.0.1:40000"),
            remote: at("127.0.0.1:7440"),
            cookie: 77,
            uid,
            inode: 12345,
        }
    }

    const OVERFLOW: u32 = 65534;

    #[test]
    fn an_exact_established_answer_from_an_allowed_uid_passes() {
        assert_eq!(judge(&[established(1000)], expected(), OVERFLOW, &[1000]), Ok(1000));
    }

    #[test]
    fn an_uid_outside_the_set_is_refused() {
        let error = judge(&[established(1001)], expected(), OVERFLOW, &[1000]).expect_err("断る");
        assert!(error.contains("1001"), "{error}");
    }

    /// 4 つ組が一つでも違う答えは見ない(見た結果 0 件で断る)。
    #[test]
    fn an_answer_with_a_different_four_tuple_is_not_counted() {
        for (local, remote) in [
            ("127.0.0.1:40001", "127.0.0.1:7440"),
            ("127.0.0.1:40000", "127.0.0.1:7441"),
            ("127.0.0.2:40000", "127.0.0.1:7440"),
            ("127.0.0.1:40000", "127.0.0.2:7440"),
            // 向きが逆(serve 側の受けたソケット)
            ("127.0.0.1:7440", "127.0.0.1:40000"),
        ] {
            let mut answer = established(1000);
            answer.local = at(local);
            answer.remote = at(remote);
            let error = judge(&[answer], expected(), OVERFLOW, &[1000]).expect_err("断る");
            assert!(error.contains("答えに無い"), "{local} {remote}: {error}");
        }
    }

    /// ESTABLISHED 以外(FIN_WAIT2 は 5、TIME_WAIT は 6、CLOSE_WAIT は 8)は断る。
    #[test]
    fn a_state_other_than_established_is_refused() {
        for state in [2u8, 4, 5, 6, 7, 8, 10] {
            let mut answer = established(1000);
            answer.state = state;
            let error = judge(&[answer], expected(), OVERFLOW, &[1000]).expect_err("断る");
            assert!(error.contains("ESTABLISHED"), "{state}: {error}");
        }
    }

    /// 切断の途中で TIME_WAIT の構造に移ったソケットの形(FIN_WAIT2・TIME_WAIT、uid 0、
    /// inode 0)は、root を許していても root と取り違えない。
    #[test]
    fn a_timewait_shaped_answer_is_not_mistaken_for_root() {
        for state in [5u8, 6] {
            let mut answer = established(0);
            answer.state = state;
            answer.inode = 0;
            assert!(judge(&[answer], expected(), OVERFLOW, &[0, 1000]).is_err(), "{state}");
        }
        // 状態が ESTABLISHED と読めても inode 0 は断る。
        let mut answer = established(0);
        answer.inode = 0;
        let error = judge(&[answer], expected(), OVERFLOW, &[0]).expect_err("断る");
        assert!(error.contains("inode"), "{error}");
    }

    /// 異なるソケットの 2 件は断る。
    #[test]
    fn two_distinct_sockets_are_refused() {
        let mut other = established(1000);
        other.cookie = 78;
        other.inode = 12346;
        let error =
            judge(&[established(1000), other], expected(), OVERFLOW, &[1000]).expect_err("断る");
        assert!(error.contains("2 個"), "{error}");
    }

    /// 同じソケットが AF_INET と、IPv4 射影の AF_INET6 の両方から返る答えは 1 つにまとめて通す。
    #[test]
    fn the_same_socket_from_both_families_counts_once() {
        let mut mapped = established(1000);
        mapped.family = platform::AF_INET6;
        mapped.local = at("[::ffff:127.0.0.1]:40000");
        mapped.remote = at("[::ffff:127.0.0.1]:7440");
        assert_eq!(judge(&[established(1000), mapped.clone()], expected(), OVERFLOW, &[1000]), Ok(1000));
        // まとめた答えの間で uid が違えば断る。
        let mut disagreeing = mapped;
        disagreeing.uid = 1001;
        assert!(judge(&[established(1000), disagreeing], expected(), OVERFLOW, &[1000, 1001]).is_err());
    }

    /// AF_INET6 のソケットから ::ffff:127.0.0.1 で繋いだ相手は、答えが AF_INET6 の射影の形で
    /// だけ返る。IPv4 に直して比べるので通る。
    #[test]
    fn an_ipv4_mapped_answer_matches_an_ipv4_connection() {
        let mut mapped = established(1000);
        mapped.family = platform::AF_INET6;
        mapped.local = at("[::ffff:127.0.0.1]:40000");
        mapped.remote = at("[::ffff:127.0.0.1]:7440");
        assert_eq!(judge(&[mapped], expected(), OVERFLOW, &[1000]), Ok(1000));
    }

    /// 写せない uid(overflowuid)は、許す集合に入っていても断る。
    #[test]
    fn the_overflow_uid_is_refused() {
        let error =
            judge(&[established(OVERFLOW)], expected(), OVERFLOW, &[OVERFLOW]).expect_err("断る");
        assert!(error.contains("overflowuid"), "{error}");
    }

    #[test]
    fn no_answer_is_refused() {
        assert!(judge(&[], expected(), OVERFLOW, &[1000]).is_err());
    }

    /// 要求の並び(期待値はリテラル。should/0137)。
    #[test]
    fn the_request_names_the_socket_in_network_order() {
        let bytes = request_bytes(platform::AF_INET, at("127.0.0.1:40000"), at("127.0.0.2:7440"), 9);
        assert_eq!(bytes.len(), 72);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 72);
        assert_eq!(u16::from_ne_bytes(bytes[4..6].try_into().unwrap()), 20);
        assert_eq!(u32::from_ne_bytes(bytes[8..12].try_into().unwrap()), 9);
        assert_eq!(&bytes[16..20], &[2, 6, 0, 0]);
        assert_eq!(&bytes[24..28], &[0x9c, 0x40, 0x1d, 0x10]); // 40000, 7440
        assert_eq!(&bytes[28..32], &[127, 0, 0, 1]);
        assert_eq!(&bytes[44..48], &[127, 0, 0, 2]);
        assert_eq!(&bytes[64..72], &[0xff; 8]);
    }

    /// 答えの読み方: inet_diag_msg を 1 つ持つデータグラムと、ENOENT の誤り。
    #[test]
    fn a_reply_is_read_into_answers() {
        let mut message = vec![0u8; 72];
        message[0] = platform::AF_INET;
        message[1] = TCP_ESTABLISHED;
        message[4..6].copy_from_slice(&40000u16.to_be_bytes());
        message[6..8].copy_from_slice(&7440u16.to_be_bytes());
        message[8..12].copy_from_slice(&[127, 0, 0, 1]);
        message[24..28].copy_from_slice(&[127, 0, 0, 1]);
        message[44..48].copy_from_slice(&77u32.to_ne_bytes());
        message[64..68].copy_from_slice(&1000u32.to_ne_bytes());
        message[68..72].copy_from_slice(&12345u32.to_ne_bytes());
        let mut reply = Vec::new();
        reply.extend_from_slice(&(16u32 + 72).to_ne_bytes());
        reply.extend_from_slice(&20u16.to_ne_bytes());
        reply.extend_from_slice(&0u16.to_ne_bytes());
        reply.extend_from_slice(&5u32.to_ne_bytes());
        reply.extend_from_slice(&0u32.to_ne_bytes());
        reply.extend_from_slice(&message);
        assert_eq!(parse_reply(&reply, 5), Ok(vec![established(1000)]));

        let mut error = Vec::new();
        error.extend_from_slice(&(16u32 + 20).to_ne_bytes());
        error.extend_from_slice(&2u16.to_ne_bytes());
        error.extend_from_slice(&0u16.to_ne_bytes());
        error.extend_from_slice(&6u32.to_ne_bytes());
        error.extend_from_slice(&0u32.to_ne_bytes());
        error.extend_from_slice(&(-2i32).to_ne_bytes());
        error.extend_from_slice(&[0u8; 16]);
        assert_eq!(parse_reply(&error, 6), Ok(Vec::new()), "ENOENT は答え 0 件");
        error[16..20].copy_from_slice(&(-13i32).to_ne_bytes());
        assert!(parse_reply(&error, 6).is_err(), "他の誤りは照会の失敗");
    }

    /// netlink の通知 1 つ(頭と中身)。
    fn netlink_message(kind: u16, flags: u16, sequence: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(16 + payload.len() as u32).to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&flags.to_ne_bytes());
        bytes.extend_from_slice(&sequence.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn diag_payload() -> Vec<u8> {
        let mut message = vec![0u8; 72];
        message[0] = platform::AF_INET;
        message[1] = TCP_ESTABLISHED;
        message[4..6].copy_from_slice(&40000u16.to_be_bytes());
        message[6..8].copy_from_slice(&7440u16.to_be_bytes());
        message[8..12].copy_from_slice(&[127, 0, 0, 1]);
        message[24..28].copy_from_slice(&[127, 0, 0, 1]);
        message[44..48].copy_from_slice(&77u32.to_ne_bytes());
        message[64..68].copy_from_slice(&1000u32.to_ne_bytes());
        message[68..72].copy_from_slice(&12345u32.to_ne_bytes());
        message
    }

    fn error_payload(code: i32) -> Vec<u8> {
        let mut payload = code.to_ne_bytes().to_vec();
        payload.extend_from_slice(&[0u8; 16]);
        payload
    }

    /// 1 件の照会の答えの形でないもの(Codex 低 3)は断る。
    #[test]
    fn a_reply_that_is_not_the_single_reply_form_is_refused() {
        let answer = netlink_message(20, 0, 5, &diag_payload());
        // NLM_F_MULTI の付いた答え(dump の形)。
        let multi = netlink_message(20, 2, 5, &diag_payload());
        let error = parse_reply(&multi, 5).expect_err("断る");
        assert!(error.contains("NLM_F_MULTI"), "{error}");
        // 誤りの値が 0 でない NLMSG_DONE。0 なら読み飛ばす。
        let mut done_error = answer.clone();
        done_error.extend_from_slice(&netlink_message(3, 0, 5, &(-13i32).to_ne_bytes()));
        assert!(parse_reply(&done_error, 5).expect_err("断る").contains("終わりの通知"));
        let mut done_ok = answer.clone();
        done_ok.extend_from_slice(&netlink_message(3, 0, 5, &0i32.to_ne_bytes()));
        assert_eq!(parse_reply(&done_ok, 5), Ok(vec![established(1000)]));
        // 末尾の端のバイト(頭に満たない)と、長さの合わない通知。
        let mut trailing = answer.clone();
        trailing.extend_from_slice(&[1, 2, 3, 4, 5]);
        assert!(parse_reply(&trailing, 5).expect_err("断る").contains("半端"));
        let mut overlong = answer.clone();
        overlong.extend_from_slice(&netlink_message(20, 0, 5, &diag_payload())[..40]);
        assert!(parse_reply(&overlong, 5).expect_err("断る").contains("長さ"));
        // 返事が 2 つ(答えと答え、答えと誤り)。
        let mut two = answer.clone();
        two.extend_from_slice(&answer);
        assert!(parse_reply(&two, 5).expect_err("断る").contains("2 つ以上"));
        let mut answer_and_error = answer.clone();
        answer_and_error.extend_from_slice(&netlink_message(2, 0, 5, &error_payload(-2)));
        assert!(parse_reply(&answer_and_error, 5).is_err());
        // 返事の無いデータグラム(空と、sequence の違う通知だけ)。違う通知は読み飛ばす。
        assert!(parse_reply(&[], 5).is_err());
        let other = netlink_message(20, 0, 4, &diag_payload());
        assert!(parse_reply(&other, 5).is_err());
        let mut other_then_ours = other.clone();
        other_then_ours.extend_from_slice(&answer);
        assert_eq!(parse_reply(&other_then_ours, 5), Ok(vec![established(1000)]));
    }

    /// NLMSG_ERROR は nlmsgerr の全長(誤りの値と元の要求の頭)を持つときだけ読み、答え 0 件に
    /// するのは ENOENT だけで、誤りの値 0 の ACK は断る(Codex 低 5)。
    #[test]
    fn an_error_reply_needs_the_full_nlmsgerr_and_an_ack_is_refused() {
        let enoent = netlink_message(2, 0, 7, &error_payload(-2));
        assert_eq!(parse_reply(&enoent, 7), Ok(Vec::new()));
        let short = netlink_message(2, 0, 7, &(-2i32).to_ne_bytes());
        assert!(parse_reply(&short, 7).expect_err("断る").contains("短い"));
        let ack = netlink_message(2, 0, 7, &error_payload(0));
        assert!(parse_reply(&ack, 7).expect_err("断る").contains("ACK"));
    }

    /// 誤りの値が i32::MIN でも符号の反転で溢れず、Err を返す(Claude 低 2)。
    #[test]
    fn an_error_code_of_i32_min_is_refused_without_overflow() {
        let reply = netlink_message(2, 0, 6, &error_payload(i32::MIN));
        let error = parse_reply(&reply, 6).expect_err("断る");
        assert!(error.contains("不正"), "{error}");
    }
}
