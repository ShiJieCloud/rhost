//! 配置文件密码加密封装（设计文档 §5.1）。
//!
//! 方案：PBKDF2-SHA256（210000 轮）从用户密码派生 32B 密钥，AES-256-GCM
//! 加密整份标准导出 Schema；envelope 携带 salt/iv/算法标识，加密封装不改
//! Schema 自身的 `version`。
//!
//! 安全红线：
//! - 密码只以前端 IPC 字符串参数存在，本模块只接收 `&str`，不落盘、不进日志；
//! - 派生密钥用 [`Zeroizing`] 包裹，函数返回即清零；调用方的 `Zeroizing<String>`
//!   在 IPC 命令结束时清零；
//! - 面向用户：GCM 认证失败（密码错误 / 密文篡改）统一提示「密码错误或文件损坏」，
//!   不区分原因，避免成为预言机；
//! - 本地日志：按 [`DecryptFail`] 细分 `bad_password`（结构合法但 GCM 认证失败）
//!   与 `corrupted`（envelope 结构/编码非法），仅写入本机 applog，不返回给前端。

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use pbkdf2::pbkdf2_hmac_array;
use serde_json::{Value, json};
use sha2::Sha256;
use std::fmt;
use zeroize::Zeroizing;

/// PBKDF2 迭代轮数（OWASP 2023 推荐下限）
pub const KDF_ITERS: u32 = 210_000;

/// 兼容导入时允许的最大迭代轮数（防止恶意文件把轮数改成天文数字造成 DoS）
const MAX_KDF_ITERS: u32 = 1_000_000;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

const ENVELOPE_VERSION: u32 = 1;
const CIPHER_NAME: &str = "aes-256-gcm";
const KDF_NAME: &str = "pbkdf2-sha256";

/// 解密失败类别（仅用于本地 applog 细分，不随 IPC 错误返回前端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecryptFail {
    /// envelope 结构合法但 GCM 认证未通过：密码错误（密文被篡改同样表现为此，
    /// 二者在密码学上无法区分）。
    BadPassword,
    /// envelope 无法解析、字段/算法标识/长度非法或解密产物非法：文件损坏或
    /// 不是本应用导出的加密文件。
    Corrupted,
}

impl DecryptFail {
    /// applog kv 中使用的机器可读原因码
    pub fn code(self) -> &'static str {
        match self {
            Self::BadPassword => "bad_password",
            Self::Corrupted => "corrupted",
        }
    }
}

/// 解密错误：携带失败类别与面向用户的文案；日志文案由 [`Self::log_message`] 另出。
#[derive(Debug)]
pub struct DecryptError {
    pub kind: DecryptFail,
    user_message: String,
}

impl DecryptError {
    fn corrupted(message: impl Into<String>) -> Self {
        Self {
            kind: DecryptFail::Corrupted,
            user_message: message.into(),
        }
    }

    fn bad_password() -> Self {
        Self {
            kind: DecryptFail::BadPassword,
            user_message: "密码错误或文件损坏".to_string(),
        }
    }

    /// 面向用户的文案（BadPassword 恒为统一文案，不泄露区分信息）
    pub fn user_message(&self) -> &str {
        &self.user_message
    }

    /// 本地日志文案：按类别细分，Corrupted 附带具体结构原因
    pub fn log_message(&self) -> String {
        match self.kind {
            DecryptFail::BadPassword => {
                "密码错误（GCM 认证失败；若确认密码无误则为文件损坏）".to_string()
            }
            DecryptFail::Corrupted => format!("文件损坏或格式无效：{}", self.user_message),
        }
    }
}

impl fmt::Display for DecryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_message)
    }
}

impl std::error::Error for DecryptError {}

/// envelope 解析阶段的纯字符串错误（base64/JSON/缺字段）一律归类为 Corrupted
impl From<String> for DecryptError {
    fn from(message: String) -> Self {
        Self::corrupted(message)
    }
}

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s).map_err(|e| format!("base64 解码失败: {e}"))
}

/// 派生 32B 密钥（返回值 Zeroizing，drop 时清零）
fn derive_key(password: &str, salt: &[u8], iters: u32) -> Zeroizing<[u8; KEY_LEN]> {
    Zeroizing::new(pbkdf2_hmac_array::<Sha256, KEY_LEN>(
        password.as_bytes(),
        salt,
        iters,
    ))
}

/// 组装加密 envelope：明文为标准导出 Schema JSON，输出 envelope JSON 字符串。
///
/// envelope 形态见 §5.1：
/// ```json
/// {"version":1,"meta":{"encrypted":true,"cipher":"aes-256-gcm",
/// "kdf":"pbkdf2-sha256","kdf_iters":210000,"salt":"...","iv":"..."},
/// "ciphertext":"..."}
/// ```
pub fn seal_envelope(plain_json: &str, password: &str) -> Result<String, String> {
    if password.is_empty() {
        return Err("加密密码不能为空".to_string());
    }

    // salt 可公开（无需保密），nonce 仅要求 GCM 同密钥下不重复，二者均走 OS CSPRNG
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let key = derive_key(password, &salt, KDF_ITERS);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_slice()));
    // 输出 = ciphertext || 16B GCM tag
    let ciphertext = cipher
        .encrypt(&nonce, plain_json.as_bytes())
        .map_err(|e| format!("加密失败: {e}"))?;

    let envelope = json!({
        "version": ENVELOPE_VERSION,
        "meta": {
            "encrypted": true,
            "cipher": CIPHER_NAME,
            "kdf": KDF_NAME,
            "kdf_iters": KDF_ITERS,
            "salt": B64.encode(salt),
            "iv": B64.encode(nonce.as_slice()),
        },
        "ciphertext": B64.encode(ciphertext),
    });
    serde_json::to_string(&envelope).map_err(|e| format!("envelope 序列化失败: {e}"))
}

/// 解析并解密 envelope，返回标准导出 Schema JSON 文本。
///
/// 返回的 [`DecryptError`] 同时携带：
/// - 面向用户的统一/具体文案（[`DecryptError::user_message`]，IPC 直接透传）；
/// - 失败类别（[`DecryptFail`]），调用方仅用于本地日志细分，不返回前端。
pub fn open_envelope(envelope_json: &str, password: &str) -> Result<String, DecryptError> {
    let doc: Value =
        serde_json::from_str(envelope_json).map_err(|e| format!("加密文件格式错误: {e}"))?;
    let meta = doc
        .get("meta")
        .and_then(Value::as_object)
        .ok_or_else(|| "加密文件格式错误：缺少 meta 节".to_string())?;

    if meta.get("encrypted").and_then(Value::as_bool) != Some(true) {
        return Err(DecryptError::corrupted("文件未加密"));
    }

    let cipher_name = meta_str(meta, "cipher")?;
    if cipher_name != CIPHER_NAME {
        return Err(DecryptError::corrupted(format!(
            "不支持的加密算法: {cipher_name}"
        )));
    }
    let kdf_name = meta_str(meta, "kdf")?;
    if kdf_name != KDF_NAME {
        return Err(DecryptError::corrupted(format!(
            "不支持的密钥派生算法: {kdf_name}"
        )));
    }
    let iters = meta
        .get("kdf_iters")
        .and_then(Value::as_u64)
        .filter(|n| (1..=u64::from(MAX_KDF_ITERS)).contains(n))
        .ok_or_else(|| "加密文件格式错误：meta.kdf_iters 缺失或不受支持".to_string())?
        as u32;

    let salt = b64_decode(meta_str(meta, "salt")?)?;
    if salt.len() != SALT_LEN {
        return Err(DecryptError::corrupted("加密文件格式错误：salt 长度非法"));
    }
    let iv = b64_decode(meta_str(meta, "iv")?)?;
    if iv.len() != NONCE_LEN {
        return Err(DecryptError::corrupted("加密文件格式错误：iv 长度非法"));
    }
    let ciphertext = b64_decode(
        doc.get("ciphertext")
            .and_then(Value::as_str)
            .ok_or_else(|| "加密文件格式错误：ciphertext 缺失或非法".to_string())?,
    )?;

    let key = derive_key(password, &salt, iters);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_slice()));
    let plain = cipher
        .decrypt(Nonce::from_slice(&iv), ciphertext.as_slice())
        .map_err(|_| DecryptError::bad_password())?;

    String::from_utf8(plain)
        .map_err(|e| DecryptError::corrupted(format!("解密内容不是合法 UTF-8 文本: {e}")))
}

fn meta_str<'a>(meta: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str, String> {
    meta.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("加密文件格式错误：meta.{key} 缺失或非法"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = r#"{"version":1,"meta":{"scope":"full"},"data":{"a":1}}"#;

    #[test]
    fn seal_then_open_roundtrips() {
        let env = seal_envelope(PLAIN, "correct horse").unwrap();
        // envelope 不含明文任何片段
        assert!(!env.contains("correct horse"));
        assert!(!env.contains(r#""scope""#));
        let v: Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["meta"]["encrypted"], true);
        assert_eq!(v["meta"]["cipher"], CIPHER_NAME);
        assert_eq!(v["meta"]["kdf"], KDF_NAME);
        assert_eq!(v["meta"]["kdf_iters"], KDF_ITERS);
        assert!(v["meta"]["salt"].is_string());
        assert!(v["meta"]["iv"].is_string());
        assert!(v["ciphertext"].is_string());

        let out = open_envelope(&env, "correct horse").unwrap();
        assert_eq!(out, PLAIN);
    }

    #[test]
    fn wrong_password_is_rejected_with_uniform_message() {
        let env = seal_envelope(PLAIN, "pw-a").unwrap();
        let err = open_envelope(&env, "pw-b").unwrap_err();
        // 面向用户：统一文案
        assert_eq!(err.to_string(), "密码错误或文件损坏");
        // 本地日志：归类密码错误，文案可直白细分
        assert_eq!(err.kind, DecryptFail::BadPassword);
        assert_eq!(err.kind.code(), "bad_password");
        assert!(err.log_message().contains("密码错误"));
    }

    #[test]
    fn empty_password_rejected_on_seal_only() {
        // seal 拒绝空密码；open 不特判空密码（避免泄露密码规则，统一走 GCM 校验）
        assert!(seal_envelope(PLAIN, "").is_err());
        let env = seal_envelope(PLAIN, "x").unwrap();
        let err = open_envelope(&env, "").unwrap_err();
        assert_eq!(err.to_string(), "密码错误或文件损坏");
        assert_eq!(err.kind, DecryptFail::BadPassword);
    }

    #[test]
    fn tampered_ciphertext_or_iv_is_rejected() {
        let env = seal_envelope(PLAIN, "pw").unwrap();
        let mut doc: Value = serde_json::from_str(&env).unwrap();

        // 翻转密文最后一个 base64 字符：结构合法、GCM 认证失败 → bad_password
        let ct = doc["ciphertext"].as_str().unwrap();
        let (head, last) = ct.split_at(ct.len() - 1);
        let flipped = format!("{head}{}", if last == "A" { "B" } else { "A" });
        doc["ciphertext"] = Value::String(flipped);
        let err = open_envelope(&serde_json::to_string(&doc).unwrap(), "pw").unwrap_err();
        assert_eq!(err.to_string(), "密码错误或文件损坏");
        assert_eq!(err.kind, DecryptFail::BadPassword);

        // iv 被替换：同上，密码学上无法与密码错误区分
        let mut doc2: Value = serde_json::from_str(&env).unwrap();
        doc2["meta"]["iv"] = json!(B64.encode([0u8; NONCE_LEN]));
        let err = open_envelope(&serde_json::to_string(&doc2).unwrap(), "pw").unwrap_err();
        assert_eq!(err.to_string(), "密码错误或文件损坏");
        assert_eq!(err.kind, DecryptFail::BadPassword);
    }

    #[test]
    fn malformed_envelope_fields_are_rejected() {
        let env = seal_envelope(PLAIN, "pw").unwrap();
        let mut doc: Value = serde_json::from_str(&env).unwrap();

        doc["meta"]["cipher"] = json!("aes-128-ecb");
        let err = open_envelope(&serde_json::to_string(&doc).unwrap(), "pw").unwrap_err();
        assert_eq!(err.kind, DecryptFail::Corrupted);

        let mut doc2: Value = serde_json::from_str(&env).unwrap();
        doc2["meta"]["kdf"] = json!("pbkdf2-md5");
        assert_eq!(
            open_envelope(&serde_json::to_string(&doc2).unwrap(), "pw")
                .unwrap_err()
                .kind,
            DecryptFail::Corrupted
        );

        let mut doc3: Value = serde_json::from_str(&env).unwrap();
        doc3["meta"]["kdf_iters"] = json!(u64::from(MAX_KDF_ITERS) + 1);
        assert_eq!(
            open_envelope(&serde_json::to_string(&doc3).unwrap(), "pw")
                .unwrap_err()
                .kind,
            DecryptFail::Corrupted
        );

        let mut doc4: Value = serde_json::from_str(&env).unwrap();
        doc4["meta"]["salt"] = json!(B64.encode([0u8; 8]));
        let err = open_envelope(&serde_json::to_string(&doc4).unwrap(), "pw").unwrap_err();
        assert_eq!(err.kind, DecryptFail::Corrupted);
        // 结构错误：用户可见具体原因，日志带「文件损坏」前缀
        assert!(err.to_string().contains("salt 长度非法"));
        assert!(err.log_message().starts_with("文件损坏或格式无效："));
        assert_eq!(err.kind.code(), "corrupted");

        let mut doc5: Value = serde_json::from_str(&env).unwrap();
        doc5["ciphertext"] = Value::Null;
        assert_eq!(
            open_envelope(&serde_json::to_string(&doc5).unwrap(), "pw")
                .unwrap_err()
                .kind,
            DecryptFail::Corrupted
        );

        // 不是 envelope（encrypted=false）
        let err = open_envelope(r#"{"meta":{"encrypted":false}}"#, "pw").unwrap_err();
        assert_eq!(err.to_string(), "文件未加密");
        assert_eq!(err.kind, DecryptFail::Corrupted);
    }

    #[test]
    fn each_seal_uses_fresh_salt_and_iv() {
        let a = seal_envelope(PLAIN, "pw").unwrap();
        let b = seal_envelope(PLAIN, "pw").unwrap();
        assert_ne!(a, b); // salt/nonce 随机，同明文同密码密文不同
    }
}
