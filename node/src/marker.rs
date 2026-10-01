//! 開いている印 `open-marker`(SPEC §5.3、APPEND_FAILURE の方針 5)。
//!
//! 書ける道で開くプロセスは、ストアを変える最初の操作より前に印へ `Running` を書き、無事な
//! 終わり方でだけ `Clean` へ書き換える。書けない状態に入ったら `Io` か `NoSpace` を書く。次に
//! 開く道は印を読み、同じブートの `Running`・`Io`・検めを通らない印なら書かずに開く(保留)。
//! 書けなかった末尾が page cache にだけ残ったまま、serve の起こし直しでその後ろに追記を重ねない
//! ためである。
//!
//! 印は固定長(4 KiB)で、作るときに `fallocate` で領域を確保し、以後は同じ場所への全長の
//! `pwrite` と `fdatasync` だけで状態を変える。形は先頭から、魔法の語(8)・版(4)・状態(4)・
//! boot_id(36)・pid(4)・nonce(16)・時刻(8)・それら全体の CRC32(4)で、残りは 0 で埋める。
//! 数はリトルエンディアン。読めない・短い・CRC が合わない・状態が不明な印は検めを通らない。

use crate::crc32::crc32;
use std::io::Read;
use std::path::{Path, PathBuf};

/// データのディレクトリの中の印の名。
pub const MARKER_NAME: &str = "open-marker";

/// 印の長さ(作るときに確保する)。
pub const MARKER_BYTES: usize = 4096;

/// boot_id の読み口。
pub const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

/// boot_id の差し替え(debug ビルドだけが読む。ホストの再起動に当たる形を試験が作る口)。
pub const BOOT_ID_ENV: &str = "UNIQNODE_BOOT_ID";

const MAGIC: [u8; 8] = *b"UNQMARK\0";
const VERSION: u32 = 1;
const BOOT_ID_LEN: usize = 36;
/// CRC の前までの長さ。
const BODY_LEN: usize = 8 + 4 + 4 + BOOT_ID_LEN + 4 + 16 + 8;

/// 印の状態(方針 5 の 4 つ)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerState {
    /// 書ける道で開いて動いている(か、無事に終わらなかった)。
    Running,
    /// 書けない状態(kind が no_space。切り詰めが永続している)。
    NoSpace,
    /// 書けない状態(kind が io)。
    Io,
    /// 無事に閉じた。
    Clean,
}

impl MarkerState {
    fn code(&self) -> u32 {
        match self {
            MarkerState::Running => 1,
            MarkerState::NoSpace => 2,
            MarkerState::Io => 3,
            MarkerState::Clean => 4,
        }
    }

    fn from_code(code: u32) -> Option<MarkerState> {
        match code {
            1 => Some(MarkerState::Running),
            2 => Some(MarkerState::NoSpace),
            3 => Some(MarkerState::Io),
            4 => Some(MarkerState::Clean),
            _ => None,
        }
    }

    /// 字句(hold-status の出力とログが言う)。
    pub fn name(&self) -> &'static str {
        match self {
            MarkerState::Running => "running",
            MarkerState::NoSpace => "no_space",
            MarkerState::Io => "io",
            MarkerState::Clean => "clean",
        }
    }
}

/// 検めを通った印の中身。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub state: MarkerState,
    pub boot_id: String,
    pub pid: u32,
    pub nonce: [u8; 16],
    /// 書いた時刻(unix 秒)。
    pub time: i64,
}

impl Marker {
    /// 固定長のバイト列にする。
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(MARKER_BYTES);
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.state.code().to_le_bytes());
        let mut boot = [b' '; BOOT_ID_LEN];
        let given = self.boot_id.as_bytes();
        boot[..given.len().min(BOOT_ID_LEN)].copy_from_slice(&given[..given.len().min(BOOT_ID_LEN)]);
        bytes.extend_from_slice(&boot);
        bytes.extend_from_slice(&self.pid.to_le_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.time.to_le_bytes());
        let crc = crc32(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.resize(MARKER_BYTES, 0);
        bytes
    }

    /// 固定長のバイト列を検めて読む。通らなければ理由を返す。
    pub fn decode(bytes: &[u8]) -> Result<Marker, String> {
        if bytes.len() != MARKER_BYTES {
            return Err(format!("長さが {} バイト({MARKER_BYTES} のはず)", bytes.len()));
        }
        if bytes[..8] != MAGIC {
            return Err("魔法の語が違う".to_string());
        }
        let stored_crc = u32::from_le_bytes(take(bytes, BODY_LEN));
        if crc32(&bytes[..BODY_LEN]) != stored_crc {
            return Err("CRC が合わない".to_string());
        }
        let version = u32::from_le_bytes(take(bytes, 8));
        if version != VERSION {
            return Err(format!("知らない版 {version}"));
        }
        let code = u32::from_le_bytes(take(bytes, 12));
        let state = MarkerState::from_code(code).ok_or_else(|| format!("知らない状態 {code}"))?;
        let boot_id = std::str::from_utf8(&bytes[16..16 + BOOT_ID_LEN])
            .map_err(|_| "boot_id が UTF-8 でない".to_string())?
            .to_string();
        let at = 16 + BOOT_ID_LEN;
        let pid = u32::from_le_bytes(take(bytes, at));
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&bytes[at + 4..at + 20]);
        let time = i64::from_le_bytes(take(bytes, at + 20));
        Ok(Marker { state, boot_id, pid, nonce, time })
    }
}

fn take<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[at..at + N]);
    out
}

/// 印を読んだ結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkerRead {
    /// 印が無い(S1b より前のストアか、初めて開く)。
    Absent,
    Valid(Marker),
    /// 検めを通らない(理由)。
    Invalid(String),
}

/// データのディレクトリの印を読む。書かない。
pub fn read(dir: &Path) -> MarkerRead {
    let path = dir.join(MARKER_NAME);
    let mut bytes = Vec::new();
    match std::fs::File::open(&path).and_then(|mut file| file.read_to_end(&mut bytes)) {
        Ok(_) => match Marker::decode(&bytes) {
            Ok(marker) => MarkerRead::Valid(marker),
            Err(reason) => MarkerRead::Invalid(reason),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MarkerRead::Absent,
        Err(error) => MarkerRead::Invalid(format!("読めない: {error}")),
    }
}

/// 開く道の判定(方針 5 の「開くとき」)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 書ける道で開いてよい。中身は状態の 1 行。
    Open(String),
    /// 保留(書けない状態で開く)。
    Hold(HoldReason),
}

/// 保留の理由と、ホストの再起動で解けるか。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldReason {
    pub reason: String,
    /// ホストの再起動(boot_id が変わる)で解けるか。検めを通らない印は boot_id を信用しないので
    /// 解けず、release-hold が要る。
    pub reboot_clears: bool,
}

/// 前のプロセスが殺された・落ちた(同じブートの `Running`)ときの理由。試験はこの定数で照らす
/// (must/0023)。
pub const UNCLEAN_REASON: &str = "前のプロセスが無事に終わらなかった";

/// 前のプロセスが書き込みに失敗した(同じブートの `Io`)ときの理由。
pub const IO_MARKER_REASON: &str = "前のプロセスが書き込みに失敗した(印は io)";

/// 検めを通らない印の理由の頭。
pub const INVALID_MARKER_REASON: &str = "open-marker が検めを通らない";

/// 書ける道で `Running` を書けなかったときの理由の頭。
pub const UNWRITABLE_MARKER_REASON: &str = "印を書けない";

/// 保留の戻し方の案内(hold-status と保留の誤りの文が言う)。試験はこの定数で照らす。
pub const HOLD_GUIDANCE: &str = "ホストを再起動する(できなければ、ストアを置いたファイルシステムを umount して \
     fsck し、mount し直して page cache を捨ててから uniqnode release-hold <dir> を打つ)";

/// 検めを通らない印の戻し方の案内(ホストを再起動しても解けない)。
pub const INVALID_HOLD_GUIDANCE: &str = "ホストを再起動しても保留のまま。ストアを置いたファイルシステムを \
     umount して fsck し、mount し直して page cache を捨ててから(ホストを再起動したならその後で) \
     uniqnode release-hold <dir> を打つ";

impl HoldReason {
    /// 戻し方の案内。
    pub fn guidance(&self) -> &'static str {
        if self.reboot_clears {
            HOLD_GUIDANCE
        } else {
            INVALID_HOLD_GUIDANCE
        }
    }
}

/// 読んだ印と今の boot_id から、開く道を決める。
pub fn judge(read: &MarkerRead, boot_id: &str) -> Verdict {
    match read {
        MarkerRead::Absent => Verdict::Open("印が無い".to_string()),
        MarkerRead::Invalid(why) => Verdict::Hold(HoldReason {
            reason: format!("{INVALID_MARKER_REASON}({why})"),
            reboot_clears: false,
        }),
        MarkerRead::Valid(marker) => {
            let same_boot = marker.boot_id == boot_id;
            match (marker.state, same_boot) {
                (MarkerState::Clean, _) => Verdict::Open("印は clean".to_string()),
                (state, false) => Verdict::Open(format!(
                    "印は {}(ホストの再起動の前の印なので、書ける道で開いてよい)",
                    state.name()
                )),
                (MarkerState::NoSpace, true) => Verdict::Open(
                    "印は no_space(切り詰めは永続しているので、書ける道で開いてよい)".to_string(),
                ),
                (MarkerState::Running, true) => Verdict::Hold(HoldReason {
                    reason: UNCLEAN_REASON.to_string(),
                    reboot_clears: true,
                }),
                (MarkerState::Io, true) => Verdict::Hold(HoldReason {
                    reason: IO_MARKER_REASON.to_string(),
                    reboot_clears: true,
                }),
            }
        }
    }
}

/// 今の boot_id。debug ビルドでは `given`(StoreConfig の欄)か BOOT_ID_ENV で差し替えられる。
pub fn current_boot_id(given: Option<&str>) -> std::io::Result<String> {
    #[cfg(debug_assertions)]
    {
        if let Some(given) = given {
            return Ok(given.to_string());
        }
        if let Ok(given) = std::env::var(BOOT_ID_ENV) {
            return Ok(given);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = given;
    let text = std::fs::read_to_string(BOOT_ID_PATH)?;
    let boot_id = text.trim().to_string();
    if boot_id.len() != BOOT_ID_LEN {
        return Err(std::io::Error::other(format!(
            "{BOOT_ID_PATH} の中身が boot_id の形でない: {boot_id:?}"
        )));
    }
    Ok(boot_id)
}

/// 開くたびの nonce(`/dev/urandom` の 16 バイト)。
pub fn new_nonce() -> std::io::Result<[u8; 16]> {
    let mut nonce = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    Ok(nonce)
}

/// 書くために開いた印。fd を持ち続け、状態の書き換えは全長の `pwrite` と `fdatasync` だけで行う。
pub struct MarkerFile {
    file: std::fs::File,
    path: PathBuf,
    boot_id: String,
    nonce: [u8; 16],
    /// 印の書き込みへの注入(debug ビルドだけ)。
    #[cfg(debug_assertions)]
    fault: Option<crate::fault::Fault>,
}

impl MarkerFile {
    /// 印を開く。無ければ作って領域を確保する(返り値の bool が真)。中身はまだ書かない。
    pub fn open_or_create(
        dir: &Path,
        boot_id: &str,
        nonce: [u8; 16],
    ) -> std::io::Result<(MarkerFile, bool)> {
        let path = dir.join(MARKER_NAME);
        let created = !path.exists();
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).open(&path)?;
        if created || file.metadata()?.len() < MARKER_BYTES as u64 {
            allocate(&file, MARKER_BYTES as u64)?;
        }
        Ok((
            MarkerFile {
                file,
                path,
                boot_id: boot_id.to_string(),
                nonce,
                #[cfg(debug_assertions)]
                fault: crate::fault::Fault::from_env().ok().flatten(),
            },
            created,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn nonce(&self) -> [u8; 16] {
        self.nonce
    }

    /// 状態を書く: 全長を 1 回の `pwrite` で書けたこと(返った長さ)と `fdatasync` の成功を
    /// 確かめる。
    pub fn write(&mut self, state: MarkerState) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        let marker = Marker {
            state,
            boot_id: self.boot_id.clone(),
            pid: std::process::id(),
            nonce: self.nonce,
            time: crate::clock::unix_now(),
        };
        let bytes = marker.encode();
        let file = &self.file;
        let write_at = |bytes: &[u8]| file.write_at(bytes, 0);
        let sync = || file.sync_data();
        #[cfg(debug_assertions)]
        let written = {
            let (mut write_at, mut sync) = (write_at, sync);
            let fired = self.fault.as_mut().and_then(|fault| fault.on_marker());
            crate::fault::marker_stage(fired, &bytes, &mut write_at, &mut sync)?
        };
        #[cfg(not(debug_assertions))]
        let written = {
            let written = write_at(&bytes)?;
            sync()?;
            written
        };
        if written != bytes.len() {
            return Err(std::io::Error::other(format!(
                "{} への書き込みが短い({written} / {} バイト)",
                self.path.display(),
                bytes.len()
            )));
        }
        crate::store::note_sync("file", &self.path);
        Ok(())
    }

    /// 作ったばかりの印の中身とメタデータ、データのディレクトリを sync する。
    pub fn sync_created(&self, dir: &Path) -> std::io::Result<()> {
        crate::store::sync_file_all(&self.file, &self.path)?;
        crate::store::sync_dir(dir)
    }
}

/// 領域の確保(`fallocate`)。ファイルシステムが対応しなければ長さだけを伸ばす。
fn allocate(file: &std::fs::File, length: u64) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn fallocate(fd: std::os::raw::c_int, mode: std::os::raw::c_int, offset: i64, len: i64)
            -> std::os::raw::c_int;
    }
    const EOPNOTSUPP: i32 = 95;
    // SAFETY: 開いている fd に対するシステムコールの薄い包みで、メモリを渡さない。
    let result = unsafe { fallocate(file.as_raw_fd(), 0, 0, length as i64) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(EOPNOTSUPP) {
        return file.set_len(length);
    }
    Err(error)
}

/// hold-status の答え。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldStatus {
    /// 書ける道で開いてよい(状態の 1 行)。
    Open(String),
    Hold(HoldReason),
}

/// ストアと認めるか(hold-status と release-hold の前置き)。印か MANIFEST があるか、node_key と
/// packs/ があるディレクトリをストアとする。認めなければ何も作らずに理由を返す。
fn require_marker_store(dir: &Path) -> crate::store::Result<()> {
    if dir.is_dir()
        && (dir.join(MARKER_NAME).exists()
            || dir.join(crate::store::MANIFEST_NAME).exists()
            || crate::store::require_store_dir(dir).is_ok())
    {
        return Ok(());
    }
    Err(crate::store::StoreError::Invalid(format!(
        "{} はストアのデータディレクトリではない({MARKER_NAME} も {} も無い。何も作っていない)",
        dir.display(),
        crate::store::MANIFEST_NAME
    )))
}

/// 保留の照会(`uniqnode hold-status <dir>`)。ストアのロックを取り、印を読むだけで、何も書かない。
pub fn hold_status(dir: &Path) -> crate::store::Result<HoldStatus> {
    require_marker_store(dir)?;
    let _lock = crate::store::acquire_store_lock(dir)?;
    let boot_id = current_boot_id(None)?;
    Ok(match judge(&read(dir), &boot_id) {
        Verdict::Open(line) => HoldStatus::Open(line),
        Verdict::Hold(reason) => HoldStatus::Hold(reason),
    })
}

/// 保留を解く(`uniqnode release-hold <dir>`)。ストアのロックを取り、印を `Clean` に書き換える
/// (`pwrite` と `fdatasync` だけ)。印が無ければ何も作らずに、その旨を返す。
pub fn release_hold(dir: &Path) -> crate::store::Result<String> {
    require_marker_store(dir)?;
    let _lock = crate::store::acquire_store_lock(dir)?;
    let before = read(dir);
    if before == MarkerRead::Absent {
        return Ok("印が無い(保留ではない)。何も書いていない".to_string());
    }
    let boot_id = current_boot_id(None)?;
    let (mut marker, _) = MarkerFile::open_or_create(dir, &boot_id, new_nonce()?)?;
    marker.write(MarkerState::Clean)?;
    Ok(match before {
        MarkerRead::Valid(previous) => {
            format!("印を {} から clean に書き換えた", previous.state.name())
        }
        _ => "検めを通らない印を clean に書き換えた".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(state: MarkerState, boot_id: &str) -> Marker {
        Marker {
            state,
            boot_id: boot_id.to_string(),
            pid: 4242,
            nonce: [7u8; 16],
            time: 1_790_000_000,
        }
    }

    const BOOT: &str = "11111111-2222-3333-4444-555555555555";
    const OTHER_BOOT: &str = "99999999-2222-3333-4444-555555555555";

    #[test]
    fn a_marker_round_trips_through_its_fixed_length_form() {
        let written = marker(MarkerState::NoSpace, BOOT);
        let bytes = written.encode();
        assert_eq!(bytes.len(), MARKER_BYTES);
        assert_eq!(Marker::decode(&bytes), Ok(written));
    }

    #[test]
    fn a_marker_with_a_bad_crc_short_length_or_unknown_state_does_not_pass() {
        let mut bytes = marker(MarkerState::Running, BOOT).encode();
        bytes[20] ^= 1;
        assert!(Marker::decode(&bytes).unwrap_err().contains("CRC"));
        let bytes = marker(MarkerState::Running, BOOT).encode();
        assert!(Marker::decode(&bytes[..100]).is_err());
        let mut bytes = marker(MarkerState::Running, BOOT).encode();
        bytes[12..16].copy_from_slice(&9u32.to_le_bytes());
        let crc = crc32(&bytes[..BODY_LEN]);
        bytes[BODY_LEN..BODY_LEN + 4].copy_from_slice(&crc.to_le_bytes());
        assert!(Marker::decode(&bytes).unwrap_err().contains("知らない状態"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-marker-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        dir
    }

    #[test]
    fn a_written_marker_keeps_its_fixed_length_and_reads_back() {
        let dir = temp_dir("write");
        std::fs::create_dir(&dir).expect("mkdir");
        let (mut file, created) =
            MarkerFile::open_or_create(&dir, BOOT, [3u8; 16]).expect("create");
        assert!(created);
        file.write(MarkerState::Running).expect("running");
        file.write(MarkerState::Clean).expect("clean");
        assert_eq!(std::fs::metadata(dir.join(MARKER_NAME)).unwrap().len(), MARKER_BYTES as u64);
        match read(&dir) {
            MarkerRead::Valid(marker) => {
                assert_eq!(marker.state, MarkerState::Clean);
                assert_eq!(marker.boot_id, BOOT);
                assert_eq!(marker.nonce, [3u8; 16]);
                assert_eq!(marker.pid, std::process::id());
            }
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn hold_status_and_release_hold_refuse_a_non_store_and_create_nothing() {
        let dir = temp_dir("not-a-store");
        assert!(hold_status(&dir).is_err());
        assert!(release_hold(&dir).is_err());
        assert!(!dir.exists(), "無い道を作らない");
        std::fs::create_dir(&dir).expect("mkdir");
        assert!(hold_status(&dir).is_err());
        assert!(release_hold(&dir).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "空のディレクトリに何も作らない");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_open_decision_follows_state_and_boot_id() {
        let valid = |state| MarkerRead::Valid(marker(state, BOOT));
        assert!(matches!(judge(&MarkerRead::Absent, BOOT), Verdict::Open(_)));
        assert!(matches!(judge(&valid(MarkerState::Clean), BOOT), Verdict::Open(_)));
        assert!(matches!(judge(&valid(MarkerState::NoSpace), BOOT), Verdict::Open(_)));
        for state in [MarkerState::Running, MarkerState::Io, MarkerState::NoSpace] {
            assert!(matches!(judge(&valid(state), OTHER_BOOT), Verdict::Open(_)), "{state:?}");
        }
        match judge(&valid(MarkerState::Running), BOOT) {
            Verdict::Hold(hold) => {
                assert_eq!(hold.reason, UNCLEAN_REASON);
                assert!(hold.reboot_clears);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(judge(&valid(MarkerState::Io), BOOT), Verdict::Hold(_)));
        match judge(&MarkerRead::Invalid("CRC が合わない".into()), OTHER_BOOT) {
            Verdict::Hold(hold) => {
                assert!(hold.reason.starts_with(INVALID_MARKER_REASON));
                assert!(!hold.reboot_clears, "検めを通らない印は boot_id を信用しない");
            }
            other => panic!("{other:?}"),
        }
    }
}
