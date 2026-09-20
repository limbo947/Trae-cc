use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 账号信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub email: String,
    pub avatar_url: String,
    pub cookies: String,
    pub jwt_token: Option<String>,
    pub token_expired_at: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    pub user_id: String,
    pub tenant_id: String,
    pub region: String,
    pub plan_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_active: bool,
    /// 账号关联的机器码
    #[serde(default)]
    pub machine_id: Option<String>,
    /// 签到设备号（`X-Device-Id`），与 `machine_id` 彻底解耦：重置设备号不碰 IDE 机器身份
    /// 为什么独立成字段：machine_id 同是 machineid 文件值与注册表 MachineGuid，
    /// 复用会让「换签到设备号」连带改写 IDE 机器身份，无法单独旋转
    #[serde(default)]
    pub device_id: Option<String>,
    /// 最近一次签到成功的日期（本地日期 YYYY-MM-DD）
    /// 为什么落盘：方案B 自动签到靠它判断「今日是否已签」，避免一天多次开机重复签到
    #[serde(default)]
    pub last_checkin_date: Option<String>,
    /// 签到冷却（跨批次记忆）：批次内失败不落盘，全部轮次结束后统一写入
    #[serde(default)]
    pub checkin_cooldown: Option<CheckinCooldown>,
    /// 账号归属应用：`traecode`（Trae CN，本工具原有能力）或 `traework`（TRAE SOLO CN）
    ///
    /// 为什么必须落盘而不是运行时推断：两个应用的切换机制是**相反**的——traecode 靠
    /// 改写 storage.json 登录态，TraeWork 靠整目录快照覆盖（其登录真源在 state.vscdb，
    /// 写 JSON 无效）。没有这个判据就无法决定一条账号记录该走哪条链路。
    /// `default = "default_app"` 保证 1.0.5 之前的 accounts.json（无此字段）加载后全部
    /// 视为 traecode，不丢账号。
    #[serde(default = "default_app")]
    pub app: String,
    /// TraeWork 的 Cloud-IDE uid（来源 `storage.json` 的 `icube_gtm.users` 键名）
    ///
    /// 为什么单独成字段而不是复用 `user_id`：`user_id` 是 traecode 从 API 拿到的用户 id，
    /// 语义与取值域都不同；TraeWork 的 uid 只能从本机登录态证据推导。混用会让两条链路
    /// 互相污染（例如导出的账号被错误地当成同一身份去重）。
    #[serde(default)]
    pub uid: Option<String>,
    /// 快照槽名（默认即 uid）
    ///
    /// 为什么允许与 uid 不等：槽位是磁盘上的目录名，将来若要支持「同一 uid 多份快照」
    /// 或用户手工改名，只需改这个字段而不必动数据格式。
    #[serde(default)]
    pub snapshot_slot: Option<String>,
}

/// `Account.app` 的 serde 默认值（旧数据兼容）：缺失即视为 traecode 账号
fn default_app() -> String {
    APP_TRAECODE.to_string()
}

/// traecode 应用标识
pub const APP_TRAECODE: &str = "traecode";
/// TraeWork（TRAE SOLO CN）应用标识
pub const APP_TRAEWORK: &str = "traework";

/// 签到冷却状态
///
/// 为什么用嵌套单字段而不是平铺两个 `Option`：平铺会出现「until 有值但 reason 为空」的
/// 非法中间态，判定与渲染都得额外防御；嵌套后「有冷却」必然带原因码。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckinCooldown {
    /// 截止时刻（UTC 秒）；`i64::MAX` 是「需人工介入」哨兵，无时钟语义
    pub until: i64,
    /// 机器可读码：auth_expired / rate_limited / risk_control / server_error
    pub reason: String,
}

impl Account {
    /// 今日是否已签到（本地日期口径）
    ///
    /// 为什么由后端判定而不是把 `last_checkin_date` 交给前端比较：签到流程写入时用的是
    /// `chrono::Local` 的本地日期，前端若用 `toISOString()` 之类的 UTC 口径重算，
    /// 中国时区下凌晨 0–8 点会得出不同的「今天」，徽标就会与实际状态矛盾。
    pub fn is_checked_in_today(&self) -> bool {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.last_checkin_date.as_deref() == Some(today.as_str())
    }

    pub fn new(
        name: String,
        email: String,
        cookies: String,
        user_id: String,
        tenant_id: String,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        // 有 user_id 就立即派生落盘（派生一次即固定）；为空则留 None，交给启动回填/解析回退
        let device_id = if user_id.trim().is_empty() {
            None
        } else {
            Some(crate::api::device_id::derive_device_id(&user_id))
        };
        Self {
            id: uuid_simple(),
            name,
            email,
            avatar_url: String::new(),
            cookies,
            jwt_token: None,
            token_expired_at: None,
            password: None,
            user_id,
            tenant_id,
            region: String::new(),
            plan_type: "Free".to_string(),
            created_at: now,
            updated_at: now,
            is_active: true,
            machine_id: Some(Uuid::new_v4().to_string()),
            device_id,
            last_checkin_date: None,
            checkin_cooldown: None,
            app: APP_TRAECODE.to_string(),
            uid: None,
            snapshot_slot: None,
        }
    }

    /// 构造 TraeWork 账号记录
    ///
    /// 为什么与 `new` 分开而不是加参数：TraeWork 账号不需要 cookies/jwt/device_id
    /// （那些是 traecode 的额度与签到链路所需），强行共用一个签名会让每个调用点都要
    /// 传一堆空串，且容易误把 traecode 的派生逻辑（如 device_id 由 user_id 派生）
    /// 套到 uid 上——uid 与 device_id 完全无关，套用会产生错误的设备号。
    pub fn new_traework(uid: String, name: String) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: uuid_simple(),
            name,
            email: String::new(),
            avatar_url: String::new(),
            cookies: String::new(),
            jwt_token: None,
            token_expired_at: None,
            password: None,
            user_id: uid.clone(),
            tenant_id: String::new(),
            region: String::new(),
            plan_type: "Free".to_string(),
            created_at: now,
            updated_at: now,
            is_active: true,
            // TraeWork 的机器身份随快照走（machineid 在快照白名单内），
            // 这里不再单独生成，避免出现「账号库里的 machine_id 与快照里的实际值不一致」
            // 这种无法自洽的第二真源
            machine_id: None,
            device_id: None,
            last_checkin_date: None,
            checkin_cooldown: None,
            app: APP_TRAEWORK.to_string(),
            uid: Some(uid),
            snapshot_slot: None,
        }
    }

    /// 是否 traecode 账号（额度查询/签到/Token 刷新等链路的准入判据）
    pub fn is_traecode(&self) -> bool {
        self.app != APP_TRAEWORK
    }

    /// 快照槽名：显式值优先，其次 uid；两者皆空则 None（调用方须报错，不得兜底）
    pub fn slot(&self) -> Option<&str> {
        self.snapshot_slot
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.uid.as_deref().filter(|s| !s.trim().is_empty()))
    }
}

/// 账号列表存储结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountStore {
    pub accounts: Vec<Account>,
    pub active_account_id: Option<String>,
    /// 当前 Trae IDE 正在使用的账号 ID
    #[serde(default)]
    pub current_account_id: Option<String>,
}

/// 简单的 UUID 生成
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap();
    format!("{:x}{:x}", duration.as_secs(), duration.subsec_nanos())
}

/// 「从 Trae IDE 读取账号」的结果状态
///
/// 为什么显式建模而不是继续用 `Option<Account>`：`None` 同时代表「本机没有登录态」与
/// 「该账号已在列表中」这两种完全不同的情形，前端只能兜底成「未找到登录账号或账号已存在」，
/// 用户既不知道发生了什么、也不知道下一步该做什么（2026-09-20 的实际报障即由此而来）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraeIdeReadStatus {
    /// 新账号，已加入账号库
    Added,
    /// 账号已存在，但用 IDE 里的信息补齐了缺失字段
    Updated,
    /// 账号已存在且没有可补的字段（未做任何改动）
    Exists,
    /// 本机没有可读的 IDE 登录态
    NoLogin,
}

/// 「从 Trae IDE 读取账号」的结果
///
/// `message` 由后端统一撰写（中文），前后端共用同一份文案，避免两处各写一遍导致口径漂移。
#[derive(Debug, Clone, Serialize)]
pub struct TraeIdeReadOutcome {
    pub status: TraeIdeReadStatus,
    /// 涉及到的账号（Added/Updated/Exists 时给出，便于前端直接刷新列表项）
    pub account: Option<Account>,
    pub message: String,
}

impl TraeIdeReadOutcome {
    /// 只有一个状态与说明、不涉及具体账号的结果（NoLogin）
    pub fn no_login(message: impl Into<String>) -> Self {
        Self {
            status: TraeIdeReadStatus::NoLogin,
            account: None,
            message: message.into(),
        }
    }
}

/// 账号简要信息（用于列表展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountBrief {
    pub id: String,
    pub name: String,
    pub email: String,
    pub avatar_url: String,
    pub plan_type: String,
    pub is_active: bool,
    pub created_at: i64,
    /// 账号关联的机器码
    pub machine_id: Option<String>,
    /// 是否是当前 Trae IDE 正在使用的账号
    pub is_current: bool,
    /// 今日是否已签到（后端用本地日期口径判定，避免前端 UTC 口径重算出错）
    pub checked_in_today: bool,
    /// 归属应用（traecode / traework）：前端据此分流到两套界面与操作
    pub app: String,
    /// TraeWork uid（traecode 账号恒为 None）
    pub uid: Option<String>,
}

impl From<&Account> for AccountBrief {
    fn from(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            email: account.email.clone(),
            avatar_url: account.avatar_url.clone(),
            plan_type: account.plan_type.clone(),
            is_active: account.is_active,
            created_at: account.created_at,
            machine_id: account.machine_id.clone(),
            is_current: false, // 默认为 false，由 AccountManager 设置
            checked_in_today: account.is_checked_in_today(),
            app: account.app.clone(),
            uid: account.uid.clone(),
        }
    }
}

impl AccountBrief {
    /// 从 Account 创建 AccountBrief，并设置 is_current 标记
    pub fn from_account(account: &Account, is_current: bool) -> Self {
        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            email: account.email.clone(),
            avatar_url: account.avatar_url.clone(),
            plan_type: account.plan_type.clone(),
            is_active: account.is_active,
            created_at: account.created_at,
            machine_id: account.machine_id.clone(),
            is_current,
            checked_in_today: account.is_checked_in_today(),
            app: account.app.clone(),
            uid: account.uid.clone(),
        }
    }
}
