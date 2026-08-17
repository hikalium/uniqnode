//! グループと k-of-n 証明書(SPEC §6.4)。
//!
//! グループは各DBノードのローカル設定(data_dir/groups.json)で定義される
//! 「管理者鍵(公開鍵)の集合 + 操作別閾値」であり、group_id は人間が付けるラベルに
//! すぎない(権威は鍵集合にある)。統一検証規則: あらゆる操作物(メンバーシップ
//! 証明書・失効文)は「検証ノードの現在の鍵集合に属す相異なる鍵による有効署名の数 ≥
//! thresholds[操作種別]」で判定する。鍵を集合から外すと、その鍵の署名に依存していた
//! 操作物は閾値を割って自然に失効する(発行物の道連れは、この規則の系)。
//!
//! groups.json の形:
//! ```json
//! {"groups":[{"group_id":"family",
//!   "keys":["<管理者公開鍵hex>", …],
//!   "thresholds":{"enroll":2,"renew":1,"revoke":1,"keyset_change":2},
//!   "revocations":[<失効文>, …]}]}
//! ```
//!
//! メンバーシップ証明書(c1 JSON。sigs 以外の正規形が署名対象):
//! ```json
//! {"v":1,"type":"membership","node_id":"<hex>","group_id":"family",
//!  "issued_at":<unix秒>,"expires_at":<unix秒>,
//!  "sigs":[{"key":"<管理者公開鍵hex>","sig":"<hex>"}, …]}
//! ```
//! 失効文は type:"revocation" で expires_at を持たない(失効は取り消さない)。

use crate::c1;
use crate::ed25519;
use crate::sha2;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct GroupConfig {
    pub group_id: String,
    pub keys: Vec<String>,
    /// 操作種別 → 必要署名数。未指定の操作は 1。
    pub thresholds: BTreeMap<String, u32>,
    pub revocations: Vec<c1::Value>,
}

impl GroupConfig {
    pub fn threshold(&self, operation: &str) -> u32 {
        self.thresholds.get(operation).copied().unwrap_or(1)
    }
}

/// data_dir/groups.json を読む。無ければ空(グループ未導入)。
pub fn read_groups(data_dir: &Path) -> Vec<GroupConfig> {
    let path = data_dir.join("groups.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value = match c1::parse(&text) {
        Ok(v) => v,
        Err(e) => {
            crate::log_line!("uniqnode: groups.json が読めない(無視して空扱い): {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    let c1::Value::Object(map) = &value else { return out };
    let Some(c1::Value::Array(items)) = map.get("groups") else { return out };
    for item in items {
        let c1::Value::Object(group) = item else { continue };
        let Some(c1::Value::Text(group_id)) = group.get("group_id") else { continue };
        let keys = match group.get("keys") {
            Some(c1::Value::Array(keys)) => keys
                .iter()
                .filter_map(|k| match k {
                    c1::Value::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut thresholds = BTreeMap::new();
        if let Some(c1::Value::Object(table)) = group.get("thresholds") {
            for (operation, value) in table {
                if let c1::Value::Integer(n) = value {
                    if (1..=64).contains(n) {
                        thresholds.insert(operation.clone(), *n as u32);
                    }
                }
            }
        }
        let revocations = match group.get("revocations") {
            Some(c1::Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        out.push(GroupConfig {
            group_id: group_id.clone(),
            keys,
            thresholds,
            revocations,
        });
    }
    out
}

/// 操作物(証明書・失効文)の署名対象 = sigs を除いた正規形。
fn signing_message(document: &BTreeMap<String, c1::Value>) -> Vec<u8> {
    let mut unsigned = document.clone();
    unsigned.remove("sigs");
    c1::to_canonical_bytes(&c1::Value::Object(unsigned))
}

/// 統一検証規則: 検証ノードの現在の鍵集合に属す相異なる鍵による有効署名の数を返す。
/// 同じ鍵の重複署名は1つに数える。集合外の鍵の署名は無視する。
pub fn count_valid_signatures(document: &c1::Value, keys: &[String]) -> usize {
    let c1::Value::Object(map) = document else { return 0 };
    let Some(c1::Value::Array(sigs)) = map.get("sigs") else { return 0 };
    let message = signing_message(map);
    let mut counted: BTreeSet<String> = BTreeSet::new();
    for entry in sigs {
        let c1::Value::Object(sig_entry) = entry else { continue };
        let Some(c1::Value::Text(key_hex)) = sig_entry.get("key") else { continue };
        let Some(c1::Value::Text(sig_hex)) = sig_entry.get("sig") else { continue };
        if !keys.contains(key_hex) || counted.contains(key_hex) {
            continue;
        }
        let Some(key_bytes) = sha2::from_hex(key_hex) else { continue };
        let Some(sig_bytes) = sha2::from_hex(sig_hex) else { continue };
        if key_bytes.len() != 32 || sig_bytes.len() != 64 {
            continue;
        }
        let mut public = [0u8; 32];
        public.copy_from_slice(&key_bytes);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&sig_bytes);
        if ed25519::verify(&public, &message, &signature) {
            counted.insert(key_hex.clone());
        }
    }
    counted.len()
}

fn text_of(map: &BTreeMap<String, c1::Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(c1::Value::Text(t)) => Some(t.clone()),
        _ => None,
    }
}

fn integer_of(map: &BTreeMap<String, c1::Value>, key: &str) -> Option<i64> {
    match map.get(key) {
        Some(c1::Value::Integer(n)) => Some(*n),
        _ => None,
    }
}

/// node_id がグループから失効させられているか(失効文は thresholds[revoke] で判定)。
fn is_revoked(node_id: &str, group: &GroupConfig) -> bool {
    let required = group.threshold("revoke") as usize;
    group.revocations.iter().any(|statement| {
        let c1::Value::Object(map) = statement else { return false };
        text_of(map, "type").as_deref() == Some("revocation")
            && text_of(map, "node_id").as_deref() == Some(node_id)
            && text_of(map, "group_id").as_deref() == Some(&group.group_id)
            && count_valid_signatures(statement, &group.keys) >= required
    })
}

/// メンバーシップ証明書の検証(統一検証規則 + 有効期間 + 失効)。
/// Ok(グループID) か、拒否理由の文字列を返す。
pub fn verify_membership(
    certificate: &c1::Value,
    groups: &[GroupConfig],
    now_unix: i64,
) -> Result<String, String> {
    let c1::Value::Object(map) = certificate else {
        return Err("証明書がオブジェクトでない".into());
    };
    if text_of(map, "type").as_deref() != Some("membership") {
        return Err("type が membership でない".into());
    }
    let node_id = text_of(map, "node_id").ok_or("node_id がない")?;
    let group_id = text_of(map, "group_id").ok_or("group_id がない")?;
    let issued_at = integer_of(map, "issued_at").ok_or("issued_at がない")?;
    let expires_at = integer_of(map, "expires_at").ok_or("expires_at がない")?;
    if now_unix < issued_at {
        return Err("まだ有効期間前".into());
    }
    if now_unix >= expires_at {
        return Err("有効期限切れ".into());
    }
    // group_id はラベルにすぎない: 検証ノードのローカル設定でその名を持つグループの
    // 鍵集合に対してのみ意味を持つ。
    let group = groups
        .iter()
        .find(|g| g.group_id == group_id)
        .ok_or_else(|| format!("グループ {group_id} は信頼設定にない"))?;
    let required = group.threshold("enroll") as usize;
    let valid = count_valid_signatures(certificate, &group.keys);
    if valid < required {
        return Err(format!(
            "署名不足: 有効 {valid} / 必要 {required}(enroll, グループ {group_id})"
        ));
    }
    if is_revoked(&node_id, group) {
        return Err(format!("node {node_id} は失効済み"));
    }
    Ok(group_id)
}

// ---- 発行(セレモニー)側 ----

/// 証明書・失効文の本体を作る(sigs なし)。
pub fn make_statement(
    statement_type: &str,
    node_id: &str,
    group_id: &str,
    issued_at: i64,
    expires_at: Option<i64>,
) -> c1::Value {
    let mut map = BTreeMap::new();
    map.insert("v".to_string(), c1::Value::Integer(1));
    map.insert("type".to_string(), c1::Value::Text(statement_type.to_string()));
    map.insert("node_id".to_string(), c1::Value::Text(node_id.to_string()));
    map.insert("group_id".to_string(), c1::Value::Text(group_id.to_string()));
    match statement_type {
        "membership" => {
            map.insert("issued_at".to_string(), c1::Value::Integer(issued_at));
            map.insert(
                "expires_at".to_string(),
                c1::Value::Integer(expires_at.unwrap_or(issued_at)),
            );
        }
        _ => {
            map.insert("at".to_string(), c1::Value::Integer(issued_at));
        }
    }
    map.insert("sigs".to_string(), c1::Value::Array(Vec::new()));
    c1::Value::Object(map)
}

/// 管理者鍵で署名を1つ追記する(k-of-n の署名収集: 各管理者が順に呼ぶ)。
/// 同じ鍵の署名が既にあれば何もしない(べき等)。
pub fn add_signature(statement: &mut c1::Value, admin_seed: &[u8; 32]) {
    let c1::Value::Object(map) = statement else { return };
    let message = signing_message(map);
    let public_hex = sha2::hex(&ed25519::public_key(admin_seed));
    let signature_hex = sha2::hex(&ed25519::sign(admin_seed, &message));
    let sigs = match map.get_mut("sigs") {
        Some(c1::Value::Array(sigs)) => sigs,
        _ => {
            map.insert("sigs".to_string(), c1::Value::Array(Vec::new()));
            match map.get_mut("sigs") {
                Some(c1::Value::Array(sigs)) => sigs,
                _ => return,
            }
        }
    };
    let already = sigs.iter().any(|entry| {
        matches!(entry, c1::Value::Object(e)
            if text_of(e, "key").as_deref() == Some(public_hex.as_str()))
    });
    if already {
        return;
    }
    let mut entry = BTreeMap::new();
    entry.insert("key".to_string(), c1::Value::Text(public_hex));
    entry.insert("sig".to_string(), c1::Value::Text(signature_hex));
    sigs.push(c1::Value::Object(entry));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin(seed_byte: u8) -> ([u8; 32], String) {
        let seed = [seed_byte; 32];
        (seed, sha2::hex(&ed25519::public_key(&seed)))
    }

    fn group(keys: &[&String], enroll: u32, revocations: Vec<c1::Value>) -> GroupConfig {
        let mut thresholds = BTreeMap::new();
        thresholds.insert("enroll".to_string(), enroll);
        thresholds.insert("revoke".to_string(), 1);
        GroupConfig {
            group_id: "family".to_string(),
            keys: keys.iter().map(|k| (*k).clone()).collect(),
            thresholds,
            revocations,
        }
    }

    fn certificate(signers: &[&[u8; 32]], now: i64) -> c1::Value {
        let mut statement =
            make_statement("membership", "node-x", "family", now - 10, Some(now + 1000));
        for seed in signers {
            add_signature(&mut statement, seed);
        }
        statement
    }

    /// k-of-n の表: 閾値未満は拒否、以上は受理。重複署名は1つに数える。
    #[test]
    fn k_of_n_thresholds_are_enforced() {
        let now = 1_000_000;
        let (seed1, key1) = admin(1);
        let (seed2, key2) = admin(2);
        let (seed3, _key3) = admin(3);
        let groups = vec![group(&[&key1, &key2], 2, Vec::new())];

        // 1署名では足りない。
        let one = certificate(&[&seed1], now);
        assert!(verify_membership(&one, &groups, now).is_err());

        // 2署名で受理。
        let two = certificate(&[&seed1, &seed2], now);
        assert_eq!(verify_membership(&two, &groups, now), Ok("family".to_string()));

        // 集合外の鍵の署名は数えない。
        let outsider = certificate(&[&seed1, &seed3], now);
        assert!(verify_membership(&outsider, &groups, now).is_err());

        // 同じ鍵を2回署名しても1つ(add_signature はべき等だが、手で重複させても
        // count_valid_signatures が重複を数えない)。
        let mut duplicated = certificate(&[&seed1], now);
        if let c1::Value::Object(map) = &mut duplicated {
            if let Some(c1::Value::Array(sigs)) = map.get("sigs") {
                let copy = sigs[0].clone();
                if let Some(c1::Value::Array(sigs)) = map.get_mut("sigs") {
                    sigs.push(copy);
                }
            }
        }
        assert_eq!(count_valid_signatures(&duplicated, &[key1.clone(), key2.clone()]), 1);
    }

    /// 鍵集合から署名鍵を外すと、既発行の証明書は閾値を割って失効する
    /// (統一検証規則の系。特別な失効処理は要らない)。
    #[test]
    fn removing_a_key_invalidates_its_certificates() {
        let now = 1_000_000;
        let (seed1, key1) = admin(1);
        let (seed2, key2) = admin(2);
        let cert = certificate(&[&seed1, &seed2], now);

        let before = vec![group(&[&key1, &key2], 2, Vec::new())];
        assert!(verify_membership(&cert, &before, now).is_ok());

        // key2 を外す → 有効署名 1 < 2。
        let after = vec![group(&[&key1], 2, Vec::new())];
        let rejected = verify_membership(&cert, &after, now).expect_err("失効するはず");
        assert!(rejected.contains("署名不足"), "{rejected}");
    }

    /// 有効期間と改竄。
    #[test]
    fn validity_window_and_tampering_are_enforced() {
        let now = 1_000_000;
        let (seed1, key1) = admin(1);
        let groups = vec![group(&[&key1], 1, Vec::new())];

        let cert = certificate(&[&seed1], now);
        assert!(verify_membership(&cert, &groups, now + 2000).is_err(), "期限切れ");
        assert!(verify_membership(&cert, &groups, now - 100).is_err(), "期間前");

        // 本文の改竄(node_id 差し替え)で署名が無効になる。
        let mut tampered = cert.clone();
        if let c1::Value::Object(map) = &mut tampered {
            map.insert("node_id".to_string(), c1::Value::Text("node-evil".to_string()));
        }
        assert!(verify_membership(&tampered, &groups, now).is_err());
    }

    /// 失効文: thresholds[revoke] を満たす署名があれば、その node の証明書は拒否される。
    /// 非メンバー鍵だけの失効文は効かない。
    #[test]
    fn revocation_statements_are_honored_by_threshold() {
        let now = 1_000_000;
        let (seed1, key1) = admin(1);
        let (seed2, key2) = admin(2);
        let (seed9, _key9) = admin(9);

        let mut revocation = make_statement("revocation", "node-x", "family", now, None);
        add_signature(&mut revocation, &seed1); // revoke 閾値は 1
        let groups = vec![group(&[&key1, &key2], 2, vec![revocation])];
        let cert = certificate(&[&seed1, &seed2], now);
        let rejected = verify_membership(&cert, &groups, now).expect_err("失効で拒否");
        assert!(rejected.contains("失効済み"), "{rejected}");

        // 非メンバー鍵だけの失効文は数えられない。
        let mut fake_revocation = make_statement("revocation", "node-x", "family", now, None);
        add_signature(&mut fake_revocation, &seed9);
        let groups = vec![group(&[&key1, &key2], 2, vec![fake_revocation])];
        assert!(verify_membership(&cert, &groups, now).is_ok(), "非メンバーの失効は無効");
    }

    /// groups.json の読み書き(閾値の既定は 1)。
    #[test]
    fn groups_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("uniqnode-groups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(read_groups(&dir).is_empty());
        std::fs::write(
            dir.join("groups.json"),
            "{\"groups\":[{\"group_id\":\"g\",\"keys\":[\"aa\",\"bb\"],\
             \"thresholds\":{\"enroll\":2}}]}",
        )
        .expect("write");
        let groups = read_groups(&dir);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].keys.len(), 2);
        assert_eq!(groups[0].threshold("enroll"), 2);
        assert_eq!(groups[0].threshold("revoke"), 1, "未指定の操作は 1");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
