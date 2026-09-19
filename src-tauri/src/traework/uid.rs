//! TraeWork 账号 uid 推导（权威来源 + 兜底证据链）。
//!
//! **权威来源：解密 `iCubeAuthInfo://icube.cloudide` 后的 `userId`。**
//! 这是登录态本体——客户端自己就是读它判断"当前是谁"。2026-09-19 实测确认该字段是该账号的
//! 真实 uid，且对"退出登录→换号登录"能立即反映。
//!
//! **`icube_gtm.users` 只是兜底，不可作为主判据。** 实测（2026-09-19）踩过的坑：
//! 用户先登录账号 A 保存、再换账号 B 保存，两次保存都被解析成同一个 uid，第二次把第一次的
//! 快照覆盖了（`.bak` 里才发现是另一个账号的 `userId`）。原因是 `icube_gtm.users` 对切换后的
//! 新账号**存在滞后**，而 `iCubeAuthInfo://usertag` 解密后是**以 uid 为键的累积表**
//! （新旧账号都在里面），两者都不是"当前登录者"的信号。
//!
//! **明确不可用的线索**：`iCubeAuthInfo://icube-dc:<id>` 里的 `<id>` 是 OAuth 设备凭证 id
//! （值为 EC P-256 设备私钥 PEM），不是账号 uid。
//!
//! **红线：推导不出就报错拒绝，绝不猜。** 猜错的后果是覆盖别人的快照／以错误身份启动客户端。

use std::path::Path;

use super::profile::Ctx;

/// `storage.json` 中承载登录态的键（tc 密文，旧版客户端可能为明文 JSON）
const AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";

/// uid 推导结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct UidEvidence {
    /// 判定结果；不可信时为 None
    pub uid: Option<String>,
    /// 是否达到可用置信度
    pub confident: bool,
    /// 全部候选（供人工确认）
    pub candidates: Vec<String>,
    /// 判定依据说明（面向用户，解释"凭什么认为是这个账号"）
    pub reason: String,
    /// 证据来源标识：auth_blob（权威）/ auth_plaintext / users_map（兜底）
    pub source: String,
}

impl UidEvidence {
    fn failed(reason: String) -> Self {
        Self {
            uid: None,
            confident: false,
            candidates: Vec::new(),
            reason,
            source: "none".to_string(),
        }
    }
}

/// 单个账号 id 的证据
#[derive(Debug, Clone)]
pub struct AccountIdEvidence {
    pub uid: String,
    /// auth_blob / auth_plaintext / users_map
    pub source: &'static str,
}

/// 从**任意一份** `storage.json` 解出它记录的登录账号 id
///
/// 为什么抽成公共函数：`discover()` 用它读实时现场判断"现在登录着谁"，而快照侧要读
/// **快照内部**的 storage.json 判断"这份快照其实是哪个账号的"——两者判据必须完全一致，
/// 否则又会出现"按 A 保存、实际存的是 B"这类名实不符。
pub fn account_id_from_storage(storage: &Path) -> Option<AccountIdEvidence> {
    let raw = std::fs::read_to_string(storage).ok()?;
    let json: serde_json::Value =
        serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;

    // ① 权威：auth 密文解密后取 userId
    if let Some(value) = json.get(AUTH_KEY).and_then(|v| v.as_str()) {
        if let Ok(plain) = crate::tc_crypto::decrypt_storage_value(value) {
            if let Some(uid) = user_id_of(&plain) {
                return Some(AccountIdEvidence {
                    uid,
                    source: "auth_blob",
                });
            }
        }
        // ② 兼容旧客户端：该键曾是明文 JSON
        if let Some(uid) = user_id_of(value) {
            return Some(AccountIdEvidence {
                uid,
                source: "auth_plaintext",
            });
        }
    }

    // ③ 兜底：icube_gtm.users 唯一键（滞后，仅在拿不到 auth 时用）
    let users = json.get("icube_gtm")?.get("users")?.as_object()?;
    let mut keys: Vec<String> = users
        .keys()
        .filter(|k| super::profile::ensure_slot_safe(k).is_ok())
        .cloned()
        .collect();
    keys.sort();
    match keys.len() {
        1 => Some(AccountIdEvidence {
            uid: keys.remove(0),
            source: "users_map",
        }),
        // 多键时无法判断当前登录者——留着让上层报"不支持"而非猜一个
        _ => None,
    }
}

/// 从明文 JSON 文本里取 `userId`（字符串或数字都接受）
fn user_id_of(json_text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json_text.trim_start_matches('\u{feff}')).ok()?;
    let raw = match v.get("userId")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let uid = raw.trim();
    if uid.is_empty() || super::profile::ensure_slot_safe(uid).is_err() {
        return None;
    }
    Some(uid.to_string())
}

/// 推导当前 TraeWork 登录的 uid
pub fn discover(ctx: &Ctx) -> UidEvidence {
    let storage = ctx
        .data_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");

    let Some(evidence) = account_id_from_storage(&storage) else {
        return UidEvidence::failed(format!(
            "无法从 {} 解出当前登录账号（文件不存在、不是合法 JSON，或 auth 密文解密失败）。请先启动一次 TraeWork 并登录。",
            storage.display()
        ));
    };

    // 交叉核对：兜底来源与权威来源不一致时如实暴露，便于排查上游格式变化
    let mismatch = cross_check(&storage, &evidence.uid);
    let reason = match (evidence.source, mismatch) {
        ("auth_blob", Some(other)) => format!(
            "已解密登录态确认当前账号为 {}（注意：storage.json 的 icube_gtm.users 记为 {other}，该字段存在滞后，已以登录态为准）",
            evidence.uid
        ),
        ("auth_blob", None) => format!(
            "已解密登录态确认当前账号为 {}（auth 密文内的 userId，权威来源）",
            evidence.uid
        ),
        ("auth_plaintext", _) => format!(
            "当前账号为 {}（auth 字段为明文 JSON，属旧版客户端格式）",
            evidence.uid
        ),
        _ => format!(
            "当前账号为 {}（来自 icube_gtm.users，未取到登录态密文：可能是旧版客户端或文件读取异常）",
            evidence.uid
        ),
    };

    UidEvidence {
        uid: Some(evidence.uid.clone()),
        confident: true,
        candidates: users_map_keys(&storage).unwrap_or_else(|| vec![evidence.uid.clone()]),
        reason,
        source: evidence.source.to_string(),
    }
}

/// `icube_gtm.users` 的键名（仅用于交叉核对与展示候选，不参与判定）
fn users_map_keys(storage: &Path) -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(storage).ok()?;
    let json: serde_json::Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let users = json.get("icube_gtm")?.get("users")?.as_object()?;
    let mut keys: Vec<String> = users.keys().cloned().collect();
    keys.sort();
    Some(keys)
}

/// 取 `icube_gtm.users` 唯一键，用于与权威 uid 比对；多键或缺失返回 None
fn cross_check(storage: &Path, authoritative: &str) -> Option<String> {
    let keys = users_map_keys(storage)?;
    if keys.len() == 1 && keys[0] != authoritative {
        Some(keys[0].clone())
    } else {
        None
    }
}

/// 快照侧：读某个快照目录内部记录的账号 id（用于"名实是否相符"的自校验与存量修复）
///
/// `dir_name` 允许带 `.bak` 后缀——修复流程需要读回退代。
pub fn slot_account_id(ctx: &Ctx, dir_name: &str) -> Option<String> {
    let dir = ctx.dir_by_name(dir_name).ok()?;
    account_id_from_storage(&dir.join("User").join("globalStorage").join("storage.json"))
        .map(|e| e.uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traework::test_support::fake_ctx;

    /// 造一份带 tc 密文 auth 字段的 storage.json（密文用生产同款加密函数生成）
    fn write_storage_with_auth(ctx: &Ctx, uid: &str, users_keys: &[&str]) {
        let plain = format!(r#"{{"token":"T","refreshToken":"R","userId":"{uid}","host":"h"}}"#);
        let enc = crate::tc_crypto::encrypt_storage_value(&plain).unwrap();
        let users: Vec<String> = users_keys.iter().map(|k| format!(r#""{k}":{{}}"#)).collect();
        let body = format!(
            r#"{{"{AUTH_KEY}":"{enc}","icube_gtm":{{"users":{{{}}}}}}}"#,
            users.join(",")
        );
        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(gs.join("storage.json"), body).unwrap();
    }

    #[test]
    fn 优先用解密后的登录态取uid() {
        let ctx = fake_ctx("uid-auth");
        write_storage_with_auth(&ctx, "3031811986829834", &["3031811986829834"]);
        let e = discover(&ctx);
        assert!(e.confident);
        assert_eq!(e.uid.as_deref(), Some("3031811986829834"));
        assert_eq!(e.source, "auth_blob");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn users滞后时以登录态为准并如实告警() {
        // 复刻 2026-09-19 的真实事故：users 表还停留在旧账号，登录态已是新账号
        let ctx = fake_ctx("uid-stale");
        write_storage_with_auth(&ctx, "3031811986829834", &["168695880747001"]);
        let e = discover(&ctx);
        assert!(e.confident, "{}", e.reason);
        assert_eq!(
            e.uid.as_deref(),
            Some("3031811986829834"),
            "必须以登录态为准，否则会覆盖别人的快照"
        );
        assert!(e.reason.contains("滞后"), "{}", e.reason);
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 明文auth字段兼容旧客户端() {
        let ctx = fake_ctx("uid-plain");
        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(
            gs.join("storage.json"),
            format!(r#"{{"{AUTH_KEY}":"{{\"userId\":\"12345678\"}}"}}"#),
        )
        .unwrap();
        let e = discover(&ctx);
        assert!(e.confident);
        assert_eq!(e.uid.as_deref(), Some("12345678"));
        assert_eq!(e.source, "auth_plaintext");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 无auth字段时回退users唯一键() {
        let ctx = fake_ctx("uid-fallback");
        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(
            gs.join("storage.json"),
            r#"{"icube_gtm":{"users":{"168695880747001":{}}}}"#,
        )
        .unwrap();
        let e = discover(&ctx);
        assert!(e.confident);
        assert_eq!(e.uid.as_deref(), Some("168695880747001"));
        assert_eq!(e.source, "users_map");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn storage缺失或非法时拒绝入池() {
        let ctx = fake_ctx("uid-missing");
        let e = discover(&ctx);
        assert!(!e.confident && e.uid.is_none());
        assert!(e.reason.contains("无法"));

        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(gs.join("storage.json"), "not json").unwrap();
        assert!(!discover(&ctx).confident);
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn users多键且无auth时不猜() {
        let ctx = fake_ctx("uid-multi");
        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(
            gs.join("storage.json"),
            r#"{"icube_gtm":{"users":{"1111111111111111":{},"2222222222222222":{}}}}"#,
        )
        .unwrap();
        let e = discover(&ctx);
        assert!(!e.confident, "多键且无 auth 时必须拒绝，不得猜");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 非法uid字符被过滤() {
        let ctx = fake_ctx("uid-invalid");
        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(
            gs.join("storage.json"),
            r#"{"icube_gtm":{"users":{"../evil":{}}}}"#,
        )
        .unwrap();
        let e = discover(&ctx);
        assert!(!e.confident);
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }
}
