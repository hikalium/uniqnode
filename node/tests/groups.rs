//! L4 グループと k-of-n の統合テスト(SPEC §6.4、§11 L4)。
//! 受け入れ基準: 証明書を持つピアの自動受け入れ / 鍵除去による発行物の失効
//! (統一検証規則)。CLI のセレモニー(admin-keygen → cert-make → cert-sign ×k →
//! cert-verify)を実プロセスで通す。

mod common;
use common::*;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_uniqnode")
}

fn run_ok(arguments: &[&str]) -> String {
    let output = Command::new(binary()).args(arguments).output().expect("run");
    assert!(
        output.status.success(),
        "uniqnode {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

fn run_with_stdin(arguments: &[&str], stdin_text: &str) -> (bool, String, String) {
    let mut child = Command::new(binary())
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin_text.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

struct Admin {
    key_file: PathBuf,
    public_key: String,
}

fn make_admin(dir: &Path, name: &str) -> Admin {
    let key_file = dir.join(format!("admin-{name}.key"));
    let output = run_ok(&["admin-keygen", key_file.to_str().expect("utf-8")]);
    let public_key = output
        .trim()
        .strip_prefix("public_key: ")
        .expect("public key line")
        .to_string();
    Admin { key_file, public_key }
}

fn write_groups(dir: &Path, keys: &[&str], enroll: u32) {
    let key_list: Vec<String> = keys.iter().map(|k| format!("\"{k}\"")).collect();
    std::fs::write(
        dir.join("groups.json"),
        format!(
            "{{\"groups\":[{{\"group_id\":\"family\",\"keys\":[{}],\
             \"thresholds\":{{\"enroll\":{enroll},\"revoke\":1}}}}]}}",
            key_list.join(",")
        ),
    )
    .expect("write groups.json");
}

/// セレモニー一式: 2-of-2 の発行 → 検証 → 鍵除去で失効(統一検証規則の系)。
#[test]
fn two_of_two_ceremony_and_keyset_removal() {
    let work = unique_dir("ceremony");
    std::fs::create_dir_all(&work).expect("mkdir");
    let admin1 = make_admin(&work, "one");
    let admin2 = make_admin(&work, "two");
    write_groups(&work, &[&admin1.public_key, &admin2.public_key], 2);

    // 本体を作り、管理者2人が順に署名する(ファイル渡しの k-of-n セレモニー)。
    let body = run_ok(&["cert-make", "node-under-test", "family", "30"]);
    let (ok1, once_signed, err1) =
        run_with_stdin(&["cert-sign", admin1.key_file.to_str().expect("utf-8")], &body);
    assert!(ok1, "{err1}");
    let (ok2, fully_signed, err2) =
        run_with_stdin(&["cert-sign", admin2.key_file.to_str().expect("utf-8")], &once_signed);
    assert!(ok2, "{err2}");

    // 1署名では 2-of-2 に足りない。
    let (accepted, _, reject_reason) =
        run_with_stdin(&["cert-verify", work.to_str().expect("utf-8")], &once_signed);
    assert!(!accepted, "1署名で受理されてはならない");
    assert!(reject_reason.contains("署名不足"), "{reject_reason}");

    // 2署名で受理。
    let (accepted, verdict, err) =
        run_with_stdin(&["cert-verify", work.to_str().expect("utf-8")], &fully_signed);
    assert!(accepted, "{err}");
    assert!(verdict.contains("ok: group family"), "{verdict}");

    // 鍵集合から admin2 を外すと、既発行の証明書は閾値を割って失効する。
    write_groups(&work, &[&admin1.public_key], 2);
    let (accepted, _, reject_reason) =
        run_with_stdin(&["cert-verify", work.to_str().expect("utf-8")], &fully_signed);
    assert!(!accepted, "鍵除去後は失効するはず");
    assert!(reject_reason.contains("署名不足"), "{reject_reason}");

    // 失効文でも拒否できる(revoke は 1-of-n)。鍵集合を戻して失効文を差し込む。
    let revocation_body = run_ok(&["revoke-make", "node-under-test", "family"]);
    let (ok, revocation, err) = run_with_stdin(
        &["cert-sign", admin1.key_file.to_str().expect("utf-8")],
        &revocation_body,
    );
    assert!(ok, "{err}");
    std::fs::write(
        work.join("groups.json"),
        format!(
            "{{\"groups\":[{{\"group_id\":\"family\",\
             \"keys\":[\"{}\",\"{}\"],\
             \"thresholds\":{{\"enroll\":2,\"revoke\":1}},\
             \"revocations\":[{}]}}]}}",
            admin1.public_key,
            admin2.public_key,
            revocation.trim()
        ),
    )
    .expect("write groups.json");
    let (accepted, _, reject_reason) =
        run_with_stdin(&["cert-verify", work.to_str().expect("utf-8")], &fully_signed);
    assert!(!accepted, "失効文で拒否されるはず");
    assert!(reject_reason.contains("失効済み"), "{reject_reason}");

    std::fs::remove_dir_all(&work).expect("cleanup");
}

/// 証明書付きピアの自動受け入れ: 有効な証明書を持つエントリはスコープに入り、
/// 鍵除去(groups.json の編集。再起動不要)で即座にスコープから外れる。
#[test]
fn certified_peer_is_accepted_into_scope_until_the_key_is_removed() {
    let a = start_server("g-a");
    let b = start_server("g-b");
    let b_id = {
        let status = body_text(&simple(&b.address, "GET", "/v1/status", b""));
        json_text_field(&status, "node_id").expect("node_id")
    };

    let admin1 = make_admin(&a.dir, "one");
    let admin2 = make_admin(&a.dir, "two");
    write_groups(&a.dir, &[&admin1.public_key, &admin2.public_key], 2);

    // B のメンバーシップ証明書を 2-of-2 で発行する。
    let body = run_ok(&["cert-make", &b_id, "family", "30"]);
    let (_, once, _) =
        run_with_stdin(&["cert-sign", admin1.key_file.to_str().expect("utf-8")], &body);
    let (_, cert, _) =
        run_with_stdin(&["cert-sign", admin2.key_file.to_str().expect("utf-8")], &once);

    // 証明書付きエントリだけの peers.json(手動アドレスなし)。
    std::fs::write(
        a.dir.join("peers.json"),
        format!(
            "{{\"peers\":[{{\"address\":\"{}\",\"certificate\":{}}}]}}",
            b.address,
            cert.trim()
        ),
    )
    .expect("write peers.json");

    // 受け入れられ、スコープ(/v1/peers)に現れる。
    let listed = body_text(&simple(&a.address, "GET", "/v1/peers", b""));
    assert!(listed.contains(&b.address), "証明書で受け入れられる: {listed}");

    // スコープとして実際に機能する(B だけが持つオブジェクトが既定スコープで見つかる)。
    let id = put_object(&b.address, b"\"held by certified peer\"");
    let response = simple(
        &a.address,
        "POST",
        "/v1/query",
        format!("{{\"kind\":\"object\",\"target\":\"{id}\",\"budget_ms\":5000}}").as_bytes(),
    );
    assert!(body_text(&response).contains("\"outcome\":\"found\""), "{}", body_text(&response));

    // 管理者鍵を1本外す(ファイル編集のみ)→ 証明書は閾値を割り、スコープから消える。
    write_groups(&a.dir, &[&admin1.public_key], 2);
    let listed = body_text(&simple(&a.address, "GET", "/v1/peers", b""));
    assert!(!listed.contains(&b.address), "鍵除去で発行物ごと失効する: {listed}");
}

/// 証明書の node_id と実際のノードが一致しない場合、健全性エンジンは接触を
/// 受け入れない(なりすましアドレスの排除)。
#[test]
fn certified_entry_with_wrong_node_id_is_not_trusted_by_the_engine() {
    let a = start_server("g-wrong-a");
    let b = start_server("g-wrong-b");

    let admin1 = make_admin(&a.dir, "one");
    write_groups(&a.dir, &[&admin1.public_key], 1);

    // B のアドレスに、別の node_id を主張する証明書を付ける。
    let body = run_ok(&["cert-make", "somebody-else", "family", "30"]);
    let (_, cert, _) =
        run_with_stdin(&["cert-sign", admin1.key_file.to_str().expect("utf-8")], &body);
    std::fs::write(
        a.dir.join("peers.json"),
        format!(
            "{{\"peers\":[{{\"address\":\"{}\",\"certificate\":{}}}]}}",
            b.address,
            cert.trim()
        ),
    )
    .expect("write peers.json");

    // 静的検証は通る(署名は本物)のでスコープには載る。
    let listed = body_text(&simple(&a.address, "GET", "/v1/peers", b""));
    assert!(listed.contains(&b.address), "{listed}");

    // だがエンジンの接触では node_id 照合で拒否され、生存者として扱われない。
    // (= B に何かを pin しても A は B を修復候補と見なさない。)
    // 接触拒否のログが出るまでではなく、拒否の観測可能な帰結として、A の status の
    // health に B が保持者として現れないことを確認する。
    let id = put_object(&a.address, b"\"pinned with an impostor peer\"");
    let pinned = simple(
        &a.address,
        "POST",
        "/v1/pins",
        format!("{{\"root\":\"{id}\",\"min_replicas\":2}}").as_bytes(),
    );
    assert_eq!(pinned.status, 200, "{}", body_text(&pinned));
    // 猶予(既定 T_prop=20s)より短い時間だけ観測し、B が保持者に化けないことを見る。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let pins = body_text(&simple(&a.address, "GET", "/v1/pins", b""));
        let b_id = {
            let status = body_text(&simple(&b.address, "GET", "/v1/status", b""));
            json_text_field(&status, "node_id").expect("node_id")
        };
        assert!(!pins.contains(&b_id), "なりすましピアが保持者に現れた: {pins}");
        std::thread::yield_now();
    }
}
