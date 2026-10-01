//! ストアの書き込みに失敗した後の扱い(APPEND_FAILURE (docs/plan/APPEND_FAILURE.md) の S1a)の
//! ストアの層の試験。失敗はプロセスの中の注入(Store::inject_fault)と、ファイルシステムの
//! 権限で起こす。どの形でも、失敗した要求は WritesDisabled を受け取り、以後の書き込みの入口は
//! 全部断り、読み出しは正しい中身を返し、開き直したストアは応答済みの書き込みを 1 つも
//! 欠かない。serve と CLI を通す形は node/tests/append_failure_serve.rs。
//!
//! 注入(Store::inject_fault と UNIQNODE_APPEND_FAULT)と sync の記録(UNIQNODE_SYNC_LOG)は
//! debug ビルドにしか無いので、それに頼る試験は #[cfg(debug_assertions)] で、
//! `cargo test --release` では走らない。権限で起こす試験は root では権限が効かないので、
//! root なら理由を出して戻る。

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(debug_assertions)]
use uniqnode::store::Cleanup;
use uniqnode::store::{FailureKind, Store, StoreConfig, StoreError, WriteOp};

fn temp_dir(name: &str) -> PathBuf {
    common::unique_dir(&format!("append-failure-{name}"))
}

/// 権限で失敗を起こす試験は root で走らない前提(root には 0555 や 0333 が効かない)。root なら
/// 理由を出して戻る(node/tests/install.rs の not_root と同じ)。
fn not_root() -> bool {
    let euid = uniqnode::install::effective_uid().expect("euid");
    if euid == 0 {
        println!("root で走っているので、権限で失敗を起こすテストは走らせない");
    }
    euid != 0
}

/// 小さい封印閾値で開く(数件の投入で封印が起きる)。
fn small_config(dir: &Path) -> StoreConfig {
    let mut config = StoreConfig::new(dir);
    config.pack_seal_bytes = 64;
    config
}

fn writes_disabled(result: Result<impl std::fmt::Debug, StoreError>, context: &str) -> uniqnode::store::WriteFailure {
    match result {
        Err(StoreError::WritesDisabled(failure)) => failure,
        other => panic!("{context}: WritesDisabled のはずが {other:?}"),
    }
}

/// 書けない状態で、全部の書き込みの入口が WritesDisabled で断ることを言う。
#[cfg(debug_assertions)]
fn assert_every_write_refused(store: &mut Store, existing: &str, context: &str) {
    writes_disabled(store.put_object(b"after-the-failure"), &format!("{context}: put_object"));
    writes_disabled(store.put_object(existing.as_bytes()), &format!("{context}: 既に在るものの put_object"));
    writes_disabled(store.set_ref("notes/after", None), &format!("{context}: set_ref"));
    writes_disabled(store.set_pin(existing, 0), &format!("{context}: set_pin"));
    writes_disabled(store.set_attest(existing, false), &format!("{context}: set_attest"));
    writes_disabled(store.gc_try_begin(), &format!("{context}: gc_try_begin"));
}

/// 応答済みの (ID, 本体, ref の道)。
#[cfg(debug_assertions)]
struct Acknowledged {
    objects: Vec<(String, Vec<u8>)>,
    refs: Vec<(String, String)>,
}

#[cfg(debug_assertions)]
fn write_acknowledged(store: &mut Store, count: usize) -> Acknowledged {
    let mut objects = Vec::new();
    let mut refs = Vec::new();
    for i in 0..count {
        let body = format!("{{\"acknowledged\":{i}}}").into_bytes();
        let (id, _) = store.put_object(&body).expect("put");
        let path = format!("notes/{i}");
        store.set_ref(&path, Some(&id)).expect("set_ref");
        refs.push((path, id.clone()));
        objects.push((id, body));
    }
    Acknowledged { objects, refs }
}

#[cfg(debug_assertions)]
fn assert_acknowledged_readable(store: &Store, acknowledged: &Acknowledged, context: &str) {
    for (id, body) in &acknowledged.objects {
        assert_eq!(
            store.get_object(id).expect("get").as_deref(),
            Some(body.as_slice()),
            "{context}: 応答済みの {id} が読めない"
        );
    }
    for (path, id) in &acknowledged.refs {
        let name = store.own_ref_name(path);
        assert_eq!(
            store.get_ref(&name).and_then(|state| state.target.clone()).as_deref(),
            Some(id.as_str()),
            "{context}: 応答済みの ref {path} が欠けた"
        );
    }
}

fn file_length(path: &Path) -> u64 {
    std::fs::metadata(path).expect("metadata").len()
}

#[cfg(debug_assertions)]
fn active_reflog(dir: &Path) -> PathBuf {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir.join("reflog"))
        .expect("read_dir")
        .map(|e| e.expect("entry").path())
        .collect();
    names.sort();
    names.pop().expect("reflog がある")
}

fn active_pack(dir: &Path, store: &Store) -> PathBuf {
    dir.join("packs").join(format!("pack-{:06}.pack", store.active_pack_number()))
}

/// before・torn・sync を pack と reflog の追記のそれぞれに掛ける。失敗した要求は
/// WritesDisabled(kind と op は種類どおり)、以後の書き込みは全部断られ、読み出しは答え、
/// 切り詰めでファイルの長さは前に戻り、開き直すと応答済みのものが全部あって失敗した 1 本は無い。
#[test]
#[cfg(debug_assertions)]
fn each_append_fault_disables_writes_keeps_reads_and_loses_nothing_acknowledged() {
    let cases = [
        ("before", FailureKind::Io, WriteOp::Write),
        ("torn:5", FailureKind::NoSpace, WriteOp::Write),
        ("torn:0", FailureKind::NoSpace, WriteOp::Write),
        ("sync", FailureKind::Io, WriteOp::Sync),
        ("nospace-sync", FailureKind::Io, WriteOp::Sync),
    ];
    for (kind_text, kind, op) in cases {
        for segment in ["pack", "reflog"] {
            let context = format!("{kind_text}@{segment}");
            let dir = temp_dir(&format!("each-{}-{segment}", kind_text.replace(':', "-")));
            let acknowledged = {
                let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
                let acknowledged = write_acknowledged(&mut store, 3);
                let (target_path, length_before) = match segment {
                    "pack" => {
                        let path = active_pack(&dir, &store);
                        let length = file_length(&path);
                        (path, length)
                    }
                    _ => {
                        let path = active_reflog(&dir);
                        let length = file_length(&path);
                        (path, length)
                    }
                };
                store.inject_fault(&format!("{kind_text}@{segment}:1")).expect("inject");
                let failure = match segment {
                    "pack" => writes_disabled(store.put_object(b"{\"failed\":1}"), &context),
                    _ => writes_disabled(
                        store.set_ref("notes/failed", Some(&acknowledged.objects[0].0)),
                        &context,
                    ),
                };
                assert_eq!(failure.kind, kind, "{context}: kind");
                assert_eq!(failure.op, op, "{context}: op");
                assert_eq!(failure.cleanup, Cleanup::Ok, "{context}: 切り詰めは成功する");
                assert!(failure.errno.is_some(), "{context}: errno を持つ");
                assert_eq!(
                    store.writes_disabled().map(|f| f.kind),
                    Some(kind),
                    "{context}: 状態に残る"
                );
                assert_eq!(
                    file_length(&target_path),
                    length_before,
                    "{context}: 前の長さへ切り詰められている"
                );
                let message = StoreError::WritesDisabled(failure.clone()).to_string();
                assert!(
                    message.starts_with(&format!("writes disabled ({}): ", kind.name())),
                    "{context}: {message}"
                );
                assert!(message.contains(failure.guidance()), "{context}: 案内が載る: {message}");
                assert_every_write_refused(&mut store, &acknowledged.objects[0].0, &context);
                assert_acknowledged_readable(&store, &acknowledged, &format!("{context}: 同じプロセス"));
                acknowledged
            };
            let store = Store::open(StoreConfig::new(&dir)).expect("reopen");
            assert!(store.writes_disabled().is_none(), "{context}: 開き直すと書ける(S1a)");
            assert_acknowledged_readable(&store, &acknowledged, &format!("{context}: 開き直した後"));
            assert!(
                !store.has_object(&uniqnode::c1::id_for_bytes(b"{\"failed\":1}")),
                "{context}: 失敗した put は残らない"
            );
            assert!(
                store.get_ref(&store.own_ref_name("notes/failed")).is_none(),
                "{context}: 失敗した ref は残らない"
            );
            assert_eq!(store.last_seq(), 3, "{context}: seq は応答済みの分だけ");
            drop(store);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// sync-keep(sync が失敗し切り詰めも失敗して、完全な 1 本がディスクに残る)の後、書けない
/// 間の export はその 1 本を返さない。開き直すと残った 1 本が適用され、応答済みのものは
/// 欠けず、次の自分のレコードの seq は重ならない。
#[test]
#[cfg(debug_assertions)]
fn sync_keep_withholds_the_unacknowledged_record_until_reopen() {
    let dir = temp_dir("sync-keep");
    let (acknowledged, node_id) = {
        let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
        let acknowledged = write_acknowledged(&mut store, 2);
        let node_id = store.node_id_hex().to_string();
        store.inject_fault("sync-keep@reflog:1").expect("inject");
        let failure = writes_disabled(
            store.set_ref("notes/kept", Some(&acknowledged.objects[0].0)),
            "sync-keep",
        );
        assert_eq!(failure.cleanup, Cleanup::Failed);
        assert_eq!(failure.kind, FailureKind::Io);
        assert_eq!(store.last_seq(), 2, "メモリの seq は進まない");
        let exported = store.export_ref_records(&node_id, 0).expect("export");
        assert_eq!(exported.len(), 2, "応答していない 1 本は export しない");
        (acknowledged, node_id)
    };
    let mut store = Store::open(StoreConfig::new(&dir)).expect("reopen");
    assert_acknowledged_readable(&store, &acknowledged, "開き直した後");
    assert_eq!(store.last_seq(), 3, "残った 1 本が適用される");
    assert!(store.get_ref(&store.own_ref_name("notes/kept")).is_some());
    let next = store.set_ref("notes/next", None).expect("set_ref");
    assert_eq!(next, 4, "seq は重ならない");
    assert_eq!(store.export_ref_records(&node_id, 0).expect("export").len(), 4);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 複製の受け側(ingest_ref_record)に sync-keep を掛けても、export はその 1 本を返さない。
#[test]
#[cfg(debug_assertions)]
fn the_replication_receiver_does_not_export_a_record_it_failed_to_sync() {
    let source_dir = temp_dir("receiver-source");
    let target_dir = temp_dir("receiver-target");
    let mut source = Store::open(StoreConfig::new(&source_dir)).expect("open source");
    let acknowledged = write_acknowledged(&mut source, 2);
    let signer = source.node_id_hex().to_string();
    let records = source.export_ref_records(&signer, 0).expect("export");
    assert_eq!(records.len(), 2);
    let mut target = Store::open(StoreConfig::new(&target_dir)).expect("open target");
    for (id, body) in &acknowledged.objects {
        let (put_id, _) = target.put_object(body).expect("put");
        assert_eq!(&put_id, id);
    }
    assert!(target.ingest_ref_record(&records[0]).expect("ingest"));
    target.inject_fault("sync-keep@reflog:1").expect("inject");
    writes_disabled(target.ingest_ref_record(&records[1]), "受け側の sync-keep");
    let exported = target.export_ref_records(&signer, 0).expect("export");
    assert_eq!(exported, vec![records[0].clone()], "sync に失敗した 1 本は流さない");
    drop(target);
    drop(source);
    let _ = std::fs::remove_dir_all(&source_dir);
    let _ = std::fs::remove_dir_all(&target_dir);
}

/// pack の torn の後、同じプロセスで既存のオブジェクトが正しい中身で読める(offset のずれた
/// 索引項目が生まれない)。
#[test]
#[cfg(debug_assertions)]
fn reads_stay_correct_after_a_torn_pack_append() {
    let dir = temp_dir("torn-reads");
    let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
    let mut stored = Vec::new();
    for i in 0..4 {
        let body = format!("object number {i} with some padding").into_bytes();
        let (id, _) = store.put_object(&body).expect("put");
        stored.push((id, body));
    }
    store.inject_fault("torn:11@pack:1").expect("inject");
    let torn_body = b"this one is torn after eleven bytes";
    writes_disabled(store.put_object(torn_body), "torn");
    assert!(!store.has_object(&uniqnode::c1::id_for_bytes(torn_body)));
    for (id, body) in &stored {
        assert_eq!(store.get_object(id).expect("get").as_deref(), Some(body.as_slice()));
    }
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 新しいセグメントの親の sync の失敗(dirsync)は io、op は dir_sync。
#[test]
#[cfg(debug_assertions)]
fn a_new_segment_directory_sync_failure_is_io() {
    let dir = temp_dir("dirsync");
    let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
    store.inject_fault("dirsync@pack:1").expect("inject");
    let failure = writes_disabled(store.put_object(b"first object"), "dirsync");
    assert_eq!(failure.kind, FailureKind::Io);
    assert_eq!(failure.op, WriteOp::DirSync);
    assert_eq!(failure.cleanup, Cleanup::NotTried);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 書く前のファイル長がメモリの数え値と違えば、書かずに書けない状態(op は length_mismatch、
/// errno 無し、kind は io)に入る。
#[test]
fn a_pack_length_mismatch_refuses_to_append() {
    let dir = temp_dir("length-mismatch");
    let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
    store.put_object(b"first object").expect("put");
    let pack = active_pack(&dir, &store);
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&pack).expect("open pack");
        file.write_all(b"xx").expect("append");
    }
    let length = file_length(&pack);
    let failure = writes_disabled(store.put_object(b"second object"), "length mismatch");
    assert_eq!(failure.op, WriteOp::LengthMismatch);
    assert_eq!(failure.errno, None);
    assert_eq!(failure.kind, FailureKind::Io);
    assert_eq!(file_length(&pack), length, "1 バイトも書かない");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 封印を起こすまで投入する。返り値は応答済みのオブジェクト。封印の直前(次の put が封印を
/// 起こす)で止める。
#[cfg(debug_assertions)]
fn fill_until_next_put_seals(store: &mut Store) -> Vec<(String, Vec<u8>)> {
    let mut stored = Vec::new();
    let mut i = 0;
    while store.write_cursor().offset < 64 {
        let body = format!("{{\"fill\":{i},\"pad\":\"0123456789abcdef0123456789\"}}").into_bytes();
        let (id, _) = store.put_object(&body).expect("put");
        stored.push((id, body));
        i += 1;
    }
    stored
}

/// manifest と manifest-dirsync を封印に掛けると、メモリの sealed_packs と追記中の番号は
/// 進まず、書けない状態(op は manifest)に入り、開き直したストアは一貫していて応答済みの
/// ものが全部読める。
#[test]
#[cfg(debug_assertions)]
fn a_manifest_failure_on_seal_keeps_memory_behind_disk_and_reopens_consistent() {
    for kind_text in ["manifest", "manifest-dirsync"] {
        let dir = temp_dir(&format!("seal-{kind_text}"));
        let stored = {
            let mut store = Store::open(small_config(&dir)).expect("open");
            let stored = fill_until_next_put_seals(&mut store);
            let sealed_before = store.sealed_pack_numbers().to_vec();
            let active_before = store.active_pack_number();
            store.inject_fault(&format!("{kind_text}:1")).expect("inject");
            let failure = writes_disabled(store.put_object(b"{\"would\":\"seal\"}"), kind_text);
            assert_eq!(failure.op, WriteOp::Manifest, "{kind_text}");
            assert_eq!(failure.kind, FailureKind::Io, "{kind_text}");
            assert_eq!(store.sealed_pack_numbers(), sealed_before.as_slice(), "{kind_text}");
            assert_eq!(store.active_pack_number(), active_before, "{kind_text}");
            let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap_or_default();
            if kind_text == "manifest-dirsync" {
                assert!(
                    manifest.contains(&format!("[{active_before}]"))
                        || manifest.contains(&format!(",{active_before}]")),
                    "rename の後の失敗なので MANIFEST は新しい中身: {manifest}"
                );
            } else {
                assert!(!manifest.contains(&active_before.to_string()), "{manifest}");
            }
            for (id, body) in &stored {
                assert_eq!(store.get_object(id).expect("get").as_deref(), Some(body.as_slice()));
            }
            stored
        };
        let store = Store::open(small_config(&dir)).expect("reopen");
        for (id, body) in &stored {
            assert_eq!(
                store.get_object(id).expect("get").as_deref(),
                Some(body.as_slice()),
                "{kind_text}: 開き直した後に読めない"
            );
        }
        let report = store.fsck().expect("fsck");
        assert!(report.errors.is_empty(), "{kind_text}: fsck: {:?}", report.errors);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 複数の pack に分かれる量を投入して ref を張り、その半分を上書きで孤児にする。返り値は
/// 生きているオブジェクト。
fn fill_with_garbage(store: &mut Store) -> Vec<(String, Vec<u8>)> {
    let mut live = Vec::new();
    for i in 0..12u32 {
        let body = format!("{{\"i\":{i:02},\"pad\":\"0123456789abcdef\"}}");
        let (id, _) = store.put_object(body.as_bytes()).expect("put");
        store.set_ref(&format!("notes/{i}"), Some(&id)).expect("set_ref");
        if i % 2 == 0 {
            let rewritten = format!("{{\"i\":{i:02},\"pad\":\"rewritten-0123456\"}}");
            let (new_id, _) = store.put_object(rewritten.as_bytes()).expect("put");
            store.set_ref(&format!("notes/{i}"), Some(&new_id)).expect("set_ref");
            live.push((new_id, rewritten.into_bytes()));
        } else {
            live.push((id, body.into_bytes()));
        }
    }
    live
}

fn run_gc(store: &Mutex<Store>) -> Result<uniqnode::gc::GcReport, StoreError> {
    uniqnode::gc::run(
        store,
        uniqnode::gc::GcOptions { threshold: uniqnode::gc::DEFAULT_THRESHOLD, dry_run: false },
    )
}

/// manifest と manifest-dirsync を gc_commit の C-3(新 pack を C-2 で据えた後)に掛けると、
/// 回収は書けない状態(op は gc_commit)で終わり、開き直した後に生きているオブジェクトが
/// 全部読めて fsck が緑である。
#[test]
#[cfg(debug_assertions)]
fn a_manifest_failure_in_gc_commit_loses_no_live_object() {
    for kind_text in ["manifest", "manifest-dirsync"] {
        let dir = temp_dir(&format!("gc-commit-{kind_text}"));
        let live = {
            let mut store = Store::open(small_config(&dir)).expect("open");
            let live = fill_with_garbage(&mut store);
            // S の封印が MANIFEST を 1 回書く(追記中の pack が空でなければ)。C-3 はその次。
            let nth = if store.write_cursor().offset > 0 { 2 } else { 1 };
            store.inject_fault(&format!("{kind_text}:{nth}")).expect("inject");
            let store = Mutex::new(store);
            let failure = writes_disabled(run_gc(&store), kind_text);
            assert_eq!(failure.op, WriteOp::GcCommit, "{kind_text}");
            assert_eq!(failure.kind, FailureKind::Io, "{kind_text}");
            let store = store.into_inner().expect("lock");
            for (id, body) in &live {
                assert_eq!(
                    store.get_object(id).expect("get").as_deref(),
                    Some(body.as_slice()),
                    "{kind_text}: 同じプロセスで読めない"
                );
            }
            live
        };
        let store = Store::open(small_config(&dir)).expect("reopen");
        for (id, body) in &live {
            assert_eq!(
                store.get_object(id).expect("get").as_deref(),
                Some(body.as_slice()),
                "{kind_text}: 開き直した後に読めない"
            );
        }
        let report = store.fsck().expect("fsck");
        assert!(report.errors.is_empty(), "{kind_text}: fsck: {:?}", report.errors);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 封印済みの pack 1 本が丸ごと孤児で、他の pack に孤児が無いストアを作る(回収は新 pack を
/// 作らずに pack 1 を消すだけになる)。返り値は生きているオブジェクト。
fn store_with_one_garbage_pack(store: &mut Store) -> Vec<(String, Vec<u8>)> {
    let orphan = b"{\"orphan\":\"0123456789abcdef0123456789abcdef0123456789abcdef\"}".to_vec();
    let (orphan_id, _) = store.put_object(&orphan).expect("put");
    store.set_ref("notes/x", Some(&orphan_id)).expect("set_ref");
    let live_body = b"{\"live\":\"0123456789abcdef0123456789abcdef0123456789\"}".to_vec();
    let (live_id, _) = store.put_object(&live_body).expect("put");
    store.set_ref("notes/x", Some(&live_id)).expect("set_ref");
    assert_eq!(store.sealed_pack_numbers(), &[1], "孤児だけの pack 1 が封印済み");
    vec![(live_id, live_body)]
}

/// GC の B の tmp の書き込みの失敗(tmp の名にディレクトリを置いて作れなくする)は、
/// 書けない状態を立てない。
#[test]
fn a_gc_tmp_write_failure_does_not_disable_writes() {
    let dir = temp_dir("gc-b");
    let mut store = Store::open(small_config(&dir)).expect("open");
    fill_with_garbage(&mut store);
    let blocker = dir
        .join("tmp")
        .join(format!("{}{}.pack", uniqnode::store::GC_TMP_PREFIX, std::process::id()));
    std::fs::create_dir(&blocker).expect("mkdir blocker");
    let store = Mutex::new(store);
    match run_gc(&store) {
        Err(StoreError::Io(_)) => {}
        other => panic!("B の失敗は Io のはずが {other:?}"),
    }
    let mut store = store.into_inner().expect("lock");
    assert!(store.writes_disabled().is_none(), "B の失敗は書けない状態を立てない");
    std::fs::remove_dir(&blocker).expect("rmdir blocker");
    store.put_object(b"still writable").expect("put");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// GC の D の削除の失敗(packs/ を 0555 にして unlink を断らせる)は書けない状態を立てない。
#[test]
fn a_gc_d_unlink_failure_does_not_disable_writes() {
    use std::os::unix::fs::PermissionsExt;
    if !not_root() {
        return;
    }
    let dir = temp_dir("gc-d-unlink");
    let mut store = Store::open(small_config(&dir)).expect("open");
    let live = store_with_one_garbage_pack(&mut store);
    let packs = dir.join("packs");
    std::fs::set_permissions(&packs, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let store = Mutex::new(store);
    let outcome = run_gc(&store);
    std::fs::set_permissions(&packs, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
    match outcome {
        Err(StoreError::Io(_)) => {}
        other => panic!("D の削除の失敗は Io のはずが {other:?}"),
    }
    let mut store = store.into_inner().expect("lock");
    assert!(store.writes_disabled().is_none(), "D の削除の失敗は書けない状態を立てない");
    store.put_object(b"still writable after D").expect("put");
    for (id, body) in &live {
        assert_eq!(store.get_object(id).expect("get").as_deref(), Some(body.as_slice()));
    }
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// GC の D の packs/ の sync の失敗(gc-dirsync)は、kind が io・op が dir_sync の書けない状態を
/// 立てる。
#[test]
#[cfg(debug_assertions)]
fn a_gc_d_directory_sync_failure_disables_writes() {
    let dir = temp_dir("gc-d-dirsync");
    let mut store = Store::open(small_config(&dir)).expect("open");
    let live = store_with_one_garbage_pack(&mut store);
    store.inject_fault("gc-dirsync:1").expect("inject");
    let store = Mutex::new(store);
    let failure = writes_disabled(run_gc(&store), "gc-dirsync");
    assert_eq!(failure.kind, FailureKind::Io);
    assert_eq!(failure.op, WriteOp::DirSync);
    let mut store = store.into_inner().expect("lock");
    assert!(store.writes_disabled().is_some());
    writes_disabled(store.put_object(b"refused"), "gc-dirsync の後");
    for (id, body) in &live {
        assert_eq!(store.get_object(id).expect("get").as_deref(), Some(body.as_slice()));
    }
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 書けない状態の間、回収は dry-run も含めて断られる(gc_try_begin が書き込みの入口)。
#[test]
#[cfg(debug_assertions)]
fn gc_is_refused_while_writes_are_disabled() {
    let dir = temp_dir("gc-refused");
    let mut store = Store::open(small_config(&dir)).expect("open");
    fill_with_garbage(&mut store);
    store.inject_fault("before:1").expect("inject");
    writes_disabled(store.put_object(b"fails"), "before");
    let store = Mutex::new(store);
    writes_disabled(run_gc(&store), "gc");
    writes_disabled(
        uniqnode::gc::run(
            &store,
            uniqnode::gc::GcOptions { threshold: uniqnode::gc::DEFAULT_THRESHOLD, dry_run: true },
        ),
        "dry-run",
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 書けない状態は引数の検査より先に言う: 不正な ref の道・存在しない target・存在しない root の
/// pin と held の表明・空の追記中 pack の封印も、Invalid や Ok(None) でなく WritesDisabled を
/// 受け取る(HTTP では 400 でなく 503 と復旧の案内になる)。
#[test]
#[cfg(debug_assertions)]
fn writes_disabled_comes_before_argument_checks_and_the_empty_pack_shortcut() {
    let dir = temp_dir("disabled-first");
    let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
    store.inject_fault("before@reflog:1").expect("inject");
    writes_disabled(store.set_ref("notes/first", None), "最初の失敗");
    assert_eq!(store.write_cursor().offset, 0, "追記中の pack は空のまま");
    let missing = uniqnode::c1::id_for_bytes(b"never stored");
    writes_disabled(store.set_ref("", None), "空の ref の道");
    writes_disabled(store.set_ref("/absolute", None), "/ で始まる ref の道");
    writes_disabled(store.set_ref("notes/missing", Some(&missing)), "存在しない target");
    writes_disabled(store.set_pin(&missing, 1), "存在しない root の pin");
    writes_disabled(store.set_attest(&missing, true), "存在しない root の held");
    writes_disabled(store.seal_active_pack_for_gc(), "空の pack の封印");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 開く道は祖先を作らない: 親の無い道は理由を言って断り、データのディレクトリも作らない。
#[test]
fn opening_a_store_whose_parent_is_missing_creates_nothing() {
    let base = temp_dir("no-parent");
    let dir = base.join("missing").join("store");
    match Store::open(StoreConfig::new(&dir)) {
        Err(StoreError::Invalid(message)) => {
            assert!(message.contains("親"), "{message}");
        }
        Err(other) => panic!("Invalid のはずが {other:?}"),
        Ok(_) => panic!("親の無い道を開いた"),
    }
    assert!(!base.exists(), "祖先を作らない");
}

/// データのディレクトリの親を開けない(0333: 書けて辿れるが読めない)なら、sync できないので
/// 開くことは理由を言って失敗する。
#[test]
fn opening_fails_when_the_parent_cannot_be_synced() {
    use std::os::unix::fs::PermissionsExt;
    if !not_root() {
        return;
    }
    let parent = temp_dir("unreadable-parent");
    std::fs::create_dir(&parent).expect("mkdir parent");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o333)).expect("chmod");
    let outcome = Store::open(StoreConfig::new(parent.join("store")));
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
    match outcome {
        Err(StoreError::Io(error)) => {
            let message = error.to_string();
            assert!(message.contains(&parent.display().to_string()), "{message}");
            assert!(message.contains("開くときの sync"), "{message}");
        }
        Err(other) => panic!("Io のはずが {other:?}"),
        Ok(_) => panic!("親を sync できないのに開いた"),
    }
    let _ = std::fs::remove_dir_all(&parent);
}

/// CLI を環境変数つきで走らせ、標準入力を渡す。返り値は (終了コード(シグナルなら None), 標準出力)。
#[cfg(debug_assertions)]
fn uniqnode_with(arguments: &[&str], envs: &[(&str, &str)], stdin: &[u8]) -> (Option<i32>, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut command = Command::new(env!("CARGO_BIN_EXE_uniqnode"));
    command.args(arguments).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn uniqnode");
    child.stdin.take().expect("stdin").write_all(stdin).expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// 開くときの sync の試験の場所: 親(base)の下のストア(base/store)と、sync の記録(base/sync.log)。
#[cfg(debug_assertions)]
struct OpenSyncFixture {
    base: PathBuf,
    dir: PathBuf,
    log: PathBuf,
}

#[cfg(debug_assertions)]
impl OpenSyncFixture {
    fn new(name: &str) -> OpenSyncFixture {
        let base = temp_dir(name);
        std::fs::create_dir(&base).expect("mkdir base");
        let dir = base.join("store");
        let log = base.join("sync.log");
        OpenSyncFixture { base, dir, log }
    }

    fn dir_text(&self) -> &str {
        self.dir.to_str().expect("utf-8")
    }

    /// 記録を取らずに CLI を走らせる(据え物を作る段)。
    fn run(&self, arguments: &[&str], envs: &[(&str, &str)], stdin: &[u8]) -> (Option<i32>, String, String) {
        uniqnode_with(arguments, envs, stdin)
    }

    /// 記録を空にしてから CLI を走らせ、sync の記録と標準出力(応答)を同じ記録のファイルへ
    /// O_APPEND で書かせる。行の順が、sync と応答の起きた順である(応答の行は println の
    /// 改行で書き出されるので、遅れて書かれることはあっても先に書かれることは無い)。
    /// 返り値は (終了コード, 記録の行, 標準エラー)。
    fn run_logged(&self, arguments: &[&str], stdin: &[u8]) -> (Option<i32>, Vec<String>, String) {
        self.run_logged_with(arguments, &[], stdin)
    }

    /// run_logged に環境変数を足す形。
    fn run_logged_with(
        &self,
        arguments: &[&str],
        envs: &[(&str, &str)],
        stdin: &[u8],
    ) -> (Option<i32>, Vec<String>, String) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let _ = std::fs::remove_file(&self.log);
        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .expect("open log for stdout");
        let mut child = Command::new(env!("CARGO_BIN_EXE_uniqnode"))
            .args(arguments)
            .env(uniqnode::store::SYNC_LOG_ENV, &self.log)
            .envs(envs.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn uniqnode");
        child.stdin.take().expect("stdin").write_all(stdin).expect("write stdin");
        let output = child.wait_with_output().expect("wait");
        let lines =
            std::fs::read_to_string(&self.log).expect("read log").lines().map(str::to_string).collect();
        (output.status.code(), lines, String::from_utf8_lossy(&output.stderr).to_string())
    }

    /// 開くたびに sync する名前(node_key の中身と、packs/・reflog/・データのディレクトリ・その親)。
    fn names_synced_on_every_open(&self) -> Vec<String> {
        vec![
            format!("file {}", self.dir.join("node_key").display()),
            format!("dir {}", self.dir.join("packs").display()),
            format!("dir {}", self.dir.join("reflog").display()),
            format!("dir {}", self.dir.display()),
            format!("dir {}", self.base.display()),
        ]
    }
}

#[cfg(debug_assertions)]
impl Drop for OpenSyncFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// `expected` の各行が記録にあり、その最初の位置が応答の行(`ack` で始まる最初の行)より前で
/// あることを言う。
#[cfg(debug_assertions)]
fn assert_synced_before_ack(lines: &[String], expected: &[String], ack: &str, context: &str) {
    let ack_position = lines
        .iter()
        .position(|line| line.starts_with(ack))
        .unwrap_or_else(|| panic!("{context}: 応答 {ack} が記録に無い: {lines:#?}"));
    for wanted in expected {
        let position = lines
            .iter()
            .position(|line| line == wanted)
            .unwrap_or_else(|| panic!("{context}: {wanted} が sync されていない: {lines:#?}"));
        assert!(
            position < ack_position,
            "{context}: {wanted} の sync({position} 行目)が応答({ack_position} 行目)より後: {lines:#?}"
        );
    }
}

/// 開くたびに、node_key・追記中の pack・reflog の全部の中身と、packs/・reflog/・データの
/// ディレクトリ・その親の名前を、応答より前に sync する(「既に在る」ことを理由に省かない)。
/// reflog を持つストアを開き直す put は reflog に書かないので、reflog の中身の sync の行は
/// 開くときの sync からしか来ない。新しい pack の最初の追記の、ファイルの sync の後・親の
/// sync の前で落とした(crash-before-dirsync)ストアを開き直して再送しても同じである。
#[test]
#[cfg(debug_assertions)]
fn every_open_syncs_contents_and_names_before_the_acknowledgement() {
    let fixture = OpenSyncFixture::new("open-sync");
    let dir_text = fixture.dir_text();
    let (status, lines, stderr) = fixture.run_logged(&["init", dir_text], b"");
    assert_eq!(status, Some(0), "init: {stderr}");
    assert_synced_before_ack(&lines, &fixture.names_synced_on_every_open(), "node_id: ", "init");

    // reflog を持つストアにする(set-ref を 2 回)。
    let first = b"{\"first\":\"object\"}";
    let (status, stdout, stderr) = fixture.run(&["put", dir_text], &[], first);
    assert_eq!(status, Some(0), "put: {stderr}");
    let first_id = uniqnode::c1::id_for_bytes(first);
    assert!(stdout.contains(&first_id), "{stdout}");
    for path in ["notes/a", "notes/b"] {
        let (status, _, stderr) = fixture.run(&["set-ref", dir_text, path, &first_id], &[], b"");
        assert_eq!(status, Some(0), "set-ref: {stderr}");
    }
    let reflogs: Vec<PathBuf> = std::fs::read_dir(fixture.dir.join("reflog"))
        .expect("read_dir reflog")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert!(!reflogs.is_empty(), "reflog がある");
    let pack = fixture.dir.join("packs").join("pack-000001.pack");

    let second = b"{\"second\":\"object\"}";
    let (status, lines, stderr) = fixture.run_logged(&["put", dir_text], second);
    assert_eq!(status, Some(0), "reflog を持つストアへの put: {stderr}");
    let mut expected = fixture.names_synced_on_every_open();
    expected.push(format!("file {}", pack.display()));
    expected.extend(reflogs.iter().map(|path| format!("file {}", path.display())));
    assert_synced_before_ack(&lines, &expected, &uniqnode::c1::id_for_bytes(second), "reflog を持つストア");

    // crash-before-dirsync: ファイルの sync の後・packs/ の sync の前で落とした新しい pack。
    let fresh = OpenSyncFixture::new("open-sync-dirsync");
    let fresh_text = fresh.dir_text();
    let (status, _, stderr) = fresh.run(&["init", fresh_text], &[], b"");
    assert_eq!(status, Some(0), "init: {stderr}");
    let body = b"{\"crash\":\"before the directory sync\"}";
    let (status, _, stderr) = fresh.run(
        &["put", fresh_text],
        &[(uniqnode::fault::APPEND_FAULT_ENV, "crash-before-dirsync@pack:1")],
        body,
    );
    assert_eq!(status, None, "abort で落ちる: {stderr}");
    let fresh_pack = fresh.dir.join("packs").join("pack-000001.pack");
    assert!(fresh_pack.exists(), "ファイルの sync までは済んでいる");
    let (status, lines, stderr) = fresh.run_logged(&["put", fresh_text], body);
    assert_eq!(status, Some(0), "再送: {stderr}");
    let mut expected = fresh.names_synced_on_every_open();
    expected.push(format!("file {}", fresh_pack.display()));
    assert_synced_before_ack(&lines, &expected, &uniqnode::c1::id_for_bytes(body), "crash-before-dirsync の後");
}

/// 追記を全部 write した後・ファイルの sync の前で落とした(crash-before-sync)ストアでは、
/// 完全な 1 本が page cache にだけある。開き直すと recover がその 1 本を採用し、開くときの
/// sync がその pack の中身を永続させてから、同じものの再送に「既に在る」と応答する。
#[test]
#[cfg(debug_assertions)]
fn a_record_written_but_not_synced_is_recovered_synced_and_then_resent() {
    let fixture = OpenSyncFixture::new("open-sync-before-sync");
    let dir_text = fixture.dir_text();
    let (status, _, stderr) = fixture.run(&["init", dir_text], &[], b"");
    assert_eq!(status, Some(0), "init: {stderr}");
    let body = b"{\"crash\":\"after write before sync\"}";
    let (status, _, stderr) = fixture.run(
        &["put", dir_text],
        &[(uniqnode::fault::APPEND_FAULT_ENV, "crash-before-sync@pack:1")],
        body,
    );
    assert_eq!(status, None, "abort で落ちる: {stderr}");
    assert!(stderr.contains("crash-before-sync"), "{stderr}");
    let pack = fixture.dir.join("packs").join("pack-000001.pack");
    assert_eq!(
        file_length(&pack),
        8 + body.len() as u64,
        "完全な 1 本(8 バイトの頭と本体)が write 済みで残る"
    );

    let id = uniqnode::c1::id_for_bytes(body);
    let (status, lines, stderr) = fixture.run_logged(&["put", dir_text], body);
    assert_eq!(status, Some(0), "再送: {stderr}");
    assert!(
        lines.contains(&format!("{id} (existing)")),
        "recover が完全な 1 本を採用したので、再送は既に在ると答える: {lines:#?}"
    );
    let mut expected = fixture.names_synced_on_every_open();
    expected.push(format!("file {}", pack.display()));
    assert_synced_before_ack(&lines, &expected, &id, "crash-before-sync の後");
}

/// packs/ と reflog/ の全部のファイルの中身(道 → バイト列)。
#[cfg(debug_assertions)]
fn segment_contents(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut contents = std::collections::BTreeMap::new();
    for directory in ["packs", "reflog"] {
        for entry in std::fs::read_dir(dir.join(directory)).expect("read_dir") {
            let path = entry.expect("entry").path();
            let bytes = std::fs::read(&path).expect("read segment");
            contents.insert(path, bytes);
        }
    }
    contents
}

/// 開くときの sync の 1 つずつ(node_key・追記中の pack・reflog・packs/・reflog/・データの
/// ディレクトリ・その親)を失敗させる(open-sync:<n>)と、開くことが失敗する(APPEND_FAILURE の
/// 方針 1a: 開くときの sync の失敗は開くことの失敗)。命令は理由を言って 1 で終わり、応答を
/// 出さず、pack と reflog は 1 バイトも変わらない。sync の記録は成功した sync の後にだけ書かれる
/// ので、記録は失敗させた sync の手前で止まり、失敗させた名前もその後の名前も載らない。
/// 注入を外して開き直すと、同じ put を受け付ける。
#[test]
#[cfg(debug_assertions)]
fn a_failed_sync_on_open_fails_the_open_and_writes_nothing() {
    let fixture = OpenSyncFixture::new("open-sync-fails");
    let dir_text = fixture.dir_text();
    let (status, _, stderr) = fixture.run(&["init", dir_text], &[], b"");
    assert_eq!(status, Some(0), "init: {stderr}");
    let first = b"{\"first\":\"object\"}";
    let (status, _, stderr) = fixture.run(&["put", dir_text], &[], first);
    assert_eq!(status, Some(0), "put: {stderr}");
    let first_id = uniqnode::c1::id_for_bytes(first);
    let (status, _, stderr) = fixture.run(&["set-ref", dir_text, "notes/a", &first_id], &[], b"");
    assert_eq!(status, Some(0), "set-ref: {stderr}");

    // 注入なしの開き直し(既に在るものの put は何も書かない)で、開くときの sync の全部と順を読む。
    let (status, lines, stderr) = fixture.run_logged(&["put", dir_text], first);
    assert_eq!(status, Some(0), "既に在るものの put: {stderr}");
    let ack = lines
        .iter()
        .position(|line| line.starts_with(&first_id))
        .unwrap_or_else(|| panic!("応答が記録に無い: {lines:#?}"));
    let open_syncs = lines[..ack].to_vec();
    // node_key・pack・reflog の中身と、packs/・reflog/・データのディレクトリ・その親。
    assert_eq!(open_syncs.len(), 7, "{open_syncs:#?}");
    let before = segment_contents(&fixture.dir);

    let body = b"{\"refused\":\"while the open fails\"}";
    for n in 1..=open_syncs.len() {
        let spec = format!("open-sync:{n}");
        let (status, lines, stderr) = fixture.run_logged_with(
            &["put", dir_text],
            &[(uniqnode::fault::APPEND_FAULT_ENV, &spec)],
            body,
        );
        assert_eq!(status, Some(1), "{spec}: 開くことが失敗する: {stderr}");
        assert!(stderr.contains("開くときの sync"), "{spec}: 理由を言う: {stderr}");
        assert_eq!(
            lines,
            open_syncs[..n - 1].to_vec(),
            "{spec}: 記録は成功した sync だけで、失敗させた sync の手前で止まり、応答は無い"
        );
        assert_eq!(segment_contents(&fixture.dir), before, "{spec}: 何も書かない");
    }

    let (status, stdout, stderr) = fixture.run(&["put", dir_text], &[], body);
    assert_eq!(status, Some(0), "注入を外した開き直し: {stderr}");
    assert!(stdout.contains(&uniqnode::c1::id_for_bytes(body)), "{stdout}");
}

/// node_key を作る途中(tmp/ に書いて sync した後・rename の前)で落としても、短い鍵や空の鍵が
/// node_key の名で残らない。開き直すと tmp/ の残骸を消して鍵を作り直し、その中身とデータの
/// ディレクトリを応答より前に sync する。
#[test]
#[cfg(debug_assertions)]
fn a_stop_in_the_middle_of_node_key_creation_leaves_no_key_and_reopens_synced() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = OpenSyncFixture::new("open-sync-node-key");
    let dir_text = fixture.dir_text();
    let (status, _, stderr) = fixture.run(
        &["init", dir_text],
        &[(uniqnode::fault::APPEND_FAULT_ENV, "crash-in-node-key:1")],
        b"",
    );
    assert_eq!(status, None, "abort で落ちる: {stderr}");
    assert!(stderr.contains("crash-in-node-key"), "{stderr}");
    let key = fixture.dir.join("node_key");
    assert!(!key.exists(), "rename の前に落ちたので node_key は無い");
    let leftovers: Vec<PathBuf> = std::fs::read_dir(fixture.dir.join("tmp"))
        .expect("read_dir tmp")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(leftovers.len(), 1, "作りかけの鍵が tmp/ に残る: {leftovers:?}");
    assert_eq!(file_length(&leftovers[0]), 32);

    let (status, lines, stderr) = fixture.run_logged(&["init", dir_text], b"");
    assert_eq!(status, Some(0), "開き直し: {stderr}");
    assert_eq!(file_length(&key), 32);
    let mode = std::fs::metadata(&key).expect("metadata").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let remaining: Vec<_> = std::fs::read_dir(fixture.dir.join("tmp")).expect("read_dir").collect();
    assert!(remaining.is_empty(), "tmp/ の残骸は消える: {remaining:?}");
    // 作り直した鍵の tmp の中身の sync は、データのディレクトリの sync(rename の永続)より前。
    let tmp_key_sync = lines
        .iter()
        .position(|line| line.starts_with(&format!("file {}", fixture.dir.join("tmp").display())))
        .unwrap_or_else(|| panic!("作り直した鍵の tmp の中身を sync していない: {lines:#?}"));
    let dir_sync = format!("dir {}", fixture.dir.display());
    let first_dir_sync = lines.iter().position(|line| line == &dir_sync).expect("データのディレクトリの sync");
    assert!(tmp_key_sync < first_dir_sync, "{lines:#?}");
    assert_synced_before_ack(&lines, &fixture.names_synced_on_every_open(), "node_id: ", "鍵の作り直し");
}

/// node_key は 0600 で作られ、作りかけの tmp を残さない。
#[test]
fn the_node_key_is_created_private_through_tmp() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("node-key");
    let store = Store::open(StoreConfig::new(&dir)).expect("open");
    let mode = std::fs::metadata(dir.join("node_key")).expect("metadata").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(file_length(&dir.join("node_key")), 32);
    let leftovers: Vec<_> = std::fs::read_dir(dir.join("tmp")).expect("read_dir").collect();
    assert!(leftovers.is_empty(), "tmp/ に残骸がある: {leftovers:?}");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
