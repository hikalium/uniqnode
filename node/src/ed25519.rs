//! Ed25519 署名(RFC 8032)。DBノードの識別と全レコードの署名(SPEC §6.1)に使う。
//!
//! 自前実装の判断は操作者裁定(2026-08-15)による。個人LAN + WireGuard 上で署名は第二の
//! 防衛層であり、実装は RFC 8032 のテストベクタと openssl との相互検証で確認する。
//! 定数時間性は目標にしない(鍵は自機にのみ存在し、攻撃者がタイミングを測れる位置に
//! いない脅威モデル)。速度も目標にしない(署名対象は低頻度のレコードとメッセージ)。

use crate::sha2::sha512;

/// 素体 GF(2^255 - 19) の元。リトルエンディアンの u64 リム4本、常に完全簡約はしない
/// (to_bytes で簡約する)。
#[derive(Clone, Copy, Debug)]
pub struct FieldElement([u64; 4]);

/// p = 2^255 - 19
const P: [u64; 4] = [
    0xffff_ffff_ffff_ffed,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0x7fff_ffff_ffff_ffff,
];

/// 群位数 L = 2^252 + 27742317777372353535851937790883648493
const L: [u64; 4] = [
    0x5812_631a_5cf5_d3ed,
    0x14de_f9de_a2f7_9cd6,
    0x0000_0000_0000_0000,
    0x1000_0000_0000_0000,
];

fn greater_or_equal(a: &[u64; 4], b: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if a[i] > b[i] {
            return true;
        }
        if a[i] < b[i] {
            return false;
        }
    }
    true
}

/// a - b。借りが出たら (false, 差) ではなく wrap した値と borrow を返す。
fn sub_with_borrow(a: &[u64; 4], b: &[u64; 4]) -> ([u64; 4], bool) {
    let mut out = [0u64; 4];
    let mut borrow = 0u64;
    for i in 0..4 {
        let (d1, b1) = a[i].overflowing_sub(b[i]);
        let (d2, b2) = d1.overflowing_sub(borrow);
        out[i] = d2;
        borrow = (b1 as u64) + (b2 as u64);
    }
    (out, borrow != 0)
}

fn add_with_carry(a: &[u64; 4], b: &[u64; 4]) -> ([u64; 4], u64) {
    let mut out = [0u64; 4];
    let mut carry = 0u128;
    for i in 0..4 {
        let sum = a[i] as u128 + b[i] as u128 + carry;
        out[i] = sum as u64;
        carry = sum >> 64;
    }
    (out, carry as u64)
}

impl FieldElement {
    pub const ZERO: FieldElement = FieldElement([0, 0, 0, 0]);
    pub const ONE: FieldElement = FieldElement([1, 0, 0, 0]);

    pub fn from_u64(value: u64) -> FieldElement {
        FieldElement([value, 0, 0, 0])
    }

    /// リトルエンディアン32バイトから読む。最上位ビットは無視する(RFC 8032 の符号ビット)。
    pub fn from_bytes(bytes: &[u8; 32]) -> FieldElement {
        let mut limbs = [0u64; 4];
        for i in 0..4 {
            let mut chunk = [0u8; 8];
            chunk.copy_from_slice(&bytes[8 * i..8 * i + 8]);
            limbs[i] = u64::from_le_bytes(chunk);
        }
        limbs[3] &= 0x7fff_ffff_ffff_ffff;
        FieldElement(limbs).reduce_once()
    }

    fn reduce_once(self) -> FieldElement {
        let mut v = self.0;
        while greater_or_equal(&v, &P) {
            let (d, _) = sub_with_borrow(&v, &P);
            v = d;
        }
        FieldElement(v)
    }

    pub fn to_bytes(self) -> [u8; 32] {
        let reduced = self.reduce_once();
        let mut out = [0u8; 32];
        for i in 0..4 {
            out[8 * i..8 * i + 8].copy_from_slice(&reduced.0[i].to_le_bytes());
        }
        out
    }

    pub fn equals(self, other: FieldElement) -> bool {
        self.to_bytes() == other.to_bytes()
    }

    pub fn is_zero(self) -> bool {
        self.to_bytes() == [0u8; 32]
    }

    pub fn square(self) -> FieldElement {
        self * self
    }

    /// self^exponent(exponent はリトルエンディアンのリム4本)。
    pub fn pow(self, exponent: &[u64; 4]) -> FieldElement {
        let mut result = FieldElement::ONE;
        for bit in (0..256).rev() {
            result = result.square();
            if (exponent[bit / 64] >> (bit % 64)) & 1 == 1 {
                result = result * self;
            }
        }
        result
    }

    pub fn invert(self) -> FieldElement {
        // フェルマーの小定理: a^(p-2)
        const P_MINUS_2: [u64; 4] = [
            0xffff_ffff_ffff_ffeb,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0x7fff_ffff_ffff_ffff,
        ];
        self.pow(&P_MINUS_2)
    }
}

impl std::ops::Add for FieldElement {
    type Output = FieldElement;
    fn add(self, other: FieldElement) -> FieldElement {
        let (sum, carry) = add_with_carry(&self.0, &other.0);
        // 両辺 < p < 2^255 なので桁あふれはない。
        debug_assert_eq!(carry, 0);
        FieldElement(sum).reduce_once()
    }
}

impl std::ops::Sub for FieldElement {
    type Output = FieldElement;
    fn sub(self, other: FieldElement) -> FieldElement {
        let (diff, borrow) = sub_with_borrow(&self.0, &other.0);
        if borrow {
            let (fixed, _) = add_with_carry(&diff, &P);
            FieldElement(fixed)
        } else {
            FieldElement(diff)
        }
    }
}

impl std::ops::Neg for FieldElement {
    type Output = FieldElement;
    fn neg(self) -> FieldElement {
        FieldElement::ZERO - self
    }
}

impl std::ops::Mul for FieldElement {
    type Output = FieldElement;
    fn mul(self, other: FieldElement) -> FieldElement {
        // 4x4 リムの教科書乗算で 512bit の積を作り、2^256 ≡ 38 (mod p) で畳み込む。
        let a = &self.0;
        let b = &other.0;
        let mut wide = [0u64; 8];
        for i in 0..4 {
            let mut carry = 0u128;
            for j in 0..4 {
                let cur = wide[i + j] as u128 + (a[i] as u128) * (b[j] as u128) + carry;
                wide[i + j] = cur as u64;
                carry = cur >> 64;
            }
            wide[i + 4] = carry as u64;
        }
        reduce_wide(&wide)
    }
}

/// 512bit 値を mod p で簡約する。2^256 ≡ 38 (mod p) を2回畳む。
fn reduce_wide(wide: &[u64; 8]) -> FieldElement {
    let low = [wide[0], wide[1], wide[2], wide[3]];
    let high = [wide[4], wide[5], wide[6], wide[7]];
    // high * 38 (最大 262bit) を5リムで作る。
    let mut folded = [0u64; 5];
    {
        let mut carry = 0u128;
        for i in 0..4 {
            let cur = (high[i] as u128) * 38 + carry;
            folded[i] = cur as u64;
            carry = cur >> 64;
        }
        folded[4] = carry as u64;
    }
    let (mut acc, carry1) = add_with_carry(&low, &[folded[0], folded[1], folded[2], folded[3]]);
    let mut overflow = folded[4] + carry1;
    // overflow * 2^256 ≡ overflow * 38 を足し込む(高々2回で収束する)。
    while overflow > 0 {
        let (next, carry2) = add_with_carry(&acc, &[overflow * 38, 0, 0, 0]);
        acc = next;
        overflow = carry2;
    }
    FieldElement(acc).reduce_once()
}

/// 群位数 L を法とするスカラー(リトルエンディアン32バイト)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scalar([u64; 4]);

impl Scalar {
    pub fn from_bytes_mod_l(bytes: &[u8]) -> Scalar {
        // 任意長(実用上は64バイト)のリトルエンディアン値を、上位ビットから
        // 1ビットずつ acc = 2*acc + bit; acc mod L で畳む。速度は不要(§冒頭)。
        let mut acc = [0u64; 4];
        for byte_index in (0..bytes.len()).rev() {
            for bit_index in (0..8).rev() {
                let bit = (bytes[byte_index] >> bit_index) & 1;
                // acc = acc * 2 + bit
                let mut carry = bit as u64;
                for limb in acc.iter_mut() {
                    let doubled = ((*limb as u128) << 1) | carry as u128;
                    *limb = doubled as u64;
                    carry = (doubled >> 64) as u64;
                }
                // acc < 2L < 2^254 なので carry は常に 0。L を高々1回引けば戻る。
                if greater_or_equal(&acc, &L) {
                    let (d, _) = sub_with_borrow(&acc, &L);
                    acc = d;
                }
            }
        }
        Scalar(acc)
    }

    /// 32バイトが正準(値 < L)ならスカラーとして読む。署名検証の S に使う(RFC 8032 §5.1.7)。
    pub fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Scalar> {
        let mut limbs = [0u64; 4];
        for i in 0..4 {
            let mut chunk = [0u8; 8];
            chunk.copy_from_slice(&bytes[8 * i..8 * i + 8]);
            limbs[i] = u64::from_le_bytes(chunk);
        }
        if greater_or_equal(&limbs, &L) {
            return None;
        }
        Some(Scalar(limbs))
    }

    pub fn to_bytes(self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..4 {
            out[8 * i..8 * i + 8].copy_from_slice(&self.0[i].to_le_bytes());
        }
        out
    }

}

impl std::ops::Add for Scalar {
    type Output = Scalar;
    fn add(self, other: Scalar) -> Scalar {
        let (sum, carry) = add_with_carry(&self.0, &other.0);
        debug_assert_eq!(carry, 0); // 両辺 < L < 2^253
        let mut v = sum;
        if greater_or_equal(&v, &L) {
            let (d, _) = sub_with_borrow(&v, &L);
            v = d;
        }
        Scalar(v)
    }
}

impl std::ops::Mul for Scalar {
    type Output = Scalar;
    fn mul(self, other: Scalar) -> Scalar {
        // 二進法の乗算(res = 2*res + bit*a を mod L で)。速度は不要。
        let mut result = Scalar([0; 4]);
        for bit in (0..256).rev() {
            result = result + result;
            if (other.0[bit / 64] >> (bit % 64)) & 1 == 1 {
                result = result + self;
            }
        }
        result
    }
}

/// 拡張座標 (X, Y, Z, T)、x = X/Z, y = Y/Z, x*y = T/Z の twisted Edwards 点。
#[derive(Clone, Copy, Debug)]
pub struct EdwardsPoint {
    x: FieldElement,
    y: FieldElement,
    z: FieldElement,
    t: FieldElement,
}

fn curve_d() -> FieldElement {
    // d = -121665 / 121666 (mod p)
    (-FieldElement::from_u64(121665)) * FieldElement::from_u64(121666).invert()
}

fn sqrt_minus_one() -> FieldElement {
    // 2^((p-1)/4)
    const EXPONENT: [u64; 4] = [
        0xffff_ffff_ffff_fffb,
        0xffff_ffff_ffff_ffff,
        0xffff_ffff_ffff_ffff,
        0x1fff_ffff_ffff_ffff,
    ];
    FieldElement::from_u64(2).pow(&EXPONENT)
}

impl EdwardsPoint {
    pub fn identity() -> EdwardsPoint {
        EdwardsPoint {
            x: FieldElement::ZERO,
            y: FieldElement::ONE,
            z: FieldElement::ONE,
            t: FieldElement::ZERO,
        }
    }

    /// 基底点 B = (x, 4/5)、x は偶数の方(RFC 8032 §5.1)。
    pub fn base() -> EdwardsPoint {
        let y = FieldElement::from_u64(4) * FieldElement::from_u64(5).invert();
        let mut bytes = y.to_bytes();
        bytes[31] &= 0x7f; // 符号ビット 0 = 偶数の x
        EdwardsPoint::decompress(&bytes).expect("基底点の復元は常に成功する")
    }

    /// RFC 8032 §5.1.4 の完備な加算式(倍加にもそのまま使える)。
    pub fn add(&self, other: &EdwardsPoint) -> EdwardsPoint {
        let d2 = curve_d() + curve_d();
        let a = (self.y - self.x) * (other.y - other.x);
        let b = (self.y + self.x) * (other.y + other.x);
        let c = self.t * d2 * other.t;
        let d = (self.z + self.z) * other.z;
        let e = b - a;
        let f = d - c;
        let g = d + c;
        let h = b + a;
        EdwardsPoint {
            x: e * f,
            y: g * h,
            z: f * g,
            t: e * h,
        }
    }

    pub fn negate(&self) -> EdwardsPoint {
        EdwardsPoint {
            x: -self.x,
            y: self.y,
            z: self.z,
            t: -self.t,
        }
    }

    /// スカラー倍(リトルエンディアン32バイト、二進法)。
    pub fn scalar_mul(&self, scalar_bytes: &[u8; 32]) -> EdwardsPoint {
        let mut result = EdwardsPoint::identity();
        for bit in (0..256).rev() {
            result = result.add(&result);
            if (scalar_bytes[bit / 8] >> (bit % 8)) & 1 == 1 {
                result = result.add(self);
            }
        }
        result
    }

    pub fn compress(&self) -> [u8; 32] {
        let z_inverse = self.z.invert();
        let x = self.x * z_inverse;
        let y = self.y * z_inverse;
        let mut out = y.to_bytes();
        out[31] |= (x.to_bytes()[0] & 1) << 7;
        out
    }

    /// RFC 8032 §5.1.3 の点復元。曲線上にない・符号が矛盾する入力は None。
    pub fn decompress(bytes: &[u8; 32]) -> Option<EdwardsPoint> {
        let sign = (bytes[31] >> 7) & 1;
        let y = FieldElement::from_bytes(bytes);
        // x^2 = (y^2 - 1) / (d y^2 + 1)
        let y_squared = y.square();
        let numerator = y_squared - FieldElement::ONE;
        let denominator = curve_d() * y_squared + FieldElement::ONE;
        let x_squared = numerator * denominator.invert();
        // 平方根: x = (x^2)^((p+3)/8)、外れたら sqrt(-1) を掛ける。
        const EXPONENT: [u64; 4] = [
            0xffff_ffff_ffff_fffe,
            0xffff_ffff_ffff_ffff,
            0xffff_ffff_ffff_ffff,
            0x0fff_ffff_ffff_ffff,
        ];
        let mut x = x_squared.pow(&EXPONENT);
        if !x.square().equals(x_squared) {
            x = x * sqrt_minus_one();
        }
        if !x.square().equals(x_squared) {
            return None;
        }
        if x.is_zero() && sign == 1 {
            return None;
        }
        if (x.to_bytes()[0] & 1) != sign {
            x = -x;
        }
        Some(EdwardsPoint {
            x,
            y,
            z: FieldElement::ONE,
            t: x * y,
        })
    }
}

fn clamp(scalar: &mut [u8; 32]) {
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
}

/// 秘密鍵(32バイトのシード)から公開鍵を導出する。
pub fn public_key(secret_seed: &[u8; 32]) -> [u8; 32] {
    let digest = sha512(secret_seed);
    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&digest[..32]);
    clamp(&mut scalar);
    EdwardsPoint::base().scalar_mul(&scalar).compress()
}

/// RFC 8032 §5.1.6 の署名。
pub fn sign(secret_seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let digest = sha512(secret_seed);
    let mut secret_scalar_bytes = [0u8; 32];
    secret_scalar_bytes.copy_from_slice(&digest[..32]);
    clamp(&mut secret_scalar_bytes);
    let prefix = &digest[32..64];
    let public = public_key(secret_seed);

    let mut r_input = Vec::with_capacity(32 + message.len());
    r_input.extend_from_slice(prefix);
    r_input.extend_from_slice(message);
    let r = Scalar::from_bytes_mod_l(&sha512(&r_input));
    let r_point_bytes = EdwardsPoint::base().scalar_mul(&r.to_bytes()).compress();

    let mut k_input = Vec::with_capacity(64 + message.len());
    k_input.extend_from_slice(&r_point_bytes);
    k_input.extend_from_slice(&public);
    k_input.extend_from_slice(message);
    let k = Scalar::from_bytes_mod_l(&sha512(&k_input));

    let secret_scalar = Scalar::from_bytes_mod_l(&secret_scalar_bytes);
    let s = k * secret_scalar + r;

    let mut signature = [0u8; 64];
    signature[..32].copy_from_slice(&r_point_bytes);
    signature[32..].copy_from_slice(&s.to_bytes());
    signature
}

/// RFC 8032 §5.1.7 の検証(S の正準性検査を含む)。
pub fn verify(public: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);
    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&signature[32..]);
    let s = match Scalar::from_canonical_bytes(&s_bytes) {
        Some(s) => s,
        None => return false,
    };
    let a = match EdwardsPoint::decompress(public) {
        Some(a) => a,
        None => return false,
    };

    let mut k_input = Vec::with_capacity(64 + message.len());
    k_input.extend_from_slice(&r_bytes);
    k_input.extend_from_slice(public);
    k_input.extend_from_slice(message);
    let k = Scalar::from_bytes_mod_l(&sha512(&k_input));

    // S*B == R + k*A  ⇔  S*B + k*(-A) == R
    let s_b = EdwardsPoint::base().scalar_mul(&s.to_bytes());
    let k_neg_a = a.negate().scalar_mul(&k.to_bytes());
    s_b.add(&k_neg_a).compress() == r_bytes
}

/// /dev/urandom から秘密鍵シードを生成する。
pub fn generate_secret_seed() -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    let mut file = std::fs::File::open("/dev/urandom")?;
    let mut seed = [0u8; 32];
    file.read_exact(&mut seed)?;
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha2::{from_hex, hex};

    fn to_array_32(v: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        out.copy_from_slice(v);
        out
    }

    #[test]
    fn base_point_encoding_matches_rfc8032() {
        assert_eq!(
            hex(&EdwardsPoint::base().compress()),
            "5866666666666666666666666666666666666666666666666666666666666666"
        );
    }

    /// RFC 8032 §7.1 TEST 1〜3。
    #[test]
    fn rfc8032_test_vectors() {
        let vectors = [
            (
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "",
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
            ),
            (
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "72",
                "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
            ),
            (
                "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "af82",
                "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
            ),
        ];
        for (secret_hex, public_hex, message_hex, signature_hex) in vectors {
            let secret = to_array_32(&from_hex(secret_hex).expect("hex"));
            let message = from_hex(message_hex).expect("hex");
            assert_eq!(hex(&public_key(&secret)), public_hex);
            let signature = sign(&secret, &message);
            assert_eq!(hex(&signature), signature_hex);
            let public = to_array_32(&from_hex(public_hex).expect("hex"));
            assert!(verify(&public, &message, &signature));
        }
    }

    #[test]
    fn verify_rejects_tampering() {
        let secret = [7u8; 32];
        let public = public_key(&secret);
        let message = b"uniqnode".to_vec();
        let signature = sign(&secret, &message);
        assert!(verify(&public, &message, &signature));

        let mut bad_message = message.clone();
        bad_message[0] ^= 1;
        assert!(!verify(&public, &bad_message, &signature));

        let mut bad_signature = signature;
        bad_signature[0] ^= 1;
        assert!(!verify(&public, &message, &bad_signature));

        let other_public = public_key(&[8u8; 32]);
        assert!(!verify(&other_public, &message, &signature));
    }

    /// openssl との相互検証。鍵生成と署名を openssl で行い、決定論的署名がバイト一致する
    /// ことと、相互に検証が通ることを確かめる。openssl が無い環境では何もせず成功する。
    #[test]
    fn openssl_interop() {
        use std::process::Command;
        let probe = Command::new("openssl").arg("version").output();
        if probe.is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("uniqnode-ed25519-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let key_path = dir.join("key.pem");
        let message_path = dir.join("message.bin");
        let signature_path = dir.join("signature.bin");
        let message = b"uniqnode interop test message";
        std::fs::write(&message_path, message).expect("write message");

        let generate = Command::new("openssl")
            .args(["genpkey", "-algorithm", "ed25519", "-out"])
            .arg(&key_path)
            .output()
            .expect("openssl genpkey");
        assert!(generate.status.success());

        // PKCS8 DER の末尾32バイトが秘密鍵シード、公開鍵 DER の末尾32バイトが公開鍵。
        let private_der = Command::new("openssl")
            .args(["pkey", "-outform", "DER", "-in"])
            .arg(&key_path)
            .output()
            .expect("openssl pkey");
        assert!(private_der.status.success());
        let seed = to_array_32(&private_der.stdout[private_der.stdout.len() - 32..]);
        let public_der = Command::new("openssl")
            .args(["pkey", "-pubout", "-outform", "DER", "-in"])
            .arg(&key_path)
            .output()
            .expect("openssl pkey -pubout");
        assert!(public_der.status.success());
        let public = to_array_32(&public_der.stdout[public_der.stdout.len() - 32..]);

        assert_eq!(public_key(&seed), public, "公開鍵導出が openssl と一致する");

        let sign_output = Command::new("openssl")
            .args(["pkeyutl", "-sign", "-rawin", "-inkey"])
            .arg(&key_path)
            .arg("-in")
            .arg(&message_path)
            .arg("-out")
            .arg(&signature_path)
            .output()
            .expect("openssl pkeyutl -sign");
        assert!(sign_output.status.success());
        let openssl_signature = std::fs::read(&signature_path).expect("read signature");
        assert_eq!(
            sign(&seed, message).to_vec(),
            openssl_signature,
            "決定論的署名がバイト一致する"
        );

        // 逆方向: こちらの署名を openssl が受理する。
        std::fs::write(&signature_path, sign(&seed, message)).expect("write signature");
        let verify_output = Command::new("openssl")
            .args(["pkeyutl", "-verify", "-rawin", "-pubin", "-inkey", "/dev/stdin"])
            .arg("-in")
            .arg(&message_path)
            .arg("-sigfile")
            .arg(&signature_path)
            .output();
        // 公開鍵PEMの標準入力渡しは環境依存なので、失敗したらファイル経由で再試行する。
        let accepted = match verify_output {
            Ok(out) if out.status.success() => true,
            _ => {
                let public_pem_path = dir.join("public.pem");
                let export = Command::new("openssl")
                    .args(["pkey", "-pubout", "-in"])
                    .arg(&key_path)
                    .arg("-out")
                    .arg(&public_pem_path)
                    .output()
                    .expect("openssl pkey export");
                assert!(export.status.success());
                let retry = Command::new("openssl")
                    .args(["pkeyutl", "-verify", "-rawin", "-pubin", "-inkey"])
                    .arg(&public_pem_path)
                    .arg("-in")
                    .arg(&message_path)
                    .arg("-sigfile")
                    .arg(&signature_path)
                    .output()
                    .expect("openssl pkeyutl -verify");
                retry.status.success()
            }
        };
        assert!(accepted, "こちらの署名を openssl が受理する");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn verify_rejects_non_canonical_s() {
        let secret = [9u8; 32];
        let message = b"m";
        let mut signature = sign(&secret, message);
        // S に L を足した非正準な署名は拒否される(S + L < 2^254 なので32バイトに収まる)。
        let l_bytes: [u8; 32] = {
            let mut out = [0u8; 32];
            let l: [u64; 4] = [
                0x5812_631a_5cf5_d3ed,
                0x14de_f9de_a2f7_9cd6,
                0,
                0x1000_0000_0000_0000,
            ];
            for i in 0..4 {
                out[8 * i..8 * i + 8].copy_from_slice(&l[i].to_le_bytes());
            }
            out
        };
        let mut carry = 0u16;
        for i in 0..32 {
            let sum = signature[32 + i] as u16 + l_bytes[i] as u16 + carry;
            signature[32 + i] = sum as u8;
            carry = sum >> 8;
        }
        let public = public_key(&secret);
        assert!(!verify(&public, message, &signature));
    }
}
