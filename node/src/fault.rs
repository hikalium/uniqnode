//! ストアの持続的な書き込みへの失敗の注入と、sync の呼び出しの記録(テスト用の口。
//! APPEND_FAILURE (docs/plan/APPEND_FAILURE.md) の「関連して直すもの」)。
//!
//! 注入は環境変数 `UNIQNODE_APPEND_FAULT=<種類>[@pack|@reflog]:<何回目>` で掛ける。読むのは
//! cfg(debug_assertions) のビルドだけで、release には入らない。プロセスの中の試験は
//! `Store::inject_fault` で同じ字句を掛ける。何回目は 1 から数え、その種類が掛かりうる操作だけを
//! 数える(追記の種類は追記ごと、`dirsync` と `crash-before-dirsync` は新しいファイルを作った
//! 追記ごと、`manifest` と `manifest-dirsync` は MANIFEST の書き込みごと、`gc-dirsync` は GC の D の
//! sync ごと)。`@pack` と `@reflog` は追記の種類をその種類のセグメントに絞る。1 度当たったら
//! それきりである(当たった時点でストアは書けない状態に入る)。
//!
//! sync の記録は環境変数 `UNIQNODE_SYNC_LOG=<道>` で、ストアと install が行う sync を 1 行ずつ
//! その道へ追記する(`file <道>` か `dir <道>`)。開くたびに何が sync されたかを、実プロセスの
//! 試験が数えるための口である。これも debug ビルドだけが読む。

use std::path::Path;

pub const APPEND_FAULT_ENV: &str = "UNIQNODE_APPEND_FAULT";
pub const SYNC_LOG_ENV: &str = "UNIQNODE_SYNC_LOG";

/// 注入の種類(APPEND_FAILURE の 10 個)。
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
        )
    }

    fn is_new_segment(&self) -> bool {
        matches!(self, FaultKind::Dirsync | FaultKind::CrashBeforeDirsync)
    }

    fn is_manifest(&self) -> bool {
        matches!(self, FaultKind::Manifest | FaultKind::ManifestDirsync)
    }
}

/// 掛けた注入 1 つと、その数え値。
#[derive(Clone, Debug)]
pub(crate) struct Fault {
    kind: FaultKind,
    /// `@pack`・`@reflog` の絞り(セグメントのディレクトリ名)。None ならどちらにも掛かる。
    segment: Option<&'static str>,
    nth: u64,
    seen: u64,
}

impl Fault {
    /// `<種類>[@pack|@reflog]:<何回目>` を読む。`torn` は `torn:<n>` が種類の字句である
    /// (例 `torn:5@pack:2`、`torn:5:1`)。
    pub(crate) fn parse(spec: &str) -> Result<Fault, String> {
        let bad = |why: &str| {
            format!(
                "{APPEND_FAULT_ENV}={spec:?} が読めない({why})。形は <種類>[@pack|@reflog]:<何回目> で、\
                 種類は before・torn:<n>・sync・sync-keep・manifest・manifest-dirsync・dirsync・\
                 nospace-sync・crash-before-dirsync・gc-dirsync"
            )
        };
        let (kind_text, segment, nth_text) = match spec.split_once('@') {
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
                let (kind_text, nth_text) =
                    spec.rsplit_once(':').ok_or_else(|| bad("何回目が無い"))?;
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
            other => match other.strip_prefix("torn:") {
                Some(n) => FaultKind::Torn(n.parse().map_err(|_| bad("torn:<n> の n が数でない"))?),
                None => return Err(bad("知らない種類")),
            },
        };
        if segment.is_some() && !(kind.is_append() || kind.is_new_segment()) {
            return Err(bad("@pack・@reflog は追記の種類にだけ付く"));
        }
        Ok(Fault { kind, segment, nth, seen: 0 })
    }

    /// 環境変数から読む(debug ビルドだけ)。無ければ None、読めなければ誤り。
    pub(crate) fn from_env() -> Result<Option<Fault>, String> {
        #[cfg(debug_assertions)]
        {
            match std::env::var(APPEND_FAULT_ENV) {
                Ok(spec) => Fault::parse(&spec).map(Some),
                Err(_) => Ok(None),
            }
        }
        #[cfg(not(debug_assertions))]
        {
            Ok(None)
        }
    }

    fn count(&mut self) -> Option<FaultKind> {
        self.seen += 1;
        (self.seen == self.nth).then_some(self.kind)
    }

    /// 追記 1 回。`directory` はセグメントのディレクトリ名、`new_file` は新しいファイルを
    /// 作った追記か。当たれば種類を返す。
    pub(crate) fn on_append(&mut self, directory: &str, new_file: bool) -> Option<FaultKind> {
        if self.segment.is_some_and(|segment| segment != directory) {
            return None;
        }
        if self.kind.is_append() || (self.kind.is_new_segment() && new_file) {
            return self.count();
        }
        None
    }

    /// MANIFEST の書き込み 1 回。
    pub(crate) fn on_manifest(&mut self) -> Option<FaultKind> {
        if self.kind.is_manifest() {
            return self.count();
        }
        None
    }

    /// GC の D の sync 1 回。
    pub(crate) fn on_gc_dirsync(&mut self) -> Option<FaultKind> {
        if self.kind == FaultKind::GcDirsync {
            return self.count();
        }
        None
    }
}

/// 注入した誤り(errno を持つ io::Error)。
pub(crate) fn injected(errno: i32) -> std::io::Error {
    std::io::Error::from_raw_os_error(errno)
}

pub(crate) const EIO: i32 = 5;
pub(crate) const ENOSPC: i32 = 28;
pub(crate) const EDQUOT: i32 = 122;

/// sync を 1 回記録する(debug ビルドで SYNC_LOG_ENV があるときだけ)。記録に失敗しても
/// 本来の sync の結果は変えない。
pub(crate) fn note_sync(what: &str, path: &Path) {
    #[cfg(debug_assertions)]
    {
        if let Some(log) = std::env::var_os(SYNC_LOG_ENV) {
            use std::io::Write;
            if let Ok(mut file) =
                std::fs::OpenOptions::new().create(true).append(true).open(log)
            {
                let _ = file.write_all(format!("{what} {}\n", path.display()).as_bytes());
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (what, path);
    }
}

/// ディレクトリを開いて sync する(記録つき)。
pub(crate) fn sync_dir(path: &Path) -> std::io::Result<()> {
    note_sync("dir", path);
    std::fs::File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fault_spec_parses_kind_segment_and_count() {
        let fault = Fault::parse("torn:5@pack:2").unwrap();
        assert_eq!(fault.kind, FaultKind::Torn(5));
        assert_eq!(fault.segment, Some("packs"));
        assert_eq!(fault.nth, 2);
        let fault = Fault::parse("torn:3:1").unwrap();
        assert_eq!(fault.kind, FaultKind::Torn(3));
        assert_eq!(fault.segment, None);
        assert_eq!(Fault::parse("sync-keep@reflog:1").unwrap().segment, Some("reflog"));
        assert_eq!(Fault::parse("gc-dirsync:1").unwrap().kind, FaultKind::GcDirsync);
        for bad in ["", "sync", "sync:0", "sync:x", "nope:1", "sync@foo:1", "manifest@pack:1"] {
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
}
