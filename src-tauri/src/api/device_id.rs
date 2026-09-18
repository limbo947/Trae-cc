//! 签到设备标识（`X-Device-Id`）
//!
//! 为什么与 `Account.machine_id` 分开：`machine_id` 同时被用作 Trae 的 `machineid` 文件值与
//! 注册表 `MachineGuid`，是 IDE 的机器身份。若签到直接复用它，「换个签到设备号」必然连带
//! 改写 IDE 机器身份，无法单独旋转。独立字段后两者彻底解耦。
//!
//! 为什么派生一次就落盘：派生算法若将来变更，未落盘的账号会被静默换号，服务端视为新设备。
//! 因此 `Account.device_id` 一经写入只读，`resolve_device_id` 仅在空值时按种子派生兜底
//! （覆盖「升级后首次启动、回填尚未落盘」的窗口期）。

use sha2::{Digest, Sha256};

use crate::account::Account;

/// 设备号位数（格式收敛点：将来若要改形态只动这一处）
pub const DEVICE_ID_LEN: usize = 16;

/// 派生域前缀：带版本号，改算法时递增可让新旧设备号不互相污染
const DERIVE_DOMAIN: &str = "trae-cc:device-id:v1:";

/// 16 位整数区间的下界与跨度（映射到 `[1e15, 1e16)`，保证恒为 16 位且首位非零）
const DEVICE_ID_MIN: u64 = 1_000_000_000_000_000;
const DEVICE_ID_SPAN: u64 = 9_000_000_000_000_000;

/// 由种子派生稳定的 16 位设备号
///
/// 用 SHA-256 而非 `rand(seed)`：rand 的算法与版本演进会改变输出，sha256 恒等。
/// 映射到 16 位整数区间而非 `% 1e16`：取模会产出不足 16 位的值、需补前导零，
/// 而任何按数字解析的下游都会丢掉前导零。
pub fn derive_device_id(seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DERIVE_DOMAIN.as_bytes());
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let raw = u64::from_be_bytes(bytes);

    let derived = (DEVICE_ID_MIN + raw % DEVICE_ID_SPAN).to_string();
    debug_assert_eq!(derived.len(), DEVICE_ID_LEN, "派生设备号必须恒为 16 位");
    derived
}

/// 解析账号当前应使用的设备号：落盘值优先 → 按 `user_id` 派生 → 按内部 `id` 派生
///
/// 三层回退只为兼容「升级后首次启动、回填尚未落盘」的窗口期。
/// **不能用固定种子兜底**：多个没有 `user_id` 的账号会撞成同一设备号，互相触发 9095。
pub fn resolve_device_id(account: &Account) -> String {
    if let Some(id) = account.device_id.as_deref().filter(|v| !v.trim().is_empty()) {
        return id.to_string();
    }
    derive_device_id(derive_seed(account))
}

/// 生成一个随机设备号（重置用，与派生号同区间）
pub fn random_device_id() -> String {
    use rand::Rng;
    let value = rand::thread_rng().gen_range(DEVICE_ID_MIN..DEVICE_ID_MIN + DEVICE_ID_SPAN);
    value.to_string()
}

/// 日志脱敏：设备号是准指纹，只保留前 4 位与后 2 位
pub fn mask_device_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 6 {
        return "***".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{}…{}", head, tail)
}

/// 派生种子：优先 `user_id`（跨机器稳定，与账号身份绑定），缺失时退回内部 `id`
fn derive_seed(account: &Account) -> &str {
    if account.user_id.trim().is_empty() {
        &account.id
    } else {
        &account.user_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 恒为 16 位纯数字（首位非零），且同一账号跨次调用稳定
    #[test]
    fn derived_id_is_stable_and_16_digits() {
        let a = derive_device_id("user-abc");
        assert_eq!(a.len(), DEVICE_ID_LEN);
        assert!(a.chars().all(|c| c.is_ascii_digit()));
        assert!(!a.starts_with('0'));
        assert_eq!(a, derive_device_id("user-abc"));
    }

    /// 不同种子必须得到不同设备号，否则多账号会互相触发 9095
    #[test]
    fn different_seeds_diverge() {
        assert_ne!(derive_device_id("user-a"), derive_device_id("user-b"));
        assert_ne!(derive_device_id(""), derive_device_id("user-a"));
    }

    /// 落盘值优先：重置后的设备号必须被沿用，不能被重新派生覆盖
    #[test]
    fn stored_value_wins_over_derivation() {
        let mut account = test_account();
        account.device_id = Some(random_device_id());
        let stored = account.device_id.clone().unwrap();
        assert_eq!(resolve_device_id(&account), stored);
    }

    /// 窗口期回退：未落盘时按 user_id 派生，`user_id` 为空则按内部 id 派生（不得用固定种子）
    #[test]
    fn resolve_falls_back_without_fixed_seed() {
        let mut account = test_account();
        account.device_id = None;
        assert_eq!(resolve_device_id(&account), derive_device_id("u1"));

        account.user_id = String::new();
        assert_eq!(resolve_device_id(&account), derive_device_id(&account.id));

        let mut other = test_account();
        other.device_id = None;
        other.user_id = String::new();
        assert_ne!(resolve_device_id(&account), resolve_device_id(&other));
    }

    /// 日志脱敏：只保留前 4 位与后 2 位
    #[test]
    fn masking_hides_middle() {
        assert_eq!(mask_device_id("1234567890123456"), "1234…56");
        assert_eq!(mask_device_id("123"), "***");
    }

    fn test_account() -> Account {
        Account::new(
            "测试".to_string(),
            "t@example.com".to_string(),
            String::new(),
            "u1".to_string(),
            "t1".to_string(),
        )
    }
}
