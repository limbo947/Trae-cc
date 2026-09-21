//! TraeWork 账号身份、展示信息与接口凭据解析（权威来源 + 兜底证据链）。
//!
//! **权威来源：解密 `iCubeAuthInfo://icube.cloudide`。** 解出的 JSON 同时给我们三样东西：
//! - `userId` —— 账号身份（也是快照槽名）；
//! - `account.username` / `nonPlainTextMobile` / `avatar_url` —— **仅用于界面展示**；
//! - `token` / `refreshToken` / `expiredAt` / `refreshExpiredAt` —— 接口凭据与寿命。
//!
//! 为什么身份与展示信息都从这一处取：它们是同一份登录态里的字段，一起取可保证「显示的账号」
//! 与「判定的账号」必然一致。分开取会出现「名字是 A、槽位是 B」这种更难排查的名实不符。
//! 凭据同理只能从这里取：账号库里 TraeWork 记录的 `jwt_token` / `cookies` 恒为空，
//! 它没有第二处真源。
//!
//! **`userId` 是唯一可用于判定的字段，展示字段绝不可参与判定**（重名、改名、空值都可能）。
//!
//! **`icube_gtm.users` 只是兜底，不可作为主判据。** 实测（2026-09-19）踩过的坑：用户先登录
//! 账号 A 保存、再换账号 B 保存，两次保存都被解析成同一个 uid，第二次把第一次的快照覆盖了
//! （`.bak` 里才发现是另一个账号的 `userId`）。原因是该字段对切换后的新账号**存在滞后**，
//! 而 `iCubeAuthInfo://usertag` 解密后是**以 uid 为键的累积表**（新旧账号都在里面），
//! 两者都不是"当前登录者"的信号。兜底路径拿不到展示信息（用户名只能靠 auth 密文）。
//!
//! **明确不可用的线索**：`iCubeAuthInfo://icube-dc:<id>` 里的 `<id>` 是 OAuth 设备凭证 id
//! （值为 EC P-256 设备私钥 PEM），不是账号 uid。
//!
//! **红线：推导不出就报错拒绝，绝不猜。** 猜错的后果是覆盖别人的快照／以错误身份启动客户端。

use std::path::{Path, PathBuf};

use super::profile::Ctx;

/// `storage.json` 中承载登录态的键（tc 密文，旧版客户端可能为明文 JSON）
const AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";

/// 账号身份 + 展示信息
///
/// 注意 `uid` 与展示字段的性质差异：`uid` 是身份（决定槽位、参与路径拼接），
/// 其余三个**只影响界面文案**。任何判定逻辑都不得读 `username`。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountProfile {
    /// 账号身份（auth 密文里的 `userId`）；槽名即取此值
    pub uid: String,
    /// `account.username`：本机实测为「似我」「用户1792205080」这类显示名
    pub username: Option<String>,
    /// `account.nonPlainTextMobile`：服务端已脱敏（如 `183******16`），可作辅助识别
    pub mobile: Option<String>,
    /// `account.avatar_url`：远端 URL。当前 UI 未使用（避免为头像引入外部图片加载与 CSP
    /// 调整），先解出来备用
    pub avatar_url: Option<String>,
    /// 顶层 `expiredAt`：access token 的到期时刻（RFC3339，**原样保留字符串**）
    ///
    /// 为什么不解析成时间戳：它只用于界面展示与「是否已过期」的判断，多一种表示就多一处
    /// 与客户端写法漂移的机会（客户端写的就是这个带毫秒的 Z 串）
    pub expired_at: Option<String>,
    /// 顶层 `refreshExpiredAt`：refreshToken 的到期时刻——**它才决定该槽位还能不能免登录**
    /// （access 过期只是触发一次静默续期）。实测口径见 doc/TraeWork账号切换计划.md
    pub refresh_expired_at: Option<String>,
    /// auth_blob（权威）/ auth_plaintext / users_map（兜底，无展示信息）
    pub source: &'static str,
}

impl AccountProfile {
    /// 适合展示的名字：用户名 → 脱敏手机 → uid
    ///
    /// 保证非空（`uid` 必然有值），调用方无需再判空。优先 `username` 是因为它是用户自己在
    /// 客户端里看到的名字；`mobile` 作次选（也是真实身份线索）；最后兜到 uid——**不返回空串**，
    /// 否则界面会出现无名条目。
    pub fn display_name(&self) -> String {
        self.username
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.mobile.clone().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| self.uid.clone())
    }

    /// access token 是否已过期（决定「切过去后客户端是否必然要续期一次」）
    ///
    /// 解析不出时按「未过期」返回：服务端格式变化时宁可不提示，也不要用一条假告警
    /// 让用户以为快照坏了
    pub fn access_expired(&self) -> bool {
        self.expired_at.as_deref().is_some_and(is_past)
    }
}

/// RFC3339 时间串是否早于当前时刻（空串/非法格式 → false）
fn is_past(at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(at.trim())
        .map(|t| t.with_timezone(&chrono::Utc) < chrono::Utc::now())
        .unwrap_or(false)
}

/// uid 推导结果（供「识别当前账号」命令返回给前端）
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
    /// 展示名（用户名 → 脱敏手机 → uid），拿不到时为 None
    pub display_name: Option<String>,
}

impl UidEvidence {
    fn failed(reason: String) -> Self {
        Self {
            uid: None,
            confident: false,
            candidates: Vec::new(),
            reason,
            source: "none".to_string(),
            display_name: None,
        }
    }
}

/// 读并解析一份 `storage.json`（剥离 BOM；缺失或非法返回 None）
fn read_storage_json(storage: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(storage).ok()?;
    serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()
}

/// 取登录态的**明文 JSON 文本**：tc 密文优先解密，失败则回退旧客户端写下的明文
///
/// 为什么单独抽出来：账号身份（`profile_from_storage`）与接口凭据（`credentials_from_storage`）
/// 读的是同一份密文。两处各写一遍解密迟早会漂移——`iCubeAuthInfo` 由明文改成 tc 密文时
/// 就发生过一次「一个读取方适配了、另一个还按明文解析」。来源标签由调用方透传，用于如实
/// 说明「这个身份是从哪儿来的」。
fn auth_plaintext(json: &serde_json::Value) -> Option<(String, &'static str)> {
    let value = json.get(AUTH_KEY)?.as_str()?;
    if let Ok(plain) = crate::tc_crypto::decrypt_storage_value(value) {
        return Some((plain, "auth_blob"));
    }
    Some((value.to_string(), "auth_plaintext"))
}

/// 从**任意一份** `storage.json` 解出账号身份与展示信息
///
/// 为什么抽成公共函数：`discover()` 用它读实时现场判断"现在登录着谁"，而快照侧要读
/// **快照内部**的 storage.json 判断"这份快照其实是哪个账号的"——两者判据必须完全一致，
/// 否则又会出现"按 A 保存、实际存的是 B"这类名实不符。
pub fn profile_from_storage(storage: &Path) -> Option<AccountProfile> {
    let json = read_storage_json(storage)?;

    // ①② 权威来源（auth_blob）与旧版明文格式（auth_plaintext）共用同一段解析
    if let Some((plain, source)) = auth_plaintext(&json) {
        if let Some(profile) = profile_of(&plain, source) {
            return Some(profile);
        }
    }

    // ③ 兜底：icube_gtm.users 唯一键（滞后，且拿不到展示信息）
    let users = json.get("icube_gtm")?.get("users")?.as_object()?;
    let mut keys: Vec<String> = users
        .keys()
        .filter(|k| super::profile::ensure_slot_safe(k).is_ok())
        .cloned()
        .collect();
    keys.sort();
    match keys.len() {
        1 => Some(AccountProfile {
            uid: keys.remove(0),
            username: None,
            mobile: None,
            avatar_url: None,
            // 兜底路径只拿得到键名，凭据时间在 auth 密文里，这里必然为空
            expired_at: None,
            refresh_expired_at: None,
            source: "users_map",
        }),
        // 多键时无法判断当前登录者——留给上层报"不支持"而非猜一个
        _ => None,
    }
}

/// 解析明文 JSON：`userId` 必需，展示字段缺失不影响返回（只少个名字）
fn profile_of(json_text: &str, source: &'static str) -> Option<AccountProfile> {
    let v: serde_json::Value = serde_json::from_str(json_text.trim_start_matches('\u{feff}')).ok()?;

    let uid = match v.get("userId")? {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    // 与槽位白名单同一套校验：uid 会被拼进路径，非法值宁可当作"解不出来"
    if uid.is_empty() || super::profile::ensure_slot_safe(&uid).is_err() {
        return None;
    }

    // 取值口径统一为「取到非空字符串才算有」——客户端曾把这些字段写成空串
    let text_of = |node: Option<&serde_json::Value>, key: &str| -> Option<String> {
        node.and_then(|n| n.get(key))
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let field = |key: &str| text_of(v.get("account"), key);

    Some(AccountProfile {
        uid,
        username: field("username"),
        mobile: field("nonPlainTextMobile"),
        avatar_url: field("avatar_url"),
        // 凭据时间在顶层，与 account 子对象同级
        expired_at: text_of(Some(&v), "expiredAt"),
        refresh_expired_at: text_of(Some(&v), "refreshExpiredAt"),
        source,
    })
}

/// 推导当前 TraeWork 登录的账号
pub fn discover(ctx: &Ctx) -> UidEvidence {
    let storage = ctx
        .data_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");

    let Some(profile) = profile_from_storage(&storage) else {
        return UidEvidence::failed(format!(
            "无法从 {} 解出当前登录账号（文件不存在、不是合法 JSON，或 auth 密文解密失败）。请先启动一次 TraeWork 并登录。",
            storage.display()
        ));
    };

    // 展示名与 uid 一并给出，界面上可写「似我（168695880747001）」而不是只有一串数字
    let label = match profile.username.as_deref() {
        Some(name) => format!("{name}（{}）", profile.uid),
        None => profile.uid.clone(),
    };

    // 交叉核对：兜底来源与权威来源不一致时如实暴露，便于排查上游格式变化
    let mismatch = cross_check(&storage, &profile.uid);
    let reason = match (profile.source, mismatch) {
        ("auth_blob", Some(other)) => format!(
            "已解密登录态确认当前账号为 {label}（注意：storage.json 的 icube_gtm.users 记为 {other}，该字段存在滞后，已以登录态为准）"
        ),
        ("auth_blob", None) => {
            format!("已解密登录态确认当前账号为 {label}（auth 密文内的 userId，权威来源）")
        }
        ("auth_plaintext", _) => {
            format!("当前账号为 {label}（auth 字段为明文 JSON，属旧版客户端格式）")
        }
        _ => format!(
            "当前账号为 {label}（来自 icube_gtm.users，未取到登录态密文：可能是旧版客户端或文件读取异常）"
        ),
    };

    UidEvidence {
        uid: Some(profile.uid.clone()),
        confident: true,
        candidates: users_map_keys(&storage).unwrap_or_else(|| vec![profile.uid.clone()]),
        reason,
        source: profile.source.to_string(),
        display_name: profile.username.clone(),
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

/// 快照侧：读某个快照目录的账号身份与展示信息（`dir_name` 允许带 `.bak` 后缀）
pub fn slot_profile(ctx: &Ctx, dir_name: &str) -> Option<AccountProfile> {
    let dir = ctx.dir_by_name(dir_name).ok()?;
    profile_from_storage(&dir.join("User").join("globalStorage").join("storage.json"))
}

/// 快照侧：只取账号 id（自校验与存量修复用）
pub fn slot_account_id(ctx: &Ctx, dir_name: &str) -> Option<String> {
    slot_profile(ctx, dir_name).map(|p| p.uid)
}

/// 调用 CN 接口（积分 / 签到）所需的 TraeWork 凭据
///
/// **刻意不 derive `Serialize`**：`token` 是可以直接冒充该账号的凭据，本结构一旦出现在任何
/// 命令返回值里就会泄露给前端（`AccountProfile` 已经 derive 了 `Serialize`，所以凭据单独成
/// 结构、而不是挂到它身上）。同理禁止写进日志或落盘到账号库——它只在该次调用的栈上流动。
pub struct TraeworkCredentials {
    pub token: String,
}

/// 从**任意一份** `storage.json` 解出接口凭据
///
/// 返回 None 的三种情形都是「拿不到凭据」而非异常：文件缺失/非法、`auth` 键不存在、
/// `auth` 里没有非空 `token`（旧客户端明文格式，或上游改了字段名）。调用方据此给出
/// 「切换到该账号并重新保存」这类可操作的提示，而不是抛一个技术性错误。
///
/// 为什么不把 token 一起塞进 `AccountProfile`：那是**给界面看**的结构，今天恰好没被任何命令
/// 返回，但只要它 derive 了 `Serialize`，将来任何一次「顺手返回 profile」都会把凭据带出去。
pub fn credentials_from_storage(storage: &Path) -> Option<TraeworkCredentials> {
    let json = read_storage_json(storage)?;
    let (plain, _) = auth_plaintext(&json)?;
    let value: serde_json::Value = serde_json::from_str(plain.trim_start_matches('\u{feff}')).ok()?;
    let token = value.get("token")?.as_str()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    Some(TraeworkCredentials { token })
}

/// 快照侧：读某个槽位快照内的凭据（`dir_name` 允许带 `.bak` 后缀）
pub fn slot_credentials(ctx: &Ctx, dir_name: &str) -> Option<TraeworkCredentials> {
    let dir = ctx.dir_by_name(dir_name).ok()?;
    credentials_from_storage(&dir.join("User").join("globalStorage").join("storage.json"))
}

/// 实时现场（客户端正在使用的那一份）的 `storage.json` 路径
///
/// 为什么需要它：客户端启动时用快照里的 `refreshToken` 换发新凭据并写回**现场**，而快照文件
/// 不会跟着更新。所以「当前正在使用的那个账号」必须读现场才能拿到未过期的 token；读快照
/// 只会拿到一份随着时间推移必然过期的旧值。
pub fn live_storage(ctx: &Ctx) -> PathBuf {
    ctx.data_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traework::test_support::fake_ctx;

    /// 造一份带 tc 密文 auth 字段的 storage.json
    ///
    /// `account` 传 None 时模拟客户端把这些字段写成缺失/空串的情形
    fn write_storage_with_auth(
        ctx: &Ctx,
        uid: &str,
        users_keys: &[&str],
        account: Option<(&str, &str)>,
    ) {
        let account_json = match account {
            Some((name, mobile)) => format!(
                r#","account":{{"username":"{name}","email":"","nonPlainTextMobile":"{mobile}","avatar_url":"https://example.invalid/a.png"}}"#
            ),
            None => r#","account":{"username":"","email":"","nonPlainTextMobile":""}"#.to_string(),
        };
        let plain = format!(
            r#"{{"token":"T","refreshToken":"R","userId":"{uid}","host":"h"{account_json}}}"#
        );
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

    fn write_raw_storage(ctx: &Ctx, body: &str) {
        let gs = ctx.data_dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(gs.join("storage.json"), body).unwrap();
    }

    /// 实时登录态的 storage.json 路径（`discover` / `profile_from_storage` 读的就是它）
    fn live_storage(ctx: &Ctx) -> std::path::PathBuf {
        ctx.data_dir.join("User").join("globalStorage").join("storage.json")
    }

    /// 在**快照槽目录**里造一份 storage.json（`slot_profile` 读的是它）
    ///
    /// 与 `write_storage_with_auth` 的区别正是这次踩的坑：后者写实时目录，
    /// 用它去断言 `slot_profile` 必然拿不到东西——两个入口读的路径不同。
    fn write_slot_storage(ctx: &Ctx, slot: &str, uid: &str, name: &str) {
        let plain = format!(
            r#"{{"userId":"{uid}","account":{{"username":"{name}","nonPlainTextMobile":"x"}}}}"#
        );
        let enc = crate::tc_crypto::encrypt_storage_value(&plain).unwrap();
        let dir = ctx
            .profiles_dir
            .join(slot)
            .join("User")
            .join("globalStorage");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("storage.json"),
            format!(r#"{{"{AUTH_KEY}":"{enc}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn 优先用解密后的登录态取uid() {
        let ctx = fake_ctx("uid-auth");
        write_storage_with_auth(&ctx, "3031811986829834", &["3031811986829834"], None);
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
        write_storage_with_auth(&ctx, "3031811986829834", &["168695880747001"], None);
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
    fn 展示名取用户名并将uid附在依据文案里() {
        let ctx = fake_ctx("uid-name");
        write_storage_with_auth(
            &ctx,
            "168695880747001",
            &["168695880747001"],
            Some(("似我", "183******16")),
        );
        let p = profile_from_storage(&live_storage(&ctx)).expect("能从实时登录态读出 profile");
        assert_eq!(p.username.as_deref(), Some("似我"));
        assert_eq!(p.mobile.as_deref(), Some("183******16"));
        assert_eq!(p.display_name(), "似我");
        assert!(p.avatar_url.is_some());

        let e = discover(&ctx);
        assert_eq!(e.display_name.as_deref(), Some("似我"));
        assert!(
            e.reason.contains("似我（168695880747001）"),
            "依据文案应同时给出展示名与 uid：{}",
            e.reason
        );
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 展示字段为空时回退到uid() {
        // 客户端确实会把这些字段写成空串（本机 email 就是空），此时不能返回空名字
        let ctx = fake_ctx("uid-noname");
        write_storage_with_auth(&ctx, "168695880747001", &[], Some(("", "")));
        let p = profile_from_storage(&live_storage(&ctx)).expect("profile 仍应可解");
        assert_eq!(p.username, None, "空串应被归一为 None");
        assert_eq!(p.display_name(), "168695880747001");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 快照槽路径也能读出用户名() {
        // 保存/切换/修复三条链路读的都是**槽目录**里的 storage.json，
        // 与实时目录是两个入口，必须各自有用例覆盖（本次就是这里写错过）
        let ctx = fake_ctx("uid-slotprofile");
        write_slot_storage(&ctx, "3031811986829834", "3031811986829834", "用户1792205080");
        let p = slot_profile(&ctx, "3031811986829834").expect("能从槽目录读出 profile");
        assert_eq!(p.username.as_deref(), Some("用户1792205080"));
        assert_eq!(p.display_name(), "用户1792205080");
        assert_eq!(
            slot_account_id(&ctx, "3031811986829834").as_deref(),
            Some("3031811986829834")
        );
        // 不存在的槽位应返回 None，而不是编一个
        assert!(slot_profile(&ctx, "9999999999999999").is_none());
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 凭据只认非空token且兼容明文旧格式() {
        let ctx = fake_ctx("uid-cred");

        // 当前客户端格式：tc 密文
        write_auth_storage(
            &ctx,
            r#"{"userId":"168695880747001","token":"T-abc","refreshToken":"R"}"#,
        );
        assert_eq!(
            credentials_from_storage(&live_storage(&ctx))
                .expect("应能解出 token")
                .token,
            "T-abc"
        );

        // 没有 token（上游改了字段名）：必须返回 None。绝不能用空串去拼 Authorization——
        // 那会把「本机拿不到凭据」变成一次必然 401 的请求，错误信息还指不回真正的原因
        write_auth_storage(&ctx, r#"{"userId":"168695880747001"}"#);
        assert!(credentials_from_storage(&live_storage(&ctx)).is_none());

        // 空白 token 同理
        write_auth_storage(&ctx, r#"{"userId":"168695880747001","token":"   "}"#);
        assert!(credentials_from_storage(&live_storage(&ctx)).is_none());

        // 旧客户端明文 auth（未加密）：凭据也要能取到，不能只有身份那条路做了兼容
        write_raw_storage(
            &ctx,
            &format!(r#"{{"{AUTH_KEY}":"{{\"userId\":\"1\",\"token\":\"T-plain\"}}"}}"#),
        );
        assert_eq!(
            credentials_from_storage(&live_storage(&ctx)).unwrap().token,
            "T-plain"
        );

        // 槽位侧与实时侧是两条路径，必须各自覆盖（历史上就有入口只改了一边）
        let slot_gs = ctx
            .profiles_dir
            .join("3031811986829834")
            .join("User")
            .join("globalStorage");
        std::fs::create_dir_all(&slot_gs).unwrap();
        let enc = crate::tc_crypto::encrypt_storage_value(
            r#"{"userId":"3031811986829834","token":"T-slot"}"#,
        )
        .unwrap();
        std::fs::write(
            slot_gs.join("storage.json"),
            format!(r#"{{"{AUTH_KEY}":"{enc}"}}"#),
        )
        .unwrap();
        assert_eq!(
            slot_credentials(&ctx, "3031811986829834").unwrap().token,
            "T-slot"
        );

        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 兜底路径无展示名但仍有uid() {
        let ctx = fake_ctx("uid-fallback");
        write_raw_storage(
            &ctx,
            r#"{"icube_gtm":{"users":{"168695880747001":{}}}}"#,
        );
        let e = discover(&ctx);
        assert!(e.confident);
        assert_eq!(e.uid.as_deref(), Some("168695880747001"));
        assert_eq!(e.source, "users_map");
        assert_eq!(e.display_name, None, "兜底路径拿不到用户名");
        assert!(e.reason.contains("168695880747001"));
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 明文auth字段兼容旧客户端() {
        let ctx = fake_ctx("uid-plain");
        write_raw_storage(
            &ctx,
            &format!(
                r#"{{"{AUTH_KEY}":"{{\"userId\":\"12345678\",\"account\":{{\"username\":\"老客户端\"}}}}"}}"#
            ),
        );
        let e = discover(&ctx);
        assert!(e.confident);
        assert_eq!(e.uid.as_deref(), Some("12345678"));
        assert_eq!(e.source, "auth_plaintext");
        assert_eq!(e.display_name.as_deref(), Some("老客户端"));
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn storage缺失或非法时拒绝入池() {
        let ctx = fake_ctx("uid-missing");
        let e = discover(&ctx);
        assert!(!e.confident && e.uid.is_none());
        assert!(e.reason.contains("无法"));

        write_raw_storage(&ctx, "not json");
        assert!(!discover(&ctx).confident);
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn users多键且无auth时不猜() {
        let ctx = fake_ctx("uid-multi");
        write_raw_storage(
            &ctx,
            r#"{"icube_gtm":{"users":{"1111111111111111":{},"2222222222222222":{}}}}"#,
        );
        let e = discover(&ctx);
        assert!(!e.confident, "多键且无 auth 时必须拒绝，不得猜");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 非法uid字符被过滤() {
        let ctx = fake_ctx("uid-invalid");
        write_raw_storage(&ctx, r#"{"icube_gtm":{"users":{"../evil":{}}}}"#);
        let e = discover(&ctx);
        assert!(!e.confident);
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    #[test]
    fn 解不出user_id时即使有用户名也不返回() {
        // 防「拿展示名当身份」：没有 userId 就必须当作解不出来
        let ctx = fake_ctx("uid-nouserid");
        let enc = crate::tc_crypto::encrypt_storage_value(r#"{"account":{"username":"似我"}}"#).unwrap();
        write_raw_storage(
            &ctx,
            &format!(r#"{{"{AUTH_KEY}":"{enc}","icube_gtm":{{"users":{{}}}}"}}"#),
        );
        assert!(profile_from_storage(
            &ctx.data_dir.join("User").join("globalStorage").join("storage.json")
        )
        .is_none());
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }

    /// 写一份只带指定 auth 明文的 storage.json（凭据时间断言用）
    fn write_auth_storage(ctx: &Ctx, plain: &str) {
        let enc = crate::tc_crypto::encrypt_storage_value(plain).unwrap();
        write_raw_storage(ctx, &format!(r#"{{"{AUTH_KEY}":"{enc}"}}"#));
    }

    #[test]
    fn 解出凭据到期时间且只在过期时报警() {
        let ctx = fake_ctx("uid-expiry");
        // 过去的 access + 未来的 refresh：access 已过期，但该槽位仍可免登录（客户端会续期）
        write_auth_storage(
            &ctx,
            r#"{"userId":"168695880747001","expiredAt":"2000-01-01T00:00:00.000Z","refreshExpiredAt":"2099-01-01T00:00:00.000Z"}"#,
        );
        let p = profile_from_storage(&live_storage(&ctx)).unwrap();
        assert_eq!(p.expired_at.as_deref(), Some("2000-01-01T00:00:00.000Z"));
        assert_eq!(p.refresh_expired_at.as_deref(), Some("2099-01-01T00:00:00.000Z"));
        assert!(p.access_expired(), "2000 年的 access 必然已过期");

        // 未来的 access 不得判为过期
        write_auth_storage(&ctx, r#"{"userId":"168695880747001","expiredAt":"2099-01-01T00:00:00.000Z"}"#);
        assert!(!profile_from_storage(&live_storage(&ctx)).unwrap().access_expired());

        // 时间串非法：原样保留供展示，但不判过期（宁可漏提示，也不给假告警）
        write_auth_storage(&ctx, r#"{"userId":"168695880747001","expiredAt":"not-a-date"}"#);
        let p = profile_from_storage(&live_storage(&ctx)).unwrap();
        assert_eq!(p.expired_at.as_deref(), Some("not-a-date"));
        assert!(!p.access_expired());

        // 兜底路径（icube_gtm.users）只拿得到键名，凭据时间必然为空
        write_raw_storage(&ctx, r#"{"icube_gtm":{"users":{"168695880747001":{}}}}"#);
        let p = profile_from_storage(&live_storage(&ctx)).unwrap();
        assert!(p.expired_at.is_none() && p.refresh_expired_at.is_none());
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
    }
}
