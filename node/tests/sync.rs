//! L1 レプリケーションの統合テスト(SPEC §7.3、§11 L1)。実プロセスの serve を
//! 2台立てて HTTP で同期する。受け入れ基準: 片方に投入した内容がもう片方から読める、
//! 片方停止→復旧→収束。

mod common;
use common::*;

fn sync(replica_address: &str, peer_address: &str) -> HttpResponse {
    simple(
        replica_address,
        "POST",
        "/v1/sync",
        format!("{{\"peer\":\"{peer_address}\"}}").as_bytes(),
    )
}

#[test]
fn replica_reads_what_the_origin_wrote() {
    let origin = start_server("origin");
    let replica = start_server("replica");

    // origin に「葉 ← 辺 ← ref」と blob と tombstone を作る。
    let leaf = put_object(&origin.address, b"\"leaf data\"");
    let edge_body = format!("{{\"v\":1,\"kind\":\"edge\",\"members\":[\"{leaf}\"]}}");
    let edge = put_object(&origin.address, edge_body.as_bytes());
    let blob = put_object(&origin.address, &[0u8, 159, 146, 150]); // 生バイト列(非 UTF-8)
    put_ref(&origin.address, "notes/graph", Some(&edge));
    put_ref(&origin.address, "notes/blob", Some(&blob));
    put_ref(&origin.address, "notes/gone", Some(&leaf));
    put_ref(&origin.address, "notes/gone", None); // tombstone

    let report = sync(&replica.address, &origin.address);
    assert_eq!(report.status, 200, "{}", body_text(&report));
    let report_text = body_text(&report);
    assert_eq!(json_integer_field(&report_text, "records_ingested"), Some(4));
    assert_eq!(json_integer_field(&report_text, "objects_fetched"), Some(3));
    assert_eq!(json_integer_field(&report_text, "objects_absent"), Some(0));
    assert_eq!(json_integer_field(&report_text, "hash_mismatches"), Some(0));

    // replica から origin の内容がバイト単位で読める。
    let edge_from_replica = simple(&replica.address, "GET", &format!("/v1/objects/{edge}"), b"");
    assert_eq!(edge_from_replica.status, 200);
    assert_eq!(edge_from_replica.body, edge_body.as_bytes());
    let blob_from_replica = simple(&replica.address, "GET", &format!("/v1/objects/{blob}"), b"");
    assert_eq!(blob_from_replica.body, vec![0u8, 159, 146, 150]);

    // ref も名前空間ごと複製されている(tombstone を含む)。
    let refs = body_text(&simple(&replica.address, "GET", "/v1/refs", b""));
    assert!(refs.contains("notes/graph"), "{refs}");
    assert!(refs.contains(&edge), "{refs}");
    assert!(refs.contains("notes/gone"), "{refs}");

    // 増分同期: origin に1件足して再同期すると、その1件だけが新規になる。
    put_ref(&origin.address, "notes/second", Some(&leaf));
    let second = body_text(&sync(&replica.address, &origin.address));
    assert_eq!(json_integer_field(&second, "records_ingested"), Some(1), "{second}");
    assert_eq!(json_integer_field(&second, "objects_fetched"), Some(0), "{second}");

    // 何も変わっていなければ何も運ばれない(べき等)。
    let third = body_text(&sync(&replica.address, &origin.address));
    assert_eq!(json_integer_field(&third, "records_ingested"), Some(0), "{third}");
}

#[test]
fn bidirectional_namespaces_do_not_conflict() {
    let a = start_server("bidir-a");
    let b = start_server("bidir-b");

    let object_a = put_object(&a.address, b"\"from a\"");
    put_ref(&a.address, "x", Some(&object_a));
    let object_b = put_object(&b.address, b"\"from b\"");
    put_ref(&b.address, "x", Some(&object_b));

    assert_eq!(sync(&b.address, &a.address).status, 200);
    assert_eq!(sync(&a.address, &b.address).status, 200);

    // 双方が両方の名前空間を持つ(single writer なので衝突は構造的に起きない)。
    for address in [&a.address, &b.address] {
        let refs = body_text(&simple(address, "GET", "/v1/refs", b""));
        assert!(refs.contains(&object_a), "{refs}");
        assert!(refs.contains(&object_b), "{refs}");
        let status = body_text(&simple(address, "GET", "/v1/status", b""));
        assert_eq!(json_integer_field(&status, "objects"), Some(2), "{status}");
    }

    // replica 経由の伝播: b が a の名前空間を持った状態で、第三者 c が b からだけ
    // 同期しても a の内容が届く(レコードは署名で自己認証されるため中継できる)。
    let c = start_server("bidir-c");
    assert_eq!(sync(&c.address, &b.address).status, 200);
    let refs_c = body_text(&simple(&c.address, "GET", "/v1/refs", b""));
    assert!(refs_c.contains(&object_a), "中継された a の ref がある: {refs_c}");
}

#[test]
fn peer_down_is_graceful_and_recovery_converges() {
    let origin_dir = unique_dir("down-origin");
    let origin = start_server_at(origin_dir.clone());
    let replica = start_server("down-replica");

    let first = put_object(&origin.address, b"\"before crash\"");
    put_ref(&origin.address, "a", Some(&first));
    assert_eq!(sync(&replica.address, &origin.address).status, 200);

    // origin を落とす。replica の同期は 502 で、状態は壊れない。
    let origin_address = origin.address.clone();
    {
        let mut origin = origin;
        origin.remove_dir_on_drop = false;
        // Drop で kill される
    }
    let failed = sync(&replica.address, &origin_address);
    assert_eq!(failed.status, 502, "{}", body_text(&failed));
    let status = body_text(&simple(&replica.address, "GET", "/v1/status", b""));
    assert_eq!(json_integer_field(&status, "objects"), Some(1), "{status}");

    // origin を同じデータで復旧(ポートは変わる)。書き足してから同期すると収束する。
    let origin2 = {
        let mut s = start_server_at(origin_dir);
        s.remove_dir_on_drop = true;
        s
    };
    let second = put_object(&origin2.address, b"\"after recovery\"");
    put_ref(&origin2.address, "b", Some(&second));
    let report = body_text(&sync(&replica.address, &origin2.address));
    assert_eq!(json_integer_field(&report, "records_ingested"), Some(1), "{report}");
    let refs = body_text(&simple(&replica.address, "GET", "/v1/refs", b""));
    assert!(refs.contains(&first) && refs.contains(&second), "{refs}");
}

#[test]
fn absent_closure_members_are_counted_not_fatal() {
    let origin = start_server("absent-origin");
    let replica = start_server("absent-replica");

    // origin 自身も持っていない参照(忘れられた歴史)を含む辺。
    let forgotten = format!("s256:{}", "ab".repeat(32));
    let edge_body = format!("{{\"v\":1,\"kind\":\"edge\",\"members\":[\"{forgotten}\"]}}");
    let edge = put_object(&origin.address, edge_body.as_bytes());
    put_ref(&origin.address, "partial", Some(&edge));

    let report = body_text(&sync(&replica.address, &origin.address));
    assert_eq!(json_integer_field(&report, "objects_fetched"), Some(1), "{report}");
    assert_eq!(json_integer_field(&report, "objects_absent"), Some(1), "{report}");

    // 辺そのものは届いていて、欠けは欠けのまま(開世界)。
    let edge_from_replica = simple(&replica.address, "GET", &format!("/v1/objects/{edge}"), b"");
    assert_eq!(edge_from_replica.status, 200);
}

#[test]
fn cli_sync_works_for_a_non_serving_replica() {
    let origin = start_server("cli-origin");
    let object = put_object(&origin.address, b"\"for cli replica\"");
    put_ref(&origin.address, "cli/x", Some(&object));

    let replica_dir = unique_dir("cli-replica");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["sync", replica_dir.to_str().expect("utf-8"), &origin.address])
        .output()
        .expect("run sync");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ingested: 1"), "{stdout}");
    assert!(stdout.contains("fetched: 1"), "{stdout}");

    // 同期後の replica ディレクトリは fsck 全件パス。
    let fsck = std::process::Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(["fsck", replica_dir.to_str().expect("utf-8")])
        .output()
        .expect("run fsck");
    assert!(fsck.status.success(), "{}", String::from_utf8_lossy(&fsck.stderr));
    std::fs::remove_dir_all(&replica_dir).expect("cleanup");
}
