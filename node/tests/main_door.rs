//! 主の口の信頼の境界(docs/plan/API_AUTH.md の 1。node/src/main_door.rs)の試験。実プロセスの
//! serve を起こし、curl が組み立てる形の生 HTTP/1.1 で当てる(should/0138)。
//!
//! 別の uid からの接続は root 無しには作れないので、「許す集合から自分の uid を外した serve」へ
//! 実際に繋いで断らせる。写せない uid(overflowuid)の接続は、serve を uid を写さない user
//! namespace(`unshare --user`)で起こして作る。どの試験もポートは :0 の自動割当で、本番の口には
//! 触れない。

mod common;
use common::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// serve を address と引数と環境変数を与えて起こし、`listening on` の行から実際の束縛先を読む。
fn start(name: &str, address: &str, args: &[&str], envs: &[(&str, &str)]) -> Server {
    start_with(&[], name, address, args, envs)
}

/// start と同じだが、serve の前に命令を置く(`unshare --user` の下で起こす用)。
fn start_with(prefix: &[&str], name: &str, address: &str, args: &[&str], envs: &[(&str, &str)]) -> Server {
    let dir = unique_dir(name);
    let binary = env!("CARGO_BIN_EXE_uniqnode");
    let mut command = match prefix.split_first() {
        Some((program, rest)) => {
            let mut command = Command::new(program);
            command.args(rest).arg(binary);
            command
        }
        None => Command::new(binary),
    };
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .args(["serve", dir.to_str().expect("utf-8"), address, "--no-log"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn serve");
    let stdout = child.stdout.take().expect("stdout");
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).expect("read listening line");
    let address = line
        .trim()
        .strip_prefix("listening on ")
        .unwrap_or_else(|| panic!("listening on が出ない: {line:?}"))
        .to_string();
    Server { child, address, dir, remove_dir_on_drop: true, stderr: None }
}

/// serve を起こして、束縛せずに終わるのを待つ(断りの試験)。
fn refused_serve(address: &str, args: &[&str]) -> std::process::Output {
    refused_serve_with(&[], address, args)
}

fn refused_serve_with(prefix: &[&str], address: &str, args: &[&str]) -> std::process::Output {
    let dir = unique_dir("refused");
    let binary = env!("CARGO_BIN_EXE_uniqnode");
    let mut command = match prefix.split_first() {
        Some((program, rest)) => {
            let mut command = Command::new(program);
            command.args(rest).arg(binary);
            command
        }
        None => Command::new(binary),
    };
    let output = command
        .args(["serve", dir.to_str().expect("utf-8"), address, "--no-log"])
        .args(args)
        .output()
        .expect("spawn serve");
    // 起動時の検査はすべてストアを開く前に済む(断った serve はデータディレクトリを作らない)。
    let opened = dir.exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!opened, "断った serve がストアを開いた: {}", String::from_utf8_lossy(&output.stderr));
    output
}

fn own_uid() -> u32 {
    uniqnode::install::effective_uid().expect("euid")
}

/// 生の要求を 1 本送って応答を読む。
fn send(address: &str, head: &str, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(address).expect("connect");
    stream.write_all(head.as_bytes()).expect("head");
    stream.write_all(body).expect("body");
    read_response(&mut BufReader::new(stream))
}

/// 相手が何も書かずに閉じたか(読みが 0 バイトの EOF か、接続のリセット)を、期限の内に見る。
fn closed_without_response(stream: &mut TcpStream, within: Duration) -> Result<(), String> {
    stream.set_read_timeout(Some(within)).expect("timeout");
    let mut buffer = [0u8; 64];
    match stream.read(&mut buffer) {
        Ok(0) => Ok(()),
        Ok(n) => Err(format!("応答が書かれた: {:?}", String::from_utf8_lossy(&buffer[..n]))),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => Ok(()),
        Err(e) => Err(format!("期限の内に閉じられなかった: {e}")),
    }
}

/// /v1/status の main_door の欄を読む。
struct Gauge {
    checking: i64,
    checking_peak: i64,
    connections: i64,
    connections_peak: i64,
    refusing: i64,
    refusing_peak: i64,
}

fn gauge(address: &str) -> Gauge {
    let status = simple(address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200, "{}", body_text(&status));
    let text = body_text(&status);
    let field = |key: &str| json_integer_field(&text, key).unwrap_or_else(|| panic!("{key} が無い: {text}"));
    Gauge {
        checking: field("checking"),
        checking_peak: field("checking_peak"),
        connections: field("connections"),
        connections_peak: field("connections_peak"),
        refusing: field("refusing"),
        refusing_peak: field("refusing_peak"),
    }
}

/// 自分以外の接続が枠を返し終えるまで待つ(自分の status の 1 本だけが接続中になる)。条件待ち
/// (should/0104)で、期限は安全網。
fn wait_until_idle(address: &str) -> Gauge {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = gauge(address);
        if seen.checking == 0 && seen.connections == 1 && seen.refusing == 0 {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "枠が戻らない: checking {} connections {} refusing {}",
            seen.checking,
            seen.connections,
            seen.refusing
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---- A1: 束縛の検査 ----

/// ループバックの IP リテラルでない束縛先は、束縛せずに理由を言って 2 で終わる。
/// should/0137: main_door::check_listen の is_loopback の判定を外すと 0.0.0.0 と 10.10.128.1 が、
/// 字面の parse を ToSocketAddrs に戻すと localhost が赤になる。
#[test]
fn serve_refuses_a_main_door_that_is_not_a_loopback_ip_literal() {
    for address in ["0.0.0.0:0", "[::]:0", "[::ffff:127.0.0.1]:0", "10.10.128.1:0", "localhost:0"] {
        let output = refused_serve(address, &[]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{address}: {stderr}");
        assert!(!stdout.contains("listening on"), "{address}: {stdout}");
        assert!(stderr.contains(address) && stderr.contains("ループバック"), "{address}: {stderr}");
    }
}

/// 束縛できない主の口(塞がれたポート)は、ストアを開く前に理由を言って 1 で終わる。
/// should/0137: main.rs の束縛を Store::open の後へ戻すと、refused_serve_with のデータ
/// ディレクトリの検査で赤になる。
#[test]
fn a_main_door_that_cannot_bind_exits_before_opening_the_store() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = held.local_addr().expect("addr").to_string();
    let output = refused_serve(&address, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(!stdout.contains("listening on"), "{stdout}");
    assert!(stderr.contains("束縛できない"), "{stderr}");
}

/// 127.0.0.0/8 の別のアドレスと [::1] は通り、自分の uid の本物の接続が両方に通る(照合が
/// 壊れて答えが 0 件になると断る側の試験だけでは見えない。Claude 低 2)。
#[test]
fn loopback_literals_bind_and_the_owner_gets_through_on_ipv4_and_ipv6() {
    for (name, address) in [("v4", "127.0.0.1:0"), ("v4b", "127.0.0.2:0"), ("v6", "[::1]:0")] {
        let server = start(&format!("bind-{name}"), address, &[], &[]);
        let status = simple(&server.address, "GET", "/v1/status", b"");
        assert_eq!(status.status, 200, "{address}: {}", body_text(&status));
        assert!(body_text(&status).contains("node_id"), "{address}");
    }
}

/// AF_INET6 のソケットから [::ffff:127.0.0.1] で IPv4 の主の口へ繋ぐ接続が通る(答えは射影の形で
/// 返り、IPv4 に直して比べる)。Host は束縛の字面で送る(射影の字面は既定の一覧に無い)。
#[test]
fn an_ipv4_mapped_connection_from_an_ipv6_socket_gets_through() {
    let server = start("mapped", "127.0.0.1:0", &[], &[]);
    let port = server.address.rsplit_once(':').expect("port").1;
    let mapped = format!("[::ffff:127.0.0.1]:{port}");
    let response = send(
        &mapped,
        &format!("GET /v1/status HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", server.address),
        b"",
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
}

// ---- A2: uid の判定 ----

/// 許す集合から自分の uid を外した serve への実際の接続は 403 で、小さな通常の要求は 403 の本文を
/// 受け取る(ECONNRESET にならない)。断った後も判定の枠が戻り、次の接続も 403 で答える。
/// should/0137: MainDoor::check の judge の結果を捨てて Ok を返すと最初の assert が赤になる。
#[test]
fn a_serve_that_does_not_allow_my_uid_refuses_my_connection_with_403() {
    let other = (own_uid() + 1).to_string();
    let server = start("other-uid", "127.0.0.1:0", &["--main-allow-uid", &other], &[]);
    for _ in 0..(uniqnode::http::CHECK_SLOTS + 4) {
        let response = simple(&server.address, "GET", "/v1/status", b"");
        assert_eq!(response.status, 403, "{}", body_text(&response));
        let text = body_text(&response);
        assert!(text.starts_with(uniqnode::http::REFUSED_PREFIX), "{text}");
        assert!(text.contains(&own_uid().to_string()), "{text}");
    }
}

/// 断られる相手の大きな本文や遅い要求は、実行されず、期限の内に閉じられる。
#[test]
fn a_large_or_slow_request_from_a_refused_peer_is_closed_within_the_deadline() {
    let other = (own_uid() + 1).to_string();
    let mut server = start("other-uid-large", "127.0.0.1:0", &["--main-allow-uid", &other], &[]);
    // 大きな本文(64 KiB の読み捨ての上限を越える)。
    let body = vec![b'x'; 4 * 1024 * 1024];
    let started = Instant::now();
    let mut stream = TcpStream::connect(&server.address).expect("connect");
    stream.set_write_timeout(Some(Duration::from_secs(5))).expect("timeout");
    let head = format!(
        "POST /v1/objects HTTP/1.1\r\nHost: {}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        server.address,
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body));
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest);
    assert!(started.elapsed() < Duration::from_secs(4), "閉じるまでに {:?}", started.elapsed());
    // 遅い相手(頭を言い切らない)。
    let started = Instant::now();
    let mut slow = TcpStream::connect(&server.address).expect("connect");
    slow.write_all(b"GET /v1/status HTTP/1.1\r\n").expect("partial head");
    slow.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
    let mut answer = Vec::new();
    let _ = slow.read_to_end(&mut answer);
    assert!(started.elapsed() < Duration::from_secs(3), "閉じるまでに {:?}", started.elapsed());
    // 実行されていない: 止めてからストアを CLI で開き、オブジェクトが 0 件であることを見る。
    let _ = server.child.kill();
    let _ = server.child.wait();
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["status", server.dir.to_str().expect("utf-8")])
        .output()
        .expect("status");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("objects: 0"), "{text}");
}

/// 送ってすぐ閉じる接続は、判定を遅らせて相手のソケットを FIN_WAIT2・TIME_WAIT の形にしてから
/// 判定させると、要求を実行されずに終わる(IPv4 と IPv6)。
/// should/0137: judge の ESTABLISHED と inode の検査を外すと、root を許した serve で PUT が通る
/// (uid 0・inode 0 の答えを root と読む)ので、ここでは root も許して起こす。
#[test]
fn a_connection_that_sends_and_closes_at_once_is_not_executed() {
    let allow = format!("{},0", own_uid());
    for (name, address) in [("v4", "127.0.0.1:0"), ("v6", "[::1]:0")] {
        let server = start(
            &format!("send-close-{name}"),
            address,
            &["--main-allow-uid", &allow],
            &[(uniqnode::main_door::CHECK_DELAY_ENV, "300")],
        );
        let body = br#"{"target":null}"#;
        {
            let mut stream = TcpStream::connect(&server.address).expect("connect");
            let head = format!(
                "PUT /v1/refs/sent-and-closed HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                server.address,
                body.len()
            );
            stream.write_all(head.as_bytes()).expect("head");
            stream.write_all(body).expect("body");
        }
        // 判定(300 ms 後)と、断りの読み捨ての期限(1.1 秒)が過ぎるまで待ってから確かめる。
        std::thread::sleep(Duration::from_millis(1600));
        let refs = simple(&server.address, "GET", "/v1/refs", b"");
        assert_eq!(refs.status, 200, "{}", body_text(&refs));
        assert!(!body_text(&refs).contains("sent-and-closed"), "{name}: {}", body_text(&refs));
    }
}

/// 判定の枠(16)を超える接続は、スレッドを作らずに応答を書かずに閉じられ、要求は実行されない。
#[test]
fn a_connection_beyond_the_check_slots_is_closed_without_a_response() {
    let server = start(
        "check-slots",
        "127.0.0.1:0",
        &[],
        &[(uniqnode::main_door::CHECK_DELAY_ENV, "1500")],
    );
    let head = format!("GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", server.address);
    let mut held = Vec::new();
    for _ in 0..uniqnode::http::CHECK_SLOTS {
        let mut stream = TcpStream::connect(&server.address).expect("connect");
        stream.write_all(head.as_bytes()).expect("head");
        held.push(stream);
    }
    // 16 本が accept されて枠を取るまでの猶予(accept はカーネルの待ち行列から即座に取る)。
    std::thread::sleep(Duration::from_millis(300));
    let mut over = TcpStream::connect(&server.address).expect("connect");
    let body = br#"{"target":null}"#;
    let put = format!(
        "PUT /v1/refs/beyond-the-slots HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        server.address,
        body.len()
    );
    let _ = over.write_all(put.as_bytes()).and_then(|_| over.write_all(body));
    closed_without_response(&mut over, Duration::from_millis(1000)).expect("枠の外は閉じる");
    for stream in held {
        let response = read_response(&mut BufReader::new(stream));
        assert_eq!(response.status, 200);
    }
    let refs = simple(&server.address, "GET", "/v1/refs", b"");
    assert!(!body_text(&refs).contains("beyond-the-slots"), "{}", body_text(&refs));
    assert_eq!(wait_until_idle(&server.address).checking_peak, uniqnode::http::CHECK_SLOTS as i64);
}

/// 断られる相手が 403 の読み捨てで粘っても許す相手は締め出されない(Claude 中 6): 判定の枠は
/// 判定が決まった時点で返り、403 を届ける断りの枠(8)が埋まった後の断りは 403 を書かずにすぐ
/// 閉じられる。断られる相手は PEER_UID_FILE_ENV で作る(別の uid の接続は root 無しには作れない)。
/// should/0137: 断りの間も判定の枠を持ち続ける形に戻すと、16 本の断りが 1 秒ほど判定の枠を埋め、
/// 許す相手の status が閉じられて赤になる。
#[test]
fn slow_refused_peers_do_not_shut_out_an_allowed_connection() {
    let flag = unique_dir("peer-uid-file");
    std::fs::write(&flag, (own_uid() + 1).to_string()).expect("write the peer uid file");
    let server = start(
        "refusal-slots",
        "127.0.0.1:0",
        &[],
        &[(uniqnode::main_door::PEER_UID_FILE_ENV, flag.to_str().expect("utf-8"))],
    );
    // 頭を言い切って黙る相手。serve は 403 を書いた後、相手が閉じるのを最長 1 秒待って読み捨てる。
    let head = format!("GET /healthz HTTP/1.1\r\nHost: {}\r\n\r\n", server.address);
    let mut held = Vec::new();
    for _ in 0..(uniqnode::http::CHECK_SLOTS * 3) {
        let mut stream = TcpStream::connect(&server.address).expect("connect");
        stream.write_all(head.as_bytes()).expect("head");
        held.push(stream);
    }
    // 全部が判定を終えるまでの猶予(判定は 1 本 1 ms に満たない。読み捨ての 1 秒よりずっと短い)。
    std::thread::sleep(Duration::from_millis(200));
    std::fs::remove_file(&flag).expect("remove the peer uid file");
    let started = Instant::now();
    let seen = gauge(&server.address);
    assert!(started.elapsed() < Duration::from_millis(500), "status に {:?}", started.elapsed());
    assert_eq!(seen.refusing, uniqnode::http::REFUSAL_SLOTS as i64, "断りの枠は埋まったまま");
    assert!(seen.checking_peak <= uniqnode::http::CHECK_SLOTS as i64, "判定中の最大 {}", seen.checking_peak);
    let (mut refused, mut closed) = (0, 0);
    for mut stream in held {
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("timeout");
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        let text = String::from_utf8_lossy(&answer);
        if text.starts_with("HTTP/1.1 403") {
            assert!(text.contains(uniqnode::http::REFUSED_PREFIX), "{text}");
            refused += 1;
        } else {
            assert!(answer.is_empty(), "403 でも空でもない: {text}");
            closed += 1;
        }
    }
    assert_eq!(refused, uniqnode::http::REFUSAL_SLOTS, "403 を届けたのは断りの枠の数だけ");
    assert_eq!(closed, uniqnode::http::CHECK_SLOTS * 3 - uniqnode::http::REFUSAL_SLOTS);
    let idle = wait_until_idle(&server.address);
    assert_eq!(idle.refusing_peak, uniqnode::http::REFUSAL_SLOTS as i64);
}

/// 認証後の接続の枠(64)。認証済みの接続を持ったまま次の接続が判定を通り、64 本を持った後の
/// 65 本目は応答を書かずに閉じられ、1 本を閉じると枠が戻る。
#[test]
fn authenticated_connections_are_capped_and_the_slot_returns_on_close() {
    let server = start("connection-slots", "127.0.0.1:0", &[], &[]);
    let head = format!("GET /healthz HTTP/1.1\r\nHost: {}\r\n\r\n", server.address);
    let mut held = Vec::new();
    for _ in 0..uniqnode::http::CONNECTION_SLOTS {
        let stream = TcpStream::connect(&server.address).expect("connect");
        let mut writer = stream.try_clone().expect("clone");
        writer.write_all(head.as_bytes()).expect("head");
        let mut reader = BufReader::new(stream);
        assert_eq!(read_response(&mut reader).status, 200, "持ったまま次が通る");
        held.push(reader);
    }
    let mut over = TcpStream::connect(&server.address).expect("connect");
    over.write_all(head.as_bytes()).expect("head");
    closed_without_response(&mut over, Duration::from_secs(2)).expect("枠の外は閉じる");
    drop(held.pop());
    // 閉じた 1 本の枠は、serve がその切断を読んだ時点で戻る(条件待ち。should/0104)。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut stream = TcpStream::connect(&server.address).expect("connect");
        stream.write_all(head.as_bytes()).expect("head");
        stream.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
        let mut first = [0u8; 12];
        if stream.read_exact(&mut first).is_ok() {
            assert_eq!(&first, b"HTTP/1.1 200");
            break;
        }
        assert!(Instant::now() < deadline, "閉じた接続の枠が戻らない");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(held);
    let idle = wait_until_idle(&server.address);
    assert_eq!(idle.connections_peak, uniqnode::http::CONNECTION_SLOTS as i64);
}

/// 大量の接続(1,000 本)を同時に開いても、判定中の接続は 16、認証後の接続は 64 を超えない。
#[test]
fn a_thousand_connections_stay_within_the_slots() {
    let server = start("thousand", "127.0.0.1:0", &[], &[]);
    let address = server.address.clone();
    let workers: Vec<_> = (0..20)
        .map(|_| {
            let address = address.clone();
            std::thread::spawn(move || {
                let head = format!("GET /healthz HTTP/1.1\r\nHost: {address}\r\n\r\n");
                let mut streams = Vec::new();
                for _ in 0..50 {
                    let Ok(mut stream) = TcpStream::connect(&address) else { continue };
                    let _ = stream.write_all(head.as_bytes());
                    streams.push(stream);
                }
                streams
            })
        })
        .collect();
    let streams: Vec<TcpStream> = workers.into_iter().flat_map(|w| w.join().expect("join")).collect();
    assert!(streams.len() > 900, "接続できたのは {} 本", streams.len());
    drop(streams);
    let idle = wait_until_idle(&server.address);
    assert!(idle.checking_peak <= uniqnode::http::CHECK_SLOTS as i64, "判定中の最大 {}", idle.checking_peak);
    assert!(
        idle.connections_peak <= uniqnode::http::CONNECTION_SLOTS as i64,
        "接続中の最大 {}",
        idle.connections_peak
    );
    assert!(idle.connections_peak > 1, "接続中の最大 {}(誰も通っていない)", idle.connections_peak);
}

/// TIME_WAIT を大量に作った状態でも判定の時間は表の大きさに依らない(照会は 1 件の指定で、
/// 表の走査ではない)。数値は目安の上限で、実測を出力に残す。
#[test]
fn the_check_time_does_not_grow_with_many_time_wait_sockets() {
    let server = start("time-wait", "127.0.0.1:0", &[], &[]);
    // serve が先に閉じる接続(Connection: close)を 3,000 本: serve 側に TIME_WAIT が積もる。
    for _ in 0..3000 {
        let response = simple(&server.address, "GET", "/healthz", b"");
        assert_eq!(response.status, 200);
    }
    let mut times = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let response = simple(&server.address, "GET", "/healthz", b"");
        assert_eq!(response.status, 200);
        times.push(started.elapsed());
    }
    times.sort();
    eprintln!("TIME_WAIT 3,000 本の後の 1 往復: 中央 {:?}、最大 {:?}", times[10], times[19]);
    assert!(times[19] < Duration::from_millis(500), "最大 {:?}", times[19]);
}

/// `unshare` が要る試験の前提(外部コマンドの規約。黙って飛ばさない)。
fn require_unshare() {
    let probe = Command::new("unshare").args(["--user", "true"]).output();
    match probe {
        Ok(output) if output.status.success() => {}
        other => panic!(
            "写せない uid の試験には、root 無しで user namespace を作れる unshare(util-linux)が\
             必要({other:?})。導入例: apt-get install util-linux。作れないなら \
             /proc/sys/kernel/unprivileged_userns_clone を 1 にする"
        ),
    }
}

/// 許す集合に overflowuid を入れた serve は理由を言って起動を断り(明示でも、uid を写さない user
/// namespace で既定の euid が overflowuid になった場合でも)、写せない uid の接続は 403 になる。
#[test]
fn the_overflow_uid_refuses_to_start_and_an_unmapped_peer_gets_403() {
    let overflow = uniqnode::main_door::read_overflow_uid().expect("overflowuid");
    let output = refused_serve("127.0.0.1:0", &["--main-allow-uid", &format!("{},{overflow}", own_uid())]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("overflowuid"), "{stderr}");

    require_unshare();
    let output = refused_serve_with(&["unshare", "--user"], "127.0.0.1:0", &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("overflowuid"), "{stderr}");

    // uid を写さない namespace の serve からは、外の自分の uid は写せない uid に見える。
    let server = start_with(
        &["unshare", "--user"],
        "unmapped",
        "127.0.0.1:0",
        &["--main-allow-uid", &own_uid().to_string()],
        &[],
    );
    let response = simple(&server.address, "GET", "/v1/status", b"");
    assert_eq!(response.status, 403, "{}", body_text(&response));
    assert!(body_text(&response).contains("overflowuid"), "{}", body_text(&response));
}

// ---- A1: ブラウザの門 ----

/// 一覧に無い Host に 421、Origin 付きと cross-site に 403、単純な要求の 3 つの型に 415、
/// JSON の道で Content-Type の無い要求に 415。--main-allow-host で Host を足せる。
#[test]
fn the_main_door_refuses_what_a_browser_would_send() {
    let server = start("browser", "127.0.0.1:0", &["--main-allow-host", "Forwarded.Example:17440"], &[]);
    let address = server.address.as_str();
    let port = address.rsplit_once(':').expect("port").1;
    let get = |host: &str, extra: &str| {
        send(address, &format!("GET /v1/status HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n"), b"")
    };
    assert_eq!(get(address, "").status, 200);
    assert_eq!(get(&format!("LocalHost:{port}"), "").status, 200, "大文字と小文字を区別しない");
    assert_eq!(get("forwarded.example:17440", "").status, 200, "--main-allow-host で足した名");
    assert_eq!(get(&format!("rebind.example:{port}"), "").status, 421);
    assert_eq!(get("localhost:1", "").status, 421, "ポートが違う");
    assert_eq!(get(address, &format!("Origin: http://{address}\r\n")).status, 403);
    assert_eq!(get(address, "Sec-Fetch-Site: cross-site\r\n").status, 403);

    let post = |path: &str, content_type: Option<&str>, body: &[u8]| {
        let mut head = format!("POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n", body.len());
        if let Some(value) = content_type {
            head.push_str(&format!("Content-Type: {value}\r\n"));
        }
        head.push_str("Connection: close\r\n\r\n");
        send(address, &head, body)
    };
    for simple_type in uniqnode::http::SIMPLE_CONTENT_TYPES {
        let response = post("/v1/admin/shutdown", Some(simple_type), b"");
        assert_eq!(response.status, 415, "{simple_type}: {}", body_text(&response));
    }
    let search = br#"{"query":"x"}"#;
    assert_eq!(post("/v1/search", None, search).status, 415);
    assert_eq!(post("/v1/search", Some("application/json"), search).status, 200);
    assert_eq!(post("/v1/objects", None, b"raw").status, 415);
    assert_eq!(post("/v1/objects", Some("application/octet-stream"), b"raw").status, 201);
}

// ---- install ----

/// install は serve と同じ束縛の判定を据え付けの前に当てる。
#[test]
fn install_refuses_a_main_door_that_is_not_a_loopback_ip_literal() {
    let work = unique_dir("install-listen");
    for listen in ["0.0.0.0:17440", "[::]:17440", "[::ffff:127.0.0.1]:17440", "10.10.128.1:17440", "localhost:17440"] {
        let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args([
                "install",
                work.join("store").to_str().expect("utf-8"),
                "--listen",
                listen,
                "--bin",
                work.join("bin").join("uniqnode").to_str().expect("utf-8"),
                "--unit-dir",
                work.join("units").to_str().expect("utf-8"),
                "--no-start",
            ])
            .output()
            .expect("install");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{listen}: {stderr}");
        assert!(stderr.contains(listen) && stderr.contains("ループバック"), "{listen}: {stderr}");
        assert!(!work.join("units").exists(), "{listen}: 断るときは unit を置かない");
    }
    // 通る側は normalize を直に呼ぶ(据え付けは systemctl を要る)。/tmp の下のストアは別の理由で
    // 断られるので、道は /srv の形にする(何も作らない)。
    for listen in ["127.0.0.1:17440", "127.0.0.2:17440", "[::1]:17440"] {
        let mut options = uniqnode::install::Options::defaults(
            std::path::PathBuf::from("/srv/uniqnode-store"),
            std::path::Path::new("/home/op"),
            uniqnode::install::Scope::User,
        );
        options.listen = listen.to_string();
        assert!(uniqnode::install::normalize(options).is_ok(), "{listen}");
    }
    let _ = std::fs::remove_dir_all(&work);
}

/// install の起動の確認は、常駐の利用者として起こした子プロセスから主の口を引く。root 無しの
/// 試験では常駐の利用者を自分にし、自分を許す serve に通ること、自分を外した serve には 403 の
/// 理由を言って失敗すること(root の直接の接続が断られるのと同じ形)を見る。
#[test]
fn the_install_check_probes_the_main_door_as_the_service_user() {
    let name = String::from_utf8(Command::new("id").arg("-un").output().expect("id").stdout)
        .expect("utf-8")
        .trim()
        .to_string();
    let account = uniqnode::install::lookup_account(&name).expect("account");
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_uniqnode"));
    let allowed = start("probe-allowed", "127.0.0.1:0", &[], &[]);
    let node_id = uniqnode::install::status_node_id_as(binary, &account, &allowed.address)
        .expect("自分を許す serve には通る");
    assert_eq!(node_id.len(), 64, "{node_id}");

    let other = (own_uid() + 1).to_string();
    let refusing = start("probe-refusing", "127.0.0.1:0", &["--main-allow-uid", &other], &[]);
    let error = uniqnode::install::status_node_id_as(binary, &account, &refusing.address)
        .expect_err("自分を外した serve は断る");
    assert!(error.contains("403") && error.contains(&name), "{error}");
}
