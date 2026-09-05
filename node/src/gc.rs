//! pack の物理回収(GC)の計画(`uniqnode gc <dir> --dry-run`)。
//!
//! ここにあるのは「何が生きていて、pack ごとにどれだけが孤児か」を数える側だけである。
//! 生きているオブジェクトの定義と出力の読み方は
//! [docs/design/GC.md](uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)、ディスクを書き直す
//! 回収そのものは未実装で、その設計は
//! [docs/plan/PACK_GC.md](uuid:f272eeda-8664-42da-9e5c-ef354bc3f3a7) にある。
//!
//! 何も書かない。ストアを読むだけで、MANIFEST・pack・reflog・tmp のどれにも触れない。
//! 到達閉包の計算は Store::reachable_closure_of_roots であり、参照の規約(SPEC §4.3)と
//! dangling の扱いをここで二重に決めない(should/0135)。

use crate::store::{Result, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// 孤児率がこれを超えた封印済み pack を回収の対象と見る、閾値の既定。根拠はまだ無く、
/// dry-run の実測で決める(PACK_GC (uuid:f272eeda-8664-42da-9e5c-ef354bc3f3a7) の
/// セルフレビュー 1)。
pub const DEFAULT_THRESHOLD: f64 = 0.25;

/// pack 1 本の集計。bytes は索引に載ったオブジェクトのペイロード合計で、used_bytes と同じ
/// 会計(レコードの 8 バイトの頭と、重複追記の 2 つ目以降は入らない)。
#[derive(Clone, Debug, PartialEq)]
pub struct PackPlan {
    pub number: u64,
    /// MANIFEST が封印済みと言う pack か。封印済みでない(追記中の)pack は対象にしない。
    pub sealed: bool,
    pub objects: usize,
    pub bytes: u64,
    pub live_objects: usize,
    pub live_bytes: u64,
    /// 孤児率が閾値を超え、かつ封印済みなので、回収の対象になる pack か。
    pub compact: bool,
}

impl PackPlan {
    pub fn garbage_objects(&self) -> usize {
        self.objects - self.live_objects
    }

    pub fn garbage_bytes(&self) -> u64 {
        self.bytes - self.live_bytes
    }

    /// 孤児のバイト数 / pack のバイト数。空の pack は 0。
    pub fn garbage_ratio(&self) -> f64 {
        garbage_ratio(self.garbage_bytes(), self.bytes)
    }
}

/// 孤児のバイト数 / 全体のバイト数。全体が 0 なら 0(空の pack に孤児は無い)。
pub fn garbage_ratio(garbage_bytes: u64, bytes: u64) -> f64 {
    if bytes == 0 {
        0.0
    } else {
        garbage_bytes as f64 / bytes as f64
    }
}

/// 孤児率が閾値を超えているか。境界は含めない: ちょうど閾値の pack は対象にならず、
/// 閾値 0 は「孤児が 1 バイトでもあれば対象」を意味する(孤児の無い pack はどんな閾値でも
/// 対象にならない)。対象の判定はここ 1 箇所にある(should/0135)。
pub fn exceeds_threshold(garbage_bytes: u64, bytes: u64, threshold: f64) -> bool {
    garbage_ratio(garbage_bytes, bytes) > threshold
}

/// 1 回の dry-run で分かったこと。命令の出力はこれをそのまま並べる。
#[derive(Clone, Debug)]
pub struct GcPlan {
    pub threshold: f64,
    /// pack 番号の昇順。封印済みの全部と、追記中の 1 本。
    pub packs: Vec<PackPlan>,
    /// 根の数(target が null でない ref の target、pin の root、held=true の attest の
    /// root の和集合。ローカルに無いものも数える)。
    pub roots: usize,
    pub objects: usize,
    pub live_objects: usize,
    /// 根を集めて閉包を辿り終えるまでの時間。回収の手順 A・C がロックを持つ長さの実測
    /// (PACK_GC のセルフレビュー 6)。
    pub live_set_elapsed: Duration,
}

impl GcPlan {
    pub fn garbage_objects(&self) -> usize {
        self.objects - self.live_objects
    }

    pub fn garbage_bytes(&self) -> u64 {
        self.packs.iter().map(PackPlan::garbage_bytes).sum()
    }

    pub fn sealed_packs(&self) -> usize {
        self.packs.iter().filter(|pack| pack.sealed).count()
    }

    pub fn compact_packs(&self) -> usize {
        self.packs.iter().filter(|pack| pack.compact).count()
    }

    pub fn compact_bytes(&self) -> u64 {
        self.packs
            .iter()
            .filter(|pack| pack.compact)
            .map(PackPlan::garbage_bytes)
            .sum()
    }
}

/// 根の集合(GC.md の定義)。全署名者の ref のうち target が null でないものの target、
/// 全 pin の root、held=true の保持表明の root。
fn roots(store: &Store) -> BTreeSet<String> {
    let mut roots: BTreeSet<String> = store
        .list_refs()
        .filter_map(|(_, state)| state.target.clone())
        .collect();
    roots.extend(store.effective_pins().into_keys());
    roots.extend(store.held_attested_roots());
    roots
}

/// 生きている集合を計算し、pack ごとの孤児率と閾値の判定まで出す。読むだけで何も書かない。
pub fn plan(store: &Store, threshold: f64) -> Result<GcPlan> {
    let started = Instant::now();
    let roots = roots(store);
    let live = store.reachable_closure_of_roots(roots.iter().map(String::as_str))?;
    let live_set_elapsed = started.elapsed();

    let empty_pack = |number: u64| PackPlan {
        number,
        sealed: store.sealed_pack_numbers().contains(&number),
        objects: 0,
        bytes: 0,
        live_objects: 0,
        live_bytes: 0,
        compact: false,
    };
    // 封印済みの全部と追記中の 1 本は、オブジェクトが 1 つも無くても行に出す。
    let mut by_pack: BTreeMap<u64, PackPlan> = store
        .sealed_pack_numbers()
        .iter()
        .copied()
        .chain(std::iter::once(store.active_pack_number()))
        .map(|number| (number, empty_pack(number)))
        .collect();
    for (id, location) in store.object_locations() {
        let pack = by_pack
            .entry(location.pack_number)
            .or_insert_with(|| empty_pack(location.pack_number));
        pack.objects += 1;
        pack.bytes += location.payload_length as u64;
        if live.contains(id) {
            pack.live_objects += 1;
            pack.live_bytes += location.payload_length as u64;
        }
    }
    let mut packs: Vec<PackPlan> = by_pack.into_values().collect();
    for pack in &mut packs {
        pack.compact =
            pack.sealed && exceeds_threshold(pack.garbage_bytes(), pack.bytes, threshold);
    }
    Ok(GcPlan {
        threshold,
        packs,
        roots: roots.len(),
        objects: store.object_count(),
        live_objects: live.len(),
        live_set_elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreConfig;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("uniqnode-gc-unit-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        dir
    }

    /// 封印を起こさない(1 pack だけ)設定。根の定義のテストは pack の分かれ方を見ない。
    fn one_pack(dir: &Path) -> Store {
        Store::open(StoreConfig::new(dir)).expect("open")
    }

    /// 生きている ID の集合(孤児は index にあって L に無いもの)を、計画から読み戻す
    /// 代わりに、計画の数で言う: 孤児の数とバイト数。
    fn garbage_of(store: &Store) -> (usize, u64) {
        let plan = plan(store, DEFAULT_THRESHOLD).expect("plan");
        (plan.garbage_objects(), plan.garbage_bytes())
    }

    /// 同じ ref パスへの上書き(再取り込み): 旧 target は孤児、新 target は生きている。
    #[test]
    fn overwriting_a_ref_orphans_the_old_target_and_keeps_the_new_one() {
        let dir = temp_dir("overwrite");
        let mut store = one_pack(&dir);
        let (old, _) = store.put_object(b"\"version 1 of the note\"").expect("put");
        store.set_ref("notes/x", Some(&old)).expect("set_ref");
        assert_eq!(garbage_of(&store), (0, 0), "上書き前は全部生きている");
        let (new, _) = store
            .put_object(b"\"version 2 of the note!\"")
            .expect("put");
        store.set_ref("notes/x", Some(&new)).expect("set_ref");
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(plan.objects, 2);
        assert_eq!(plan.live_objects, 1, "新しい target だけが生きている");
        assert_eq!(plan.garbage_objects(), 1);
        assert_eq!(
            plan.garbage_bytes(),
            b"\"version 1 of the note\"".len() as u64,
            "孤児のバイト数は旧 target の大きさ"
        );
        assert_eq!(plan.roots, 1, "根は現在の target 1 つ");
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// tombstone(target=null)にすると、指していたものは孤児になる。
    #[test]
    fn a_tombstone_orphans_its_former_target() {
        let dir = temp_dir("tombstone");
        let mut store = one_pack(&dir);
        let (id, _) = store.put_object(b"\"to be forgotten\"").expect("put");
        store.set_ref("notes/gone", Some(&id)).expect("set_ref");
        assert_eq!(garbage_of(&store), (0, 0));
        store.set_ref("notes/gone", None).expect("tombstone");
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(plan.roots, 0, "null の target は根にならない");
        assert_eq!(plan.live_objects, 0);
        assert_eq!(
            (plan.garbage_objects(), plan.garbage_bytes()),
            (1, b"\"to be forgotten\"".len() as u64)
        );
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// put_object の後に set_ref に至らなかった残骸は、最初から孤児である。
    #[test]
    fn an_object_that_never_got_a_ref_is_garbage() {
        let dir = temp_dir("leftover");
        let mut store = one_pack(&dir);
        let (kept, _) = store.put_object(b"\"kept\"").expect("put");
        store.set_ref("notes/kept", Some(&kept)).expect("set_ref");
        store.put_object(b"\"never named by a ref\"").expect("put");
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(plan.objects, 2);
        assert_eq!(plan.live_objects, 1);
        assert_eq!(
            plan.garbage_bytes(),
            b"\"never named by a ref\"".len() as u64
        );
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 他ノードの ref が指すものは、自分の ref が無くても生きている(機会層の複製)。
    /// 他ノードの署名済みレコードは、別のストアで発行して export し、ingest_ref_record で
    /// 入れる(同期の受け側と同じ道)。
    #[test]
    fn an_object_named_only_by_a_foreign_ref_is_alive() {
        let dir_mine = temp_dir("foreign-mine");
        let dir_other = temp_dir("foreign-other");
        let body = b"{\"kind\":\"node\",\"replica\":true,\"v\":1}";
        let records = {
            let mut other = Store::open(StoreConfig::new(&dir_other)).expect("open other");
            let (id, _) = other.put_object(body).expect("put");
            other.set_ref("notes/theirs", Some(&id)).expect("set_ref");
            other
                .export_ref_records(other.node_id_hex(), 0)
                .expect("export")
        };
        let mut mine = one_pack(&dir_mine);
        // 複製として同じバイト列を持っているが、自分の ref は張っていない。
        mine.put_object(body).expect("put replica");
        assert_eq!(
            garbage_of(&mine),
            (1, body.len() as u64),
            "ref を入れる前は孤児"
        );
        for record in &records {
            assert!(mine.ingest_ref_record(record).expect("ingest"));
        }
        let plan = plan(&mine, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(plan.roots, 1, "他ノードの ref の target が根");
        assert_eq!(plan.live_objects, 1, "他ノードの ref だけで生きている");
        assert_eq!(plan.garbage_objects(), 0);
        drop(mine);
        std::fs::remove_dir_all(&dir_mine).expect("cleanup");
        std::fs::remove_dir_all(&dir_other).expect("cleanup");
    }

    /// pin の root と、held=true の保持表明の root は、ref が無くても根になる。
    #[test]
    fn a_pinned_root_and_a_held_attested_root_are_alive_without_any_ref() {
        let dir = temp_dir("pin-attest");
        let mut store = one_pack(&dir);
        let (pinned, _) = store.put_object(b"\"pinned root\"").expect("put");
        let (attested, _) = store.put_object(b"\"attested root\"").expect("put");
        assert_eq!(
            garbage_of(&store),
            (2, 28),
            "ref も pin も attest も無ければ孤児"
        );
        store.set_pin(&pinned, 2).expect("pin");
        assert_eq!(garbage_of(&store).0, 1, "pin で 1 つ生き返る");
        store.set_attest(&attested, true).expect("attest");
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(plan.roots, 2);
        assert_eq!(
            plan.live_objects, 2,
            "pin の root と attest の root が生きている"
        );
        assert_eq!(plan.garbage_objects(), 0);
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 撤回した保持表明(held=false)と解除した pin(min_replicas=0)は根にならない。
    #[test]
    fn a_withdrawn_attest_and_an_unset_pin_are_not_roots() {
        let dir = temp_dir("withdrawn");
        let mut store = one_pack(&dir);
        let (was_attested, _) = store
            .put_object(b"\"attested then withdrawn\"")
            .expect("put");
        let (was_pinned, _) = store.put_object(b"\"pinned then unpinned\"").expect("put");
        store.set_attest(&was_attested, true).expect("attest");
        store.set_pin(&was_pinned, 1).expect("pin");
        assert_eq!(garbage_of(&store).0, 0, "表明と pin が在る間は生きている");
        store.set_attest(&was_attested, false).expect("withdraw");
        store.set_pin(&was_pinned, 0).expect("unpin");
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(
            plan.roots, 0,
            "held=false の attest と min_replicas=0 の pin は根でない"
        );
        assert_eq!(plan.live_objects, 0);
        assert_eq!(plan.garbage_objects(), 2);
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 根から c1 の参照(s256: の文字列)で辿れる子は生きている。ref は根にだけ張る。
    /// 誰からも辿れないものは、参照の形をしていても孤児のまま。
    #[test]
    fn objects_reachable_from_a_root_are_alive_and_unreachable_ones_are_not() {
        let dir = temp_dir("closure");
        let mut store = one_pack(&dir);
        let (leaf, _) = store.put_object(b"\"leaf\"").expect("put");
        let (grandchild, _) = store.put_object(b"\"grandchild\"").expect("put");
        let child_body = format!("{{\"kind\":\"node\",\"next\":\"{grandchild}\",\"v\":1}}");
        let (child, _) = store.put_object(child_body.as_bytes()).expect("put");
        let root_body =
            format!("{{\"kind\":\"edge\",\"members\":[\"{leaf}\",\"{child}\"],\"v\":1}}");
        let (root, _) = store.put_object(root_body.as_bytes()).expect("put");
        let (stray, _) = store
            .put_object(b"\"stray, referenced by nobody\"")
            .expect("put");
        store.set_ref("docs/root", Some(&root)).expect("set_ref");
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        assert_eq!(plan.roots, 1, "根は ref の target だけ");
        assert_eq!(plan.objects, 5);
        assert_eq!(
            plan.live_objects, 4,
            "root, leaf, child, grandchild が生きている"
        );
        assert_eq!(plan.garbage_objects(), 1, "stray だけが孤児: {stray}");
        assert_eq!(
            plan.garbage_bytes(),
            b"\"stray, referenced by nobody\"".len() as u64
        );
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 孤児率と閾値の判定。境界は含めず、空の pack と孤児の無い pack は閾値 0 でも対象に
    /// ならない。
    #[test]
    fn the_threshold_is_exceeded_strictly_and_never_by_a_pack_without_garbage() {
        assert_eq!(garbage_ratio(0, 0), 0.0, "空の pack の孤児率は 0");
        assert_eq!(garbage_ratio(25, 100), 0.25);
        assert_eq!(garbage_ratio(100, 100), 1.0);
        assert!(
            !exceeds_threshold(25, 100, 0.25),
            "ちょうど閾値は超えていない"
        );
        assert!(exceeds_threshold(26, 100, 0.25));
        assert!(!exceeds_threshold(24, 100, 0.25));
        assert!(
            exceeds_threshold(1, 100, 0.0),
            "閾値 0 は孤児が 1 バイトでもあれば対象"
        );
        assert!(
            !exceeds_threshold(0, 100, 0.0),
            "孤児が無ければ閾値 0 でも対象でない"
        );
        assert!(!exceeds_threshold(0, 0, 0.0), "空の pack は対象でない");
        assert!(
            !exceeds_threshold(100, 100, 1.0),
            "全部孤児でも閾値 1.0 は超えられない"
        );
    }

    /// pack ごとの集計: 小さい封印閾値で 3 本に分け、全部孤児の封印済み pack は対象、
    /// 全部生きている封印済み pack は対象外、孤児しか無くても追記中の pack は対象外。
    /// 閾値ちょうどの pack は対象にならず、少し下げると対象になる。
    #[test]
    fn packs_are_judged_one_by_one_and_the_active_pack_is_never_a_target() {
        let dir = temp_dir("packs");
        let mut config = StoreConfig::new(&dir);
        // 1 レコード = 8 + 20 バイト。2 件で 56 バイト、3 件目の前に 64 を超えないので
        // 3 件で 84 バイトになってから封印される: pack 1 に 3 件、pack 2 に 3 件、pack 3 に残り。
        config.pack_seal_bytes = 64;
        let mut store = Store::open(config).expect("open");
        let body = |i: u32| format!("{{\"i\":{i:02},\"pad\":\"abc\"}}");
        assert_eq!(body(0).len(), 20, "本体の大きさが計算の前提");
        let ids: Vec<String> = (0..7u32)
            .map(|i| store.put_object(body(i).as_bytes()).expect("put").0)
            .collect();
        assert_eq!(store.sealed_pack_numbers(), &[1, 2], "pack 1・2 が封印済み");
        assert_eq!(store.active_pack_number(), 3);
        // pack 1(ids 0..3): 全部孤児。pack 2(ids 3..6): 全部生きている。
        // pack 3(id 6): 追記中で、孤児。
        for id in &ids[3..6] {
            store
                .set_ref(&format!("notes/{id}"), Some(id))
                .expect("set_ref");
        }
        let plan = plan(&store, DEFAULT_THRESHOLD).expect("plan");
        let rows: Vec<(u64, bool, usize, u64, u64, bool)> = plan
            .packs
            .iter()
            .map(|p| {
                (
                    p.number,
                    p.sealed,
                    p.objects,
                    p.bytes,
                    p.garbage_bytes(),
                    p.compact,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (1, true, 3, 60, 60, true),
                (2, true, 3, 60, 0, false),
                (3, false, 1, 20, 20, false),
            ],
            "(番号, 封印済み, 件数, バイト, 孤児バイト, 対象)"
        );
        assert_eq!(plan.packs[0].garbage_ratio(), 1.0);
        assert_eq!(plan.packs[2].garbage_ratio(), 1.0, "追記中でも孤児率は出す");
        assert_eq!(plan.sealed_packs(), 2);
        assert_eq!(plan.compact_packs(), 1);
        assert_eq!(plan.compact_bytes(), 60, "対象の pack から戻るバイト数");
        assert_eq!(plan.garbage_bytes(), 80, "孤児の総量は追記中の分も含む");
        assert_eq!(plan.objects, 7);
        assert_eq!(plan.live_objects, 3);

        // pack 1 の 1 件を生き返らせると孤児率は 2/3。閾値ちょうど(2/3)は対象でなく、
        // それより少し低い閾値なら対象。
        store
            .set_ref("notes/revived", Some(&ids[0]))
            .expect("set_ref");
        let two_thirds = 40.0 / 60.0;
        let at_boundary = plan_for(&store, two_thirds);
        assert_eq!(at_boundary.packs[0].garbage_bytes(), 40);
        assert!(
            !at_boundary.packs[0].compact,
            "孤児率ちょうどの閾値では対象にならない"
        );
        let below = plan_for(&store, 0.6);
        assert!(below.packs[0].compact, "閾値を下回る孤児率なら対象");
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    fn plan_for(store: &Store, threshold: f64) -> GcPlan {
        let plan = plan(store, threshold).expect("plan");
        assert_eq!(plan.threshold, threshold);
        plan
    }
}
