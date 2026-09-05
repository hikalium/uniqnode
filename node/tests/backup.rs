//! バックアップと復元の統合テスト。「写しからの復元が一度は実証されている」を、毎回の
//! `cargo test` で言い直せる形にしたもの。手順は BACKUP (uuid:e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)。
//!
//! 本番の入口である `uniqnode backup` を子プロセスとして走らせ、写し先をストアとして
//! 開いて観測する(should/0137)。写し元は serve 相当にストアを開いたまま(ロックを持ったまま)
//! にしておく。

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};
use uniqnode::store::{Store, StoreConfig};

fn temp_dir(name: &str) -> PathBuf {
    common::unique_dir(&format!("backup-{name}"))
}

/// 小さい封印閾値で開く(数件の投入で複数の pack に分かれる)。
fn small_config(dir: &Path) -> StoreConfig {
    let mut config = StoreConfig::new(dir);
    config.pack_seal_bytes = 64;
    config
}

struct CommandOutcome {
    status: i32,
    stdout: String,
    stderr: String,
}

fn uniqnode(arguments: &[&str]) -> CommandOutcome {
    let output = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
        .args(arguments)
        .output()
        .expect("spawn uniqnode");
    CommandOutcome {
        status: output.status.code().expect("exit code"),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

fn backup(source: &Path, destination: &Path) -> CommandOutcome {
    uniqnode(&[
        "backup",
        source.to_str().expect("utf-8"),
        destination.to_str().expect("utf-8"),
    ])
}

/// 複数の pack に分かれる量を投入し、ref も張る。返り値は投入した ID の列。
fn fill(store: &mut Store, start: u32, count: u32) -> Vec<String> {
    let mut ids = Vec::new();
    for i in start..start + count {
        let body = format!("{{\"i\":{i},\"pad\":\"0123456789abcdef\"}}");
        let (id, _) = store.put_object(body.as_bytes()).expect("put");
        store
            .set_ref(&format!("notes/{i}"), Some(&id))
            .expect("set_ref");
        ids.push(id);
    }
    ids
}

fn sealed_packs_in(manifest_path: &Path) -> String {
    std::fs::read_to_string(manifest_path).expect("MANIFEST を読む")
}

/// (1) ストアを開いたまま backup を取り、写し先を開くと全オブジェクトが読めて ref も
/// 揃い、fsck が緑である。MANIFEST は写し元と同じ内容。
#[test]
fn a_backup_taken_beside_an_open_store_opens_with_every_object_and_a_green_fsck() {
    let source = temp_dir("open-store-source");
    let destination = temp_dir("open-store-destination");
    let mut store = Store::open(small_config(&source)).expect("open source");
    let ids = fill(&mut store, 0, 20);
    let node_id = store.node_id_hex().to_string();
    assert!(
        std::fs::read_dir(source.join("packs"))
            .expect("packs")
            .count()
            >= 3,
        "複数の pack に分かれている前提"
    );

    // 写し元は開いたまま(serve 相当。ロックを持っている)。
    let outcome = backup(&source, &destination);
    assert_eq!(
        outcome.status, 0,
        "backup は緑で終わるべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome
            .stdout
            .contains("verify: objects 20 refs 20 errors 0"),
        "検証の集計行がある: {}",
        outcome.stdout
    );

    let copy = Store::open(StoreConfig::new(&destination)).expect("写し先が開ける");
    assert_eq!(
        copy.node_id_hex(),
        node_id,
        "node_key が写っている(同じノードとして開く)"
    );
    assert_eq!(copy.object_count(), 20);
    for id in &ids {
        let bytes = copy
            .get_object(id)
            .expect("read")
            .unwrap_or_else(|| panic!("写し先で {id} が読めない"));
        assert_eq!(uniqnode::c1::id_for_bytes(&bytes), *id);
    }
    assert_eq!(copy.last_seq(), 20, "ref が全部写っている");
    let report = copy.fsck().expect("fsck");
    assert!(
        report.errors.is_empty(),
        "写し先の fsck: {:?}",
        report.errors
    );
    assert_eq!(
        sealed_packs_in(&destination.join("MANIFEST")),
        sealed_packs_in(&source.join("MANIFEST")),
        "MANIFEST は写し元と同じ内容"
    );
    drop(copy);
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (2) 2 回目の backup は増分である: 写し済みの封印済み pack は触らず(mtime が動かない)、
/// 未封印の pack と reflog は毎回写す(mtime が動く)。追記の後にもう 1 度取ると、新しく
/// 封印された pack だけが写る。
#[test]
fn a_second_backup_leaves_sealed_packs_untouched_and_recopies_only_the_active_ones() {
    let source = temp_dir("incremental-source");
    let destination = temp_dir("incremental-destination");
    let mut store = Store::open(small_config(&source)).expect("open source");
    fill(&mut store, 0, 20);
    let first = backup(&source, &destination);
    assert_eq!(first.status, 0, "{}\n{}", first.stdout, first.stderr);
    assert!(
        first
            .stdout
            .contains("backup: sealed copied 9 unchanged 0, active 2,"),
        "初回は封印済み 9 本を写す(1 pack に 2 件): {}",
        first.stdout
    );

    // 写し先の mtime を昔の時刻に揃えておき、2 回目の後にどれが動いたかを見る。
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    let sealed = destination.join("packs").join("pack-000001.pack");
    let active_pack = destination.join("packs").join("pack-000010.pack");
    let reflog = destination.join("reflog").join("reflog-000001.log");
    for path in [&sealed, &active_pack, &reflog] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open for stamping")
            .set_modified(stamp)
            .expect("set mtime");
    }
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .expect("metadata")
            .modified()
            .expect("mtime")
    };
    assert_eq!(modified(&sealed), stamp, "刻印が効いている前提");

    let second = backup(&source, &destination);
    assert_eq!(second.status, 0, "{}\n{}", second.stdout, second.stderr);
    assert!(
        second
            .stdout
            .contains("backup: sealed copied 0 unchanged 9, active 2,"),
        "2 回目は封印済みを 1 本も写さない: {}",
        second.stdout
    );
    assert_eq!(
        modified(&sealed),
        stamp,
        "写し済みの封印済み pack は触らない"
    );
    assert_ne!(modified(&active_pack), stamp, "未封印の pack は毎回写す");
    assert_ne!(modified(&reflog), stamp, "reflog(未封印)は毎回写す");

    // 追記して新しい封印が起きた後の 3 回目は、新しく封印された分だけを写す。
    let more = fill(&mut store, 20, 10);
    let third = backup(&source, &destination);
    assert_eq!(third.status, 0, "{}\n{}", third.stdout, third.stderr);
    assert!(
        third.stdout.contains("backup: sealed copied 4 unchanged 10, active 2,"),
        "3 回目は新しく封印された pack 11〜14 だけを写す(pack 10 は前回アクティブとして完全に写っており、同じ大きさなので写さない): {}",
        third.stdout
    );
    let copy = Store::open(StoreConfig::new(&destination)).expect("写し先が開ける");
    assert_eq!(copy.object_count(), 30);
    assert!(
        more.iter().all(|id| copy.has_object(id)),
        "追記分が写っている"
    );
    drop(copy);
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (3) 写し先の封印済み pack を 1 バイト壊すと、fsck も次の backup も
/// 「バックアップからの復元が必要」と言って赤になる。壊れた 1 本を消して取り直せば、
/// 写し直されて緑に戻る(文書に書いた修復の道)。
#[test]
fn a_corrupted_sealed_pack_in_the_backup_turns_fsck_red_and_is_recopied_after_removal() {
    let source = temp_dir("corrupt-source");
    let destination = temp_dir("corrupt-destination");
    let mut store = Store::open(small_config(&source)).expect("open source");
    fill(&mut store, 0, 20);
    let first = backup(&source, &destination);
    assert_eq!(first.status, 0, "{}\n{}", first.stdout, first.stderr);

    let sealed = destination.join("packs").join("pack-000001.pack");
    let mut bytes = std::fs::read(&sealed).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&sealed, &bytes).expect("write corrupted");

    let fsck = uniqnode(&["fsck", destination.to_str().expect("utf-8")]);
    assert_ne!(
        fsck.status, 0,
        "壊れた写しの fsck は赤であるべき: {}",
        fsck.stdout
    );
    assert!(
        fsck.stderr
            .contains("封印済み pack 1 が壊れている(バックアップからの復元が必要)"),
        "診断が復元の必要を言う: {}",
        fsck.stderr
    );

    // 同じ大きさなので増分の判定では触られず、検証が赤になる。黙って緑にしない。
    let again = backup(&source, &destination);
    assert_ne!(
        again.status, 0,
        "壊れた写し先への backup は赤であるべき: {}",
        again.stdout
    );
    assert!(
        again
            .stderr
            .contains("封印済み pack 1 が壊れている(バックアップからの復元が必要)"),
        "{}",
        again.stderr
    );

    std::fs::remove_file(&sealed).expect("壊れた 1 本を消す");
    let repaired = backup(&source, &destination);
    assert_eq!(
        repaired.status, 0,
        "{}\n{}",
        repaired.stdout, repaired.stderr
    );
    assert!(
        repaired.stdout.contains("copied packs/pack-000001.pack\n"),
        "消した 1 本だけが写し直される: {}",
        repaired.stdout
    );
    assert!(
        repaired
            .stdout
            .contains("backup: sealed copied 1 unchanged 8,"),
        "{}",
        repaired.stdout
    );
    let copy = Store::open(StoreConfig::new(&destination)).expect("写し先が開ける");
    assert!(copy.fsck().expect("fsck").errors.is_empty());
    drop(copy);
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (4) derived/ logs/ tmp/ は写さず、写さなかったことを言う。
#[test]
fn derived_data_logs_and_scratch_are_not_copied_and_are_named_in_the_report() {
    let source = temp_dir("not-copied-source");
    let destination = temp_dir("not-copied-destination");
    let mut store = Store::open(small_config(&source)).expect("open source");
    fill(&mut store, 0, 5);
    std::fs::create_dir_all(source.join("derived").join("embeddings")).expect("mkdir");
    std::fs::write(
        source.join("derived").join("embeddings").join("bge-m3.vec"),
        b"vectors",
    )
    .expect("write");
    std::fs::create_dir_all(source.join("logs")).expect("mkdir");
    std::fs::write(source.join("logs").join("serve.log"), b"a line\n").expect("write");
    std::fs::write(source.join("tmp").join("write-leftover"), b"scratch").expect("write");
    std::fs::write(source.join("peers.json"), b"[]").expect("write");

    let outcome = backup(&source, &destination);
    assert_eq!(outcome.status, 0, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(!destination.join("derived").exists(), "derived/ は写さない");
    assert!(!destination.join("logs").exists(), "logs/ は写さない");
    assert!(
        !destination.join("tmp").join("write-leftover").exists(),
        "tmp/ の中身は写さない"
    );
    assert!(destination.join("peers.json").exists(), "設定は写す");
    assert!(
        outcome.stdout.contains("not copied: derived logs tmp"),
        "写さなかったものを言う: {}",
        outcome.stdout
    );
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (5) 実プロセスの serve がロックを持って走っている隣で backup が通り、写しから serve が
/// 受け付けたオブジェクトが読める。
#[test]
fn a_backup_runs_beside_a_serving_process_without_taking_its_lock() {
    let server = common::start_server("backup-beside-serve");
    let id = common::put_object(&server.address, b"{\"kind\":\"node\",\"v\":1}");
    common::put_ref(&server.address, "notes/live", Some(&id));
    let destination = temp_dir("beside-serve-destination");

    let outcome = backup(&server.dir, &destination);
    assert_eq!(
        outcome.status, 0,
        "serve が走っていても backup は通るべき\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );
    let copy = Store::open(StoreConfig::new(&destination)).expect("写し先が開ける");
    assert_eq!(
        copy.get_object(&id).expect("read").as_deref(),
        Some(&b"{\"kind\":\"node\",\"v\":1}"[..])
    );
    assert!(
        copy.get_ref(&copy.own_ref_name("notes/live")).is_some(),
        "ref も写っている"
    );
    // serve はまだ生きている(backup がロックを奪っていない)。
    let status = common::simple(&server.address, "GET", "/v1/status", b"");
    assert_eq!(status.status, 200);
    drop(copy);
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (6) 写し元で書き込み途中のレコード(未封印 pack の尻尾)は、写し先を開くときに切り詰め
/// られ、何バイト捨てたかを言う。有効なレコードはすべて残る。
#[test]
fn a_torn_tail_on_the_active_pack_is_cut_at_verification_and_reported() {
    let source = temp_dir("torn-source");
    let destination = temp_dir("torn-destination");
    let ids;
    {
        let mut store = Store::open(small_config(&source)).expect("open source");
        ids = fill(&mut store, 0, 3);
    }
    // 未封印 pack の末尾に書きかけのレコード(len だけ書けて本体が無い)を置く。
    let active = source.join("packs").join("pack-000002.pack");
    assert!(
        active.exists(),
        "3 件で pack-000002 が未封印のアクティブになる前提"
    );
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&active)
            .expect("open");
        file.write_all(&[0x40, 0x00, 0x00, 0x00, 0xde, 0xad])
            .expect("garbage");
    }

    let outcome = backup(&source, &destination);
    assert_eq!(outcome.status, 0, "{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome
            .stdout
            .contains("cut packs/pack-000002.pack (6 bytes"),
        "切り詰めた尻尾を言う: {}",
        outcome.stdout
    );
    let copy = Store::open(StoreConfig::new(&destination)).expect("写し先が開ける");
    assert_eq!(copy.object_count(), 3);
    assert!(ids.iter().all(|id| copy.has_object(id)));
    drop(copy);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// 復元: 写しをそのまま新しい data_dir として backup で写し戻し、fsck が緑で、元と同じ
/// node id・同じオブジェクト・同じ ref を持つ。
#[test]
fn restoring_is_a_backup_in_the_other_direction_followed_by_fsck() {
    let source = temp_dir("restore-source");
    let destination = temp_dir("restore-destination");
    let restored = temp_dir("restore-restored");
    let (ids, node_id) = {
        let mut store = Store::open(small_config(&source)).expect("open source");
        let ids = fill(&mut store, 0, 20);
        (ids, store.node_id_hex().to_string())
    };
    assert_eq!(backup(&source, &destination).status, 0);
    std::fs::remove_dir_all(&source).expect("元を失う");

    let outcome = backup(&destination, &restored);
    assert_eq!(outcome.status, 0, "{}\n{}", outcome.stdout, outcome.stderr);
    let fsck = uniqnode(&["fsck", restored.to_str().expect("utf-8")]);
    assert_eq!(fsck.status, 0, "{}\n{}", fsck.stdout, fsck.stderr);
    assert!(
        fsck.stdout.contains("objects: 20 refs: 20 errors: 0"),
        "{}",
        fsck.stdout
    );
    let store = Store::open(StoreConfig::new(&restored)).expect("復元先が開ける");
    assert_eq!(store.node_id_hex(), node_id, "同じノードとして復元される");
    assert!(ids.iter().all(|id| store.has_object(id)));
    assert_eq!(store.last_seq(), 20);
    drop(store);
    std::fs::remove_dir_all(&destination).expect("cleanup");
    std::fs::remove_dir_all(&restored).expect("cleanup");
}

/// 回収(gc)が写し元に残す姿を作る: 封印済み pack `number` を写し元の MANIFEST から外し、
/// ファイルを消す。gc の本体はまだ無いので、その結果(MANIFEST に無く、packs/ にも無い番号)
/// だけをここで作る。写し元のストアは閉じてから呼ぶ(開いたままだと、次の封印で手元の一覧が
/// 書き戻される)。
fn reclaim_pack_from_source(source: &Path, number: u64) {
    let manifest_path = source.join("MANIFEST");
    let text = std::fs::read_to_string(&manifest_path).expect("MANIFEST を読む");
    let mut value = uniqnode::c1::parse(&text).expect("MANIFEST は c1");
    let uniqnode::c1::Value::Object(map) = &mut value else {
        panic!("MANIFEST はオブジェクト");
    };
    let Some(uniqnode::c1::Value::Array(sealed)) = map.get_mut("sealed_packs") else {
        panic!("sealed_packs がある");
    };
    let before = sealed.len();
    sealed.retain(|v| *v != uniqnode::c1::Value::Integer(number as i64));
    assert_eq!(sealed.len(), before - 1, "pack {number} は封印済みだった前提");
    std::fs::write(&manifest_path, uniqnode::c1::to_canonical_bytes(&value)).expect("MANIFEST");
    std::fs::remove_file(source.join("packs").join(format!("pack-{number:06}.pack")))
        .expect("回収された pack を消す");
}

/// 残骸のテストの共通の準備: 20 件(1 pack に 2 件、pack 1〜9 封印済み、10 がアクティブ)を
/// 投入して 1 回目の backup を取り、pack 5 の 2 件(i=8,9)の ref を tombstone してから
/// 写し元で pack 5 を回収する。返り値は (写し元, 写し先, 回収された 2 件の ID)。写し先には
/// 1 回目に写った pack-000005.pack が残骸として残っている。
fn source_with_pack_5_reclaimed_after_first_backup(
    name: &str,
) -> (PathBuf, PathBuf, Vec<String>) {
    let source = temp_dir(&format!("{name}-source"));
    let destination = temp_dir(&format!("{name}-destination"));
    let ids = {
        let mut store = Store::open(small_config(&source)).expect("open source");
        let ids = fill(&mut store, 0, 20);
        let first = backup(&source, &destination);
        assert_eq!(first.status, 0, "{}\n{}", first.stdout, first.stderr);
        store.set_ref("notes/8", None).expect("tombstone");
        store.set_ref("notes/9", None).expect("tombstone");
        ids
    };
    reclaim_pack_from_source(&source, 5);
    let reclaimed = vec![ids[8].clone(), ids[9].clone()];
    let store = Store::open(small_config(&source)).expect("回収後の写し元が開ける");
    assert_eq!(store.object_count(), 18, "pack 5 の 2 件が消えている前提");
    assert!(
        reclaimed.iter().all(|id| !store.has_object(id)),
        "i=8,9 が pack 5 に居た前提(1 pack に 2 件)"
    );
    assert!(store.fsck().expect("fsck").errors.is_empty());
    drop(store);
    assert!(
        destination.join("packs").join("pack-000005.pack").exists(),
        "1 回目の写しが残骸として残っている前提"
    );
    (source, destination, reclaimed)
}

/// 2 回目の backup が残骸を消して名を言い、写し先のオブジェクト数が写し元と一致し
/// (残骸の中身が生き返っていない)、fsck が緑であることを見る。写し元は開いたまま(serve 相当)。
fn assert_remnant_removed_by_second_backup(
    source: &Path,
    destination: &Path,
    reclaimed: &[String],
) {
    let store = Store::open(small_config(source)).expect("open source");
    let second = backup(source, destination);
    assert_eq!(
        second.status, 0,
        "残骸を持つ写し先への backup は緑で終わるべき\nstdout:\n{}\nstderr:\n{}",
        second.stdout, second.stderr
    );
    assert!(
        second
            .stdout
            .contains("removed packs/pack-000005.pack (not in source MANIFEST)\n"),
        "消した残骸を名で言う: {}",
        second.stdout
    );
    assert!(
        second
            .stdout
            .contains("backup: sealed copied 0 unchanged 8, active 2, removed 1,"),
        "{}",
        second.stdout
    );
    assert!(
        second.stdout.contains("verify: objects 18 refs 20 errors 0"),
        "残骸の 2 件は生き返らない: {}",
        second.stdout
    );
    assert!(
        !destination.join("packs").join("pack-000005.pack").exists(),
        "残骸が消えている"
    );
    let copy = Store::open(StoreConfig::new(destination)).expect("写し先が開ける");
    assert_eq!(copy.object_count(), 18, "写し元と同じオブジェクト数");
    assert!(
        reclaimed.iter().all(|id| !copy.has_object(id)),
        "回収済みのオブジェクトが写し先で生き返っていない"
    );
    assert!(
        copy.fsck().expect("fsck").errors.is_empty(),
        "写し先の fsck は緑"
    );
    drop(copy);
    drop(store);
}

/// (7) 回収の後の backup: 写し元の MANIFEST から消えた番号の pack が写し先に残っていたら、
/// 2 回目の backup がそれを消して報告に名を出し、写し先のオブジェクト数は写し元と一致する。
/// 消さなければ、残骸は未封印として走査され、回収前の孤児が写し先で生き返る。
#[test]
fn a_pack_the_source_manifest_no_longer_lists_is_removed_from_the_backup_and_named() {
    let (source, destination, reclaimed) =
        source_with_pack_5_reclaimed_after_first_backup("reclaimed-intact");
    assert_remnant_removed_by_second_backup(&source, &destination, &reclaimed);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (8) 残骸が torn tail を持っていても同じ。消さなければ、写し先を開くとき「封印済みでも
/// 最後でもない pack が尻切れ」として破損と誤判定され、backup が赤になる道。
#[test]
fn a_remnant_pack_with_a_torn_tail_is_removed_instead_of_being_mistaken_for_corruption() {
    let (source, destination, reclaimed) =
        source_with_pack_5_reclaimed_after_first_backup("reclaimed-torn");
    let remnant = destination.join("packs").join("pack-000005.pack");
    let length = std::fs::metadata(&remnant).expect("metadata").len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&remnant)
        .expect("open")
        .set_len(length - 3)
        .expect("末尾を 3 バイト切る");
    assert_remnant_removed_by_second_backup(&source, &destination, &reclaimed);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (9) 写し先だけにある設定ファイルは pack と違って消さない: 写し元で消した peers.json は
/// `only in backup` で言うだけで残る。
#[test]
fn a_settings_file_only_in_the_backup_is_kept_and_named() {
    let source = temp_dir("settings-only-source");
    let destination = temp_dir("settings-only-destination");
    let mut store = Store::open(small_config(&source)).expect("open source");
    fill(&mut store, 0, 4);
    std::fs::write(source.join("peers.json"), b"[]").expect("write");
    let first = backup(&source, &destination);
    assert_eq!(first.status, 0, "{}\n{}", first.stdout, first.stderr);
    assert!(destination.join("peers.json").exists(), "設定は写る前提");

    std::fs::remove_file(source.join("peers.json")).expect("写し元で消す");
    let second = backup(&source, &destination);
    assert_eq!(second.status, 0, "{}\n{}", second.stdout, second.stderr);
    assert!(
        second.stdout.contains("only in backup peers.json"),
        "消していないことを言う: {}",
        second.stdout
    );
    assert!(
        destination.join("peers.json").exists(),
        "写し先だけの設定ファイルは消さない"
    );
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}

/// (10) 境界: 写し元に MANIFEST が無い(一度も封印していない)ときは、封印済みの一覧という
/// 権威が無いので、写し先だけの pack を消さず `only in backup` で言う。reflog は MANIFEST が
/// あっても無くても消さない(回収が無い)。
#[test]
fn without_a_source_manifest_extra_packs_and_reflogs_in_the_backup_are_kept_and_named() {
    let source = temp_dir("no-manifest-source");
    let destination = temp_dir("no-manifest-destination");
    let mut store = Store::open(small_config(&source)).expect("open source");
    fill(&mut store, 0, 1);
    assert!(
        !source.join("MANIFEST").exists(),
        "1 件では封印されず MANIFEST が無い前提"
    );
    let first = backup(&source, &destination);
    assert_eq!(first.status, 0, "{}\n{}", first.stdout, first.stderr);
    let packs = destination.join("packs");
    let reflog = destination.join("reflog");
    std::fs::copy(
        packs.join("pack-000001.pack"),
        packs.join("pack-000002.pack"),
    )
    .expect("写し先だけの pack を置く");
    std::fs::copy(
        reflog.join("reflog-000001.log"),
        reflog.join("reflog-000002.log"),
    )
    .expect("写し先だけの reflog を置く");

    let second = backup(&source, &destination);
    assert_eq!(second.status, 0, "{}\n{}", second.stdout, second.stderr);
    assert!(
        second.stdout.contains("only in backup packs/pack-000002.pack"),
        "MANIFEST の無い写し元では pack を消さずに言う: {}",
        second.stdout
    );
    assert!(
        second.stdout.contains("only in backup reflog/reflog-000002.log"),
        "reflog は消さずに言う: {}",
        second.stdout
    );
    assert!(
        second.stdout.contains("active 2, removed 0,") && !second.stdout.contains("removed packs/"),
        "何も消していない: {}",
        second.stdout
    );
    assert!(packs.join("pack-000002.pack").exists());
    assert!(reflog.join("reflog-000002.log").exists());
    drop(store);
    std::fs::remove_dir_all(&source).expect("cleanup");
    std::fs::remove_dir_all(&destination).expect("cleanup");
}
