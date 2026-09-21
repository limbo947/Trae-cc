//! 账号的设备标识（机器码）：以 `user_id` 为键**跨应用共享**的一套值。
//!
//! 为什么从 `account_manager.rs` 拆出来：该文件已远超单文件上限（第 6 节 800 行），而这一组规则
//! 是**自洽的一块**——「键是什么、值从哪来、怎么对齐」三件事只在账号域内使用，且全是纯函数
//! （不碰文件、不持锁），拆出后既能单独测，也便于与 `traework::device`（消费方）对照阅读。
//!
//! ## 为什么设备标识归属 `user_id` 而不是「记录 / app」
//!
//! 两个约束必须同时满足，而且它们指向同一个答案：
//!
//! - **不同真实账号 → 不同设备**：否则「一台设备绑太多账号」会触发设备级限制
//!   （「该设备绑定的账户数量已达上限」）。
//! - **同一真实账号 → 同一设备**：官方口径是「**同一台电脑同时登录 TraeCode 和 TraeWork 只算
//!   1 台设备**」。而账号库里同一个真实账号本就有**两条记录**（`app` 不同、`user_id` 相同），
//!   若各持一套标识（2026-09-21 实测：traecode 侧 `caf505a4…`、traework 侧 `65e414a1…`），
//!   服务端就把它当两台设备——这是**账号级风控**（`Login Abnormality`）的成因方向。
//!
//! 只满足第一条是不够的：「一台设备多账号」与「一个账号多设备」是两条**相反**的风控特征，
//! 只拆不合并等于拆东墙补西墙。

use super::types::Account;

/// 设备标识的归属键：`user_id`（**跨应用共享**）；为空时退化为记录 id
///
/// 为什么空 `user_id` 要退化而不是共用空键：否则所有空 `user_id` 的记录会被归成同一个键、
/// 共用一个设备标识，把互不相关的账号混成「一台设备」——比「一账号多设备」更危险的风控特征。
pub fn device_identity_key(account: &Account) -> String {
    let uid = account.user_id.trim();
    if uid.is_empty() {
        account.id.clone()
    } else {
        uid.to_string()
    }
}

/// 无既有值时按 `user_id` **稳定派生**设备标识（UUID 观感，实际是 sha256 前 16 字节）
///
/// 为什么派生而不是随机生成：保存登录态的流程是「**先保存、后登记账号**」，也就是说设备标识
/// 在「账号还没进账号库」的时刻就要用。随机值那时没有落脚点，两次调用会得到两个不同结果，
/// 会出现「传给快照的对齐值」与「最终落库的值」不一致（而且从界面和日志上都看不出来）。
/// 派生值天然幂等，不必为了记住一个随机值而提前登记账号、打破既有的正确顺序。
///
/// 为什么拼成 UUID 形状：客户端把它当不透明字符串使用，但保持 UUID 观感便于人工比对与排查
/// （与 `machine::generate_machine_guid` 的产物同形）。
pub fn derived_machine_id(user_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    // 域前缀，与签到设备号（`api/device_id.rs`）同一思路：把不同用途的派生结构性分域，
    // 避免将来某处改口径时两套值意外撞在一起
    hasher.update(b"trae-cc:device-identity:v1:");
    hasher.update(user_id.as_bytes());
    let digest = hasher.finalize();

    let mut hex = String::with_capacity(32);
    for b in digest.iter().take(16) {
        hex.push_str(&format!("{b:02x}"));
    }
    // 直接按字节改写版本位与变体位（全是 ASCII 十六进制字符，改字节不会破坏 UTF-8）
    let mut bytes = hex.into_bytes();
    bytes[12] = b'4'; // 版本位
    bytes[16] = b'8'; // 变体位（8..b 之一，固定取 8）
    let s = String::from_utf8(bytes).expect("hex 串必为 ASCII");

    format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

/// 取某 `user_id` 的设备标识（**跨应用共享**，永不为空）
///
/// 取值顺序：traecode 记录上已有的值 → 任意 app 的已有值 → `derived_machine_id` 派生。
/// 为什么 traecode 优先：那一直是被写进注册表与 `machineid` 文件的值，沿用它可以避免凭空换掉
/// 服务端已经认过的身份（换掉意味着「这台机器变成了新设备」，是风控强特征）。
pub fn shared_machine_id(accounts: &[Account], user_id: &str) -> String {
    let uid = user_id.trim();
    if uid.is_empty() {
        return derived_machine_id(uid);
    }
    let pick = |only_traecode: bool| -> Option<String> {
        accounts
            .iter()
            .filter(|a| a.user_id.trim() == uid)
            .filter(|a| !only_traecode || a.is_traecode())
            .filter_map(|a| a.machine_id.as_deref())
            .find(|m| !m.trim().is_empty())
            .map(|m| m.to_string())
    };
    pick(true)
        .or_else(|| pick(false))
        .unwrap_or_else(|| derived_machine_id(uid))
}

/// 把每条记录的设备标识统一成「其归属键的目标值」，返回是否有改动
///
/// 目标值的取值顺序与 `shared_machine_id` 完全一致（traecode 优先），因此两处不会漂移：
/// 本函数只是把同一规则批量应用到整个账号库上，供启动回填使用。因为空缺值来自稳定派生，
/// 本函数**幂等**、可反复执行。
pub fn align_device_identities(accounts: &mut [Account]) -> bool {
    use std::collections::HashMap;

    let mut resolved: HashMap<String, String> = HashMap::new();
    for a in accounts.iter().filter(|a| a.is_traecode()) {
        if let Some(mid) = a.machine_id.as_deref().filter(|m| !m.trim().is_empty()) {
            resolved.insert(device_identity_key(a), mid.to_string());
        }
    }
    for a in accounts.iter() {
        if let Some(mid) = a.machine_id.as_deref().filter(|m| !m.trim().is_empty()) {
            resolved
                .entry(device_identity_key(a))
                .or_insert_with(|| mid.to_string());
        }
    }

    let mut changed = false;
    for a in accounts.iter_mut() {
        let key = device_identity_key(a);
        if !resolved.contains_key(&key) {
            // 派生用**键**而不是 `user_id`：空 user_id 时键是记录 id，若仍按 user_id（空串）派生，
            // 两条无关记录会得到同一个值——正是要避免的「混成一台设备」
            resolved.insert(key.clone(), derived_machine_id(&key));
        }
        let target = resolved[&key].clone();
        if a.machine_id.as_deref() != Some(target.as_str()) {
            a.machine_id = Some(target);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同 `user_id` 跨应用共用同一设备标识，而不同 `user_id` 之间互不相同
    ///
    /// 这两条就是本模块存在的全部理由，必须同一个用例里一起断言——只查一条会看不出「拆东墙补西墙」
    #[test]
    fn 同user_id跨应用共用设备标识而不同user_id互不相同() {
        let mut tc_a = Account::new(
            "A".to_string(),
            String::new(),
            String::new(),
            "uid-a".to_string(),
            String::new(),
        );
        tc_a.machine_id = Some("mid-a".to_string());
        // TraeWork 侧同账号的一条记录，设备标识尚未分配
        let mut tw_a = Account::new_traework("uid-a".to_string(), "A".to_string());
        tw_a.machine_id = None;

        let mut tc_b = Account::new(
            "B".to_string(),
            String::new(),
            String::new(),
            "uid-b".to_string(),
            String::new(),
        );
        tc_b.machine_id = Some("mid-b".to_string());

        let mut accounts = vec![tc_a, tw_a, tc_b];
        assert!(align_device_identities(&mut accounts), "应产生对齐改动");

        // 同 user_id：TraeWork 记录被对齐到 TraeCode 的值（traecode 优先，避免凭空换身份）
        assert_eq!(accounts[1].machine_id.as_deref(), Some("mid-a"));
        // 不同 user_id：保持互不相同
        assert_ne!(accounts[0].machine_id, accounts[2].machine_id);

        // 幂等：再跑一次不该再产生改动（每次启动都会跑，不能反复改值）
        assert!(!align_device_identities(&mut accounts));

        assert_eq!(shared_machine_id(&accounts, "uid-a"), "mid-a");
        assert_eq!(shared_machine_id(&accounts, "uid-b"), "mid-b");
    }

    /// 无既有值时按 `user_id` 稳定派生：同一 uid 两次取值必须相同
    ///
    /// 否则「传给快照的对齐值」与「最终落库的值」会是两个不同的值，而且从界面/日志上都看不出来
    /// （保存流程是「先保存、后登记账号」，取值时账号还没进库）
    #[test]
    fn 无既有值时的设备标识按user_id稳定派生() {
        let a = shared_machine_id(&[], "uid-x");
        assert_eq!(a, shared_machine_id(&[], "uid-x"), "派生必须幂等");
        assert_ne!(a, shared_machine_id(&[], "uid-y"));
        // UUID 观感（客户端当不透明字符串用，但便于人工比对与排查）
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    /// 空 `user_id` 不得被归成同一个键：否则互不相关的记录会共用设备标识，
    /// 把多个账号混成「一台设备」——比「一账号多设备」更危险的风控特征
    #[test]
    fn 空user_id的记录不共用设备标识() {
        let mut a = Account::new(
            "A".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
        let mut b = Account::new(
            "B".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
        a.machine_id = None;
        b.machine_id = None;

        let mut accounts = vec![a, b];
        align_device_identities(&mut accounts);

        assert!(accounts[0].machine_id.is_some());
        assert_ne!(
            accounts[0].machine_id, accounts[1].machine_id,
            "空 user_id 的两条记录被归成了同一台设备"
        );
    }
}
