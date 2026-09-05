//! ストアのバックアップ(`uniqnode backup <data_dir> <backup_dir>`)。
//!
//! SPEC §5.1 の規律(データは「封印後不変のセグメント」と「atomic rename の MANIFEST」
//! だけ)を、無停止・増分のバックアップに還元したもの。ロックは取らない。走っている serve の
//! 隣で読むだけで足りる: 封印済みセグメントは二度と変わらず、アクティブなセグメントは
//! 追記されるだけなので、どの瞬間の写しも「有効なレコード列 + 書き込み途中の尻尾」であり、
//! 尻尾は CRC で切り詰められる(SPEC §5.2)。
//!
//! 順序が整合性を決める。MANIFEST を最初に読んで手元に留め、それが封印済みと言う
//! セグメントを写し、最後にその MANIFEST を写し先へ据える。MANIFEST を先に読むので、
//! 「封印済み」と記されたセグメントは写す時点で不変であり、写しは完全である。写す途中で
//! 封印が進んでも、その分は次回の増分に回るだけで、写し先が壊れることはない。
//! 写し先の MANIFEST を最後に据えるのは、途中で止まったときに写し先が「前回の写し +
//! 未封印のセグメント」として開けるようにするためである。
//!
//! 写し先にあって写し元に無い pack は、写し元に MANIFEST が在るときだけ削除する。封印済みの
//! 一覧は MANIFEST に完全に書かれているので、そこに無く写し元の packs/ にも無い番号は、
//! 回収(gc)で書き直された後の残骸である。残しておくと、写し先を開くときそれが未封印として
//! 走査され、torn tail があれば破損と誤判定され、回収前の孤児が写し先で生き返る。削除は
//! 写し先の MANIFEST を据える前に行う(据えた後に残骸が残ったまま止まると、写し先の回復が
//! 残骸を走査する)。黙って消さず、1 本ずつ名を言う(must/0022)。設定ファイルと reflog は
//! 消さない(理由は `reconcile_only_in_backup`)。
//!
//! 写した後は写し先をストアとして開き、fsck(全オブジェクトの再ハッシュ)まで通す。検証の
//! 実装は開くときと fsck そのものであり、バックアップ専用の検証は持たない(should/0135)。
//! 手順は [docs/mop/BACKUP.md](uuid:e026a5e7-1ece-4f4e-b6b8-ee96c62883a2)。

use crate::store::{
    self, atomic_write, FsckReport, SegmentKind, Store, StoreConfig, StoreError, MANIFEST_NAME,
    NODE_KEY_NAME, PACK, REFLOG,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// ストアのデータではないが、失うと困るので一緒に写す設定ファイル(SPEC §5.1)。
/// node_key は必須(ストアを一度でも開けば在る)、残りは在れば写す。
pub const SETTINGS_FILES: [&str; 4] = [NODE_KEY_NAME, "node.json", "peers.json", "groups.json"];

/// 写さないデータディレクトリ直下の項目。derived/ は消しても作り直せる導出データ
/// (埋め込みのキャッシュ、写しの作業ファイル)、logs/ は運用ログ、tmp/ は atomic
/// rename の作業場である。
pub const NOT_COPIED_DIRECTORIES: [&str; 3] = ["derived", "logs", "tmp"];

/// 1 回のバックアップで何が起きたか。命令の出力はこれをそのまま並べる。
#[derive(Debug, Default)]
pub struct BackupReport {
    /// 今回写した封印済みセグメント(`packs/pack-000001.pack` の形)。
    pub sealed_copied: Vec<String>,
    /// 写し先に同じ大きさで在ったので写さなかった封印済みセグメント。
    pub sealed_unchanged: Vec<String>,
    /// 毎回写す未封印(アクティブ)のセグメント。
    pub active_copied: Vec<String>,
    /// 写した設定ファイル。
    pub settings_copied: Vec<String>,
    /// 写し元には無く写し先だけに残っている設定ファイル(消さずに言う)。
    pub settings_only_in_backup: Vec<String>,
    /// 写し元の MANIFEST に無く packs/ にも無いので、写し先から削除した pack(回収済みの残骸)。
    pub packs_removed: Vec<String>,
    /// 写し元には無く写し先だけに残っているセグメントのうち、消さなかったもの: 写し元に
    /// MANIFEST が無い(権威が無い)ときの pack と、reflog のすべて。
    pub segments_only_in_backup: Vec<String>,
    /// 写し元の直下にあって写さなかった項目(derived/ logs/ tmp/ と、知らない名前)。
    pub not_copied: Vec<String>,
    /// 今回書いたバイト数。
    pub copied_bytes: u64,
    /// 検証で開いたときに切り詰められた、写し先のセグメントの末尾(名前, バイト数)。
    /// 写し元で書き込み途中だったレコードの尻尾で、次回の増分で完全な形が写る。
    pub torn_tails_cut: Vec<(String, u64)>,
    /// 写し先を開いて fsck した結果。
    pub verification: FsckReport,
}

impl BackupReport {
    /// 写し先がストアとして開け、fsck に異常が無いか。
    pub fn is_clean(&self) -> bool {
        self.verification.errors.is_empty()
    }
}

/// 写し元のバイト列を写し先の tmp/ に書き、fsync してから rename で据える。途中で止まって
/// も、据えた名前の下に不完全なファイルは現れない。許可ビットは写し元に合わせる
/// (node_key の 0600 を保つ)。返り値は写したバイト数。
fn copy_into_place(source: &Path, destination_dir: &Path, relative: &Path) -> store::Result<u64> {
    let target = destination_dir.join(relative);
    let tmp_path = destination_dir
        .join("tmp")
        .join(format!("backup-{}", std::process::id()));
    let mut input = std::fs::File::open(source)?;
    let mut written = 0u64;
    {
        let mut output = std::fs::File::create(&tmp_path)?;
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            written += read as u64;
        }
        output.set_permissions(std::fs::metadata(source)?.permissions())?;
        output.sync_all()?;
    }
    std::fs::rename(&tmp_path, &target)?;
    std::fs::File::open(target.parent().expect("親ディレクトリがある"))?.sync_all()?;
    Ok(written)
}

/// 種類ごとのセグメントを写す。sealed は写し元の MANIFEST(手元に留めた写し)が封印済みと
/// 言う番号。封印済みで写し先に同じ大きさのものが在れば写さない(封印後は不変で、内容は
/// 後の fsck が読み直す)。未封印のものは毎回まるごと写す。返り値は写し元にあった番号
/// (封印済みをすべて含むことを確かめてある)で、写し先に在るべきセグメントの全体である。
fn copy_segments(
    kind: SegmentKind,
    source: &Path,
    destination: &Path,
    sealed: &[u64],
    report: &mut BackupReport,
) -> store::Result<Vec<u64>> {
    std::fs::create_dir_all(destination.join(kind.directory))?;
    let present = kind.numbers(source)?;
    for number in sealed {
        if !present.contains(number) {
            return Err(StoreError::Corruption(format!(
                "写し元の MANIFEST が封印済みと言う {} が無い",
                kind.file_name(*number)
            )));
        }
    }
    for number in present.iter().copied() {
        let relative = PathBuf::from(kind.directory).join(kind.file_name(number));
        let label = relative.display().to_string();
        let source_path = source.join(&relative);
        if sealed.contains(&number) {
            let source_length = std::fs::metadata(&source_path)?.len();
            let destination_length = match std::fs::metadata(destination.join(&relative)) {
                Ok(metadata) => Some(metadata.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if destination_length == Some(source_length) {
                report.sealed_unchanged.push(label);
                continue;
            }
            report.copied_bytes += copy_into_place(&source_path, destination, &relative)?;
            report.sealed_copied.push(label);
        } else {
            report.copied_bytes += copy_into_place(&source_path, destination, &relative)?;
            report.active_copied.push(label);
        }
    }
    Ok(present)
}

/// 写し先にあって写し元に無いセグメントを、削除するか名を言うだけにする。`in_source` は
/// copy_segments が返した写し元の番号(封印済みとアクティブの全体)。
///
/// pack は `remove` のとき(写し元に MANIFEST が在るとき)削除する。封印済みの一覧が MANIFEST
/// に完全に書かれているので、そこに無く写し元にも無い番号は回収済みの残骸で、MANIFEST が
/// 権威である。MANIFEST が無い(初回封印前の)写し元には権威が無いので消さず、名を言う
/// (PACK_GC.md の回復の規則と同じ場合分け)。
///
/// reflog は消さない。reflog に回収は無く(PACK_GC.md 未決事項 8 で別の問題)、封印も
/// まだ実装されていない(sealed_reflogs は常に空)ので、「MANIFEST に無い」は reflog の
/// 全部に当たり、pack と同じ規則を対称のために当てると署名済みの ref の記録を根拠なく
/// 消すことになる。残骸を作る仕組みが無いところで消す規則を持たず、在れば名を言う。
fn reconcile_only_in_backup(
    kind: SegmentKind,
    destination: &Path,
    in_source: &[u64],
    remove: bool,
    report: &mut BackupReport,
) -> store::Result<()> {
    let mut removed_any = false;
    for number in kind.numbers(destination)? {
        if in_source.contains(&number) {
            continue;
        }
        let relative = PathBuf::from(kind.directory).join(kind.file_name(number));
        let label = relative.display().to_string();
        if remove {
            std::fs::remove_file(destination.join(&relative))?;
            removed_any = true;
            report.packs_removed.push(label);
        } else {
            report.segments_only_in_backup.push(label);
        }
    }
    if removed_any {
        // 削除を MANIFEST の rename より先に永続化する(MANIFEST が据わった後に残骸だけ
        // 戻ってくる順序を作らない)。
        std::fs::File::open(destination.join(kind.directory))?.sync_all()?;
    }
    Ok(())
}

/// 写し元と写し先が同じ場所、または写し先が別のノードの写しなら断る。
fn refuse_wrong_destination(source: &Path, destination: &Path) -> store::Result<()> {
    if destination.exists() {
        let same_place = std::fs::canonicalize(source)? == std::fs::canonicalize(destination)?;
        if same_place {
            return Err(StoreError::Invalid(format!(
                "写し元と写し先が同じ場所を指している: {}",
                source.display()
            )));
        }
    }
    let destination_key = destination.join(NODE_KEY_NAME);
    if destination_key.exists()
        && std::fs::read(&destination_key)? != std::fs::read(source.join(NODE_KEY_NAME))?
    {
        return Err(StoreError::Invalid(format!(
            "{} は別のノードの写し(node_key が写し元と違う)",
            destination.display()
        )));
    }
    Ok(())
}

/// 写し先をストアとして開き(未封印の尻尾はここで切り詰められる)、fsck する。写しが健全か
/// という判断はここ 1 箇所にあり、backup 命令の最後の検証と、install が timer の unit を
/// 1 回走らせた後に写し先を見る確認が同じ答えを見る(should/0135)。開くのは在るストア
/// だけの道: 写しが写っていなければ(install が unit を走らせたのに写し先が空、など)、
/// 空のストアを作って緑と言うのではなく、ストアではないと断る。
pub fn verify_copy(destination: &Path) -> store::Result<FsckReport> {
    let backup_store = Store::open_existing(StoreConfig::new(destination))?;
    backup_store.fsck()
}

/// バックアップを 1 回行い、写し先を開いて fsck した結果まで返す。ロックは取らない(serve が
/// 走っていてよい)。写し先の異常は Err ではなく報告の verification に載る(fsck 命令と
/// 同じ扱い)。開けないほど壊れていれば Err。
pub fn run(source: &Path, destination: &Path) -> store::Result<BackupReport> {
    store::require_store_dir(source)?;
    std::fs::create_dir_all(destination.join("tmp"))?;
    refuse_wrong_destination(source, destination)?;

    // MANIFEST は最初に読んで手元に留める。以後の判断はすべてこの写しに従う。
    let manifest = match std::fs::read(source.join(MANIFEST_NAME)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let (sealed_packs, sealed_reflogs) = match &manifest {
        Some(bytes) => store::parse_manifest(bytes)?,
        None => (Vec::new(), Vec::new()),
    };
    if manifest.is_none() && destination.join(MANIFEST_NAME).exists() {
        return Err(StoreError::Invalid(format!(
            "写し先 {} には MANIFEST があるが写し元には無い(別のストアの写し先ではないか)",
            destination.display()
        )));
    }

    let mut report = BackupReport::default();
    let packs_in_source = copy_segments(PACK, source, destination, &sealed_packs, &mut report)?;
    let reflogs_in_source =
        copy_segments(REFLOG, source, destination, &sealed_reflogs, &mut report)?;
    // 残骸の削除は写し先の MANIFEST を据える前に行う(モジュール冒頭)。
    reconcile_only_in_backup(
        PACK,
        destination,
        &packs_in_source,
        manifest.is_some(),
        &mut report,
    )?;
    reconcile_only_in_backup(REFLOG, destination, &reflogs_in_source, false, &mut report)?;

    for name in SETTINGS_FILES {
        let source_path = source.join(name);
        if source_path.exists() {
            report.copied_bytes += copy_into_place(&source_path, destination, Path::new(name))?;
            report.settings_copied.push(name.to_string());
        } else if destination.join(name).exists() {
            report.settings_only_in_backup.push(name.to_string());
        }
    }

    if let Some(bytes) = &manifest {
        atomic_write(destination, &destination.join(MANIFEST_NAME), bytes)?;
        report.copied_bytes += bytes.len() as u64;
    }

    for entry in std::fs::read_dir(source)? {
        let name = entry?.file_name().to_string_lossy().to_string();
        let copied = name == MANIFEST_NAME
            || name == PACK.directory
            || name == REFLOG.directory
            || SETTINGS_FILES.contains(&name.as_str());
        if !copied {
            report.not_copied.push(name);
        }
    }
    report.not_copied.sort();

    // 検証: 写し先をストアとして開き(未封印の尻尾はここで切り詰められる)、fsck する。
    let active_lengths: Vec<(String, u64)> = report
        .active_copied
        .iter()
        .map(|relative| {
            let length = std::fs::metadata(destination.join(relative))?.len();
            Ok((relative.clone(), length))
        })
        .collect::<store::Result<_>>()?;
    report.verification = verify_copy(destination)?;
    for (relative, before) in active_lengths {
        let after = std::fs::metadata(destination.join(&relative))?.len();
        if after < before {
            report.torn_tails_cut.push((relative, before - after));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "uniqnode-backup-unit-{}-{name}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        dir
    }

    /// 写し先が別のノードの写しなら断る。混ぜると、復元したノードがどちらの鍵で署名して
    /// いたのか分からなくなる。
    #[test]
    fn a_backup_of_another_node_is_refused_as_a_destination() {
        let source = temp_dir("refuse-source");
        let other = temp_dir("refuse-other");
        drop(Store::open(StoreConfig::new(&source)).expect("open source"));
        drop(Store::open(StoreConfig::new(&other)).expect("open other"));
        match run(&source, &other) {
            Err(StoreError::Invalid(message)) => {
                assert!(message.contains("別のノード"), "{message}")
            }
            other_outcome => panic!("別のノードの写しへは断るべき: {other_outcome:?}"),
        }
        std::fs::remove_dir_all(&source).expect("cleanup");
        std::fs::remove_dir_all(&other).expect("cleanup");
    }

    /// 写し元と写し先が同じ場所なら断る(自分の上に自分を写して壊さない)。
    #[test]
    fn the_same_directory_is_refused_as_a_destination() {
        let source = temp_dir("refuse-same");
        drop(Store::open(StoreConfig::new(&source)).expect("open source"));
        match run(&source, &source) {
            Err(StoreError::Invalid(message)) => assert!(message.contains("同じ場所"), "{message}"),
            other_outcome => panic!("同じ場所へは断るべき: {other_outcome:?}"),
        }
        std::fs::remove_dir_all(&source).expect("cleanup");
    }

    /// ストアでない場所を写し元に指されたら、鍵と packs/ が要ると言って断る。
    #[test]
    fn a_directory_that_is_not_a_store_is_refused_as_a_source() {
        let source = temp_dir("refuse-not-store");
        std::fs::create_dir_all(&source).expect("mkdir");
        let destination = temp_dir("refuse-not-store-destination");
        match run(&source, &destination) {
            Err(StoreError::Invalid(message)) => {
                assert!(message.contains("データディレクトリではない"), "{message}")
            }
            other_outcome => panic!("ストアでない写し元は断るべき: {other_outcome:?}"),
        }
        std::fs::remove_dir_all(&source).expect("cleanup");
    }
}
