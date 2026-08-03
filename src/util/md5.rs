//! MD5 工具（兼容 legacy genEncryptedPassword）

use md5::{Digest, Md5};

/// md5 十六进制
pub fn md5_encode(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// legacy 密码加密：md5(md5(password + salt) + salt)
pub fn gen_encrypted_password(password: &str, salt: &str) -> String {
    md5_encode(&format!("{}{}", md5_encode(&format!("{password}{salt}")), salt))
}

/// 章节 URL → 稳定 i64 哈希（md5 前 15 位十六进制 = 60 位，恒为正、跨进程稳定）。
/// 用作书源书正文缓存的 book_chapters.chapter_index 键（与本地书顺序索引 0..n 键域不重叠）。
pub fn chapter_url_hash(url: &str) -> i64 {
    let hex = md5_encode(url);
    i64::from_str_radix(&hex[..15], 16).unwrap_or(0)
}
