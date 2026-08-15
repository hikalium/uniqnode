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
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io: {e}"),
            StoreError::Corruption(m) => write!(f, "corruption: {m}"),
            StoreError::Invalid(m) => write!(f, "invalid: {m}"),
        }
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
}

impl StoreConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> StoreConfig {
        StoreConfig {
            data_dir: data_dir.into(),
            pack_seal_bytes: 256 * 1024 * 1024,
            max_record_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ObjectLocation {
    pack_number: u64,
    /// レコードのペイロード先頭のファイル内オフセット。
    payload_offset: u64,
    payload_length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefState {
    pub target: Option<String>,
    pub seq: u64,
    pub at: i64,
}

pub struct Store {
    config: StoreConfig,
    secret_seed: [u8; 32],
    node_id_hex: String,
    /// オブジェクトID → 位置。導出データ(起動時に pack 走査で再構築)。
    object_index: BTreeMap<String, ObjectLocation>,
    refs: BTreeMap<String, RefState>,
    next_seq: u64,
    sealed_packs: Vec<u64>,
    sealed_reflogs: Vec<u64>,
    active_pack_number: u64,
    active_pack_length: u64,
    active_reflog_number: u64,
}

fn pack_path(dir: &Path, number: u64) -> PathBuf {
    dir.join("packs").join(format!("pack-{number:06}.pack"))
}

fn reflog_path(dir: &Path, number: u64) -> PathBuf {
    dir.join("reflog").join(format!("reflog-{number:06}.log"))
}

/// 追記レコード: [u32 len][u32 crc32][payload]、いずれもリトルエンディアン。
fn append_record(path: &Path, payload: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut record = Vec::with_capacity(8 + payload.len());
    record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    record.extend_from_slice(&crc32(payload).to_le_bytes());
    record.extend_from_slice(payload);
    file.write_all(&record)?;
    file.sync_data()?;
    Ok(())
}

struct RecordScan {
    /// (ペイロード先頭オフセット, ペイロード)の列。
    records: Vec<(u64, Vec<u8>)>,
    /// CRC 不一致・尻切れを検出した位置(そこまでが有効)。
    valid_length: u64,
    truncated: bool,
}

fn scan_records(path: &Path, max_record_bytes: u32) -> Result<RecordScan> {
    let bytes = std::fs::read(path)?;
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
        records.push((payload_start as u64, payload.to_vec()));
        position = payload_end;
    }
    Ok(RecordScan {
        records,
        valid_length: position as u64,
        truncated: (position as u64) < bytes.len() as u64,
    })
}

fn atomic_write(dir: &Path, target: &Path, content: &[u8]) -> Result<()> {
    let tmp_dir = dir.join("tmp");
    let tmp_path = tmp_dir.join(format!("write-{}", std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, target)?;
    // rename を含むディレクトリエントリの永続化。
    std::fs::File::open(target.parent().expect("親ディレクトリがある"))?.sync_all()?;
    Ok(())
}

fn unix_now() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

impl Store {
    pub fn node_id_hex(&self) -> &str {
        &self.node_id_hex
    }

    pub fn open(config: StoreConfig) -> Result<Store> {
        let dir = config.data_dir.clone();
        std::fs::create_dir_all(dir.join("packs"))?;
        std::fs::create_dir_all(dir.join("reflog"))?;
        std::fs::create_dir_all(dir.join("tmp"))?;

        let secret_seed = Self::load_or_create_key(&dir)?;
        let node_id_hex = sha2::hex(&ed25519::public_key(&secret_seed));

        let (sealed_packs, sealed_reflogs) = Self::load_manifest(&dir)?;

        let mut store = Store {
            config,
            secret_seed,
            node_id_hex,
            object_index: BTreeMap::new(),
            refs: BTreeMap::new(),
            next_seq: 1,
            sealed_packs,
            sealed_reflogs,
            active_pack_number: 1,
            active_pack_length: 0,
            active_reflog_number: 1,
        };
        store.recover()?;
        Ok(store)
    }

    fn load_or_create_key(dir: &Path) -> Result<[u8; 32]> {
        let key_path = dir.join("node_key");
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
            let seed = ed25519::generate_secret_seed()?;
            std::fs::write(&key_path, seed)?;
            let mut permissions = std::fs::metadata(&key_path)?.permissions();
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
            std::fs::set_permissions(&key_path, permissions)?;
            Ok(seed)
        }
    }

    fn load_manifest(dir: &Path) -> Result<(Vec<u64>, Vec<u64>)> {
        let manifest_path = dir.join("MANIFEST");
        if !manifest_path.exists() {
            return Ok((Vec::new(), Vec::new()));
        }
        let text = std::fs::read_to_string(&manifest_path)?;
        let value = c1::parse(&text)
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

    fn write_manifest(&self) -> Result<()> {
        let mut map = BTreeMap::new();
        map.insert("v".to_string(), c1::Value::Integer(1));
        map.insert(
            "format".to_string(),
            c1::Value::Text("uniqnode-store-1".to_string()),
        );
        map.insert(
            "sealed_packs".to_string(),
            c1::Value::Array(
                self.sealed_packs.iter().map(|n| c1::Value::Integer(*n as i64)).collect(),
            ),
        );
        map.insert(
            "sealed_reflogs".to_string(),
            c1::Value::Array(
                self.sealed_reflogs.iter().map(|n| c1::Value::Integer(*n as i64)).collect(),
            ),
        );
        let content = c1::to_canonical_bytes(&c1::Value::Object(map));
        atomic_write(&self.config.data_dir, &self.config.data_dir.join("MANIFEST"), &content)
    }

    /// 起動時回復。封印済みセグメントは完全でなければならず(破損は Corruption)、
    /// 未封印(アクティブ)セグメントは最後の1つに限り torn tail を切り詰めてよい。
    fn recover(&mut self) -> Result<()> {
        let dir = self.config.data_dir.clone();
        let list = |sub: &str, prefix: &str, suffix: &str| -> Result<Vec<u64>> {
            let mut numbers = Vec::new();
            for entry in std::fs::read_dir(dir.join(sub))? {
                let name = entry?.file_name().to_string_lossy().to_string();
                if let Some(rest) = name.strip_prefix(prefix) {
                    if let Some(number_text) = rest.strip_suffix(suffix) {
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
        };

        // pack の走査(オブジェクト索引の再構築 = 導出データ、I4)。
        let pack_numbers = list("packs", "pack-", ".pack")?;
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
            for (offset, payload) in &scan.records {
                let id = c1::id_for_bytes(payload);
                // 重複追記(クラッシュ再送)は最初の1つだけ索引に載せる。
                self.object_index.entry(id).or_insert(ObjectLocation {
                    pack_number: *number,
                    payload_offset: *offset,
                    payload_length: payload.len() as u32,
                });
            }
            if !sealed && is_last {
                self.active_pack_number = *number;
                self.active_pack_length = std::fs::metadata(&path)?.len();
            }
        }
        if let Some(max) = pack_numbers.last() {
            if self.sealed_packs.contains(max) {
                self.active_pack_number = max + 1;
                self.active_pack_length = 0;
            }
        }

        // reflog の再生。
        let reflog_numbers = list("reflog", "reflog-", ".log")?;
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
                self.apply_ref_record(payload)?;
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

    /// reflog レコード(c1 JSON)を検証してメモリ状態に適用する。
    fn apply_ref_record(&mut self, payload: &[u8]) -> Result<()> {
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
        let name = text_field("name")?;
        let seq = integer_field("seq")? as u64;
        let at = integer_field("at")?;
        let signer = text_field("signer")?;
        let signature_hex = text_field("sig")?;
        let target = match map.get("target") {
            Some(c1::Value::Text(t)) => Some(t.clone()),
            Some(c1::Value::Null) => None,
            _ => return Err(StoreError::Corruption("reflog レコードの target が不正".into())),
        };

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
        if !name.starts_with(&format!("{signer}/")) {
            return Err(StoreError::Corruption(format!(
                "ref {name} は signer の名前空間でない(single writer 違反)"
            )));
        }
        if seq != self.next_seq {
            return Err(StoreError::Corruption(format!(
                "seq が飛んでいる: 期待 {} 実際 {seq}",
                self.next_seq
            )));
        }
        self.next_seq = seq + 1;
        self.refs.insert(name, RefState { target, seq, at });
        Ok(())
    }

    // ---- 書き込み ----

    /// オブジェクト投入(べき等)。返り値は (ID, 新規に保存されたか)。
    pub fn put_object(&mut self, bytes: &[u8]) -> Result<(String, bool)> {
        if bytes.len() as u64 > self.config.max_record_bytes as u64 {
            return Err(StoreError::Invalid(format!(
                "オブジェクトが大きすぎる({} bytes)",
                bytes.len()
            )));
        }
        let id = c1::id_for_bytes(bytes);
        if self.object_index.contains_key(&id) {
            return Ok((id, false));
        }
        if self.active_pack_length >= self.config.pack_seal_bytes {
            self.seal_active_pack()?;
        }
        let path = pack_path(&self.config.data_dir, self.active_pack_number);
        let offset_before = self.active_pack_length;
        append_record(&path, bytes)?;
        self.active_pack_length += 8 + bytes.len() as u64;
        self.object_index.insert(
            id.clone(),
            ObjectLocation {
                pack_number: self.active_pack_number,
                payload_offset: offset_before + 8,
                payload_length: bytes.len() as u32,
            },
        );
        Ok((id, true))
    }

    fn seal_active_pack(&mut self) -> Result<()> {
        self.sealed_packs.push(self.active_pack_number);
        self.write_manifest()?;
        self.active_pack_number += 1;
        self.active_pack_length = 0;
        Ok(())
    }

    /// 自分の名前空間の ref を更新する。target=None が tombstone。
    pub fn set_ref(&mut self, path: &str, target: Option<&str>) -> Result<u64> {
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
        let seq = self.next_seq;
        let at = unix_now();

        let mut map = BTreeMap::new();
        map.insert("v".to_string(), c1::Value::Integer(1));
        map.insert("type".to_string(), c1::Value::Text("set_ref".to_string()));
        map.insert("name".to_string(), c1::Value::Text(name.clone()));
        map.insert(
            "target".to_string(),
            match target {
                Some(t) => c1::Value::Text(t.to_string()),
                None => c1::Value::Null,
            },
        );
        map.insert("seq".to_string(), c1::Value::Integer(seq as i64));
        map.insert("at".to_string(), c1::Value::Integer(at));
        map.insert("signer".to_string(), c1::Value::Text(self.node_id_hex.clone()));
        let message = c1::to_canonical_bytes(&c1::Value::Object(map.clone()));
        let signature = ed25519::sign(&self.secret_seed, &message);
        map.insert("sig".to_string(), c1::Value::Text(sha2::hex(&signature)));
        let payload = c1::to_canonical_bytes(&c1::Value::Object(map));

        let log_path = reflog_path(&self.config.data_dir, self.active_reflog_number);
        append_record(&log_path, &payload)?;
        self.apply_ref_record(&payload)?;
        Ok(seq)
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

    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// root から c1 参照(SPEC §4.3)を辿って到達可能な閉包を返す。
    /// ローカルに存在しない参照先は結果に含めない(開世界: dangling は無害)。
    pub fn reachable_closure(&self, root: &str) -> Result<Vec<String>> {
        let mut seen = std::collections::BTreeSet::new();
        let mut present = Vec::new();
        let mut queue = vec![root.to_string()];
        while let Some(id) = queue.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let bytes = match self.get_object(&id)? {
                None => continue, // dangling: 世界が忘れた参照。結果に含めない
                Some(b) => b,
            };
            if let Ok(text) = std::str::from_utf8(&bytes) {
                if let Ok(value) = c1::parse(text) {
                    let mut references = Vec::new();
                    c1::collect_references(&value, &mut references);
                    for r in references {
                        if !seen.contains(&r) {
                            queue.push(r);
                        }
                    }
                }
            }
            present.push(id);
        }
        present.sort();
        Ok(present)
    }

    // ---- 検証 ----

    /// 全 pack の再走査+再ハッシュと、ref の整合検査。content-addressed なので
    /// 「内容が正しいか」まで機械的に検証できる(SPEC §5.5)。
    pub fn fsck(&self) -> Result<FsckReport> {
        let mut report = FsckReport::default();
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
                    report.errors.push(format!("ref {name} の target {target} が存在しない"));
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
    pub errors: Vec<String>,
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
