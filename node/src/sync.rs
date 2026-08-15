//! レプリケーション(SPEC §7.3)。pull 型: 相手の署名者一覧と署名済み ref レコードを
//! 取り寄せ、対象オブジェクトの到達閉包を have/want(= GET と 404)で埋める。
//!
//! 取り寄せた内容はどちらも自己認証的である: ref レコードは所有者の署名で、
//! オブジェクトは content-addressing(受信バイト列のハッシュ = 要求 ID の照合)で
//! 検証されるため、トランスポートにも相手にも真正性を頼らない。相手が持っていない
//! ものは欠けとして数えるだけでエラーにしない(開世界)。

use crate::c1;
use crate::http::{Request, Response};
use crate::store::{Store, StoreError};
use std::collections::BTreeSet;

#[derive(Debug)]
pub enum SyncError {
    Peer(String),
    Store(StoreError),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Peer(m) => write!(f, "peer: {m}"),
            SyncError::Store(e) => write!(f, "store: {e}"),
        }
    }
}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> Self {
        SyncError::Store(e)
    }
}

/// 相手から見た ref の像(分散クエリの応答)。署名付きレコードそのものではないため、
/// これを ref として取り込んではならない(取り込みは sync の署名済みレコード経由のみ)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefRecordView {
    pub name: String,
    pub target: Option<String>,
    pub seq: u64,
    pub at: i64,
    pub signer: String,
}

/// 同期・クエリの取り寄せ元。HTTP 越し(HttpPeer)とプロセス内(LocalPeer)が同じ核を使う
/// (should/0135: 判定は一箇所)。
pub trait PeerSource {
    fn signers(&self) -> Result<Vec<(String, u64)>, SyncError>;
    fn refs_since(&self, signer: &str, since: u64) -> Result<Vec<Vec<u8>>, SyncError>;
    /// None = 相手も持っていない(開世界: 不存在の言明ではない)。
    fn fetch_object(&self, id: &str) -> Result<Option<Vec<u8>>, SyncError>;
    /// None = 相手はその ref を知らない(同上)。
    fn fetch_ref(&self, name: &str) -> Result<Option<RefRecordView>, SyncError>;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub signers_seen: usize,
    pub records_ingested: usize,
    pub records_already_known: usize,
    pub objects_fetched: usize,
    /// 相手も持っていなかった参照先の数(エラーではない)。
    pub objects_absent: usize,
    /// 要求 ID とハッシュが一致しなかった応答の数(捨てた。相手の不具合か改竄)。
    pub hash_mismatches: usize,
}

/// 相手から ref レコードとオブジェクト閉包を取り寄せる。何度実行しても安全(べき等)で、
/// 前回途中で失敗していても続きから埋まる。
pub fn sync_from_peer(store: &mut Store, peer: &dyn PeerSource) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::default();

    // 1. ref レコードの取り寄せ(gossip の pull 版)。
    for (signer, peer_last_seq) in peer.signers()? {
        report.signers_seen += 1;
        let local_last = store
            .signers()
            .into_iter()
            .find(|(s, _)| *s == signer)
            .map(|(_, seq)| seq)
            .unwrap_or(0);
        if peer_last_seq <= local_last {
            continue;
        }
        for payload in peer.refs_since(&signer, local_last)? {
            if store.ingest_ref_record(&payload)? {
                report.records_ingested += 1;
            } else {
                report.records_already_known += 1;
            }
        }
    }

    // 2. have/want: すべての ref の対象から到達閉包を歩き、欠けているオブジェクトを
    //    取り寄せる。過去の同期が途中で死んでいても、ここで穴が埋まる。
    let mut queue: Vec<String> = store
        .list_refs()
        .filter_map(|(_, state)| state.target.clone())
        .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let bytes = match store.get_object(&id)? {
            Some(bytes) => bytes,
            None => match peer.fetch_object(&id)? {
                None => {
                    report.objects_absent += 1;
                    continue;
                }
                Some(bytes) => {
                    // content-addressing による検証: 受信バイト列が要求 ID と一致しない
                    // 応答は保存しない(SPEC §7.1)。
                    if c1::id_for_bytes(&bytes) != id {
                        report.hash_mismatches += 1;
                        continue;
                    }
                    store.put_object(&bytes)?;
                    report.objects_fetched += 1;
                    bytes
                }
            },
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
    }
    Ok(report)
}

/// プロセス内の別ストアを取り寄せ元にする(モデルテスト・将来のツール用)。
pub struct LocalPeer<'a>(pub &'a Store);

impl<'a> PeerSource for LocalPeer<'a> {
    fn signers(&self) -> Result<Vec<(String, u64)>, SyncError> {
        Ok(self.0.signers())
    }
    fn refs_since(&self, signer: &str, since: u64) -> Result<Vec<Vec<u8>>, SyncError> {
        Ok(self.0.export_ref_records(signer, since)?)
    }
    fn fetch_object(&self, id: &str) -> Result<Option<Vec<u8>>, SyncError> {
        Ok(self.0.get_object(id)?)
    }
    fn fetch_ref(&self, name: &str) -> Result<Option<RefRecordView>, SyncError> {
        Ok(self.0.get_ref(name).map(|state| RefRecordView {
            name: name.to_string(),
            target: state.target.clone(),
            seq: state.seq,
            at: state.at,
            signer: name.split('/').next().unwrap_or("").to_string(),
        }))
    }
}

/// HTTP のノードローカル API(SPEC §10)を取り寄せ元にする。
pub struct HttpPeer {
    pub address: String,
}

impl HttpPeer {
    /// 1リクエスト1接続の最小 HTTP/1.1 クライアント。
    fn get(&self, path: &str) -> Result<(u16, Vec<u8>), SyncError> {
        use std::io::{BufRead, BufReader, Read, Write};
        let stream = std::net::TcpStream::connect(&self.address)
            .map_err(|e| SyncError::Peer(format!("{} に接続できない: {e}", self.address)))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .map_err(|e| SyncError::Peer(format!("timeout 設定: {e}")))?;
        let mut writer = stream.try_clone().map_err(|e| SyncError::Peer(e.to_string()))?;
        writer
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
            )
            .map_err(|e| SyncError::Peer(format!("送信: {e}")))?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader
            .read_line(&mut status_line)
            .map_err(|e| SyncError::Peer(format!("応答読み取り: {e}")))?;
        let status: u16 = status_line
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| SyncError::Peer(format!("応答ラインが不正: {status_line:?}")))?;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| SyncError::Peer(format!("ヘッダ読み取り: {e}")))?;
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    content_length = value
                        .trim()
                        .parse()
                        .map_err(|_| SyncError::Peer("Content-Length が不正".into()))?;
                }
            }
        }
        let mut body = vec![0u8; content_length];
        reader
            .read_exact(&mut body)
            .map_err(|e| SyncError::Peer(format!("ボディ読み取り: {e}")))?;
        Ok((status, body))
    }

    fn get_json(&self, path: &str) -> Result<c1::Value, SyncError> {
        let (status, body) = self.get(path)?;
        if status != 200 {
            return Err(SyncError::Peer(format!("{path} が {status} を返した")));
        }
        let text = std::str::from_utf8(&body)
            .map_err(|_| SyncError::Peer(format!("{path} の応答が UTF-8 でない")))?;
        c1::parse(text).map_err(|e| SyncError::Peer(format!("{path} の応答が JSON でない: {e}")))
    }
}

impl PeerSource for HttpPeer {
    fn signers(&self) -> Result<Vec<(String, u64)>, SyncError> {
        let value = self.get_json("/v1/replication/signers")?;
        let items = match &value {
            c1::Value::Object(map) => match map.get("signers") {
                Some(c1::Value::Array(items)) => items.clone(),
                _ => return Err(SyncError::Peer("signers 配列がない".into())),
            },
            _ => return Err(SyncError::Peer("応答がオブジェクトでない".into())),
        };
        let mut out = Vec::new();
        for item in items {
            if let c1::Value::Object(map) = item {
                let signer = match map.get("signer") {
                    Some(c1::Value::Text(t)) => t.clone(),
                    _ => return Err(SyncError::Peer("signer がない".into())),
                };
                let last_seq = match map.get("last_seq") {
                    Some(c1::Value::Integer(n)) => *n as u64,
                    _ => return Err(SyncError::Peer("last_seq がない".into())),
                };
                out.push((signer, last_seq));
            }
        }
        Ok(out)
    }

    fn refs_since(&self, signer: &str, since: u64) -> Result<Vec<Vec<u8>>, SyncError> {
        let value =
            self.get_json(&format!("/v1/replication/refs?signer={signer}&since={since}"))?;
        let items = match &value {
            c1::Value::Object(map) => match map.get("records") {
                Some(c1::Value::Array(items)) => items.clone(),
                _ => return Err(SyncError::Peer("records 配列がない".into())),
            },
            _ => return Err(SyncError::Peer("応答がオブジェクトでない".into())),
        };
        // レコードは c1 正規形で運ばれるので、再直列化 = 元の署名対象バイト列。
        Ok(items.iter().map(c1::to_canonical_bytes).collect())
    }

    fn fetch_object(&self, id: &str) -> Result<Option<Vec<u8>>, SyncError> {
        let (status, body) = self.get(&format!("/v1/objects/{id}"))?;
        match status {
            200 => Ok(Some(body)),
            404 => Ok(None),
            other => Err(SyncError::Peer(format!("objects/{id} が {other} を返した"))),
        }
    }

    fn fetch_ref(&self, name: &str) -> Result<Option<RefRecordView>, SyncError> {
        let (status, body) = self.get(&format!("/v1/refs/{name}"))?;
        match status {
            404 => Ok(None),
            200 => {
                let text = std::str::from_utf8(&body)
                    .map_err(|_| SyncError::Peer("refs 応答が UTF-8 でない".into()))?;
                let value = c1::parse(text)
                    .map_err(|e| SyncError::Peer(format!("refs 応答が JSON でない: {e}")))?;
                let map = match &value {
                    c1::Value::Object(m) => m,
                    _ => return Err(SyncError::Peer("refs 応答がオブジェクトでない".into())),
                };
                let target = match map.get("target") {
                    Some(c1::Value::Text(t)) => Some(t.clone()),
                    Some(c1::Value::Null) => None,
                    _ => return Err(SyncError::Peer("refs 応答の target が不正".into())),
                };
                let seq = match map.get("seq") {
                    Some(c1::Value::Integer(n)) => *n as u64,
                    _ => return Err(SyncError::Peer("refs 応答の seq が不正".into())),
                };
                let at = match map.get("at") {
                    Some(c1::Value::Integer(n)) => *n,
                    _ => 0,
                };
                Ok(Some(RefRecordView {
                    name: name.to_string(),
                    target,
                    seq,
                    at,
                    signer: name.split('/').next().unwrap_or("").to_string(),
                }))
            }
            other => Err(SyncError::Peer(format!("refs/{name} が {other} を返した"))),
        }
    }
}

/// POST /v1/sync のハンドラ本体(api.rs から呼ばれる)。
pub fn handle_sync_request(store: &mut Store, request: &Request) -> Response {
    let body_text = match std::str::from_utf8(&request.body) {
        Ok(t) => t,
        Err(_) => return error_json(400, "ボディが UTF-8 でない"),
    };
    let peer_address = match c1::parse(body_text) {
        Ok(c1::Value::Object(map)) => match map.get("peer") {
            Some(c1::Value::Text(t)) => t.clone(),
            _ => return error_json(400, "peer がない"),
        },
        _ => return error_json(400, "JSON オブジェクトを期待した"),
    };
    let peer = HttpPeer { address: peer_address };
    match sync_from_peer(store, &peer) {
        Ok(report) => {
            let mut map = std::collections::BTreeMap::new();
            map.insert("signers_seen".to_string(), c1::Value::Integer(report.signers_seen as i64));
            map.insert(
                "records_ingested".to_string(),
                c1::Value::Integer(report.records_ingested as i64),
            );
            map.insert(
                "records_already_known".to_string(),
                c1::Value::Integer(report.records_already_known as i64),
            );
            map.insert(
                "objects_fetched".to_string(),
                c1::Value::Integer(report.objects_fetched as i64),
            );
            map.insert(
                "objects_absent".to_string(),
                c1::Value::Integer(report.objects_absent as i64),
            );
            map.insert(
                "hash_mismatches".to_string(),
                c1::Value::Integer(report.hash_mismatches as i64),
            );
            Response::json(200, c1::to_canonical_bytes(&c1::Value::Object(map)))
        }
        // 相手に届かない・相手の応答が不正 → 502。ストア側の異常 → 500。
        Err(SyncError::Peer(message)) => error_json(502, &message),
        Err(SyncError::Store(e)) => error_json(500, &format!("{e}")),
    }
}

fn error_json(status: u16, message: &str) -> Response {
    let mut map = std::collections::BTreeMap::new();
    map.insert("error".to_string(), c1::Value::Text(message.to_string()));
    Response::json(status, c1::to_canonical_bytes(&c1::Value::Object(map)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreConfig;
    use std::path::PathBuf;

    fn temp_store(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("uniqnode-sync-test-{}-{name}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("cleanup");
        }
        let store = Store::open(StoreConfig::new(&dir)).expect("open");
        (store, dir)
    }

    #[test]
    fn local_peer_sync_replicates_and_is_idempotent() {
        let (mut origin, dir_a) = temp_store("local-origin");
        let (mut replica, dir_b) = temp_store("local-replica");
        let (leaf, _) = origin.put_object(b"\"leaf\"").expect("put");
        let edge = format!("{{\"kind\":\"edge\",\"members\":[\"{leaf}\"],\"v\":1}}");
        let (edge_id, _) = origin.put_object(edge.as_bytes()).expect("put");
        origin.set_ref("graph", Some(&edge_id)).expect("set");

        let first = sync_from_peer(&mut replica, &LocalPeer(&origin)).expect("sync");
        assert_eq!(first.records_ingested, 1);
        assert_eq!(first.objects_fetched, 2, "辺と葉の閉包が届く");
        assert!(replica.has_object(&leaf) && replica.has_object(&edge_id));

        let second = sync_from_peer(&mut replica, &LocalPeer(&origin)).expect("resync");
        assert_eq!(second.records_ingested, 0);
        assert_eq!(second.objects_fetched, 0);

        std::fs::remove_dir_all(&dir_a).expect("cleanup");
        std::fs::remove_dir_all(&dir_b).expect("cleanup");
    }

    /// 要求 ID と異なるバイト列を返す不正・故障ピア。content-addressing の検証で
    /// 捨てられ、ストアには入らない(SPEC §7.1: どのピアから受けてもハッシュ検証)。
    struct EvilPeer {
        signer: String,
        last_seq: u64,
        records: Vec<Vec<u8>>,
    }

    impl PeerSource for EvilPeer {
        fn signers(&self) -> Result<Vec<(String, u64)>, SyncError> {
            Ok(vec![(self.signer.clone(), self.last_seq)])
        }
        fn refs_since(&self, _signer: &str, _since: u64) -> Result<Vec<Vec<u8>>, SyncError> {
            Ok(self.records.clone())
        }
        fn fetch_object(&self, _id: &str) -> Result<Option<Vec<u8>>, SyncError> {
            Ok(Some(b"WRONG BYTES".to_vec()))
        }
        fn fetch_ref(&self, _name: &str) -> Result<Option<RefRecordView>, SyncError> {
            Ok(None)
        }
    }

    #[test]
    fn mislabeled_object_bytes_are_discarded() {
        let (mut origin, dir_a) = temp_store("evil-origin");
        let (mut replica, dir_b) = temp_store("evil-replica");
        let (id, _) = origin.put_object(b"\"the real object\"").expect("put");
        origin.set_ref("x", Some(&id)).expect("set");
        let origin_id = origin.node_id_hex().to_string();
        let records = origin.export_ref_records(&origin_id, 0).expect("export");

        let evil = EvilPeer { signer: origin_id, last_seq: 1, records };
        let report = sync_from_peer(&mut replica, &evil).expect("sync");
        assert_eq!(report.records_ingested, 1, "署名済みレコード自体は本物なので入る");
        assert_eq!(report.hash_mismatches, 1, "偽のバイト列は数えられて捨てられる");
        assert_eq!(report.objects_fetched, 0);
        assert!(!replica.has_object(&id), "偽の内容は保存されない");
        assert!(replica.fsck().expect("fsck").errors.is_empty());

        std::fs::remove_dir_all(&dir_a).expect("cleanup");
        std::fs::remove_dir_all(&dir_b).expect("cleanup");
    }
}
