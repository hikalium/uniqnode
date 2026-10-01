//! ストアの持続的な書き込みへの失敗の注入(テスト用の口。APPEND_FAILURE
//! (docs/plan/APPEND_FAILURE.md) の「関連して直すもの」)。
//!
//! このモジュールは cfg(debug_assertions) のビルドにだけ入る(lib.rs)。release には注入の型も、
//! `Store::inject_fault` も、abort の枝も無い。sync の記録(`UNIQNODE_SYNC_LOG`)と sync の
//! 本体は本番も使うので、ここではなく store.rs に置く。
//!
//! 注入は環境変数 `UNIQNODE_APPEND_FAULT=<種類>[@pack|@reflog]:<何回目>` で掛ける。`,` で区切って
//! 複数を並べられ(例 `torn:5@pack:1,marker-skip:2`)、各々が自分の数え値を持つ。プロセスの
//! 中の試験は `Store::inject_fault` で同じ字句を掛ける。何回目は 1 から数え、その種類が掛かり
//! うる操作だけを数える(追記の種類は追記ごと、`dirsync` と `crash-before-dirsync` は新しい
//! ファイルを作った追記ごと、`manifest` と `manifest-dirsync` は MANIFEST の書き込みごと、
//! `gc-dirsync` は GC の D の sync ごと、`crash-in-node-key` は node_key の作成ごと、
//! `open-sync` は開くときの sync の 1 つごと、`marker-*` は `open-marker` の書き込みごと)。
//! `@pack` と `@reflog` は追記の種類をその種類のセグメントに絞る。1 度当たったらそれきりである
//! (当たった時点でストアは書けない状態に入る)。

use crate::store::{sync_dir, sync_file_data, AtomicStep, ENOSPC};
use std::io::Write;
use std::path::Path;

const EIO: i32 = 5;

pub const APPEND_FAULT_ENV: &str = "UNIQNODE_APPEND_FAULT";

/// 注入の種類(APPEND_FAILURE の 10 個と、開くときの sync の試験が使う 2 つの abort と 1 つの
/// 誤りと、S1b の印の書き込みの 3 つ)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FaultKind {
    /// 1 バイトも書かずに誤り(EIO)。
    Before,
    /// n バイトだけ書いて誤り(ENOSPC。切り詰めは成功させる)。
    Torn(usize),
    /// 全部書いてから sync の位置で誤り(EIO。切り詰めは成功させる)。
    Sync,
    /// 全部書いてから sync の位置で誤り、切り詰めも失敗させる(完全な 1 本が残る)。
    SyncKeep,
    /// MANIFEST の rename の前で誤り。
    Manifest,
    /// MANIFEST の rename の後のディレクトリの sync で誤り。
    ManifestDirsync,
    /// 新しいセグメントを作った追記の、親ディレクトリの sync で誤り。
    Dirsync,
    /// sync の段で ENOSPC を返す(kind が io になる)。
    NospaceSync,
    /// 新しいセグメントの最初の追記の、ファイルの sync の後・親の sync の前で abort する。
    CrashBeforeDirsync,
    /// GC の D の最後の packs/ のディレクトリの sync で誤り。
    GcDirsync,
    /// 追記の全部を write した後・ファイルの sync の前で abort する(完全な 1 本が page cache
    /// にだけある形。次に開くときの sync が永続させる)。
    CrashBeforeSync,
    /// node_key を作る途中(tmp/ に書いて sync した後・rename の前)で abort する。
    CrashInNodeKey,
    /// 開くときの sync(node_key・追記中の pack と reflog・packs/・reflog/・データの
    /// ディレクトリ・その親を 1 つずつ)の 1 つを、行わずに誤り(EIO)にする。
    OpenSync,
    /// `open-marker` の書き込みの全長を書いた後、`fdatasync` を行わずに誤り(EIO)にする。
    MarkerSync,
    /// `open-marker` の書き込みを 1 バイトも行わずに誤り(EIO)にする(前の状態が残る)。
    MarkerSkip,
    /// `open-marker` の書き込みの先頭の一部だけを書いて誤り(EIO)にする(CRC が合わなくなる)。
    MarkerTorn,
}

impl FaultKind {
    fn is_append(&self) -> bool {
        matches!(
            self,
            FaultKind::Before
                | FaultKind::Torn(_)
                | FaultKind::Sync
                | FaultKind::SyncKeep
                | FaultKind::NospaceSync
                | FaultKind::CrashBeforeSync
        )
    }

    fn is_new_segment(&self) -> bool {
        matches!(self, FaultKind::Dirsync | FaultKind::CrashBeforeDirsync)
    }

    fn is_manifest(&self) -> bool {
        matches!(self, FaultKind::Manifest | FaultKind::ManifestDirsync)
    }

    fn is_marker(&self) -> bool {
        matches!(self, FaultKind::MarkerSync | FaultKind::MarkerSkip | FaultKind::MarkerTorn)
    }
}

/// 掛けた注入 1 つと、その数え値。
#[derive(Clone, Debug)]
struct Entry {
    kind: FaultKind,
    /// `@pack`・`@reflog` の絞り(セグメントのディレクトリ名)。None ならどちらにも掛かる。
    segment: Option<&'static str>,
    nth: u64,
    seen: u64,
}

impl Entry {
    fn count(&mut self) -> Option<FaultKind> {
        self.seen += 1;
        (self.seen == self.nth).then_some(self.kind)
    }
}

/// 掛けた注入の組(`,` で並べた字句の各々)。
#[derive(Clone, Debug)]
pub(crate) struct Fault {
    entries: Vec<Entry>,
}

const KINDS_TEXT: &str = "before・torn:<n>・sync・sync-keep・manifest・manifest-dirsync・dirsync・\
     nospace-sync・crash-before-dirsync・gc-dirsync・crash-before-sync・crash-in-node-key・\
     open-sync・marker-sync・marker-skip・marker-torn";

impl Fault {
    /// `<種類>[@pack|@reflog]:<何回目>` を `,` で並べたものを読む。`torn` は `torn:<n>` が種類の
    /// 字句である(例 `torn:5@pack:2`、`torn:5:1`)。
    pub(crate) fn parse(spec: &str) -> Result<Fault, String> {
        let entries = spec
            .split(',')
            .map(|one| parse_entry(spec, one))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Fault { entries })
    }

    /// 環境変数から読む。無ければ None、読めなければ誤り。
    pub(crate) fn from_env() -> Result<Option<Fault>, String> {
        match std::env::var(APPEND_FAULT_ENV) {
            Ok(spec) => Fault::parse(&spec).map(Some),
            Err(_) => Ok(None),
        }
    }

    /// 条件に合う全部の項目を数え、最初に当たった種類を返す。
    fn count_where(&mut self, applies: impl Fn(&Entry) -> bool) -> Option<FaultKind> {
        let mut fired = None;
        for entry in self.entries.iter_mut().filter(|entry| applies(entry)) {
            if let Some(kind) = entry.count() {
                fired.get_or_insert(kind);
            }
        }
        fired
    }

    /// 追記 1 回。`directory` はセグメントのディレクトリ名、`new_file` は新しいファイルを
    /// 作った追記か。当たれば種類を返す。
    pub(crate) fn on_append(&mut self, directory: &str, new_file: bool) -> Option<FaultKind> {
        self.count_where(|entry| {
            !entry.segment.is_some_and(|segment| segment != directory)
                && (entry.kind.is_append() || (entry.kind.is_new_segment() && new_file))
        })
    }

    /// MANIFEST の書き込み 1 回。
    pub(crate) fn on_manifest(&mut self) -> Option<FaultKind> {
        self.count_where(|entry| entry.kind.is_manifest())
    }

    /// GC の D の sync 1 回。
    pub(crate) fn on_gc_dirsync(&mut self) -> Option<FaultKind> {
        self.count_where(|entry| entry.kind == FaultKind::GcDirsync)
    }

    /// 開くときの sync 1 回。open-sync が当たれば、その sync の代わりに返す誤り。
    pub(crate) fn on_open_sync(&mut self) -> std::io::Result<()> {
        if self.count_where(|entry| entry.kind == FaultKind::OpenSync).is_some() {
            return Err(injected(EIO));
        }
        Ok(())
    }

    /// node_key の作成 1 回。crash-in-node-key が当たればここで abort する。
    pub(crate) fn on_node_key(&mut self) {
        if self.count_where(|entry| entry.kind == FaultKind::CrashInNodeKey).is_some() {
            crash("crash-in-node-key");
        }
    }

    /// `open-marker` の書き込み 1 回。当たれば種類を返す。
    pub(crate) fn on_marker(&mut self) -> Option<FaultKind> {
        self.count_where(|entry| entry.kind.is_marker())
    }
}

fn parse_entry(spec: &str, one: &str) -> Result<Entry, String> {
    let bad = |why: &str| {
        format!(
            "{APPEND_FAULT_ENV}={spec:?} が読めない({why})。形は <種類>[@pack|@reflog]:<何回目> を \
             , で並べたもので、種類は {KINDS_TEXT}"
        )
    };
    let (kind_text, segment, nth_text) = match one.split_once('@') {
        Some((kind_text, rest)) => {
            let (segment_text, nth_text) =
                rest.split_once(':').ok_or_else(|| bad("何回目が無い"))?;
            let segment = match segment_text {
                "pack" => crate::store::PACK.directory,
                "reflog" => crate::store::REFLOG.directory,
                _ => return Err(bad("@ の後は pack か reflog")),
            };
            (kind_text, Some(segment), nth_text)
        }
        None => {
            let (kind_text, nth_text) = one.rsplit_once(':').ok_or_else(|| bad("何回目が無い"))?;
            (kind_text, None, nth_text)
        }
    };
    let nth: u64 = nth_text.parse().map_err(|_| bad("何回目が数でない"))?;
    if nth == 0 {
        return Err(bad("何回目は 1 から"));
    }
    let kind = match kind_text {
        "before" => FaultKind::Before,
        "sync" => FaultKind::Sync,
        "sync-keep" => FaultKind::SyncKeep,
        "manifest" => FaultKind::Manifest,
        "manifest-dirsync" => FaultKind::ManifestDirsync,
        "dirsync" => FaultKind::Dirsync,
        "nospace-sync" => FaultKind::NospaceSync,
        "crash-before-dirsync" => FaultKind::CrashBeforeDirsync,
        "gc-dirsync" => FaultKind::GcDirsync,
        "crash-before-sync" => FaultKind::CrashBeforeSync,
        "crash-in-node-key" => FaultKind::CrashInNodeKey,
        "open-sync" => FaultKind::OpenSync,
        "marker-sync" => FaultKind::MarkerSync,
        "marker-skip" => FaultKind::MarkerSkip,
        "marker-torn" => FaultKind::MarkerTorn,
        other => match other.strip_prefix("torn:") {
            Some(n) => FaultKind::Torn(n.parse().map_err(|_| bad("torn:<n> の n が数でない"))?),
            None => return Err(bad("知らない種類")),
        },
    };
    if segment.is_some() && !(kind.is_append() || kind.is_new_segment()) {
        return Err(bad("@pack・@reflog は追記の種類にだけ付く"));
    }
    Ok(Entry { kind, segment, nth, seen: 0 })
}

/// abort する(試験が実プロセスを途中で落とすための口)。
fn crash(name: &str) -> ! {
    eprintln!("uniqnode: store: {APPEND_FAULT_ENV}={name} なので abort する(テスト用の口)");
    std::process::abort();
}

/// 注入した誤り(errno を持つ io::Error)。
fn injected(errno: i32) -> std::io::Error {
    std::io::Error::from_raw_os_error(errno)
}

/// 追記の write の段。当たっていなければ本来の write をする。
pub(crate) fn write_stage(
    fired: Option<FaultKind>,
    file: &mut std::fs::File,
    record: &[u8],
) -> std::io::Result<()> {
    match fired {
        Some(FaultKind::Before) => Err(injected(EIO)),
        Some(FaultKind::Torn(n)) => {
            file.write_all(&record[..n.min(record.len())]).and(Err(injected(ENOSPC)))
        }
        Some(FaultKind::CrashBeforeSync) => {
            file.write_all(record)?;
            crash("crash-before-sync")
        }
        _ => file.write_all(record),
    }
}

/// 追記の sync の段。当たっていなければ本来の sync_data をする(成功したら記録する)。
pub(crate) fn sync_stage(
    fired: Option<FaultKind>,
    file: &std::fs::File,
    path: &Path,
) -> std::io::Result<()> {
    match fired {
        Some(FaultKind::Sync | FaultKind::SyncKeep) => Err(injected(EIO)),
        Some(FaultKind::NospaceSync) => Err(injected(ENOSPC)),
        _ => sync_file_data(file, path),
    }
}

/// 切り詰めを失敗させるか(sync-keep)。
pub(crate) fn fails_cleanup(fired: Option<FaultKind>) -> bool {
    fired == Some(FaultKind::SyncKeep)
}

/// 新しいセグメントの親の sync の段。crash-before-dirsync ならここで abort する。
pub(crate) fn dirsync_stage(fired: Option<FaultKind>, parent: &Path) -> std::io::Result<()> {
    match fired {
        Some(FaultKind::CrashBeforeDirsync) => crash("crash-before-dirsync"),
        Some(FaultKind::Dirsync) => Err(injected(EIO)),
        _ => sync_dir(parent),
    }
}

/// MANIFEST の atomic_write の段ごとの誤り。
pub(crate) fn manifest_step(fired: Option<FaultKind>, step: AtomicStep) -> std::io::Result<()> {
    match (step, fired) {
        (AtomicStep::BeforeRename, Some(FaultKind::Manifest))
        | (AtomicStep::BeforeDirSync, Some(FaultKind::ManifestDirsync)) => Err(injected(EIO)),
        _ => Ok(()),
    }
}

/// GC の D の packs/ の sync の段。
pub(crate) fn gc_dirsync_stage(fired: Option<FaultKind>, packs: &Path) -> std::io::Result<()> {
    match fired {
        Some(_) => Err(injected(EIO)),
        None => sync_dir(packs),
    }
}

/// `open-marker` の書き込みの段。`write_at` が全長を書く本来の書き込み、`sync` が本来の
/// `fdatasync`。当たっていなければ両方を行う。
pub(crate) fn marker_stage(
    fired: Option<FaultKind>,
    bytes: &[u8],
    write_at: &mut dyn FnMut(&[u8]) -> std::io::Result<usize>,
    sync: &mut dyn FnMut() -> std::io::Result<()>,
) -> std::io::Result<usize> {
    match fired {
        Some(FaultKind::MarkerSkip) => Err(injected(EIO)),
        Some(FaultKind::MarkerTorn) => {
            write_at(&bytes[..bytes.len().min(40)])?;
            Err(injected(EIO))
        }
        Some(FaultKind::MarkerSync) => {
            write_at(bytes)?;
            Err(injected(EIO))
        }
        _ => {
            let written = write_at(bytes)?;
            sync()?;
            Ok(written)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(fault: &Fault) -> &Entry {
        assert_eq!(fault.entries.len(), 1);
        &fault.entries[0]
    }

    #[test]
    fn the_fault_spec_parses_kind_segment_and_count() {
        let fault = Fault::parse("torn:5@pack:2").unwrap();
        assert_eq!(only(&fault).kind, FaultKind::Torn(5));
        assert_eq!(only(&fault).segment, Some("packs"));
        assert_eq!(only(&fault).nth, 2);
        let fault = Fault::parse("torn:3:1").unwrap();
        assert_eq!(only(&fault).kind, FaultKind::Torn(3));
        assert_eq!(only(&fault).segment, None);
        assert_eq!(only(&Fault::parse("sync-keep@reflog:1").unwrap()).segment, Some("reflog"));
        assert_eq!(only(&Fault::parse("gc-dirsync:1").unwrap()).kind, FaultKind::GcDirsync);
        assert_eq!(only(&Fault::parse("open-sync:3").unwrap()).kind, FaultKind::OpenSync);
        assert_eq!(only(&Fault::parse("marker-torn:2").unwrap()).kind, FaultKind::MarkerTorn);
        for bad in [
            "",
            "sync",
            "sync:0",
            "sync:x",
            "nope:1",
            "sync@foo:1",
            "manifest@pack:1",
            "marker-sync@pack:1",
            "sync:1,",
        ] {
            assert!(Fault::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_count_includes_only_operations_the_kind_applies_to() {
        let mut fault = Fault::parse("dirsync@reflog:2").unwrap();
        assert_eq!(fault.on_append("reflog", false), None);
        assert_eq!(fault.on_append("packs", true), None);
        assert_eq!(fault.on_manifest(), None);
        assert_eq!(fault.on_append("reflog", true), None);
        assert_eq!(fault.on_append("reflog", true), Some(FaultKind::Dirsync));
        assert_eq!(fault.on_append("reflog", true), None);
    }

    #[test]
    fn listed_faults_count_independently() {
        let mut fault = Fault::parse("torn:5@pack:1,marker-skip:2").unwrap();
        assert_eq!(fault.on_marker(), None);
        assert_eq!(fault.on_append("packs", true), Some(FaultKind::Torn(5)));
        assert_eq!(fault.on_marker(), Some(FaultKind::MarkerSkip));
        assert_eq!(fault.on_marker(), None);
    }
}
