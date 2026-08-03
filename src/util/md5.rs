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
