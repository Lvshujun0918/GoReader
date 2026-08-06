//! 常量时间字符串比较（防时序侧信道）
//!
//! 用途：secureKey 管理密码、legacy MD5 密码哈希、OPDS SHA-256 哈希等
//! 秘密值比较。普通 `==` 在首个不同字节处即返回，攻击者可借耗时差异
//! 逐字节探测秘密值；本实现无论结果如何都遍历两串中较长者（越界按 0
//! 处理），循环次数 = max(len(a), len(b))，仅按长度差 + 字节 XOR 累积判定。

/// 常量时间相等比较：
/// - 长度不同 → false（但循环仍跑满 max(len(a), len(b)) 次，不提前返回）
/// - 长度相同且逐字节全等 → true
pub fn ct_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let max = ab.len().max(bb.len());
    // 始终遍历较长者（越界按 0 处理），循环次数与内容无关
    let mut diff = 0u8;
    for i in 0..max {
        diff |= ab.get(i).copied().unwrap_or(0) ^ bb.get(i).copied().unwrap_or(0);
    }
    // 长度差异在循环后判定（内容比较不提前返回；长度本身不可避免会泄露）
    diff == 0 && ab.len() == bb.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ct_eq_equal_and_unequal() {
        assert!(ct_eq("secret-key", "secret-key"));
        assert!(!ct_eq("secret-key", "secret-keY"));
        assert!(!ct_eq("secret-key", "secret-ke0"));
    }

    #[test]
    fn test_ct_eq_empty() {
        assert!(ct_eq("", ""));
        assert!(!ct_eq("", "x"));
        assert!(!ct_eq("x", ""));
    }

    #[test]
    fn test_ct_eq_different_length() {
        assert!(!ct_eq("secret", "secret-key"));
        assert!(!ct_eq("secret-key", "secret"));
        // 前缀相同的长短串也必须不等
        assert!(!ct_eq("abc", "abcd"));
    }

    #[test]
    fn test_ct_eq_utf8() {
        // 多字节字符：按字节比较（UTF-8 编码不同字节序即不等）
        assert!(ct_eq("管理密码", "管理密码"));
        assert!(!ct_eq("管理密码", "管理密碼")); // 繁简不同
    }

    #[test]
    fn test_ct_eq_unicode_normalization_not_applied() {
        // 不做规范化（调用方负责）；仅确认不会 panic 于非法 UTF-8 边界场景
        assert!(!ct_eq("a\u{0}", "a"));
    }
}
