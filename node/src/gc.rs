//! pack の物理回収(GC): 孤児を数える dry-run と、封印済み pack を書き直して孤児のバイト列を
//! ディスクから取り戻す回収。`uniqnode gc <dir> [--dry-run]` と serve の
//! `POST /v1/admin/gc` の両方が [run] を呼ぶ(should/0135: dry-run も同じ道を通る)。
//!
//! 生きているオブジェクトの定義、手順 S・P・A・B・C・D、クラッシュの各点の回復、出力の
//! 読み方は [docs/design/GC.md](uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)。未実装の段階
//! (evict)は [docs/plan/PACK_GC.md](uuid:f272eeda-8664-42da-9e5c-ef354bc3f3a7)。
//!
//! ロックは `Mutex<Store>` の guard で、S・A・C の間だけ持つ。P(封印済み pack の解析)と
//! B(生きているレコードの写し)と D(旧 pack の削除)は guard を持たない。到達閉包の探索は
//! Store の closure_over(参照の出どころだけを差し替える)で、参照の規約と dangling の扱い
//! をここで二重に決めない(should/0135)。

use crate::store::{
    self, closure_over, scan_pack_references, CopiedObject, GcCommit, ObjectLocation,
    ReferenceEntries, Result, Store, StoreError, WriteCursor, GC_TMP_PREFIX,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 孤児率がこれを超えた封印済み pack を回収の対象と見る、閾値の既定。根拠はまだ無く、
/// dry-run の実測で決める(PACK_GC (uuid:f272eeda-8664-42da-9e5c-ef354bc3f3a7) の
/// セルフレビュー 1)。
pub const DEFAULT_THRESHOLD: f64 = 0.25;

/// 参照表の置き場(データディレクトリからの相対)。導出データなので backup は写さず、無ければ
/// 作るだけ。
pub const REFS_DIRECTORY: &str = "derived/refs";

/// 参照表の 1 行目の頭。この後に pack ファイルの大きさが続き、一致すれば流用する(封印済み
/// pack は不変なので、大きさが同じなら中身も同じ)。
const REFS_FORMAT: &str = "uniqnode-refs-1";

/// テスト用の口: cfg(debug_assertions) のビルドだけが読む環境変数。値が相の名と一致すれば、
/// その相の直後に std::process::abort() する。クラッシュの各点からの回復を実プロセスで
/// 固定するため(node/tests/gc_crash.rs)。release ビルドには入らない。
pub const CRASH_AFTER_ENV: &str = "UNIQNODE_GC_CRASH_AFTER";
pub const CRASH_AFTER_S: &str = "S";
/// B の途中(新 pack へ最初の 1 件を写した直後)。
pub const CRASH_AFTER_B: &str = "B";
pub const CRASH_AFTER_C1: &str = "C1";
pub const CRASH_AFTER_C2: &str = "C2";
pub const CRASH_AFTER_C3: &str = "C3";

/// テスト用の口: cfg(debug_assertions) のビルドだけが読む環境変数。値は道で、B の後(C の
/// 前)に `<道>.ready` を作ってから `<道>` が現れるまで待つ。A と C の間に別の書き手を挟む
/// (孤児を指す新しい ref = 生き返り)テストが時機を作るため。release ビルドには入らない。
pub const WAIT_AFTER_COPY_ENV: &str = "UNIQNODE_GC_WAIT_AFTER_B";
pub const WAIT_READY_SUFFIX: &str = ".ready";

/// 同じストアで回収が走っている間に 2 つ目を頼まれたときの断りの文(admin の 409 が見る)。
pub const GC_ALREADY_RUNNING: &str = "gc は既に走っている";

/// 相の直後に落とす(テスト用。上の CRASH_AFTER_ENV)。
pub(crate) fn crash_point(phase: &str) {
    #[cfg(debug_assertions)]
    {
        if std::env::var(CRASH_AFTER_ENV).is_ok_and(|value| value == phase) {
            eprintln!("uniqnode: gc: {CRASH_AFTER_ENV}={phase} なので abort する(テスト用の口)");
            std::process::abort();
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = phase;
    }
}

/// B の後に待つ(テスト用。上の WAIT_AFTER_COPY_ENV)。
fn wait_after_copy() -> Result<()> {
    #[cfg(debug_assertions)]
    {
        let Ok(path) = std::env::var(WAIT_AFTER_COPY_ENV) else { return Ok(()) };
        let go = PathBuf::from(&path);
        std::fs::write(format!("{path}{WAIT_READY_SUFFIX}"), b"")?;
        // 条件待ち(should/0104): 合図のファイルが現れたら即座に進む。期限はテストが合図を
        // 出し忘れたときに永久に止まらないための安全網で、切れたら大きな声で失敗する。
        let deadline = Instant::now() + Duration::from_secs(30);
        while !go.exists() {
            if Instant::now() > deadline {
                return Err(StoreError::Invalid(format!(
                    "{WAIT_AFTER_COPY_ENV}: {path} が 30 秒現れない(テストの合図が来ない)"
                )));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}

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

/// 1 回の実行の指定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GcOptions {
    pub threshold: f64,
    /// 数えるだけで、封印も写しも差し替えもしない(参照表だけは作る。導出データ)。
    pub dry_run: bool,
}

/// 各相の所要。S・A・C がロックの中。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseTimes {
    pub seal: Duration,
    pub table: Duration,
    pub analyze: Duration,
    pub copy: Duration,
    pub commit: Duration,
    pub delete: Duration,
}

impl PhaseTimes {
    /// ロックを持っていた長さの合計(S + A + C)。
    pub fn locked(&self) -> Duration {
        self.seal + self.analyze + self.commit
    }
}

/// 1 回の実行で分かったこと・したこと。命令の出力と admin の応答はこれをそのまま並べる。
#[derive(Clone, Debug)]
pub struct GcReport {
    pub threshold: f64,
    pub dry_run: bool,
    /// pack 番号の昇順。封印済みの全部と、追記中の 1 本(A の時点)。
    pub packs: Vec<PackPlan>,
    /// 根の数(target が null でない ref の target、pin の root、held=true の attest の
    /// root の和集合。ローカルに無いものも数える)。
    pub roots: usize,
    pub objects: usize,
    pub live_objects: usize,
    /// S で封印したアクティブ pack(空で封印しなかった、または dry-run なら None)。
    pub sealed_in_seal_phase: Option<u64>,
    /// P で derived/refs/ から流用した参照表の数と、作った数。
    pub tables_reused: usize,
    pub tables_built: usize,
    /// 実際に書き直した(MANIFEST から外して消した)pack。dry-run なら空。
    pub compacted: Vec<u64>,
    /// C-1 で封印したアクティブ pack。
    pub sealed_active: Option<u64>,
    /// 生きているものを写した新しい pack の番号。写すものが無ければ None。
    pub new_pack: Option<u64>,
    /// 差分検査(C)で生き返り、新 pack に写し足したオブジェクトの数。
    pub revived_objects: usize,
    /// used_bytes から戻ったバイト数(索引から外した孤児のペイロード合計)。
    pub reclaimed_bytes: u64,
    /// packs/ のファイルの大きさで見た減り(消した pack の合計 − 新 pack)。
    pub disk_bytes_freed: u64,
    pub phases: PhaseTimes,
}

impl GcReport {
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

    /// 対象の pack だけを書き直したときに戻る(dry-run)/戻るはずだった(A の時点の)バイト数。
    pub fn compact_bytes(&self) -> u64 {
        self.packs
            .iter()
            .filter(|pack| pack.compact)
            .map(PackPlan::garbage_bytes)
            .sum()
    }

    /// 根を集めて閉包を辿り終えるまでの時間(A。ロックの中)。
    pub fn live_set_elapsed(&self) -> Duration {
        self.phases.analyze
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

/// 走っている回収の印。落ちるときに(成功でも失敗でも)ストアの gc_running を下ろす。
/// ストアのロックを取るので、MutexGuard を持ったまま落としてはならない。
struct RunningGuard<'a>(&'a Mutex<Store>);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        lock(self.0).gc_end();
    }
}

fn refs_table_path(dir: &Path, number: u64) -> PathBuf {
    dir.join(REFS_DIRECTORY).join(format!("pack-{number:06}"))
}

/// 封印済み pack の参照表。derived/refs/ に在って pack の大きさが一致すれば流用し、無ければ
/// (または壊れていれば)pack を読んで作り、置く。返り値の bool は流用したか。
fn load_or_build_table(
    dir: &Path,
    max_record_bytes: u32,
    number: u64,
) -> Result<(ReferenceEntries, bool)> {
    let path = refs_table_path(dir, number);
    let file_length = std::fs::metadata(store::PACK.path(dir, number))?.len();
    if let Some(entries) = read_table(&path, file_length)? {
        return Ok((entries, true));
    }
    let scanned = scan_pack_references(dir, max_record_bytes, number)?;
    if scanned.truncated {
        return Err(StoreError::Corruption(format!(
            "封印済み pack {number} が壊れている(バックアップからの復元が必要)"
        )));
    }
    let entries: ReferenceEntries = scanned
        .entries
        .into_iter()
        .filter(|(_, references)| !references.is_empty())
        .collect();
    write_table(dir, number, scanned.file_length, &entries)?;
    Ok((entries, false))
}

/// 参照表を置く(tmp/ に書いて fsync し rename)。参照の無いオブジェクトの行は書かない。
fn write_table(dir: &Path, number: u64, file_length: u64, entries: &[(String, Vec<String>)]) -> Result<()> {
    let mut content = format!("{REFS_FORMAT} {file_length}\n");
    for (id, references) in entries {
        if references.is_empty() {
            continue;
        }
        content.push_str(id);
        for reference in references {
            content.push(' ');
            content.push_str(reference);
        }
        content.push('\n');
    }
    std::fs::create_dir_all(dir.join(REFS_DIRECTORY))?;
    store::atomic_write(dir, &refs_table_path(dir, number), content.as_bytes())
}

/// 参照表を読む。無い・形が違う・pack の大きさが違う・ID の形でない語がある、のどれでも None
/// (作り直す)。
fn read_table(path: &Path, file_length: u64) -> Result<Option<ReferenceEntries>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut lines = text.lines();
    let Some(header) = lines.next() else { return Ok(None) };
    let Some(recorded) = header.strip_prefix(REFS_FORMAT).and_then(|rest| rest.trim().parse::<u64>().ok())
    else {
        return Ok(None);
    };
    if recorded != file_length || !text.ends_with('\n') {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for line in lines {
        let mut words = line.split(' ');
        let Some(id) = words.next() else { return Ok(None) };
        let references: Vec<String> = words.map(str::to_string).collect();
        if !crate::c1::is_object_id(id)
            || references.is_empty()
            || references.iter().any(|r| !crate::c1::is_object_id(r))
        {
            return Ok(None);
        }
        entries.push((id.to_string(), references));
    }
    Ok(Some(entries))
}

/// tmp/ に書く新しい pack。レコードの形は Store の追記と同じ [len][crc32][payload]。
struct PackWriter {
    path: PathBuf,
    file: std::fs::File,
    written: u64,
    /// gc_commit が索引をこれに差し替える。
    entries: Vec<CopiedObject>,
}

impl PackWriter {
    fn create(path: PathBuf) -> Result<PackWriter> {
        let file = std::fs::File::create(&path)?;
        Ok(PackWriter { path, file, written: 0, entries: Vec::new() })
    }

    fn append(&mut self, id: &str, payload: &[u8]) -> Result<()> {
        let mut record = Vec::with_capacity(8 + payload.len());
        record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        record.extend_from_slice(&crate::crc32::crc32(payload).to_le_bytes());
        record.extend_from_slice(payload);
        self.file.write_all(&record)?;
        self.entries.push((id.to_string(), self.written + 8, payload.len() as u32));
        self.written += record.len() as u64;
        Ok(())
    }

    fn sync(&self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }
}

/// 旧 pack からペイロードを読む(索引の位置で)。ファイルは開いたまま使い回す。
fn read_payload(
    dir: &Path,
    files: &mut BTreeMap<u64, std::fs::File>,
    location: &ObjectLocation,
) -> Result<Vec<u8>> {
    let file = match files.entry(location.pack_number) {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(std::fs::File::open(store::PACK.path(dir, location.pack_number))?)
        }
    };
    file.seek(SeekFrom::Start(location.payload_offset))?;
    let mut payload = vec![0u8; location.payload_length as usize];
    file.read_exact(&mut payload)?;
    Ok(payload)
}

fn lock(store: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    store.lock().expect("store lock")
}

fn merge_references(into: &mut BTreeMap<String, Vec<String>>, entries: ReferenceEntries) {
    for (id, references) in entries {
        if !references.is_empty() {
            into.insert(id, references);
        }
    }
}

/// pack ごとの集計(A の時点の索引と生きている集合から)。封印済みの全部と追記中の 1 本は、
/// オブジェクトが 1 つも無くても行に出す。
fn pack_rows(
    sealed: &[u64],
    active: u64,
    locations: &BTreeMap<String, ObjectLocation>,
    live: &BTreeSet<String>,
    threshold: f64,
) -> Vec<PackPlan> {
    let empty_pack = |number: u64| PackPlan {
        number,
        sealed: sealed.contains(&number),
        objects: 0,
        bytes: 0,
        live_objects: 0,
        live_bytes: 0,
        compact: false,
    };
    let mut by_pack: BTreeMap<u64, PackPlan> = sealed
        .iter()
        .copied()
        .chain(std::iter::once(active))
        .map(|number| (number, empty_pack(number)))
        .collect();
    for (id, location) in locations {
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
    packs
}

/// 回収を 1 回走らせる(dry_run なら数えるだけ)。手順は GC.md の S・P・A・B・C・D。
pub fn run(store: &Mutex<Store>, options: GcOptions) -> Result<GcReport> {
    let mut phases = PhaseTimes::default();

    // S: ロックの中。アクティブを封印し(dry-run では封印せず位置だけ読む)、以後ロックの外で
    // 読む封印済み pack の一覧と、追記の現在位置を写し取る。
    let started = Instant::now();
    let (dir, max_record_bytes, sealed_after_seal, cursor, sealed_in_seal_phase) = {
        let mut guard = lock(store);
        if !guard.gc_try_begin() {
            return Err(StoreError::Invalid(GC_ALREADY_RUNNING.into()));
        }
        let sealed = if options.dry_run {
            None
        } else {
            match guard.seal_active_pack_for_gc() {
                Ok(sealed) => sealed,
                Err(error) => {
                    guard.gc_end();
                    return Err(error);
                }
            }
        };
        (
            guard.data_dir().to_path_buf(),
            guard.max_record_bytes(),
            guard.sealed_pack_numbers().to_vec(),
            guard.write_cursor(),
            sealed,
        )
    };
    // ここから先はどの道で抜けても gc_running を下ろす(ロックを持っていない所で落ちる)。
    let _running = RunningGuard(store);
    phases.seal = started.elapsed();
    crash_point(CRASH_AFTER_S);

    // P: ロックの外。封印済み pack の参照表(ID → 参照先)を流用するか作る。dry-run では
    // 追記中の pack も今ある分だけ読んでおく(封印しないので。表には置かない。書き込み途中の
    // 末尾は捨て、A がカーソル以後を読み直す)。
    let started = Instant::now();
    let mut references: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut tables_reused = 0usize;
    let mut tables_built = 0usize;
    for number in &sealed_after_seal {
        let (entries, reused) = load_or_build_table(&dir, max_record_bytes, *number)?;
        if reused {
            tables_reused += 1;
        } else {
            tables_built += 1;
        }
        merge_references(&mut references, entries);
    }
    if options.dry_run && store::PACK.path(&dir, cursor.pack).exists() {
        let scanned = scan_pack_references(&dir, max_record_bytes, cursor.pack)?;
        merge_references(&mut references, scanned.entries);
    }
    phases.table = started.elapsed();

    // A: ロックの中。根の表と索引を写し取り、S 以後に追記された分だけ解析して参照表に足し、
    // 閉包を辿って生きている集合を得る。回収するなら、以後の put の記録を始める。
    let started = Instant::now();
    let (roots_at_analysis, locations, live, cursor_after_analysis, packs) = {
        let mut guard = lock(store);
        let roots = roots(&guard);
        let (tail, cursor_after) = guard.references_since(cursor)?;
        merge_references(&mut references, tail);
        let locations: BTreeMap<String, ObjectLocation> = guard
            .object_locations()
            .map(|(id, location)| (id.clone(), *location))
            .collect();
        let live = closure_over(roots.iter().map(String::as_str), |id| {
            Ok(locations
                .contains_key(id)
                .then(|| references.get(id).cloned().unwrap_or_default()))
        })?;
        let packs = pack_rows(
            guard.sealed_pack_numbers(),
            guard.active_pack_number(),
            &locations,
            &live,
            options.threshold,
        );
        if !options.dry_run && packs.iter().any(|pack| pack.compact) {
            guard.gc_begin_touch_log();
        }
        (roots, locations, live, cursor_after, packs)
    };
    phases.analyze = started.elapsed();

    let targets: Vec<u64> = packs.iter().filter(|pack| pack.compact).map(|pack| pack.number).collect();
    let mut report = GcReport {
        threshold: options.threshold,
        dry_run: options.dry_run,
        roots: roots_at_analysis.len(),
        objects: locations.len(),
        live_objects: live.len(),
        packs,
        sealed_in_seal_phase,
        tables_reused,
        tables_built,
        compacted: Vec::new(),
        sealed_active: None,
        new_pack: None,
        revived_objects: 0,
        reclaimed_bytes: 0,
        disk_bytes_freed: 0,
        phases,
    };
    if options.dry_run || targets.is_empty() {
        return Ok(report);
    }

    let tmp_path = dir.join("tmp").join(format!("{GC_TMP_PREFIX}{}.pack", std::process::id()));
    let outcome = compact(
        store,
        &dir,
        &tmp_path,
        &targets,
        &roots_at_analysis,
        &locations,
        &live,
        &mut references,
        cursor_after_analysis,
        &mut report.phases,
    );
    match outcome {
        Ok((commit, revived, disk_bytes_freed)) => {
            report.compacted = targets;
            report.sealed_active = commit.sealed_active;
            report.new_pack = commit.new_pack;
            report.reclaimed_bytes = commit.reclaimed_bytes;
            report.revived_objects = revived;
            report.disk_bytes_freed = disk_bytes_freed;
            Ok(report)
        }
        Err(error) => {
            // 途中で失敗したら、書きかけの新 pack を消す(消せなくても開くときに tmp/ が空に
            // される)。put の記録は RunningGuard が止める。旧 pack と MANIFEST は C-3 の前なら
            // 無傷である。
            if tmp_path.exists() {
                if let Err(remove_error) = std::fs::remove_file(&tmp_path) {
                    crate::log_line!(
                        "uniqnode: gc: 書きかけの {} を消せない: {remove_error}",
                        tmp_path.display()
                    );
                }
            }
            Err(error)
        }
    }
}

/// B・C・D。返り値は (差し替えの結果, 生き返った数, ディスクの減り)。
#[allow(clippy::too_many_arguments)]
fn compact(
    store: &Mutex<Store>,
    dir: &Path,
    tmp_path: &Path,
    targets: &[u64],
    roots_at_analysis: &BTreeSet<String>,
    locations: &BTreeMap<String, ObjectLocation>,
    live: &BTreeSet<String>,
    references: &mut BTreeMap<String, Vec<String>>,
    cursor_after_analysis: WriteCursor,
    phases: &mut PhaseTimes,
) -> Result<(GcCommit, usize, u64)> {
    let in_targets = |id: &str| {
        locations
            .get(id)
            .is_some_and(|location| targets.contains(&location.pack_number))
    };

    // B: ロックの外。対象 pack の生きているレコードを tmp/ の新 pack へ写す。旧 pack は
    // 封印済みで不変なので、ロックなしで読める。ハッシュを照合しながら写す。
    let started = Instant::now();
    let target_file_bytes: u64 = targets
        .iter()
        .map(|number| std::fs::metadata(store::PACK.path(dir, *number)).map(|m| m.len()))
        .sum::<std::io::Result<u64>>()?;
    let mut writer = PackWriter::create(tmp_path.to_path_buf())?;
    let mut files = BTreeMap::new();
    let mut first = true;
    for (id, location) in locations {
        if !targets.contains(&location.pack_number) || !live.contains(id) {
            continue;
        }
        let payload = read_payload(dir, &mut files, location)?;
        let actual = crate::c1::id_for_bytes(&payload);
        if actual != *id {
            return Err(StoreError::Corruption(format!(
                "pack {} のオブジェクト {id} の内容が一致しない(実際 {actual})",
                location.pack_number
            )));
        }
        writer.append(id, &payload)?;
        if first {
            first = false;
            crash_point(CRASH_AFTER_B);
        }
    }
    writer.sync()?;
    phases.copy = started.elapsed();
    wait_after_copy()?;

    // C: ロックの中。差分検査 → 差し替え。
    let started = Instant::now();
    let (commit, revived, copied_ids) = {
        let mut guard = lock(store);
        // A 以後に増えた根(全署名者の ref・pin・attest から組む根の表の差分。新しい根は必ず
        // 新しい reflog レコードとして現れ、根の表はそれを全部適用した後の姿なので、
        // signer_last_seq の差分から reflog を読み直すのと同じ集合に至る)。
        let roots_now = roots(&guard);
        let new_roots: Vec<&str> = roots_now
            .difference(roots_at_analysis)
            .map(String::as_str)
            .collect();
        // A 以後に追記されたオブジェクトの参照(新しい根がそこから孤児を指しうる)。
        let (tail, _) = guard.references_since(cursor_after_analysis)?;
        merge_references(references, tail);
        // A 以後に put で「既に在る」と答えた ID(ref を張る途中かもしれない)。
        let touched = guard.gc_take_touch_log();
        let mut revive = closure_over(new_roots, |id| {
            Ok(guard
                .has_object(id)
                .then(|| references.get(id).cloned().unwrap_or_default()))
        })?;
        revive.extend(touched);
        let revive: Vec<String> = revive
            .into_iter()
            .filter(|id| in_targets(id) && !live.contains(id))
            .collect();
        for id in &revive {
            let payload = guard.get_object(id)?.ok_or_else(|| {
                StoreError::Corruption(format!("生き返った {id} が読めない"))
            })?;
            writer.append(id, &payload)?;
        }
        if !revive.is_empty() {
            writer.sync()?;
        }
        let new_pack = if writer.entries.is_empty() {
            drop(writer.file);
            std::fs::remove_file(&writer.path)?;
            None
        } else {
            Some((writer.path.as_path(), writer.entries.as_slice()))
        };
        let commit = guard.gc_commit(targets, new_pack)?;
        let copied_ids: Vec<String> = writer.entries.iter().map(|(id, _, _)| id.clone()).collect();
        (commit, revive.len(), copied_ids)
    };
    phases.commit = started.elapsed();

    // D: ロックの外。旧 pack と、その参照表を消す。MANIFEST に無いので、ここで落ちても回復が
    // 残骸として消す。
    let started = Instant::now();
    for number in targets {
        std::fs::remove_file(store::PACK.path(dir, *number))?;
        let table = refs_table_path(dir, *number);
        if table.exists() {
            std::fs::remove_file(&table)?;
        }
    }
    std::fs::File::open(dir.join(store::PACK.directory))?.sync_all()?;
    // 新 pack の参照表は、写したオブジェクトの参照が手元(P で解析済み)にあるので、pack を
    // 読み直さずに置く。次回の P がこれを流用する。
    let new_pack_bytes = match commit.new_pack {
        Some(number) => {
            let file_length = std::fs::metadata(store::PACK.path(dir, number))?.len();
            let entries: ReferenceEntries = copied_ids
                .into_iter()
                .filter_map(|id| references.get(&id).cloned().map(|refs| (id, refs)))
                .collect();
            write_table(dir, number, file_length, &entries)?;
            file_length
        }
        None => 0,
    };
    phases.delete = started.elapsed();
    Ok((commit, revived, target_file_bytes.saturating_sub(new_pack_bytes)))
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

    fn dry_run(store: &Mutex<Store>, threshold: f64) -> GcReport {
        let report = run(store, GcOptions { threshold, dry_run: true }).expect("dry-run");
        assert_eq!(report.threshold, threshold);
        assert!(report.dry_run);
        assert!(report.compacted.is_empty(), "dry-run は何も書き直さない");
        report
    }

    fn plan_for(store: &Mutex<Store>, threshold: f64) -> GcReport {
        dry_run(store, threshold)
    }

    /// 孤児の数とバイト数。
    fn garbage_of(store: &Mutex<Store>) -> (usize, u64) {
        let report = dry_run(store, DEFAULT_THRESHOLD);
        (report.garbage_objects(), report.garbage_bytes())
    }

    /// 同じ ref パスへの上書き(再取り込み): 旧 target は孤児、新 target は生きている。
    #[test]
    fn overwriting_a_ref_orphans_the_old_target_and_keeps_the_new_one() {
        let dir = temp_dir("overwrite");
        let store = Mutex::new(one_pack(&dir));
        let (old, _) = lock(&store).put_object(b"\"version 1 of the note\"").expect("put");
        lock(&store).set_ref("notes/x", Some(&old)).expect("set_ref");
        assert_eq!(garbage_of(&store), (0, 0), "上書き前は全部生きている");
        let (new, _) = lock(&store)
            .put_object(b"\"version 2 of the note!\"")
            .expect("put");
        lock(&store).set_ref("notes/x", Some(&new)).expect("set_ref");
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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
        let store = Mutex::new(one_pack(&dir));
        let (id, _) = lock(&store).put_object(b"\"to be forgotten\"").expect("put");
        lock(&store).set_ref("notes/gone", Some(&id)).expect("set_ref");
        assert_eq!(garbage_of(&store), (0, 0));
        lock(&store).set_ref("notes/gone", None).expect("tombstone");
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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
        let store = Mutex::new(one_pack(&dir));
        let (kept, _) = lock(&store).put_object(b"\"kept\"").expect("put");
        lock(&store).set_ref("notes/kept", Some(&kept)).expect("set_ref");
        lock(&store).put_object(b"\"never named by a ref\"").expect("put");
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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
        let mine = Mutex::new(one_pack(&dir_mine));
        // 複製として同じバイト列を持っているが、自分の ref は張っていない。
        lock(&mine).put_object(body).expect("put replica");
        assert_eq!(
            garbage_of(&mine),
            (1, body.len() as u64),
            "ref を入れる前は孤児"
        );
        for record in &records {
            assert!(lock(&mine).ingest_ref_record(record).expect("ingest"));
        }
        let plan = plan_for(&mine, DEFAULT_THRESHOLD);
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
        let store = Mutex::new(one_pack(&dir));
        let (pinned, _) = lock(&store).put_object(b"\"pinned root\"").expect("put");
        let (attested, _) = lock(&store).put_object(b"\"attested root\"").expect("put");
        assert_eq!(
            garbage_of(&store),
            (2, 28),
            "ref も pin も attest も無ければ孤児"
        );
        lock(&store).set_pin(&pinned, 2).expect("pin");
        assert_eq!(garbage_of(&store).0, 1, "pin で 1 つ生き返る");
        lock(&store).set_attest(&attested, true).expect("attest");
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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
        let store = Mutex::new(one_pack(&dir));
        let (was_attested, _) = lock(&store)
            .put_object(b"\"attested then withdrawn\"")
            .expect("put");
        let (was_pinned, _) = lock(&store).put_object(b"\"pinned then unpinned\"").expect("put");
        lock(&store).set_attest(&was_attested, true).expect("attest");
        lock(&store).set_pin(&was_pinned, 1).expect("pin");
        assert_eq!(garbage_of(&store).0, 0, "表明と pin が在る間は生きている");
        lock(&store).set_attest(&was_attested, false).expect("withdraw");
        lock(&store).set_pin(&was_pinned, 0).expect("unpin");
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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
        let store = Mutex::new(one_pack(&dir));
        let (leaf, _) = lock(&store).put_object(b"\"leaf\"").expect("put");
        let (grandchild, _) = lock(&store).put_object(b"\"grandchild\"").expect("put");
        let child_body = format!("{{\"kind\":\"node\",\"next\":\"{grandchild}\",\"v\":1}}");
        let (child, _) = lock(&store).put_object(child_body.as_bytes()).expect("put");
        let root_body =
            format!("{{\"kind\":\"edge\",\"members\":[\"{leaf}\",\"{child}\"],\"v\":1}}");
        let (root, _) = lock(&store).put_object(root_body.as_bytes()).expect("put");
        let (stray, _) = lock(&store)
            .put_object(b"\"stray, referenced by nobody\"")
            .expect("put");
        lock(&store).set_ref("docs/root", Some(&root)).expect("set_ref");
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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

    /// 小さい封印閾値で 3 本に分ける設定。1 レコード = 8 + 20 バイト。2 件で 56 バイト、
    /// 3 件目の前に 64 を超えないので 3 件で 84 バイトになってから封印される: pack 1 に 3 件、
    /// pack 2 に 3 件、pack 3 に残り。
    fn three_packs(dir: &Path) -> (Store, Vec<String>) {
        let mut config = StoreConfig::new(dir);
        config.pack_seal_bytes = 64;
        let mut store = Store::open(config).expect("open");
        let body = |i: u32| format!("{{\"i\":{i:02},\"pad\":\"abc\"}}");
        assert_eq!(body(0).len(), 20, "本体の大きさが計算の前提");
        let ids: Vec<String> = (0..7u32)
            .map(|i| store.put_object(body(i).as_bytes()).expect("put").0)
            .collect();
        assert_eq!(store.sealed_pack_numbers(), &[1, 2], "pack 1・2 が封印済み");
        assert_eq!(store.active_pack_number(), 3);
        (store, ids)
    }

    /// pack ごとの集計: 全部孤児の封印済み pack は対象、全部生きている封印済み pack は
    /// 対象外、孤児しか無くても追記中の pack は対象外。閾値ちょうどの pack は対象にならず、
    /// 少し下げると対象になる。
    #[test]
    fn packs_are_judged_one_by_one_and_the_active_pack_is_never_a_target() {
        let dir = temp_dir("packs");
        let (mut store, ids) = three_packs(&dir);
        // pack 1(ids 0..3): 全部孤児。pack 2(ids 3..6): 全部生きている。
        // pack 3(id 6): 追記中で、孤児。
        for id in &ids[3..6] {
            store
                .set_ref(&format!("notes/{id}"), Some(id))
                .expect("set_ref");
        }
        let store = Mutex::new(store);
        let plan = plan_for(&store, DEFAULT_THRESHOLD);
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
        assert_eq!(plan.tables_built, 2, "初回は封印済み 2 本の参照表を作る");
        assert_eq!(plan.tables_reused, 0);

        // pack 1 の 1 件を生き返らせると孤児率は 2/3。閾値ちょうど(2/3)は対象でなく、
        // それより少し低い閾値なら対象。
        lock(&store)
            .set_ref("notes/revived", Some(&ids[0]))
            .expect("set_ref");
        let two_thirds = 40.0 / 60.0;
        let at_boundary = plan_for(&store, two_thirds);
        assert_eq!(at_boundary.packs[0].garbage_bytes(), 40);
        assert!(
            !at_boundary.packs[0].compact,
            "孤児率ちょうどの閾値では対象にならない"
        );
        assert_eq!(at_boundary.tables_reused, 2, "2 回目は参照表を流用する");
        let below = plan_for(&store, 0.6);
        assert!(below.packs[0].compact, "閾値を下回る孤児率なら対象");
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 回収そのもの: 対象の pack が消え、生きているものは新 pack から読め、孤児は索引から
    /// 消え、used_bytes とディスクが減る。再び開いても同じ。
    #[test]
    fn compaction_rewrites_the_target_pack_and_drops_only_the_garbage() {
        let dir = temp_dir("compact");
        let (mut store, ids) = three_packs(&dir);
        // pack 1: ids 0 生きている、1・2 孤児。pack 2 と pack 3(追記中): 全部生きている。
        for id in std::iter::once(&ids[0]).chain(&ids[3..7]) {
            store
                .set_ref(&format!("notes/{id}"), Some(id))
                .expect("set_ref");
        }
        let used_before = store.used_bytes();
        let store = Mutex::new(store);
        let report = run(&store, GcOptions { threshold: 0.5, dry_run: false }).expect("gc");
        assert_eq!(report.compacted, vec![1], "孤児率 2/3 の pack 1 だけが対象");
        assert_eq!(report.sealed_in_seal_phase, Some(3), "S で追記中の pack 3 を封印する");
        assert_eq!(report.sealed_active, None, "S 以後に書き込みが無いので C-1 は封印しない");
        assert_eq!(report.new_pack, Some(4), "新 pack は空だったアクティブの番号 4");
        assert_eq!(report.reclaimed_bytes, 40, "孤児 2 件 × 20 バイト");
        assert_eq!(report.revived_objects, 0);
        assert!(report.disk_bytes_freed > 0, "{report:?}");
        {
            let guard = lock(&store);
            assert_eq!(guard.used_bytes(), used_before - 40);
            assert_eq!(guard.sealed_pack_numbers(), &[2, 3, 4]);
            assert_eq!(guard.active_pack_number(), 5);
            assert!(guard.get_object(&ids[0]).expect("get").is_some(), "生きているものは読める");
            assert!(guard.get_object(&ids[1]).expect("get").is_none(), "孤児は消えた");
            assert!(guard.get_object(&ids[2]).expect("get").is_none());
            assert!(guard.get_object(&ids[6]).expect("get").is_some(), "S で封印した pack の中身も読める");
            assert!(!dir.join("packs/pack-000001.pack").exists(), "旧 pack が消えている");
            assert!(dir.join("packs/pack-000004.pack").exists(), "新 pack が在る");
            assert!(!refs_table_path(&dir, 1).exists(), "旧 pack の参照表も消えている");
            let report = guard.fsck().expect("fsck");
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        drop(store);
        let mut config = StoreConfig::new(&dir);
        config.pack_seal_bytes = 64;
        let reopened = Store::open(config).expect("reopen");
        assert_eq!(reopened.object_count(), 5);
        assert_eq!(reopened.used_bytes(), used_before - 40);
        assert!(reopened.get_object(&ids[0]).expect("get").is_some());
        assert!(reopened.fsck().expect("fsck").errors.is_empty());
        drop(reopened);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 参照表は pack の大きさが一致すれば流用し、壊れていれば作り直す。
    #[test]
    fn a_damaged_reference_table_is_rebuilt_instead_of_trusted() {
        let dir = temp_dir("table");
        let (mut store, ids) = three_packs(&dir);
        store.set_ref("notes/a", Some(&ids[0])).expect("set_ref");
        let store = Mutex::new(store);
        let first = dry_run(&store, DEFAULT_THRESHOLD);
        assert_eq!((first.tables_built, first.tables_reused), (2, 0));
        let table = refs_table_path(&dir, 1);
        let content = std::fs::read_to_string(&table).expect("read table");
        assert!(content.starts_with(REFS_FORMAT), "{content}");
        // 大きさが合わない表は流用しない。
        std::fs::write(&table, format!("{REFS_FORMAT} 1\n")).expect("write");
        let second = dry_run(&store, DEFAULT_THRESHOLD);
        assert_eq!((second.tables_built, second.tables_reused), (1, 1), "壊した 1 本だけ作り直す");
        assert_eq!(std::fs::read_to_string(&table).expect("read"), content, "作り直した表は元と同じ");
        // 途中で切れた表(末尾に改行が無い)も流用しない。
        std::fs::write(&table, content.trim_end_matches('\n')).expect("write");
        let third = dry_run(&store, DEFAULT_THRESHOLD);
        assert_eq!((third.tables_built, third.tables_reused), (1, 1));
        let fourth = dry_run(&store, DEFAULT_THRESHOLD);
        assert_eq!((fourth.tables_built, fourth.tables_reused), (0, 2));
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 参照表の読み取り: 形が違えば None。
    #[test]
    fn a_reference_table_with_a_foreign_word_is_rejected() {
        let dir = temp_dir("table-format");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("table");
        let id = crate::c1::id_for_bytes(b"x");
        let other = crate::c1::id_for_bytes(b"y");
        std::fs::write(&path, format!("{REFS_FORMAT} 10\n{id} {other}\n")).expect("write");
        assert_eq!(
            read_table(&path, 10).expect("read"),
            Some(vec![(id.clone(), vec![other.clone()])])
        );
        assert_eq!(read_table(&path, 11).expect("read"), None, "大きさが違う");
        std::fs::write(&path, format!("{REFS_FORMAT} 10\n{id} not-an-id\n")).expect("write");
        assert_eq!(read_table(&path, 10).expect("read"), None, "ID でない語");
        std::fs::write(&path, format!("{REFS_FORMAT} 10\n{id}\n")).expect("write");
        assert_eq!(read_table(&path, 10).expect("read"), None, "参照の無い行は書かない形");
        std::fs::write(&path, "something-else 10\n").expect("write");
        assert_eq!(read_table(&path, 10).expect("read"), None, "頭が違う");
        assert_eq!(read_table(&dir.join("missing"), 10).expect("read"), None, "無い");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
