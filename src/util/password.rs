//! 用户密码哈希（argon2id PHC + legacy 双 MD5 兼容校验/自动升级）
//!
//! 存储格式：`users.password` 列存 PHC 字符串
//! `$argon2id$v=19$m=65536,t=3,p=4$<salt>$<hash>`（盐为 argon2 随机 16 字节）。
//! 旧数据为 legacy 双 MD5（`gen_encrypted_password`：md5(md5(pwd+salt)+salt)，
//! salt 存 users.salt 列）——校验兼容，通过后登录成功路径自动升级为 argon2id。

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use crate::model::User;
use crate::storage::Storage;

/// argon2id 内存成本（KiB）：64 MiB
pub const ARGON2_M_COST: u32 = 65536;
/// argon2id 迭代次数
pub const ARGON2_T_COST: u32 = 3;
/// argon2id 并行度
pub const ARGON2_P_COST: u32 = 4;

/// 生成 argon2id PHC 哈希（`$argon2id$v=19$m=65536,t=3,p=4$salt$hash`；随机 16 字节盐）
pub fn hash_password(password: &str) -> String {
    let params =
        Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32)).expect("argon2id 参数合法");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2id 哈希失败")
        .to_string()
}

/// 校验 argon2id PHC 字符串（成本参数以存储串内嵌值为准）
pub fn verify_argon2id(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 是否为 argon2id PHC 存储
pub fn is_argon2id(stored: &str) -> bool {
    stored.starts_with("$argon2id$")
}

/// 统一密码校验（纯函数，不落库）：argon2id 优先；否则 legacy 双 MD5。
/// 返回 `(是否通过, 通过时是否需要升级为 argon2id)`。
pub fn check_password(user: &User, password: &str) -> (bool, bool) {
    if is_argon2id(&user.password) {
        (verify_argon2id(password, &user.password), false)
    } else {
        let ok = crate::util::md5::gen_encrypted_password(password, &user.salt) == user.password;
        (ok, ok)
    }
}

/// 统一密码校验 + 自动升级钩子：argon2id / legacy MD5 自动分支；
/// 校验通过且为 legacy MD5 时，用同一明文密码生成 argon2id PHC 并更新
/// `users.password`（登录成功路径逐个自动迁移；升级失败仅告警、不影响登录）。
pub async fn verify_password(storage: &Storage, user: &User, password: &str) -> bool {
    let (ok, need_upgrade) = check_password(user, password);
    if ok && need_upgrade {
        let phc = hash_password(password);
        match storage
            .upgrade_user_password_hash(&user.username, &phc)
            .await
        {
            Ok(_) => tracing::info!("用户 {} 密码已自动升级为 argon2id", user.username),
            Err(e) => tracing::warn!("用户 {} 密码自动升级失败: {e}", user.username),
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5_user(username: &str, password: &str, salt: &str) -> User {
        User {
            username: username.into(),
            password: crate::util::md5::gen_encrypted_password(password, salt),
            salt: salt.into(),
            ..Default::default()
        }
    }

    #[test]
    fn test_hash_password_phc_format() {
        let phc = hash_password("pass1234");
        // PHC 格式：$argon2id$v=19$m=65536,t=3,p=4$salt$hash
        assert!(
            phc.starts_with("$argon2id$v=19$m=65536,t=3,p=4$"),
            "PHC 前缀应含约定参数: {phc}"
        );
        assert_eq!(phc.split('$').count(), 6, "PHC 应为 5 段: {phc}");
        assert!(is_argon2id(&phc));
        // 随机盐 → 两次哈希不同
        assert_ne!(phc, hash_password("pass1234"));
    }

    #[test]
    fn test_hash_verify_roundtrip() {
        let phc = hash_password("correct-horse");
        assert!(verify_argon2id("correct-horse", &phc));
        assert!(!verify_argon2id("wrong", &phc));
        assert!(!verify_argon2id("correct-horse", "$argon2id$v=19$m=8,t=1,p=1$badsalt$badhash"));
        assert!(!verify_argon2id("correct-horse", "not-a-phc"));
    }

    #[test]
    fn test_check_password_argon2id() {
        let user = User {
            username: "carol".into(),
            password: hash_password("pass1234"),
            ..Default::default()
        };
        assert_eq!(check_password(&user, "pass1234"), (true, false));
        assert_eq!(check_password(&user, "wrong"), (false, false));
    }

    #[test]
    fn test_check_password_legacy_md5() {
        let user = md5_user("dave", "oldpass1", "legacysalt");
        // MD5 通过 → 标记需升级
        assert_eq!(check_password(&user, "oldpass1"), (true, true));
        assert_eq!(check_password(&user, "wrong"), (false, false));
        // 非 MD5 非 argon2 的杂串 → 拒绝且不升级
        let weird = User {
            username: "eve".into(),
            password: "garbage".into(),
            salt: "s".into(),
            ..Default::default()
        };
        assert_eq!(check_password(&weird, "garbage"), (false, false));
    }
}
