//! 単一DBノードのストレージ(SPEC §5)。
//!
//! ディスク上のファイルは2種類だけ: 追記専用で封印(seal)後は不変のセグメントと、
//! atomic rename で差し替える MANIFEST。オブジェクトは「バイト列」であり、構造化
//! オブジェクトか blob かの区別は保存しない(c1 として解釈できるかは読み手が決める。
//! ID はどちらも同じ規則 = バイト列の SHA-256 なので衝突しない)。
//!
//! 書き込み順序(SPEC §5.3): オブジェクト追記+fsync → ref 追記+fsync → メモリ適用。
//! この順序により「ref が指す先が存在しない」状態はクラッシュを挟んでも生じない。

use crate::c1;
use crate::clock::unix_now;
use crate::crc32::crc32;
use crate::ed25519;
use crate::sha2;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    /// 封印済みセグメントの破損など、自動回復してはならない状態。
    Corruption(String),
    /// 呼び出し側の誤り(存在しない対象への ref、名前空間違反など)。
    Invalid(String),
    /// 持続的な書き込みの I/O が 1 度失敗したので、このプロセスではもう書かない
    /// (APPEND_FAILURE の方針 2)。書けない状態に入れた要求自身もこれを受け取る。
    WritesDisabled(WriteFailure),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io: {e}"),
            StoreError::Corruption(m) => write!(f, "corruption: {m}"),
            StoreError::Invalid(m) => write!(f, "invalid: {m}"),
            StoreError::WritesDisabled(failure) => write!(f, "{}", failure.message()),
        }
    }
}

/// 書けない状態に入れた操作の段(APPEND_FAILURE の方針 2)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOp {
    /// 追記の write(と、開いた後の fstat)。
    Write,
    /// 追記の sync。
    Sync,
    /// 新しいセグメントの親ディレクトリの sync と、GC の D の packs/ の sync。
    DirSync,
    /// MANIFEST の書き込み(封印)。
    Manifest,
    /// GC の確定(C-1 より後の rename・ディレクトリの sync・MANIFEST)。
    GcCommit,
    /// 書く前のファイル長がメモリの数え値と違った(方針 3 のずれの検算)。
    LengthMismatch,
}

impl WriteOp {
    pub fn name(&self) -> &'static str {
        match self {
            WriteOp::Write => "write",
            WriteOp::Sync => "sync",
            WriteOp::DirSync => "dir_sync",
            WriteOp::Manifest => "manifest",
            WriteOp::GcCommit => "gc_commit",
            WriteOp::LengthMismatch => "length_mismatch",
        }
    }
}

/// 失敗した追記の切り詰め(best-effort)の結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleanup {
    Ok,
    Failed,
    NotTried,
}

impl Cleanup {
    pub fn name(&self) -> &'static str {
        match self {
            Cleanup::Ok => "ok",
            Cleanup::Failed => "failed",
            Cleanup::NotTried => "not_tried",
        }
    }
}

/// 戻し方を分ける種類(方針 5)。NoSpace は、write の段の ENOSPC か EDQUOT で、切り詰めと
/// その sync が成功したときだけ。他は全部 Io。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    NoSpace,
    Io,
}

impl FailureKind {
    pub fn name(&self) -> &'static str {
        match self {
            FailureKind::NoSpace => "no_space",
            FailureKind::Io => "io",
        }
    }
}

/// 書けない状態の中身。最初の失敗の原因を持つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteFailure {
    pub reason: String,
    pub op: WriteOp,
    pub errno: Option<i32>,
    pub cleanup: Cleanup,
    pub kind: FailureKind,
    /// 入った時刻(unix 秒)。
    pub since: i64,
}

/// kind が no_space のときの戻し方の案内(方針 5)。試験はこの定数で照らす(must/0023)。
pub const NO_SPACE_GUIDANCE: &str = "ストアを置いたファイルシステムに空きを作ってから serve を再起動する\
     (空きを足しても自動では戻らない)";

/// kind が io のときの戻し方の案内(方針 5)。
pub const IO_GUIDANCE: &str = "ホストを再起動する(できなければ、ストアを置いたファイルシステムを umount して \
     fsck し、mount し直してから起こす)。serve の再起動だけでは、page cache に残った\
     書けなかった末尾の後ろに書くことになりうる";

impl WriteFailure {
    /// 戻し方の案内(方針 5)。
    pub fn guidance(&self) -> &'static str {
        match self.kind {
            FailureKind::NoSpace => NO_SPACE_GUIDANCE,
            FailureKind::Io => IO_GUIDANCE,
        }
    }

    /// 誤りの文(HTTP の 503 の `error`、CLI の誤り、MCP の誤りの文が同じこれを言う)。
    pub fn message(&self) -> String {
        format!(
            "writes disabled ({}): {}; {}",
            self.kind.name(),
            self.reason,
            self.guidance()
        )
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub data_dir: PathBuf,
    /// アクティブ pack がこのサイズを超えたら封印する。
    pub pack_seal_bytes: u64,
    /// 1レコードの上限(len フィールドの暴走値からの防御)。
    pub max_record_bytes: u32,
    /// オブジェクト合計の容量上限。None = 無制限。超える put は拒否される
    /// (機会層の evict と pack の物理回収は docs/plan/RAG.md の項目 8 (uuid:4ed764d8-a570-4809-bd0c-80de0d4b5545))。
    pub capacity_bytes: Option<u64>,
}

impl StoreConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> StoreConfig {
        StoreConfig {
            data_dir: data_dir.into(),
            pack_seal_bytes: 256 * 1024 * 1024,
            max_record_bytes: 64 * 1024 * 1024,
            capacity_bytes: None,
        }
    }
}

/// オブジェクトの置き場所(索引の値)。gc が pack ごとの集計に読む(GC
/// (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。
#[derive(Clone, Copy, Debug)]
pub struct ObjectLocation {
    pub pack_number: u64,
    /// レコードのペイロード先頭のファイル内オフセット。
    pub payload_offset: u64,
    pub payload_length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefState {
    pub target: Option<String>,
    pub seq: u64,
    pub at: i64,
}

/// 署名済みレコードの本体(SPEC §4.4, §4.5)。reflog は3種のレコードを同じ
/// per-signer seq の流れで運ぶため、pin と保持表明も L1 の同期でそのまま複製される。
enum RecordBody {
    SetRef { name: String, target: Option<String> },
    Pin { root: String, min_replicas: u32 },
    Attest { root: String, held: bool },
}

/// 検証済みレコード。
struct VerifiedRecord {
    body: RecordBody,
    seq: u64,
    at: i64,
    signer: String,
}

enum Verified {
    New(VerifiedRecord),
    AlreadyKnown,
}

pub struct Store {
    config: StoreConfig,
    secret_seed: [u8; 32],
    node_id_hex: String,
    /// オブジェクトID → 位置。導出データ(起動時に pack 走査で再構築)。
    object_index: BTreeMap<String, ObjectLocation>,
    refs: BTreeMap<String, RefState>,
    /// pin: root → (署名者 → min_replicas)。実効値は最大値(SPEC §4.5)。
    pins: BTreeMap<String, BTreeMap<String, u32>>,
    /// 保持表明: root → (保持者 → held)。false は撤回の記録。
    attests: BTreeMap<String, BTreeMap<String, bool>>,
    /// 全オブジェクトの合計バイト数(容量会計)。
    used_bytes: u64,
    /// 署名者(DBノード)ごとの適用済み最終 seq。レプリケーションのカーソル(I5)。
    signer_last_seq: BTreeMap<String, u64>,
    sealed_packs: Vec<u64>,
    sealed_reflogs: Vec<u64>,
    active_pack_number: u64,
    active_pack_length: u64,
    active_reflog_number: u64,
    /// オブジェクトの集合が変わるたびに進む番号(put で 1 つ増える、gc の回収で 1 つ増える)。
    /// 導出データ(逆引き索引)が「作ってから集合が変わっていないか」を見る札。件数で見ると、
    /// 回収で k 件減った後に k 件増えた集合を同じと誤認する。
    object_generation: u64,
    /// gc が A と C の間に置く記録: put_object が「既に在る」と答えた ID。回収の対象に
    /// なっている孤児を、呼び手が put し直して ref を張ろうとしている(SPEC §5.3 の順序:
    /// オブジェクト → ref)途中かもしれないので、C はこれを生きているものとして写し足す
    /// (GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。None なら記録しない。
    gc_touched: Option<std::collections::BTreeSet<String>>,
    /// 回収が走っている(S から D の終わりまで)。同じストアに 2 つの回収を重ねない。
    gc_running: bool,
    /// 書けない状態(APPEND_FAILURE の方針 2)。Some なら全ての書き込みの入口が断る。戻るのは
    /// ストアを開き直したときだけである。
    write_failure: Option<WriteFailure>,
    /// 失敗の注入(テスト用の口。node/src/fault.rs)。debug ビルドだけにある。
    #[cfg(debug_assertions)]
    fault: Option<crate::fault::Fault>,
    /// 同一データディレクトリの二重オープン防止(プロセス終了で自動解放される
    /// Linux 抽象名前空間ソケットをロックとして使う)。
    _lock: std::os::unix::net::UnixListener,
}

/// 封印済みセグメントの一覧を持つファイル(SPEC §5.2)。
pub(crate) const MANIFEST_NAME: &str = "MANIFEST";

/// ノード鍵(ed25519 の秘密シード 32 バイト)。ストアのデータではないが、失うとこのノードの
/// 名前空間の ref に二度と署名できなくなる。
pub(crate) const NODE_KEY_NAME: &str = "node_key";

/// 追記専用セグメントの種類(pack と reflog)。置き場・名前の形・番号の列挙を 1 箇所で
/// 決め、回復とバックアップが同じ答えを読む(should/0135)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SegmentKind {
    /// データディレクトリ直下のサブディレクトリ名。
    pub(crate) directory: &'static str,
    prefix: &'static str,
    suffix: &'static str,
}

pub(crate) const PACK: SegmentKind =
    SegmentKind { directory: "packs", prefix: "pack-", suffix: ".pack" };
pub(crate) const REFLOG: SegmentKind =
    SegmentKind { directory: "reflog", prefix: "reflog-", suffix: ".log" };

impl SegmentKind {
    pub(crate) fn file_name(&self, number: u64) -> String {
        format!("{}{number:06}{}", self.prefix, self.suffix)
    }

    pub(crate) fn path(&self, dir: &Path, number: u64) -> PathBuf {
        dir.join(self.directory).join(self.file_name(number))
    }

    /// データディレクトリにあるこの種類のセグメント番号を昇順で返す。名前の形に合わない
    /// ファイルは破損として拒む(黙って飛ばすと、写し忘れや取り違えが見えなくなる)。
    pub(crate) fn numbers(&self, dir: &Path) -> Result<Vec<u64>> {
        let mut numbers = Vec::new();
        for entry in std::fs::read_dir(dir.join(self.directory))? {
            let name = entry?.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix(self.prefix) {
                if let Some(number_text) = rest.strip_suffix(self.suffix) {
                    match number_text.parse::<u64>() {
                        Ok(n) => numbers.push(n),
                        Err(_) => {
                            return Err(StoreError::Corruption(format!(
                                "解釈できないファイル名: {name}"
                            )))
                        }
                    }
                }
            }
        }
        numbers.sort_unstable();
        Ok(numbers)
    }
}

fn pack_path(dir: &Path, number: u64) -> PathBuf {
    PACK.path(dir, number)
}

fn reflog_path(dir: &Path, number: u64) -> PathBuf {
    REFLOG.path(dir, number)
}

/// 追記レコード: [u32 len][u32 crc32][payload]、いずれもリトルエンディアン。書くのは
/// Store::append_durable だけである(APPEND_FAILURE の「書き込みの口を寄せる」)。
fn encode_record(payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(8 + payload.len());
    record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    record.extend_from_slice(&crc32(payload).to_le_bytes());
    record.extend_from_slice(payload);
    record
}

struct RecordScan {
    /// (ペイロード先頭オフセット, ペイロード)の列。
    records: Vec<(u64, Vec<u8>)>,
    /// CRC 不一致・尻切れを検出した位置(そこまでが有効)。
    valid_length: u64,
    truncated: bool,
}

fn scan_records(path: &Path, max_record_bytes: u32) -> Result<RecordScan> {
    scan_records_from(path, max_record_bytes, 0)
}

/// ファイル内オフセット `from` から末尾までのレコードを読む。オフセットは絶対値で返す
/// (from = 0 なら scan_records と同じ)。gc が「前回見た位置より後に追記された分」だけを
/// 解析するのに使う。from がファイルの長さを超えていれば空。
fn scan_records_from(path: &Path, max_record_bytes: u32, from: u64) -> Result<RecordScan> {
    let mut file = std::fs::File::open(path)?;
    let total = file.metadata()?.len();
    if from >= total {
        return Ok(RecordScan { records: Vec::new(), valid_length: total, truncated: false });
    }
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::with_capacity((total - from) as usize);
    file.read_to_end(&mut bytes)?;
    let mut records = Vec::new();
    let mut position = 0usize;
    loop {
        if position + 8 > bytes.len() {
            break;
        }
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&bytes[position..position + 4]);
        let length = u32::from_le_bytes(len_bytes);
        let mut crc_bytes = [0u8; 4];
        crc_bytes.copy_from_slice(&bytes[position + 4..position + 8]);
        let expected_crc = u32::from_le_bytes(crc_bytes);
        if length > max_record_bytes {
            break;
        }
        let payload_start = position + 8;
        let payload_end = payload_start + length as usize;
        if payload_end > bytes.len() {
            break;
        }
        let payload = &bytes[payload_start..payload_end];
        if crc32(payload) != expected_crc {
            break;
        }
        records.push((from + payload_start as u64, payload.to_vec()));
        position = payload_end;
    }
    Ok(RecordScan {
        records,
        valid_length: from + position as u64,
        truncated: (position as u64) < bytes.len() as u64,
    })
}

/// オブジェクトのバイト列が持つ参照(SPEC §4.3: c1 値の中の `s256:` + 16 進 64 桁の
/// 文字列)。c1 として解釈できないバイト列(生 blob)は定義上参照を持たない。閉包の探索・
/// 逆引き索引・gc の参照表がみな同じこの 1 つを呼ぶ(should/0135)。
pub fn references_in(bytes: &[u8]) -> Vec<String> {
    let mut references = Vec::new();
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(value) = c1::parse(text) {
            c1::collect_references(&value, &mut references);
        }
    }
    references
}

/// 根から参照を辿った到達閉包。`references_of(id)` は、ローカルに在るオブジェクトなら
/// その参照の列を Some で、無ければ None を返す。None(dangling)は結果に含めず、その先も
/// 辿らない(開世界: 世界が忘れた参照は無害)。参照の出どころ(オブジェクトを読んで解釈
/// するか、解析済みの表を引くか)は呼び手が決め、探索の規約はここ 1 箇所にある
/// (should/0135)。結果は ID 昇順。
pub fn closure_over<'a, F>(
    roots: impl IntoIterator<Item = &'a str>,
    mut references_of: F,
) -> Result<std::collections::BTreeSet<String>>
where
    F: FnMut(&str) -> Result<Option<Vec<String>>>,
{
    let mut seen = std::collections::BTreeSet::new();
    let mut present = std::collections::BTreeSet::new();
    let mut queue: Vec<String> = roots.into_iter().map(str::to_string).collect();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let references = match references_of(&id)? {
            None => continue,
            Some(references) => references,
        };
        for r in references {
            if !seen.contains(&r) {
                queue.push(r);
            }
        }
        present.insert(id);
    }
    Ok(present)
}

/// (オブジェクト ID, そのオブジェクトが持つ参照先の列) の列。参照表の中身。
pub type ReferenceEntries = Vec<(String, Vec<String>)>;

/// 回収の新 pack に写したオブジェクト: (ID, ペイロードのオフセット, 長さ)。
pub type CopiedObject = (String, u64, u32);

/// 1 本の pack の参照表の材料: レコードごとの (オブジェクト ID, 参照先の列) と、走査した
/// ファイルの長さ。gc がロックの外で封印済み pack(不変)を解析するのに使う
/// (GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。
pub(crate) struct PackReferences {
    pub(crate) entries: ReferenceEntries,
    pub(crate) file_length: u64,
    /// 末尾に不完全なレコードがあったか。封印済み pack なら破損、追記中の pack なら書き込み
    /// 途中を読んだだけ(有効な分までを返している)。
    pub(crate) truncated: bool,
}

/// pack のレコードを全部読み、ID と参照を取る。Store を借りずにデータディレクトリの道だけで
/// 動くので、ロックの外で呼べる。
pub(crate) fn scan_pack_references(
    dir: &Path,
    max_record_bytes: u32,
    number: u64,
) -> Result<PackReferences> {
    let path = pack_path(dir, number);
    let file_length = std::fs::metadata(&path)?.len();
    let scan = scan_records_from(&path, max_record_bytes, 0)?;
    let entries = scan
        .records
        .iter()
        .map(|(_, payload)| (c1::id_for_bytes(payload), references_in(payload)))
        .collect();
    Ok(PackReferences { entries, file_length, truncated: scan.truncated })
}

/// gc が tmp/ に書く新しい pack の名前の頭。開くときに tmp/ を空にするので、書きかけの
/// 新 pack はクラッシュ後に残らない。
pub const GC_TMP_PREFIX: &str = "gc-";

/// 追記の位置(pack 番号とその中のオフセット)。gc が「ここより後に追記されたレコード」を
/// 問うためのカーソル(Store::references_since)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteCursor {
    pub pack: u64,
    pub offset: u64,
}

/// MANIFEST の本文から (封印済み pack 番号, 封印済み reflog 番号) を読む。開くときと
/// バックアップが同じ読み方をする(should/0135)。
pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<(Vec<u64>, Vec<u64>)> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| StoreError::Corruption("MANIFEST が UTF-8 でない".into()))?;
    let value = c1::parse(text)
        .map_err(|e| StoreError::Corruption(format!("MANIFEST が読めない: {e}")))?;
    let map = match &value {
        c1::Value::Object(m) => m,
        _ => return Err(StoreError::Corruption("MANIFEST がオブジェクトでない".into())),
    };
    let numbers = |key: &str| -> Result<Vec<u64>> {
        match map.get(key) {
            None => Ok(Vec::new()),
            Some(c1::Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    c1::Value::Integer(n) if *n >= 0 => Ok(*n as u64),
                    _ => Err(StoreError::Corruption(format!("MANIFEST {key} が不正"))),
                })
                .collect(),
            Some(_) => Err(StoreError::Corruption(format!("MANIFEST {key} が不正"))),
        }
    };
    Ok((numbers("sealed_packs")?, numbers("sealed_reflogs")?))
}

/// tmp/ の中の、呼び出しごとに一意な名前(`<頭>-<pid>-<単調増加の数>`)。同じプロセスの
/// 2 つのスレッド(GC の参照表の書き込みと MANIFEST など)が同じ tmp を取り合わない
/// (APPEND_FAILURE の欠陥 6)。
pub(crate) fn unique_tmp_path(dir: &Path, head: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join("tmp").join(format!("{head}-{}-{number}", std::process::id()))
}

/// sync の記録の口。`UNIQNODE_SYNC_LOG=<道>` で、ストアと install が行う sync を 1 行ずつ
/// その道へ追記する(`file <道>` か `dir <道>`)。開くたびに何が sync されたかを、実プロセスの
/// 試験が数えるための口で、debug ビルドだけが読む。
pub const SYNC_LOG_ENV: &str = "UNIQNODE_SYNC_LOG";

pub(crate) const ENOSPC: i32 = 28;
pub(crate) const EDQUOT: i32 = 122;

/// sync を 1 回記録する(debug ビルドで SYNC_LOG_ENV があるときだけ)。記録に失敗しても
/// 本来の sync の結果は変えない。
pub(crate) fn note_sync(what: &str, path: &Path) {
    #[cfg(debug_assertions)]
    {
        if let Some(log) = std::env::var_os(SYNC_LOG_ENV) {
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

/// atomic_write の段。注入の口(write_manifest)が段ごとに誤りを差し込む。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtomicStep {
    BeforeRename,
    BeforeDirSync,
}

/// `dir/tmp/` に書いて fsync し、rename で target に据える(SPEC §5.2 の MANIFEST の規律)。
pub(crate) fn atomic_write(dir: &Path, target: &Path, content: &[u8]) -> Result<()> {
    atomic_write_steps(dir, target, content, &mut |_| Ok(()))
        .map_err(|(_, error)| StoreError::Io(error))
}

/// atomic_write の本体。誤りはどの段で起きたかの短い言葉と組で返す。`hook` は各段の前に
/// 呼ばれ、誤りを返せばその段の誤りとして扱う。
fn atomic_write_steps(
    dir: &Path,
    target: &Path,
    content: &[u8],
    hook: &mut dyn FnMut(AtomicStep) -> std::io::Result<()>,
) -> std::result::Result<(), (&'static str, std::io::Error)> {
    let tmp_path = unique_tmp_path(dir, "write");
    let written = (|| {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(content)?;
        note_sync("file", &tmp_path);
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(("tmp への書き込み", error));
    }
    if let Err(error) = hook(AtomicStep::BeforeRename).and_then(|_| std::fs::rename(&tmp_path, target)) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(("rename", error));
    }
    // rename を含むディレクトリエントリの永続化。
    hook(AtomicStep::BeforeDirSync)
        .and_then(|_| sync_dir(target.parent().expect("親ディレクトリがある")))
        .map_err(|error| ("rename の後のディレクトリの sync", error))
}

/// ストアのロックの名前。データディレクトリの正規化した道の SHA-256 から名付けた抽象名前空間の
/// unix socket で、同じディレクトリを指すどの道からでも(unit の中と外の CLI でも)同じロックに
/// なる。ロックを取る側(Store::open)と、取らずに持ち主の有無だけを調べる側
/// (opened_by_another_process)が同じ名前を見るための一箇所(should/0135)。
fn lock_address(dir: &Path) -> Result<std::os::unix::net::SocketAddr> {
    use std::os::linux::net::SocketAddrExt;
    let canonical = std::fs::canonicalize(dir)?;
    let name = format!(
        "uniqnode-lock-{}",
        sha2::hex(&sha2::sha256(canonical.as_os_str().as_encoded_bytes()))
    );
    Ok(std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?)
}

/// 別のプロセスがこのディレクトリのストアを開いている(ロックを持っている)か。ロックを取らずに
/// 調べる: ロックの socket へ connect し、繋がれば持ち主が居る(listen しているが accept は
/// しないので、接続は backlog に載って成功する)、ECONNREFUSED なら居ない。ディレクトリが
/// 無ければ誰も開けないので false。install が unit を起こす前に「起こしても別プロセスが
/// 開いている、で落ちる」を先に言うために使う。
pub fn opened_by_another_process(dir: &Path) -> Result<bool> {
    if !dir.exists() {
        return Ok(false);
    }
    let address = lock_address(dir)?;
    match std::os::unix::net::UnixStream::connect_addr(&address) {
        Ok(_holder) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// このディレクトリをストアのデータディレクトリと認めるか。認めなければ、何が要るかを
/// 言う Invalid を返す。条件は node_key と packs/ が在ること: node_key はストアを一度でも
/// 開けば在り、MANIFEST は最初の封印まで書かれないので条件にできない(小さなストアは
/// MANIFEST を持たないまま正しく動いている)。「ストアであるか」の判断はここ 1 箇所にあり、
/// 在るストアだけを開く open_existing と、写し元を検める backup が同じ答えを見る
/// (should/0135)。
pub fn require_store_dir(dir: &Path) -> Result<()> {
    if dir.join(NODE_KEY_NAME).exists() && dir.join(PACK.directory).is_dir() {
        return Ok(());
    }
    Err(StoreError::Invalid(format!(
        "{} はストアのデータディレクトリではない({NODE_KEY_NAME} と {}/ が要る)",
        dir.display(),
        PACK.directory
    )))
}

/// データのディレクトリの親(相対の 1 要素なら今のディレクトリ)。
fn data_dir_parent(dir: &Path) -> Result<PathBuf> {
    match dir.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(PathBuf::from(".")),
        Some(parent) => Ok(parent.to_path_buf()),
        None => Err(StoreError::Invalid(format!(
            "{} はデータのディレクトリにできない(親が無い)",
            dir.display()
        ))),
    }
}

/// ストアを開く命令が、ログを開くより前に呼ぶ: データのディレクトリが無ければ `mkdir` で
/// 作る(親が無ければ作らずに断る)。祖先は作らない: データのディレクトリの親と、根から
/// そこまでの経路は、既に在って永続しているものとする(APPEND_FAILURE の方針 1a)。
/// Store::open も最初にこれを通る。
pub fn prepare_data_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if dir.exists() {
        return Err(StoreError::Invalid(format!(
            "{} はディレクトリでない",
            dir.display()
        )));
    }
    let parent = data_dir_parent(dir)?;
    if !parent.is_dir() {
        return Err(StoreError::Invalid(format!(
            "{} の親 {} が無い(データのディレクトリの親と、根からそこまでの経路は、既に在って\
             永続しているものとし、作らない。先に作ってから打ち直す)",
            dir.display(),
            parent.display()
        )));
    }
    match std::fs::create_dir(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(error) => Err(StoreError::Io(std::io::Error::new(
            error.kind(),
            format!("{} を作れない: {error}", dir.display()),
        ))),
    }
}

/// ディレクトリが無ければ 1 つだけ作る(祖先は作らない)。
fn mkdir_if_missing(path: &Path) -> Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// 開くときの sync の誤りに道を添える(開くことの失敗として理由を言う)。
fn open_sync_error(what: &str, path: &Path, error: std::io::Error) -> StoreError {
    StoreError::Io(std::io::Error::new(
        error.kind(),
        format!("開くときの sync: {what} {} を sync できない: {error}", path.display()),
    ))
}

impl Store {
    pub fn node_id_hex(&self) -> &str {
        &self.node_id_hex
    }

    /// このDBノードの鍵でメッセージに署名する(SPEC §6.1: プロトコルメッセージは発行
    /// DBノードの署名を持つ)。ref レコードの署名は append_own_record が内側で行うので、
    /// この口を使うのはストアに残らないメッセージ、すなわち分散検索の QUERY と ANSWER
    /// (DISTRIBUTED_SEARCH (uuid:e577f6db-659e-4eb8-a152-3b7780e4a9d1))である。秘密鍵は
    /// ストアの外へ出さない: 署名する物を渡してもらい、署名だけを返す。
    pub fn sign_message(&self, message: &[u8]) -> [u8; 64] {
        ed25519::sign(&self.secret_seed, message)
    }

    pub fn open(config: StoreConfig) -> Result<Store> {
        let dir = config.data_dir.clone();
        #[cfg(debug_assertions)]
        let mut fault = crate::fault::Fault::from_env().map_err(StoreError::Invalid)?;
        // 順は APPEND_FAILURE の方針 1a の末尾: データのディレクトリの mkdir(ロックの名は
        // 正規化した道から作るので先に要る)→ ロック → packs/・reflog/・tmp/ の mkdir →
        // tmp/ の片付け → node_key → recover → 開くときの sync → 書き込みの受け付け。
        // 祖先は作らない(prepare_data_dir)。
        prepare_data_dir(&dir)?;
        let lock = Self::acquire_lock(&dir)?;
        mkdir_if_missing(&dir.join(PACK.directory))?;
        mkdir_if_missing(&dir.join(REFLOG.directory))?;
        mkdir_if_missing(&dir.join("tmp"))?;
        // tmp/ は作業場で、据えられなかった書きかけ(atomic_write の途中、gc の新 pack、
        // backup の写しかけ、node_key の作りかけ)しか残らない。持ち主はロックを取ったこの
        // プロセスだけなので、開くときに空にする。
        for entry in std::fs::read_dir(dir.join("tmp"))? {
            let path = entry?.path();
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
        }

        // node_key を作る途中で落とす口(debug ビルドだけ。crash-in-node-key)。
        #[cfg(debug_assertions)]
        let mut before_key_rename = || {
            if let Some(fault) = fault.as_mut() {
                fault.on_node_key();
            }
        };
        #[cfg(not(debug_assertions))]
        let mut before_key_rename = || {};
        let secret_seed = Self::load_or_create_key(&dir, &mut before_key_rename)?;
        let node_id_hex = sha2::hex(&ed25519::public_key(&secret_seed));

        let (sealed_packs, sealed_reflogs) = Self::load_manifest(&dir)?;

        let mut store = Store {
            config,
            secret_seed,
            node_id_hex,
            object_index: BTreeMap::new(),
            refs: BTreeMap::new(),
            pins: BTreeMap::new(),
            attests: BTreeMap::new(),
            used_bytes: 0,
            signer_last_seq: BTreeMap::new(),
            sealed_packs,
            sealed_reflogs,
            active_pack_number: 1,
            active_pack_length: 0,
            active_reflog_number: 1,
            object_generation: 0,
            gc_touched: None,
            gc_running: false,
            write_failure: None,
            #[cfg(debug_assertions)]
            fault,
            _lock: lock,
        };
        store.recover()?;
        store.sync_on_open()?;
        Ok(store)
    }

    /// 開くときの sync(APPEND_FAILURE の方針 1a)。recover が採用したセグメントの中身と名前を、
    /// 最初の応答より前に永続させる。「既に在る」ことを理由に省かない。対象は node_key の中身、
    /// MANIFEST に封印済みとして載っている pack を除く全部の pack と reflog の中身、packs/・
    /// reflog/・データのディレクトリ自身・その親。失敗は開くことの失敗である。
    fn sync_on_open(&self) -> Result<()> {
        let dir = &self.config.data_dir;
        let key_path = dir.join(NODE_KEY_NAME);
        note_sync("file", &key_path);
        std::fs::File::open(&key_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| open_sync_error("鍵", &key_path, error))?;
        for number in PACK.numbers(dir)? {
            if self.sealed_packs.contains(&number) {
                continue;
            }
            let path = pack_path(dir, number);
            note_sync("file", &path);
            std::fs::File::open(&path)
                .and_then(|file| file.sync_data())
                .map_err(|error| open_sync_error("pack", &path, error))?;
        }
        for number in REFLOG.numbers(dir)? {
            let path = reflog_path(dir, number);
            note_sync("file", &path);
            std::fs::File::open(&path)
                .and_then(|file| file.sync_data())
                .map_err(|error| open_sync_error("reflog", &path, error))?;
        }
        let parent = data_dir_parent(dir)?;
        for directory in [dir.join(PACK.directory), dir.join(REFLOG.directory), dir.clone(), parent]
        {
            sync_dir(&directory)
                .map_err(|error| open_sync_error("ディレクトリ", &directory, error))?;
        }
        Ok(())
    }

    // ---- 書けない状態(APPEND_FAILURE の方針 2) ----

    /// 書けない状態の中身。None なら書ける。
    pub fn writes_disabled(&self) -> Option<&WriteFailure> {
        self.write_failure.as_ref()
    }

    /// 書き込みの入口の先頭で呼ぶ。書けない状態なら WritesDisabled。
    pub fn check_writable(&self) -> Result<()> {
        match &self.write_failure {
            Some(failure) => Err(StoreError::WritesDisabled(failure.clone())),
            None => Ok(()),
        }
    }

    /// 失敗の注入を掛ける(テスト用の口。字句は node/src/fault.rs)。前に掛けたものは捨てる。
    /// debug ビルドだけにあり、release のライブラリには無い。
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn inject_fault(&mut self, spec: &str) -> Result<()> {
        self.fault = Some(crate::fault::Fault::parse(spec).map_err(StoreError::Invalid)?);
        Ok(())
    }

    /// 書けない状態に入れ、その要求へ返す誤りを作る。既に入っていれば最初の原因を保つ。
    fn enter_writes_disabled(
        &mut self,
        op: WriteOp,
        context: &str,
        error: &std::io::Error,
        cleanup: Cleanup,
    ) -> StoreError {
        if let Some(failure) = &self.write_failure {
            return StoreError::WritesDisabled(failure.clone());
        }
        let errno = error.raw_os_error();
        let kind = match (op, errno, cleanup) {
            (WriteOp::Write, Some(ENOSPC | EDQUOT), Cleanup::Ok) => {
                FailureKind::NoSpace
            }
            _ => FailureKind::Io,
        };
        let failure = WriteFailure {
            reason: format!("{context}: {error}"),
            op,
            errno,
            cleanup,
            kind,
            since: unix_now(),
        };
        crate::log_line!(
            "uniqnode: store: writes disabled: op {} cleanup {} errno {}: {}",
            op.name(),
            cleanup.name(),
            errno.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
            failure.message()
        );
        self.write_failure = Some(failure.clone());
        StoreError::WritesDisabled(failure)
    }

    /// 追記の唯一の口(APPEND_FAILURE の方針 1・1a)。書く前のファイル長を開いたハンドルの
    /// fstat で読み、pack ならメモリの数え値と照らす。write か sync が誤りを返したら、前の長さ
    /// への切り詰めとその sync を 1 度だけ試み、結果に関わらず書けない状態に入れる。新しい
    /// ファイルを作った追記は、親ディレクトリの sync までを成功とする。開くこと自体の誤りは
    /// 1 バイトも書いていないので、書けない状態に入れない。
    fn append_durable(&mut self, segment: SegmentKind, number: u64, payload: &[u8]) -> Result<()> {
        self.check_writable()?;
        let path = segment.path(&self.config.data_dir, number);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        let before = match file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                let context = format!("{} の fstat", path.display());
                return Err(self.enter_writes_disabled(
                    WriteOp::Write,
                    &context,
                    &error,
                    Cleanup::NotTried,
                ));
            }
        };
        if segment == PACK && before != self.active_pack_length {
            // errno の無い誤り(io::Error::other は raw_os_error が None)。kind は Io になる。
            let error = std::io::Error::other(format!(
                "ファイルの長さ {before} がメモリの数え値 {} と違う",
                self.active_pack_length
            ));
            let context = format!("{} の書く前の検算", path.display());
            return Err(self.enter_writes_disabled(
                WriteOp::LengthMismatch,
                &context,
                &error,
                Cleanup::NotTried,
            ));
        }
        let new_file = before == 0;
        // 注入(debug ビルドだけ。node/src/fault.rs)。release では各段が本来の操作だけになる。
        #[cfg(debug_assertions)]
        let fired =
            self.fault.as_mut().and_then(|fault| fault.on_append(segment.directory, new_file));
        let record = encode_record(payload);

        // write の段。
        #[cfg(debug_assertions)]
        let written = crate::fault::write_stage(fired, &mut file, &record);
        #[cfg(not(debug_assertions))]
        let written = file.write_all(&record);
        // sync の段。
        let (op, outcome) = match written {
            Err(error) => (WriteOp::Write, Err(error)),
            Ok(()) => {
                note_sync("file", &path);
                #[cfg(debug_assertions)]
                let synced = crate::fault::sync_stage(fired, &file);
                #[cfg(not(debug_assertions))]
                let synced = file.sync_data();
                (WriteOp::Sync, synced)
            }
        };
        if let Err(error) = outcome {
            #[cfg(debug_assertions)]
            let fail_cleanup = crate::fault::fails_cleanup(fired);
            #[cfg(not(debug_assertions))]
            let fail_cleanup = false;
            // 切り詰めは再起動の再生を軽くするための best-effort で、正しさは書けない状態が持つ。
            let cleanup = if fail_cleanup {
                Cleanup::Failed
            } else {
                match file.set_len(before).and_then(|_| file.sync_all()) {
                    Ok(()) => Cleanup::Ok,
                    Err(_) => Cleanup::Failed,
                }
            };
            let context = format!("{} への追記", path.display());
            return Err(self.enter_writes_disabled(op, &context, &error, cleanup));
        }
        drop(file);
        if new_file {
            let parent = path.parent().expect("セグメントには親がある").to_path_buf();
            #[cfg(debug_assertions)]
            let synced = crate::fault::dirsync_stage(fired, &parent);
            #[cfg(not(debug_assertions))]
            let synced = sync_dir(&parent);
            if let Err(error) = synced {
                let context = format!("新しいセグメント {} の親の sync", path.display());
                return Err(self.enter_writes_disabled(
                    WriteOp::DirSync,
                    &context,
                    &error,
                    Cleanup::NotTried,
                ));
            }
        }
        Ok(())
    }

    /// 既に在るストアだけを開く。ストアでない場所(空のディレクトリ・存在しない道)には、
    /// 何も作らずに require_store_dir の理由で断る。open は無ければ初期化する(serve の
    /// 初回起動や init はそれでよい)が、検査や閲覧の命令がその道を通ると、検査が状態を
    /// 作ってしまう: 復元先を先に fsck した写しが別ノードの node_key を持ち、backup に
    /// 断られる、という形で現れた。
    pub fn open_existing(config: StoreConfig) -> Result<Store> {
        require_store_dir(&config.data_dir)?;
        Self::open(config)
    }

    /// 二重オープンの防止。抽象名前空間ソケットはプロセス終了(kill -9 を含む)で
    /// カーネルが解放するため、クラッシュ後にロックが残らない。
    fn acquire_lock(dir: &Path) -> Result<std::os::unix::net::UnixListener> {
        let address = lock_address(dir)?;
        // AddrInUse は短い有界の再試行で判別する。子プロセス生成(Command)は fork→exec の
        // 窓の間、親の全FD(CLOEXEC 付きを含む)の複製を子に持たせるため(CLOEXEC が閉じるのは
        // exec の瞬間)、同プロセスの別スレッドが spawn 中だと、直前に解放したロックの抽象
        // ソケットが一瞬 AddrInUse に見える。実測: spawn 並行下の drop→即 bind で 1.4%、
        // 複製の解放まで最悪 3.6ms(docs/analysis/20260816-lock-inheritance-race.md)。
        // 本物の保持者は解放しないので、250ms 待っても塞がっていれば二重オープンと確定する。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        loop {
            match std::os::unix::net::UnixListener::bind_addr(&address) {
                Ok(listener) => return Ok(listener),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    if std::time::Instant::now() >= deadline {
                        return Err(StoreError::Invalid(format!(
                            "{} は別プロセスが開いている",
                            dir.display()
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// `before_rename` は作るときだけ、tmp/ に書いて sync した後・rename の前に呼ぶ(注入の口)。
    fn load_or_create_key(dir: &Path, before_rename: &mut dyn FnMut()) -> Result<[u8; 32]> {
        let key_path = dir.join(NODE_KEY_NAME);
        if key_path.exists() {
            let bytes = std::fs::read(&key_path)?;
            if bytes.len() != 32 {
                return Err(StoreError::Corruption(format!(
                    "node_key は32バイトのはずが {} バイト",
                    bytes.len()
                )));
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            Ok(seed)
        } else {
            // tmp/ に 0600 で作ってから中身を書き(umask に依らず、秘密鍵が一瞬も他の利用者に
            // 読めない)、sync して rename し、データのディレクトリを sync する。途中で落ちても
            // 短い鍵が node_key の名で残ることは無い(残るのは tmp/ の残骸で、次に開くとき消す。
            // APPEND_FAILURE の方針 1a)。
            let seed = ed25519::generate_secret_seed()?;
            let tmp_path = unique_tmp_path(dir, "node_key");
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&tmp_path)?;
                file.write_all(&seed)?;
                note_sync("file", &tmp_path);
                file.sync_all()?;
            }
            before_rename();
            std::fs::rename(&tmp_path, &key_path)?;
            sync_dir(dir)?;
            Ok(seed)
        }
    }

    fn load_manifest(dir: &Path) -> Result<(Vec<u64>, Vec<u64>)> {
        let manifest_path = dir.join(MANIFEST_NAME);
        if !manifest_path.exists() {
            return Ok((Vec::new(), Vec::new()));
        }
        parse_manifest(&std::fs::read(&manifest_path)?)
    }

    /// MANIFEST を `sealed_packs` の中身で書く。メモリの表は変えない: 呼び手は成功してから
    /// 進める(ディスクが先、メモリが後。APPEND_FAILURE の方針 3)。誤りはどの段でも書けない
    /// 状態に入れ、`op` を原因の段として残す(封印は Manifest、GC の確定は GcCommit)。
    fn write_manifest(&mut self, sealed_packs: &[u64], op: WriteOp) -> Result<()> {
        self.check_writable()?;
        let content = self.manifest_content(sealed_packs);
        #[cfg(debug_assertions)]
        let fired = self.fault.as_mut().and_then(|fault| fault.on_manifest());
        #[cfg(debug_assertions)]
        let mut hook = |step| crate::fault::manifest_step(fired, step);
        #[cfg(not(debug_assertions))]
        let mut hook = |_| Ok(());
        let dir = self.config.data_dir.clone();
        let written = atomic_write_steps(&dir, &dir.join(MANIFEST_NAME), &content, &mut hook);
        match written {
            Ok(()) => Ok(()),
            Err((step, error)) => {
                let context = format!("MANIFEST の{step}");
                Err(self.enter_writes_disabled(op, &context, &error, Cleanup::NotTried))
            }
        }
    }

    fn manifest_content(&self, sealed_packs: &[u64]) -> Vec<u8> {
        let mut map = BTreeMap::new();
        map.insert("v".to_string(), c1::Value::Integer(1));
        map.insert(
            "format".to_string(),
            c1::Value::Text("uniqnode-store-1".to_string()),
        );
        map.insert(
            "sealed_packs".to_string(),
            c1::Value::Array(
                sealed_packs.iter().map(|n| c1::Value::Integer(*n as i64)).collect(),
            ),
        );
        map.insert(
            "sealed_reflogs".to_string(),
            c1::Value::Array(
                self.sealed_reflogs.iter().map(|n| c1::Value::Integer(*n as i64)).collect(),
            ),
        );
        c1::to_canonical_bytes(&c1::Value::Object(map))
    }

    /// 起動時回復。封印済みセグメントは完全でなければならず(破損は Corruption)、
    /// 未封印(アクティブ)セグメントは最後の1つに限り torn tail を切り詰めてよい。
    ///
    /// MANIFEST が在るとき、MANIFEST に無く最後でもない pack は、回収(gc)が MANIFEST を
    /// 書き換えた後・旧 pack を消す前に落ちた残骸である(封印済みの一覧は MANIFEST に完全に
    /// 書かれているので、それ以外の道でこの形は生じない)。走査せず、名を出して削除する。
    /// MANIFEST が無い(初回封印前か、失った)ストアでは全部が未封印に見えるので、この規則は
    /// 使わず、従来どおり走査する(GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。
    fn recover(&mut self) -> Result<()> {
        let dir = self.config.data_dir.clone();
        let manifest_present = dir.join(MANIFEST_NAME).exists();

        // pack の走査(オブジェクト索引の再構築 = 導出データ、I4)。
        let mut pack_numbers = PACK.numbers(&dir)?;
        if manifest_present {
            // 追記中の pack は封印済みのどれよりも大きい番号を持つ(番号は単調に進む)。
            // 最後であっても封印済みの最大より小さい番号なら、追記中ではなく残骸である
            // (回収が新 pack を作らずに旧 pack を MANIFEST から外した直後に落ちた形)。
            let last = pack_numbers.last().copied();
            let max_sealed = self.sealed_packs.iter().max().copied().unwrap_or(0);
            let mut kept = Vec::with_capacity(pack_numbers.len());
            for number in pack_numbers {
                let is_active_candidate = Some(number) == last && number > max_sealed;
                if !self.sealed_packs.contains(&number) && !is_active_candidate {
                    let path = pack_path(&dir, number);
                    crate::log_line!(
                        "uniqnode: store: {} は MANIFEST に無く最後でもない(回収済みの残骸)ので削除する",
                        path.display()
                    );
                    std::fs::remove_file(&path)?;
                } else {
                    kept.push(number);
                }
            }
            pack_numbers = kept;
        }
        for (position, number) in pack_numbers.iter().enumerate() {
            let sealed = self.sealed_packs.contains(number);
            let is_last = position == pack_numbers.len() - 1;
            let path = pack_path(&dir, *number);
            let scan = scan_records(&path, self.config.max_record_bytes)?;
            if scan.truncated {
                if sealed || !is_last {
                    return Err(StoreError::Corruption(format!(
                        "封印済み pack {number} が壊れている(バックアップからの復元が必要)"
                    )));
                }
                // アクティブ末尾の torn write。有効部分まで切り詰める。
                let file = std::fs::OpenOptions::new().write(true).open(&path)?;
                file.set_len(scan.valid_length)?;
                file.sync_all()?;
            }
            let mut added = 0usize;
            for (offset, payload) in &scan.records {
                let id = c1::id_for_bytes(payload);
                // 重複追記(クラッシュ再送)は最初の1つだけ索引と容量に数える。
                if !self.object_index.contains_key(&id) {
                    self.used_bytes += payload.len() as u64;
                    self.object_index.insert(
                        id,
                        ObjectLocation {
                            pack_number: *number,
                            payload_offset: *offset,
                            payload_length: payload.len() as u32,
                        },
                    );
                    added += 1;
                }
            }
            if !sealed && is_last {
                // 最後の未封印 pack のレコードが 1 件以上あって全部が前の pack の重複なら、
                // 回収が新 pack を packs/ へ据えた後・MANIFEST を書く前に落ちた形である
                // (put_object はべき等で、通常の追記は重複を作らない)。中身は全部前の pack
                // に在るので、消しても失うものは無い。残せば、以後の追記がその後ろに続いて
                // 重複の分が二度と回収されない。
                if manifest_present && !scan.records.is_empty() && added == 0 {
                    crate::log_line!(
                        "uniqnode: store: {} は MANIFEST に無く中身が全部前の pack の重複(回収が \
                         MANIFEST を書く前に落ちた新 pack)なので削除する",
                        path.display()
                    );
                    std::fs::remove_file(&path)?;
                    self.active_pack_number = *number;
                    self.active_pack_length = 0;
                } else {
                    self.active_pack_number = *number;
                    self.active_pack_length = std::fs::metadata(&path)?.len();
                }
            }
        }
        if let Some(max) = pack_numbers.last() {
            if self.sealed_packs.contains(max) {
                self.active_pack_number = max + 1;
                self.active_pack_length = 0;
            }
        }
        self.object_generation = self.object_index.len() as u64;

        // reflog の再生。
        let reflog_numbers = REFLOG.numbers(&dir)?;
        for (position, number) in reflog_numbers.iter().enumerate() {
            let sealed = self.sealed_reflogs.contains(number);
            let is_last = position == reflog_numbers.len() - 1;
            let path = reflog_path(&dir, *number);
            let scan = scan_records(&path, self.config.max_record_bytes)?;
            if scan.truncated {
                if sealed || !is_last {
                    return Err(StoreError::Corruption(format!(
                        "封印済み reflog {number} が壊れている(バックアップからの復元が必要)"
                    )));
                }
                let file = std::fs::OpenOptions::new().write(true).open(&path)?;
                file.set_len(scan.valid_length)?;
                file.sync_all()?;
            }
            for (_, payload) in &scan.records {
                match self.verify_ref_record(payload)? {
                    Verified::New(verified) => self.apply_verified(verified),
                    Verified::AlreadyKnown => {}
                }
            }
            if !sealed && is_last {
                self.active_reflog_number = *number;
            }
        }
        if let Some(max) = reflog_numbers.last() {
            if self.sealed_reflogs.contains(max) {
                self.active_reflog_number = max + 1;
            }
        }
        Ok(())
    }

    /// reflog レコード(c1 JSON)を検証する。メモリ状態は変更しない。
    fn verify_ref_record(&self, payload: &[u8]) -> Result<Verified> {
        let text = std::str::from_utf8(payload)
            .map_err(|_| StoreError::Corruption("reflog レコードが UTF-8 でない".into()))?;
        let value = c1::parse(text)
            .map_err(|e| StoreError::Corruption(format!("reflog レコードが c1 でない: {e}")))?;
        let map = match &value {
            c1::Value::Object(m) => m,
            _ => return Err(StoreError::Corruption("reflog レコードが object でない".into())),
        };
        let text_field = |key: &str| -> Result<String> {
            match map.get(key) {
                Some(c1::Value::Text(t)) => Ok(t.clone()),
                _ => Err(StoreError::Corruption(format!("reflog レコードに {key} がない"))),
            }
        };
        let integer_field = |key: &str| -> Result<i64> {
            match map.get(key) {
                Some(c1::Value::Integer(n)) => Ok(*n),
                _ => Err(StoreError::Corruption(format!("reflog レコードに {key} がない"))),
            }
        };
        let record_type = text_field("type")?;
        let seq = integer_field("seq")? as u64;
        let at = integer_field("at")?;
        let signer = text_field("signer")?;
        let signature_hex = text_field("sig")?;

        // 既知の seq は署名検証の前に冪等スキップする(状態を変えないので危険がなく、
        // 再同期のたびに全レコードを検証し直すコストを避ける)。
        let last = self.signer_last_seq.get(&signer).copied().unwrap_or(0);
        if seq <= last {
            return Ok(Verified::AlreadyKnown);
        }

        // 署名検証: sig を除いた正規形に対する署名(SPEC §4.4)。
        let mut unsigned = map.clone();
        unsigned.remove("sig");
        let message = c1::to_canonical_bytes(&c1::Value::Object(unsigned));
        let public_bytes = sha2::from_hex(&signer)
            .ok_or_else(|| StoreError::Corruption("signer が16進でない".into()))?;
        let signature_bytes = sha2::from_hex(&signature_hex)
            .ok_or_else(|| StoreError::Corruption("sig が16進でない".into()))?;
        if public_bytes.len() != 32 || signature_bytes.len() != 64 {
            return Err(StoreError::Corruption("signer/sig の長さが不正".into()));
        }
        let mut public = [0u8; 32];
        public.copy_from_slice(&public_bytes);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&signature_bytes);
        if !ed25519::verify(&public, &message, &signature) {
            return Err(StoreError::Corruption(format!(
                "reflog レコード(seq={seq})の署名が不正"
            )));
        }
        let body = match record_type.as_str() {
            "set_ref" => {
                let name = text_field("name")?;
                if !name.starts_with(&format!("{signer}/")) {
                    return Err(StoreError::Corruption(format!(
                        "ref {name} は signer の名前空間でない(single writer 違反)"
                    )));
                }
                let target = match map.get("target") {
                    Some(c1::Value::Text(t)) => Some(t.clone()),
                    Some(c1::Value::Null) => None,
                    _ => {
                        return Err(StoreError::Corruption("レコードの target が不正".into()))
                    }
                };
                RecordBody::SetRef { name, target }
            }
            "pin" => {
                let min_replicas = integer_field("min_replicas")?;
                if !(0..=1024).contains(&min_replicas) {
                    return Err(StoreError::Corruption("min_replicas が範囲外".into()));
                }
                RecordBody::Pin {
                    root: text_field("root")?,
                    min_replicas: min_replicas as u32,
                }
            }
            "attest" => {
                let held = match map.get("held") {
                    Some(c1::Value::Bool(b)) => *b,
                    _ => return Err(StoreError::Corruption("attest の held が不正".into())),
                };
                RecordBody::Attest { root: text_field("root")?, held }
            }
            other => {
                return Err(StoreError::Corruption(format!("未知のレコード種別 {other}")))
            }
        };
        if seq != last + 1 {
            return Err(StoreError::Corruption(format!(
                "signer {signer} の seq が飛んでいる: 期待 {} 実際 {seq}",
                last + 1
            )));
        }
        Ok(Verified::New(VerifiedRecord { body, seq, at, signer }))
    }

    fn apply_verified(&mut self, verified: VerifiedRecord) {
        self.signer_last_seq.insert(verified.signer.clone(), verified.seq);
        match verified.body {
            RecordBody::SetRef { name, target } => {
                self.refs
                    .insert(name, RefState { target, seq: verified.seq, at: verified.at });
            }
            RecordBody::Pin { root, min_replicas } => {
                if min_replicas == 0 {
                    if let Some(entry) = self.pins.get_mut(&root) {
                        entry.remove(&verified.signer);
                        if entry.is_empty() {
                            self.pins.remove(&root);
                        }
                    }
                } else {
                    self.pins.entry(root).or_default().insert(verified.signer, min_replicas);
                }
            }
            RecordBody::Attest { root, held } => {
                self.attests.entry(root).or_default().insert(verified.signer, held);
            }
        }
    }

    /// 他DBノード由来の署名済み ref レコードを取り込む(レプリケーションの受け側)。
    /// 永続化(reflog 追記)してからメモリに適用する。既知の seq は冪等にスキップする。
    pub fn ingest_ref_record(&mut self, payload: &[u8]) -> Result<bool> {
        self.check_writable()?;
        match self.verify_ref_record(payload)? {
            Verified::AlreadyKnown => Ok(false),
            Verified::New(verified) => {
                self.append_durable(REFLOG, self.active_reflog_number, payload)?;
                self.apply_verified(verified);
                Ok(true)
            }
        }
    }

    // ---- 書き込み ----

    /// オブジェクト投入(べき等)。返り値は (ID, 新規に保存されたか)。
    pub fn put_object(&mut self, bytes: &[u8]) -> Result<(String, bool)> {
        self.check_writable()?;
        if bytes.len() as u64 > self.config.max_record_bytes as u64 {
            return Err(StoreError::Invalid(format!(
                "オブジェクトが大きすぎる({} bytes)",
                bytes.len()
            )));
        }
        let id = c1::id_for_bytes(bytes);
        if self.object_index.contains_key(&id) {
            if let Some(touched) = &mut self.gc_touched {
                touched.insert(id.clone());
            }
            return Ok((id, false));
        }
        if let Some(capacity) = self.config.capacity_bytes {
            if self.used_bytes + bytes.len() as u64 > capacity {
                return Err(StoreError::Invalid(format!(
                    "容量超過: used {} + {} > {capacity}",
                    self.used_bytes,
                    bytes.len()
                )));
            }
        }
        if self.active_pack_length >= self.config.pack_seal_bytes {
            self.seal_active_pack()?;
        }
        let offset_before = self.active_pack_length;
        self.append_durable(PACK, self.active_pack_number, bytes)?;
        self.active_pack_length += 8 + bytes.len() as u64;
        self.object_index.insert(
            id.clone(),
            ObjectLocation {
                pack_number: self.active_pack_number,
                payload_offset: offset_before + 8,
                payload_length: bytes.len() as u32,
            },
        );
        self.used_bytes += bytes.len() as u64;
        self.object_generation += 1;
        Ok((id, true))
    }

    /// 封印。番号を足した写しで MANIFEST を書き、成功してからメモリの表を進める
    /// (APPEND_FAILURE の方針 3)。
    fn seal_active_pack(&mut self) -> Result<()> {
        self.check_writable()?;
        let mut sealed = self.sealed_packs.clone();
        sealed.push(self.active_pack_number);
        self.write_manifest(&sealed, WriteOp::Manifest)?;
        self.sealed_packs = sealed;
        self.active_pack_number += 1;
        self.active_pack_length = 0;
        Ok(())
    }

    // ---- 回収(gc)がロックの中で呼ぶ口 ----
    // 手順の全体は node/src/gc.rs と GC (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5)。
    // ここにあるのは、Store の内部(索引・MANIFEST・アクティブ pack)に触れる相だけである。

    /// データディレクトリ。gc がロックの外で封印済み pack を読むために持ち出す。
    pub(crate) fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }

    pub(crate) fn max_record_bytes(&self) -> u32 {
        self.config.max_record_bytes
    }

    /// 回収の始まり(S)で呼ぶ。既に走っていれば false(2 つ目は始めない: 1 つ目が対象を消した
    /// 後の pack を読みに行くことになる)。書けない状態なら WritesDisabled(dry-run も同じ入口を
    /// 通るので断られる)。
    pub fn gc_try_begin(&mut self) -> Result<bool> {
        self.check_writable()?;
        if self.gc_running {
            return Ok(false);
        }
        self.gc_running = true;
        Ok(true)
    }

    /// GC の D の最後の packs/ のディレクトリの sync。失敗は書けない状態(kind は Io、op は
    /// DirSync)に入れる: D がその誤りを受け取るとディレクトリの errseq に「報告済み」の印が
    /// 付き、後の新しいセグメントの名前の sync が成功を返して、書き戻せなかった更新を帳消しに
    /// しうる(APPEND_FAILURE の方針 3 の D の項)。呼び手はストアのロックの中で呼ぶ(失敗の
    /// 記録がロックの中で済む)。
    pub(crate) fn gc_sync_packs_dir(&mut self) -> Result<()> {
        let packs = self.config.data_dir.join(PACK.directory);
        #[cfg(debug_assertions)]
        let synced = {
            let fired = self.fault.as_mut().and_then(|fault| fault.on_gc_dirsync());
            crate::fault::gc_dirsync_stage(fired, &packs)
        };
        #[cfg(not(debug_assertions))]
        let synced = sync_dir(&packs);
        match synced {
            Ok(()) => Ok(()),
            Err(error) => {
                let context = format!("GC の D の {} の sync", packs.display());
                Err(self.enter_writes_disabled(WriteOp::DirSync, &context, &error, Cleanup::NotTried))
            }
        }
    }

    /// 回収の終わり(成功でも失敗でも)で呼ぶ。
    pub fn gc_end(&mut self) {
        self.gc_running = false;
        self.gc_touched = None;
    }

    /// 追記の現在位置。
    pub fn write_cursor(&self) -> WriteCursor {
        WriteCursor { pack: self.active_pack_number, offset: self.active_pack_length }
    }

    /// 手順 S: 追記中の pack を封印して MANIFEST に載せる。封印した番号を返す。空(1 件も
    /// 追記されていない)なら封印せず None(空の pack を封印すると次回の回収の対象に
    /// なるだけで、意味が無い)。
    pub fn seal_active_pack_for_gc(&mut self) -> Result<Option<u64>> {
        // 空でも書けない状態を先に言う(空の早期 return が禁止状態を素通りさせない)。
        self.check_writable()?;
        if self.active_pack_length == 0 {
            return Ok(None);
        }
        let number = self.active_pack_number;
        self.seal_active_pack()?;
        Ok(Some(number))
    }

    /// カーソルより後に追記されたレコードの (ID, 参照の列) と、新しいカーソル。カーソルの
    /// pack のオフセット以後と、それより大きい番号の pack(その間に封印が起きていれば
    /// 複数)の全部。ロックの中で呼ぶので、追記中の pack は末尾まで完全である(不完全なら
    /// 破損)。
    pub fn references_since(
        &self,
        cursor: WriteCursor,
    ) -> Result<(ReferenceEntries, WriteCursor)> {
        let mut entries = Vec::new();
        let numbers = self
            .sealed_packs
            .iter()
            .copied()
            .chain(std::iter::once(self.active_pack_number))
            .filter(|number| *number >= cursor.pack);
        for number in numbers {
            let path = pack_path(&self.config.data_dir, number);
            if !path.exists() {
                continue; // 追記中の pack は最初の追記までファイルが無い
            }
            let from = if number == cursor.pack { cursor.offset } else { 0 };
            let scan = scan_records_from(&path, self.config.max_record_bytes, from)?;
            if scan.truncated {
                return Err(StoreError::Corruption(format!(
                    "pack {number} の末尾が不完全(ロックの中で見えてはならない)"
                )));
            }
            for (_, payload) in &scan.records {
                entries.push((c1::id_for_bytes(payload), references_in(payload)));
            }
        }
        Ok((entries, self.write_cursor()))
    }

    /// 手順 A の終わりで呼ぶ: 以後 put_object が「既に在る」と答えた ID を記録し始める。
    pub fn gc_begin_touch_log(&mut self) {
        self.gc_touched = Some(std::collections::BTreeSet::new());
    }

    /// 手順 C で呼ぶ: 記録を取り出して止める。
    pub fn gc_take_touch_log(&mut self) -> std::collections::BTreeSet<String> {
        self.gc_touched.take().unwrap_or_default()
    }

    /// 手順 C の差し替え。`targets` は MANIFEST から外す封印済み pack、`new_pack` は tmp/ に
    /// 書いて fsync 済みの新しい pack と、そこに写したオブジェクトの (ID, ペイロードの
    /// オフセット, 長さ)。写したものが 1 件も無ければ None(新しい pack は作らない)。
    ///
    /// 順序は回復の仮定「封印済みでない最後の 1 本がアクティブ」を守るためにある:
    /// 1. アクティブを封印し MANIFEST を書く(空なら封印せず、その番号を新 pack に譲る)。
    ///    先に据えると、追記中の pack が最後でなくなり、torn tail が破損に見える。
    /// 2. 新 pack を次の番号で packs/ へ rename(参照される前に fsync 済み。SPEC §5.2)。
    /// 3. MANIFEST を書く(対象を外し、新 pack を足す)。
    /// 4. 索引を新しい位置に差し替え、対象に残った孤児を索引と used_bytes から外す。
    /// 5. 新しいアクティブの番号を新 pack の次にする。
    ///
    /// 各点で落ちたときの回復は recover と GC.md の表。
    pub(crate) fn gc_commit(
        &mut self,
        targets: &[u64],
        new_pack: Option<(&Path, &[CopiedObject])>,
    ) -> Result<GcCommit> {
        // B の間に別の要求が書けない状態を立てたかもしれない。
        self.check_writable()?;
        for target in targets {
            if !self.sealed_packs.contains(target) {
                return Err(StoreError::Invalid(format!(
                    "pack {target} は封印済みでないので回収できない"
                )));
            }
        }
        // C-1
        let sealed_active = self.seal_active_pack_for_gc()?;
        crate::gc::crash_point(crate::gc::CRASH_AFTER_C1);
        // C-2
        let new_number = match new_pack {
            Some((tmp_path, _)) => {
                let number = self.active_pack_number;
                let target = pack_path(&self.config.data_dir, number);
                // C-1 より後の誤りは全部書けない状態に入れる(APPEND_FAILURE の方針 2)。
                if let Err(error) = std::fs::rename(tmp_path, &target) {
                    let context = format!("GC の新 pack の {} への rename", target.display());
                    return Err(self.enter_writes_disabled(
                        WriteOp::GcCommit,
                        &context,
                        &error,
                        Cleanup::NotTried,
                    ));
                }
                let packs = target.parent().expect("packs/ がある").to_path_buf();
                if let Err(error) = sync_dir(&packs) {
                    let context = format!("GC の新 pack の rename の後の {} の sync", packs.display());
                    return Err(self.enter_writes_disabled(
                        WriteOp::GcCommit,
                        &context,
                        &error,
                        Cleanup::NotTried,
                    ));
                }
                crate::gc::crash_point(crate::gc::CRASH_AFTER_C2);
                Some(number)
            }
            None => None,
        };
        // C-3: 写しで MANIFEST を書き、成功してからメモリの表を書き換える(方針 3)。
        let mut sealed: Vec<u64> =
            self.sealed_packs.iter().copied().filter(|number| !targets.contains(number)).collect();
        if let Some(number) = new_number {
            sealed.push(number);
            sealed.sort_unstable();
        }
        self.write_manifest(&sealed, WriteOp::GcCommit)?;
        self.sealed_packs = sealed;
        crate::gc::crash_point(crate::gc::CRASH_AFTER_C3);
        // C-4
        let copied: BTreeMap<&str, (u64, u32)> = new_pack
            .map(|(_, entries)| {
                entries
                    .iter()
                    .map(|(id, offset, length)| (id.as_str(), (*offset, *length)))
                    .collect()
            })
            .unwrap_or_default();
        let in_targets: Vec<String> = self
            .object_index
            .iter()
            .filter(|(_, location)| targets.contains(&location.pack_number))
            .map(|(id, _)| id.clone())
            .collect();
        let mut reclaimed_bytes = 0u64;
        let mut removed_objects = 0usize;
        for id in in_targets {
            match (copied.get(id.as_str()), new_number) {
                (Some((offset, length)), Some(number)) => {
                    self.object_index.insert(
                        id,
                        ObjectLocation {
                            pack_number: number,
                            payload_offset: *offset,
                            payload_length: *length,
                        },
                    );
                }
                _ => {
                    let location = self.object_index.remove(&id).expect("索引にある");
                    self.used_bytes -= location.payload_length as u64;
                    reclaimed_bytes += location.payload_length as u64;
                    removed_objects += 1;
                }
            }
        }
        self.object_generation += 1;
        // C-5
        if let Some(number) = new_number {
            self.active_pack_number = number + 1;
            self.active_pack_length = 0;
        }
        Ok(GcCommit { sealed_active, new_pack: new_number, reclaimed_bytes, removed_objects })
    }

    /// 自分の名前空間の ref を更新する。target=None が tombstone。
    pub fn set_ref(&mut self, path: &str, target: Option<&str>) -> Result<u64> {
        // 書けない状態は引数の検査より先に言う(不正な引数でも 400 でなく 503 と復旧の案内)。
        self.check_writable()?;
        if path.is_empty() || path.starts_with('/') {
            return Err(StoreError::Invalid("ref パスが不正".into()));
        }
        if let Some(t) = target {
            if !self.object_index.contains_key(t) {
                return Err(StoreError::Invalid(format!(
                    "target {t} が存在しない(先に put_object する)"
                )));
            }
        }
        let name = format!("{}/{}", self.node_id_hex, path);
        let mut map = BTreeMap::new();
        map.insert("v".to_string(), c1::Value::Integer(1));
        map.insert("type".to_string(), c1::Value::Text("set_ref".to_string()));
        map.insert("name".to_string(), c1::Value::Text(name));
        map.insert(
            "target".to_string(),
            match target {
                Some(t) => c1::Value::Text(t.to_string()),
                None => c1::Value::Null,
            },
        );
        self.append_own_record(map)
    }

    /// pin(root の到達閉包に min_replicas を要求する。0 で解除。SPEC §4.5)。
    pub fn set_pin(&mut self, root: &str, min_replicas: u32) -> Result<u64> {
        self.check_writable()?;
        if min_replicas > 0 && !self.object_index.contains_key(root) {
            return Err(StoreError::Invalid(format!("root {root} が存在しない")));
        }
        let mut map = BTreeMap::new();
        map.insert("v".to_string(), c1::Value::Integer(1));
        map.insert("type".to_string(), c1::Value::Text("pin".to_string()));
        map.insert("root".to_string(), c1::Value::Text(root.to_string()));
        map.insert("min_replicas".to_string(), c1::Value::Integer(min_replicas as i64));
        self.append_own_record(map)
    }

    /// 保持表明(SPEC §4.5)。held=true は root の閉包を確約層で保持していることの表明。
    pub fn set_attest(&mut self, root: &str, held: bool) -> Result<u64> {
        self.check_writable()?;
        if held && !self.object_index.contains_key(root) {
            return Err(StoreError::Invalid(format!(
                "保持していない root {root} に held=true は表明できない"
            )));
        }
        let mut map = BTreeMap::new();
        map.insert("v".to_string(), c1::Value::Integer(1));
        map.insert("type".to_string(), c1::Value::Text("attest".to_string()));
        map.insert("root".to_string(), c1::Value::Text(root.to_string()));
        map.insert("held".to_string(), c1::Value::Bool(held));
        self.append_own_record(map)
    }

    /// 自分の署名でレコードを1件発行する(seq/at/signer/sig を埋め、取り込み側と同じ
    /// 検証を通してから永続化・適用する。判定の一本化 = should/0135)。
    fn append_own_record(&mut self, mut map: BTreeMap<String, c1::Value>) -> Result<u64> {
        self.check_writable()?;
        let seq = self.signer_last_seq.get(&self.node_id_hex).copied().unwrap_or(0) + 1;
        map.insert("seq".to_string(), c1::Value::Integer(seq as i64));
        map.insert("at".to_string(), c1::Value::Integer(unix_now()));
        map.insert("signer".to_string(), c1::Value::Text(self.node_id_hex.clone()));
        let message = c1::to_canonical_bytes(&c1::Value::Object(map.clone()));
        let signature = ed25519::sign(&self.secret_seed, &message);
        map.insert("sig".to_string(), c1::Value::Text(sha2::hex(&signature)));
        let payload = c1::to_canonical_bytes(&c1::Value::Object(map));

        let verified = match self.verify_ref_record(&payload)? {
            Verified::New(v) => v,
            Verified::AlreadyKnown => {
                return Err(StoreError::Corruption("自レコードの seq が既知になっている".into()))
            }
        };
        self.append_durable(REFLOG, self.active_reflog_number, &payload)?;
        self.apply_verified(verified);
        Ok(seq)
    }

    // ---- pin / 保持表明 / 容量の読み取り ----

    /// 実効 pin: root → min_replicas の最大値(0 は除去済み)。
    pub fn effective_pins(&self) -> BTreeMap<String, u32> {
        self.pins
            .iter()
            .filter_map(|(root, by_signer)| {
                by_signer.values().max().map(|min| (root.clone(), *min))
            })
            .collect()
    }

    /// root の保持を表明している(held=true)DBノードの一覧。
    pub fn attest_holders(&self, root: &str) -> Vec<String> {
        self.attests
            .get(root)
            .map(|by_holder| {
                by_holder
                    .iter()
                    .filter(|(_, held)| **held)
                    .map(|(holder, _)| holder.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 誰か(自分でも他ノードでも)が held=true を表明している root。held=false しか残って
    /// いない root(撤回済み)は含めない。gc の根の一つ(GC
    /// (uuid:9b1ceac3-f3cf-4595-87cb-6e40ce0900e5))。
    pub fn held_attested_roots(&self) -> Vec<String> {
        self.attests
            .iter()
            .filter(|(_, by_holder)| by_holder.values().any(|held| *held))
            .map(|(root, _)| root.clone())
            .collect()
    }

    /// 自分が held=true を表明している root(確約層)。
    pub fn own_attested_roots(&self) -> Vec<String> {
        self.attests
            .iter()
            .filter(|(_, by_holder)| {
                by_holder.get(&self.node_id_hex).copied().unwrap_or(false)
            })
            .map(|(root, _)| root.clone())
            .collect()
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    pub fn capacity_bytes(&self) -> Option<u64> {
        self.config.capacity_bytes
    }

    pub fn free_bytes(&self) -> Option<u64> {
        self.config.capacity_bytes.map(|c| c.saturating_sub(self.used_bytes))
    }

    /// root の到達閉包(ローカルに在る分)の合計バイト数。
    pub fn closure_bytes(&self, root: &str) -> Result<u64> {
        let mut total = 0u64;
        for id in self.reachable_closure(root)? {
            if let Some(location) = self.object_index.get(&id) {
                total += location.payload_length as u64;
            }
        }
        Ok(total)
    }

    // ---- 読み取り ----

    pub fn has_object(&self, id: &str) -> bool {
        self.object_index.contains_key(id)
    }

    pub fn get_object(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let location = match self.object_index.get(id) {
            None => return Ok(None),
            Some(l) => *l,
        };
        let path = pack_path(&self.config.data_dir, location.pack_number);
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(location.payload_offset))?;
        let mut payload = vec![0u8; location.payload_length as usize];
        file.read_exact(&mut payload)?;
        Ok(Some(payload))
    }

    /// 完全名(`<node_id>/<path>`)で ref を引く。
    pub fn get_ref(&self, name: &str) -> Option<&RefState> {
        self.refs.get(name)
    }

    pub fn own_ref_name(&self, path: &str) -> String {
        format!("{}/{}", self.node_id_hex, path)
    }

    pub fn list_refs(&self) -> impl Iterator<Item = (&String, &RefState)> {
        self.refs.iter()
    }

    pub fn object_count(&self) -> usize {
        self.object_index.len()
    }

    /// オブジェクトの集合の世代。put で 1 件増えるたび、gc の回収で減るたびに進む。導出
    /// データ(逆引き索引)が最新かを見る札で、件数の一致より強い(回収で k 件減った後に
    /// k 件増えた集合を同じと言わない)。
    pub fn object_generation(&self) -> u64 {
        self.object_generation
    }

    /// 全オブジェクトの ID(昇順)。逆引き索引の構築(ReferrerIndex::build)が使う。
    pub fn object_ids(&self) -> impl Iterator<Item = &String> {
        self.object_index.keys()
    }

    /// 全オブジェクトの ID と置き場所(ID 昇順)。gc が pack ごとに生きているバイト数と
    /// 孤児のバイト数を集計するのに読む。索引は重複追記を最初の 1 つだけ数えるので、
    /// ここに出る合計は used_bytes と同じ会計である。
    pub fn object_locations(&self) -> impl Iterator<Item = (&String, &ObjectLocation)> {
        self.object_index.iter()
    }

    /// 封印済み pack の番号(MANIFEST の写し。昇順)。
    pub fn sealed_pack_numbers(&self) -> &[u64] {
        &self.sealed_packs
    }

    /// 追記中の pack の番号。封印済みでない唯一の pack で、gc はこれに触れない。
    pub fn active_pack_number(&self) -> u64 {
        self.active_pack_number
    }

    /// 自分の名前空間の最終 seq。
    pub fn last_seq(&self) -> u64 {
        self.signer_last_seq.get(&self.node_id_hex).copied().unwrap_or(0)
    }

    /// 知っている署名者と、その適用済み最終 seq(レプリケーションのカーソル)。
    pub fn signers(&self) -> Vec<(String, u64)> {
        self.signer_last_seq
            .iter()
            .map(|(signer, seq)| (signer.clone(), *seq))
            .collect()
    }

    /// signer の since より後の署名済み ref レコード(生ペイロード)を seq 順で返す。
    /// reflog への追記は署名者ごとに seq 昇順なので、走査順がそのまま seq 順になる。
    /// メモリに適用済みの seq(signer_last_seq)より後は返さない: sync が失敗して切り詰めにも
    /// 失敗した 1 本は、ディスクにあっても応答していないので、ピアへ流さない(APPEND_FAILURE
    /// の「外から見える形」)。
    pub fn export_ref_records(&self, signer: &str, since: u64) -> Result<Vec<Vec<u8>>> {
        let applied = self.signer_last_seq.get(signer).copied().unwrap_or(0);
        let dir = &self.config.data_dir;
        let mut numbers = Vec::new();
        for entry in std::fs::read_dir(dir.join("reflog"))? {
            let name = entry?.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("reflog-") {
                if let Some(number_text) = rest.strip_suffix(".log") {
                    if let Ok(n) = number_text.parse::<u64>() {
                        numbers.push(n);
                    }
                }
            }
        }
        numbers.sort_unstable();
        let mut out = Vec::new();
        let signer_prefix = format!("\"signer\":\"{signer}\"");
        for number in numbers {
            let scan = scan_records(&reflog_path(dir, number), self.config.max_record_bytes)?;
            for (_, payload) in scan.records {
                // 高速な事前フィルタ(正規形なので部分文字列が安定)の後、seq を正確に読む。
                let text = match std::str::from_utf8(&payload) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if !text.contains(&signer_prefix) {
                    continue;
                }
                let value = match c1::parse(text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let c1::Value::Object(map) = &value {
                    let seq = match map.get("seq") {
                        Some(c1::Value::Integer(n)) => *n as u64,
                        _ => continue,
                    };
                    let record_signer = match map.get("signer") {
                        Some(c1::Value::Text(t)) => t.as_str(),
                        _ => continue,
                    };
                    if record_signer == signer && seq > since && seq <= applied {
                        out.push(payload);
                    }
                }
            }
        }
        Ok(out)
    }

    /// root から c1 参照(SPEC §4.3)を辿って到達可能な閉包を返す。
    /// ローカルに存在しない参照先は結果に含めない(開世界: dangling は無害)。
    pub fn reachable_closure(&self, root: &str) -> Result<Vec<String>> {
        Ok(self
            .reachable_closure_of_roots(std::iter::once(root))?
            .into_iter()
            .collect())
    }

    /// 複数の根からまとめて辿った到達閉包(ローカルに在る分。ID 昇順)。1 本ずつ
    /// reachable_closure を呼ぶと共有される部分木を根の数だけ辿り直すので、gc の
    /// 「生きている集合」はこちらで 1 回の走査にする。参照の規約と dangling の扱いは
    /// reachable_closure と同じ 1 つの実装である(should/0135)。
    pub fn reachable_closure_of_roots<'a>(
        &self,
        roots: impl IntoIterator<Item = &'a str>,
    ) -> Result<std::collections::BTreeSet<String>> {
        closure_over(roots, |id| {
            Ok(self.get_object(id)?.map(|bytes| references_in(&bytes)))
        })
    }

    // ---- 検証 ----

    /// 全 pack の再走査+再ハッシュと、ref の整合検査。content-addressed なので
    /// 「内容が正しいか」まで機械的に検証できる(SPEC §5.5)。
    pub fn fsck(&self) -> Result<FsckReport> {
        let mut report = FsckReport::default();
        // MANIFEST に無く最後でもない pack。MANIFEST が在れば開くときに残骸として削除して
        // いるので、ここに出るのは MANIFEST の無い(失った)ストアの場合だけである。エラー
        // ではなく事実として報告する(封印されていたのか追記中だったのかは、もう分からない)。
        let pack_numbers = PACK.numbers(&self.config.data_dir)?;
        let last = pack_numbers.last().copied();
        report.unlisted_packs = pack_numbers
            .iter()
            .copied()
            .filter(|number| !self.sealed_packs.contains(number) && Some(*number) != last)
            .collect();
        // 索引の全エントリについて、実データを読み直してハッシュを照合する。
        for (id, location) in &self.object_index {
            report.objects_checked += 1;
            let path = pack_path(&self.config.data_dir, location.pack_number);
            let mut file = std::fs::File::open(&path)?;
            file.seek(SeekFrom::Start(location.payload_offset))?;
            let mut payload = vec![0u8; location.payload_length as usize];
            file.read_exact(&mut payload)?;
            let actual = c1::id_for_bytes(&payload);
            if actual != *id {
                report
                    .errors
                    .push(format!("オブジェクト {id} の内容が一致しない(実際 {actual})"));
            }
        }
        for (name, state) in &self.refs {
            report.refs_checked += 1;
            if let Some(target) = &state.target {
                if !self.object_index.contains_key(target) {
                    if name.starts_with(&self.node_id_hex) {
                        // 自分の ref は書き込み時に存在を強制しているので、欠けは破損。
                        report
                            .errors
                            .push(format!("ref {name} の target {target} が存在しない"));
                    } else {
                        // 他DBノードの ref の対象は未取得であり得る(レプリケーション未完了
                        // または開世界の忘却)。エラーではなく事実として数える。
                        report.foreign_targets_absent += 1;
                    }
                }
            }
        }
        Ok(report)
    }
}

#[derive(Debug, Default)]
pub struct FsckReport {
    pub objects_checked: usize,
    pub refs_checked: usize,
    pub foreign_targets_absent: usize,
    /// MANIFEST に無く最後でもない pack の番号。事実であってエラーではない(fsck の説明)。
    pub unlisted_packs: Vec<u64>,
    pub errors: Vec<String>,
}

/// gc_commit の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcCommit {
    /// C-1 で封印したアクティブ pack の番号(空で封印しなかったなら None)。
    pub sealed_active: Option<u64>,
    /// packs/ に据えた新しい pack の番号(写すものが無く作らなかったなら None)。
    pub new_pack: Option<u64>,
    /// 索引と used_bytes から外した孤児のバイト数。
    pub reclaimed_bytes: u64,
    pub removed_objects: usize,
}

/// 逆引き索引: あるオブジェクト ID を参照している既知オブジェクトの一覧。逆向きの知識は
/// 「いま自分が知っている言明の集合」に相対的な事実であり言明にできないので、各DBノードが
/// 手元で再構成する導出データ(I4)として持つ(ASSERTIONS
/// (uuid:c05379e2-2d30-41bc-8342-62103d94bb21) の原理 3 帰結 2)。
/// 遅延構築であり、Store::open には触れない(open はオブジェクトをパースせずハッシュだけを
/// 見る唯一の共有経路であり、そこに全件パースを足すと壊れたオブジェクト 1 個でストアが
/// 開かなくなる。INGEST (uuid:47d69a3e-c39a-4e76-9814-e9c24240293b) の「逆引き」節)。
pub struct ReferrerIndex {
    /// 構築時点の Store::object_generation。オブジェクトの集合が変わる(put で増える、gc の
    /// 回収で減る)たびに進む番号なので、これが現在値と一致する限り索引は最新。
    generation: u64,
    /// 参照先 ID → 参照している既知オブジェクトの ID 列(昇順)。
    map: BTreeMap<String, Vec<String>>,
}

impl ReferrerIndex {
    /// 全オブジェクトを一度走査して構築する。c1 としてパースできないオブジェクト
    /// (生 blob 等)は参照ゼロとして飛ばす(参照の規約 SPEC §4.3 は c1 値の中の
    /// s256: 文字列だけを参照と見なすため、パースできない内容は定義上参照を持たない)。
    pub fn build(store: &Store) -> Result<ReferrerIndex> {
        let generation = store.object_generation();
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for id in store.object_ids() {
            let Some(bytes) = store.get_object(id)? else { continue };
            let mut references = references_in(&bytes);
            references.sort();
            references.dedup();
            for target in references {
                // 走査は ID 昇順なので、各参照先の一覧も自然に昇順で積み上がる。
                map.entry(target).or_default().push(id.clone());
            }
        }
        Ok(ReferrerIndex { generation, map })
    }

    /// この索引が store の現在のオブジェクト集合について最新か(構築後に put も回収も
    /// 無い)。
    pub fn is_current(&self, store: &Store) -> bool {
        self.generation == store.object_generation()
    }

    /// id を参照している既知オブジェクトの一覧(昇順)。知らない ID は空を返す
    /// (逆引きは「自分の知る範囲」の導出データであり、空は不在の言明ではない)。
    pub fn referrers_of(&self, id: &str) -> &[String] {
        self.map.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-store-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        dir
    }

    fn small_config(dir: &Path) -> StoreConfig {
        let mut config = StoreConfig::new(dir);
        config.pack_seal_bytes = 64; // テストでは小さく封印させる
        config
    }

    /// atomic_write の tmp 名は呼び出しごとに違う(同じプロセスの 2 つの書き手が取り合わない)。
    /// 並べて書いても、どちらの中身も据わる。
    #[test]
    fn atomic_write_uses_a_unique_tmp_name_per_call() {
        let dir = temp_dir("unique-tmp");
        std::fs::create_dir_all(dir.join("tmp")).expect("mkdir");
        let first = unique_tmp_path(&dir, "write");
        let second = unique_tmp_path(&dir, "write");
        assert_ne!(first, second);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    for round in 0..50 {
                        let target = dir.join(format!("target-{i}"));
                        let content = format!("{i}-{round}");
                        atomic_write(&dir, &target, content.as_bytes()).expect("atomic_write");
                        assert_eq!(std::fs::read_to_string(&target).expect("read"), content);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("join");
        }
        assert_eq!(std::fs::read_dir(dir.join("tmp")).expect("read_dir").count(), 0);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn put_get_ref_round_trip_across_reopen() {
        let dir = temp_dir("roundtrip");
        let node_id;
        {
            let mut store = Store::open(small_config(&dir)).expect("open");
            node_id = store.node_id_hex().to_string();
            let (id, new) = store.put_object(b"{\"kind\":\"node\",\"v\":1}").expect("put");
            assert!(new);
            let (id2, new2) = store.put_object(b"{\"kind\":\"node\",\"v\":1}").expect("put");
            assert_eq!(id, id2);
            assert!(!new2, "同一内容の再投入はべき等");
            store.set_ref("notes/a", Some(&id)).expect("set_ref");
            store.set_ref("notes/b", Some(&id)).expect("set_ref");
            store.set_ref("notes/b", None).expect("tombstone");
        }
        {
            let store = Store::open(small_config(&dir)).expect("reopen");
            assert_eq!(store.node_id_hex(), node_id, "鍵が永続化されている");
            assert_eq!(store.object_count(), 1);
            let name = store.own_ref_name("notes/a");
            let state = store.get_ref(&name).expect("ref がある");
            assert!(state.target.is_some());
            let tombstone = store.get_ref(&store.own_ref_name("notes/b")).expect("ある");
            assert_eq!(tombstone.target, None);
            assert_eq!(store.last_seq(), 3);
            let report = store.fsck().expect("fsck");
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 逆引き索引(訂正の段): 参照される側から参照している既知オブジェクトが引け、
    /// c1 でない blob は参照ゼロとして飛び、知らない ID は空。書き込みが挟まると
    /// 世代(object_count)がずれ、作り直しで新しい参照が見える。
    #[test]
    fn the_referrer_index_answers_reverse_lookups_and_detects_staleness() {
        let dir = temp_dir("referrer-index");
        let mut store = Store::open(StoreConfig::new(&dir)).expect("open");
        // UTF-8 でない生 blob(パースできないオブジェクトの代表)。
        let (blob_id, _) = store.put_object(&[0x89, 0x50, 0x4e, 0x47, 0x00]).expect("put");
        let (node_id, _) =
            store.put_object(b"{\"contents\":\"x\",\"kind\":\"node\",\"v\":1}").expect("put");
        let referrer_body = format!("{{\"kind\":\"node\",\"target\":\"{node_id}\",\"v\":1}}");
        let (referrer_id, _) = store.put_object(referrer_body.as_bytes()).expect("put");

        let index = ReferrerIndex::build(&store).expect("build");
        assert!(index.is_current(&store));
        assert_eq!(index.referrers_of(&node_id), std::slice::from_ref(&referrer_id));
        assert!(index.referrers_of(&blob_id).is_empty(), "blob は誰からも参照されていない");
        // 知らない ID は空(「自分の知る範囲」の導出データであり、不在の言明ではない)。
        let unknown = format!("s256:{}", "0".repeat(64));
        assert!(index.referrers_of(&unknown).is_empty());

        // 書き込みが挟まると世代がずれ、作り直しで新しい参照が見える。
        let second_body = format!("{{\"kind\":\"node\",\"note\":\"{blob_id}\",\"v\":1}}");
        let (second_id, _) = store.put_object(second_body.as_bytes()).expect("put");
        assert!(!index.is_current(&store), "書き込み後の索引を最新と誤認してはならない");
        let rebuilt = ReferrerIndex::build(&store).expect("rebuild");
        assert!(rebuilt.is_current(&store));
        assert_eq!(rebuilt.referrers_of(&blob_id), std::slice::from_ref(&second_id));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn packs_seal_and_manifest_survives() {
        let dir = temp_dir("seal");
        {
            let mut store = Store::open(small_config(&dir)).expect("open");
            // 64バイト閾値を超えて複数 pack に分かれる量を投入する。
            for i in 0..20u32 {
                let body = format!("{{\"i\":{i},\"pad\":\"0123456789abcdef\"}}");
                store.put_object(body.as_bytes()).expect("put");
            }
            assert!(store.sealed_packs.len() >= 2, "封印が発生している");
        }
        {
            let store = Store::open(small_config(&dir)).expect("reopen");
            assert_eq!(store.object_count(), 20);
            assert!(store.fsck().expect("fsck").errors.is_empty());
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 在るストアだけを開く道は、空のディレクトリを初期化しない。断った後にディレクトリが
    /// 空のままであることまで見る(node_key が 1 つ作られれば、そこは別のノードになる)。
    #[test]
    fn open_existing_refuses_an_empty_directory_and_leaves_it_empty() {
        let dir = temp_dir("open-existing-empty");
        std::fs::create_dir_all(&dir).expect("mkdir");
        match Store::open_existing(StoreConfig::new(&dir)) {
            Err(StoreError::Invalid(message)) => {
                assert!(message.contains("データディレクトリではない"), "{message}")
            }
            Ok(_) => panic!("空のディレクトリを開いてはならない(初期化してしまう)"),
            Err(other) => panic!("断りの種類が違う: {other}"),
        }
        let left: Vec<_> = std::fs::read_dir(&dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert!(left.is_empty(), "断ったのに何か作られている: {left:?}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 存在しない道にも同じく断り、ディレクトリを作らない。
    #[test]
    fn open_existing_refuses_a_missing_directory_and_does_not_create_it() {
        let dir = temp_dir("open-existing-missing");
        assert!(!dir.exists());
        assert!(
            matches!(
                Store::open_existing(StoreConfig::new(&dir)),
                Err(StoreError::Invalid(_))
            ),
            "存在しない道は Invalid で断る"
        );
        assert!(!dir.exists(), "断ったのにディレクトリが作られている");
    }

    /// 一度でも開いたストアは、封印が起きておらず MANIFEST が無くても在るストアである。
    #[test]
    fn open_existing_opens_a_store_that_has_no_manifest_yet() {
        let dir = temp_dir("open-existing-no-manifest");
        let (id, node_id) = {
            let mut store = Store::open(StoreConfig::new(&dir)).expect("init");
            let (id, _) = store.put_object(b"one object, no seal").expect("put");
            (id, store.node_id_hex().to_string())
        };
        assert!(
            !dir.join(MANIFEST_NAME).exists(),
            "封印前に MANIFEST は無い"
        );
        let store = Store::open_existing(StoreConfig::new(&dir)).expect("在るストアは開ける");
        assert_eq!(store.node_id_hex(), node_id, "同じノードとして開く");
        assert!(store.has_object(&id));
        drop(store);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn torn_tail_on_active_segment_is_truncated() {
        let dir = temp_dir("torn");
        let id;
        {
            let mut store = Store::open(small_config(&dir)).expect("open");
            let (put_id, _) = store.put_object(b"intact object").expect("put");
            id = put_id;
        }
        // アクティブ pack の末尾に不完全なレコード(torn write)を偽造する。
        {
            let path = pack_path(&dir, 1);
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("open");
            file.write_all(&[0x99, 0x00, 0x00, 0x00, 0xde, 0xad]).expect("garbage");
        }
        {
            let store = Store::open(small_config(&dir)).expect("recover");
            assert!(store.has_object(&id), "有効部分は生きている");
            assert_eq!(store.object_count(), 1);
            assert!(store.fsck().expect("fsck").errors.is_empty());
        }
        // 再オープン後はファイルが切り詰められており、再度の回復も安定している。
        {
            let store = Store::open(small_config(&dir)).expect("stable");
            assert_eq!(store.object_count(), 1);
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn corrupted_sealed_segment_is_an_error_not_a_truncation() {
        let dir = temp_dir("sealed-corruption");
        {
            let mut store = Store::open(small_config(&dir)).expect("open");
            for i in 0..20u32 {
                let body = format!("{{\"i\":{i},\"pad\":\"0123456789abcdef\"}}");
                store.put_object(body.as_bytes()).expect("put");
            }
        }
        // 封印済み pack の中身を1バイト壊す。
        {
            let path = pack_path(&dir, 1);
            let mut bytes = std::fs::read(&path).expect("read");
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
            std::fs::write(&path, bytes).expect("write");
        }
        match Store::open(small_config(&dir)) {
            Err(StoreError::Corruption(_)) => {}
            Err(other) => panic!("封印済みの破損は Corruption になるべき: {other}"),
            Ok(_) => panic!("封印済みの破損を見逃して開けてしまった"),
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn ref_to_missing_object_is_rejected() {
        let dir = temp_dir("dangling");
        let mut store = Store::open(small_config(&dir)).expect("open");
        let missing = c1::id_for_bytes(b"never stored");
        match store.set_ref("x", Some(&missing)) {
            Err(StoreError::Invalid(_)) => {}
            other => panic!("存在しない target は Invalid になるべき: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn tampered_reflog_record_is_rejected_loudly() {
        let dir = temp_dir("tamper");
        {
            let mut store = Store::open(small_config(&dir)).expect("open");
            let (id, _) = store.put_object(b"x").expect("put");
            store.set_ref("a", Some(&id)).expect("set_ref");
        }
        // reflog の署名済みレコード内の1バイトを書き換え、CRC は付け直す
        // (=ディスク事故ではなく改竄。署名検証で検出されるべき)。
        {
            let path = reflog_path(&dir, 1);
            let bytes = std::fs::read(&path).expect("read");
            let mut payload = bytes[8..].to_vec();
            let position = payload
                .windows(5)
                .position(|w| w == b"\"at\":")
                .map(|p| p + 5)
                .expect("at フィールドがある");
            payload[position] = if payload[position] == b'1' { b'2' } else { b'1' };
            let mut rewritten = Vec::new();
            rewritten.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            rewritten.extend_from_slice(&crc32(&payload).to_le_bytes());
            rewritten.extend_from_slice(&payload);
            std::fs::write(&path, rewritten).expect("write");
        }
        match Store::open(small_config(&dir)) {
            Err(StoreError::Corruption(message)) => {
                assert!(message.contains("署名"), "署名エラーであるべき: {message}")
            }
            Err(other) => panic!("改竄は Corruption になるべき: {other}"),
            Ok(_) => panic!("改竄を見逃して開けてしまった"),
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    fn tamper_at_field(payload: &mut [u8]) {
        let position = payload
            .windows(5)
            .position(|w| w == b"\"at\":")
            .map(|p| p + 5)
            .expect("at フィールドがある");
        payload[position] = if payload[position] == b'1' { b'2' } else { b'1' };
    }

    #[test]
    fn replication_records_ingest_export_and_survive_reopen() {
        let dir_a = temp_dir("repl-a");
        let dir_b = temp_dir("repl-b");
        let a_id;
        let records;
        {
            let mut a = Store::open(small_config(&dir_a)).expect("open a");
            a_id = a.node_id_hex().to_string();
            let (leaf, _) = a.put_object(b"\"leaf\"").expect("put");
            a.set_ref("x", Some(&leaf)).expect("set x");
            a.set_ref("y", Some(&leaf)).expect("set y");
            a.set_ref("y", None).expect("tombstone y");
            records = a.export_ref_records(&a_id, 0).expect("export");
            assert_eq!(records.len(), 3);
            assert_eq!(a.export_ref_records(&a_id, 2).expect("export since").len(), 1);
        }
        {
            let mut b = Store::open(small_config(&dir_b)).expect("open b");
            for record in &records {
                assert!(b.ingest_ref_record(record).expect("ingest"), "新規として入る");
            }
            assert!(!b.ingest_ref_record(&records[0]).expect("replay"), "再取り込みは冪等");
            // 対象オブジェクト未取得でも ref は立ち、fsck はエラーではなく欠け数として出す。
            let report = b.fsck().expect("fsck");
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(report.foreign_targets_absent, 1, "x のみ(y は tombstone)");
            // レプリカは受け取ったレコードをそのまま再輸出できる(中継)。
            assert_eq!(b.export_ref_records(&a_id, 0).expect("re-export"), records);
        }
        {
            let b = Store::open(small_config(&dir_b)).expect("reopen b");
            assert_eq!(b.signers(), vec![(a_id.clone(), 3)], "reopen 後も署名者カーソルが残る");
            assert_eq!(b.last_seq(), 0, "自分の名前空間は未使用のまま");
        }
        std::fs::remove_dir_all(&dir_a).expect("cleanup");
        std::fs::remove_dir_all(&dir_b).expect("cleanup");
    }

    #[test]
    fn ingest_rejects_gaps_and_tampering() {
        let dir_a = temp_dir("gap-a");
        let dir_b = temp_dir("gap-b");
        let records;
        {
            let mut a = Store::open(small_config(&dir_a)).expect("open a");
            let (leaf, _) = a.put_object(b"\"leaf\"").expect("put");
            for i in 0..3 {
                a.set_ref(&format!("k{i}"), Some(&leaf)).expect("set");
            }
            records = a.export_ref_records(a.node_id_hex(), 0).expect("export");
        }
        let mut b = Store::open(small_config(&dir_b)).expect("open b");
        assert!(b.ingest_ref_record(&records[0]).expect("seq 1"));
        match b.ingest_ref_record(&records[2]) {
            Err(StoreError::Corruption(message)) => {
                assert!(message.contains("飛んでいる"), "{message}")
            }
            other => panic!("seq の飛びは Corruption になるべき: {other:?}"),
        }
        let mut tampered = records[1].clone();
        tamper_at_field(&mut tampered);
        match b.ingest_ref_record(&tampered) {
            Err(StoreError::Corruption(message)) => assert!(message.contains("署名"), "{message}"),
            other => panic!("改竄は Corruption になるべき: {other:?}"),
        }
        std::fs::remove_dir_all(&dir_a).expect("cleanup");
        std::fs::remove_dir_all(&dir_b).expect("cleanup");
    }

    #[test]
    fn double_open_of_the_same_directory_is_rejected() {
        let dir = temp_dir("lock");
        let first = Store::open(small_config(&dir)).expect("open");
        match Store::open(small_config(&dir)) {
            Err(StoreError::Invalid(message)) => assert!(message.contains("開いている"), "{message}"),
            Err(other) => panic!("二重オープンは Invalid になるべき: {other}"),
            Ok(_) => panic!("二重オープンできてしまった"),
        }
        drop(first);
        Store::open(small_config(&dir)).expect("解放後は開ける");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn reachable_closure_follows_references_and_tolerates_dangling() {
        let dir = temp_dir("closure");
        let mut store = Store::open(small_config(&dir)).expect("open");
        let (leaf, _) = store.put_object(b"\"leaf\"").expect("put");
        let dangling = c1::id_for_bytes(b"forgotten history");
        let edge = format!(
            "{{\"kind\":\"edge\",\"members\":[\"{leaf}\",\"{dangling}\"],\"v\":1}}"
        );
        let (root, _) = store.put_object(edge.as_bytes()).expect("put");
        let closure = store.reachable_closure(&root).expect("closure");
        assert!(closure.contains(&root));
        assert!(closure.contains(&leaf));
        assert!(!closure.contains(&dangling), "存在しない参照先は含めない");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
