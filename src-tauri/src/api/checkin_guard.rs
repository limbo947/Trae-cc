//! 签到冷却状态机 + 进程级防重入
//!
//! 为什么需要冷却：`9074` 原本只弹一次 toast 就能被立刻重试，连点「全部签到」或
//! GUI + `--silent` 双进程会重复打服务端。冷却把「失败后多久才允许再试」变成有界退避。
//!
//! 为什么需要防重入：三个入口（单账号 / 全部 / 自动）可并发，而 `account_manager` 锁
//! 只保护「读列表 / 写日期」两个瞬间，不覆盖网络段。
//!
//! 语义分层（关键，防冷却与重试互相架空）：冷却管**跨批次记忆**，批次内的重试轮次管
//! **批次内退避**。因此批次运行期间产生的失败只进内存待重试集合、不落盘冷却，全部轮次
//! 结束后统一落盘——否则 10 分钟的 9074 冷却会把 30 秒后的重试直接拦死。

use std::sync::LazyLock;

use tokio::sync::{Mutex, MutexGuard};

use crate::account::{Account, CheckinCooldown};

/// 冷却原因码（机器可读，前端按字符串判定渲染，不读 `until` 数值）
pub const REASON_AUTH_EXPIRED: &str = "auth_expired";
pub const REASON_RATE_LIMITED: &str = "rate_limited";
pub const REASON_RISK_CONTROL: &str = "risk_control";
pub const REASON_SERVER_ERROR: &str = "server_error";
/// TraeWork 专属：快照凭据已失效（该账号没有 Cookies 刷新链路，人工介入是
/// 「切过去 + 保存当前登录态」，该动作即清除入口）
pub const REASON_CREDENTIAL_STALE: &str = "credential_stale";

/// 进程级签到互斥锁
///
/// 为什么用 static 而不是 `AppState` 字段：`--silent`（lib.rs）自建 `AccountManager`、
/// 不构建 `AppState`，字段作用域覆盖不到无头进程。
/// 为什么用 `tokio::sync::Mutex`：`MutexGuard<'_, ()>` 是 `Send`，可跨 `.await` 持有
/// （`std::sync::Mutex` 会让 future 非 `Send` 直接编译失败）；`try_lock` 是对信号量的
/// CAS，无 TOCTOU，恰好一个调用方获胜。
///
/// 已知边界：static 只保护同进程；GUI 与开机自启的 `--silent` 仍可能并发。
/// 不做跨进程文件锁——最坏得到 9095/9074，且 9074 有冷却兜底，复杂度与收益不成比例。
static CHECKIN_GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 尝试取得签到互斥权；返回 `None` 表示已有批次在跑
///
/// 调用方必须持锁到**全部结束**（含重试轮次的 sleep），否则睡 90s 期间会被第二个批次穿透。
/// 注意持锁期间单账号手动签到同样被拒——这是简化锁粒度的有意取舍，不拆分。
pub fn try_acquire() -> Option<MutexGuard<'static, ()>> {
    CHECKIN_GUARD.try_lock().ok()
}

/// 冷却触发源 → 策略表的唯一入口
///
/// 集中成枚举而非散落各处的 if：策略调整时只改这一处，避免「同一 code 在两处不同时长」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownTrigger {
    /// 9074 账号级限流（实测同一时刻不同账号结果不同，与设备号无关）
    RateLimited,
    /// HTTP 429：服务端显式限流
    Http429,
    /// HTTP 404：端点短暂不可用
    Http404,
    /// 5xx / 网络超时
    Transient,
    /// 1005 权益不足：账号不满足活动条件
    EntitlementDenied,
    /// 401 / 1001 Token 失效
    AuthExpired,
    /// TraeWork 快照凭据失效：本工具没有该账号的刷新链路（refreshToken 续期端点
    /// 未找到），唯一自救是用户「切过去 + 保存当前登录态」——冷却必须能被那个动作清掉
    CredentialStale,
}

impl CooldownTrigger {
    pub fn reason(self) -> &'static str {
        match self {
            Self::RateLimited | Self::Http429 => REASON_RATE_LIMITED,
            Self::Http404 | Self::Transient => REASON_SERVER_ERROR,
            Self::EntitlementDenied => REASON_RISK_CONTROL,
            Self::AuthExpired => REASON_AUTH_EXPIRED,
            Self::CredentialStale => REASON_CREDENTIAL_STALE,
        }
    }

    fn duration_secs(self) -> i64 {
        match self {
            Self::RateLimited => 10 * 60,
            Self::Http429 | Self::Http404 => 60,
            Self::Transient => 2 * 60,
            Self::EntitlementDenied => 12 * 3600,
            Self::AuthExpired => i64::MAX,
            // 与 EntitlementDenied 同级：access 过期后本工具无法续期，短冷却只会让
            // 批次反复白打请求；12h 与「等用户下次想起来切换」的节奏相符，且被钳到当日边界
            Self::CredentialStale => 12 * 3600,
        }
    }

    /// 是否值得在批次内重试（限流与暂时性故障可重试；权益不足与 Token 失效不可）
    pub fn retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Http429 | Self::Http404 | Self::Transient)
    }
}

/// 按策略表构造冷却；落盘前已钳到当日边界，写入值即最终值
pub fn cooldown_for(trigger: CooldownTrigger) -> CheckinCooldown {
    let duration = trigger.duration_secs();
    // i64::MAX 哨兵无时钟语义，禁止参与时间换算
    let until = if duration == i64::MAX {
        i64::MAX
    } else {
        chrono::Utc::now().timestamp() + duration
    };
    CheckinCooldown {
        until: clamp_to_today_end(until),
        reason: trigger.reason().to_string(),
    }
}

/// 把冷却截止时刻钳到**本地时区**当日 23:59:59
///
/// 为什么必须用本地时区：`last_checkin_date` 是 `chrono::Local` 本地日期，若按 UTC 日界
/// 钳制，在中国时区会钳到北京时间 07:59:59——项目约定「9074 不写日期、下次启动自动重试」，
/// 冷却一旦跨天就会静默放弃一整天。钳到当日边界后「跨天必重试」成为结构性保证。
///
/// 为什么读取侧也要钳：读取侧钳制天然覆盖手改 `accounts.json` 与时钟回拨，无需启动校正逻辑。
pub fn clamp_to_today_end(until: i64) -> i64 {
    use chrono::{Local, NaiveTime, TimeZone};
    // i64::MAX 哨兵原样透传（它表示「需人工介入」，与时间无关）
    if until == i64::MAX {
        return until;
    }
    let today_end = Local::now()
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(23, 59, 59).expect("23:59:59 必然合法"));
    match Local.from_local_datetime(&today_end).earliest() {
        Some(dt) => until.min(dt.timestamp()),
        // 时区解析异常时宁可不钳：错误地缩短冷却比不缩短更糟
        None => until,
    }
}

/// 批次内的单账号签到落盘结果（日期与冷却二选一或都无）
///
/// 成功/Already → 写 `last_checkin_date`；失败 → 写冷却。两者独立，不能互相覆盖，
/// 因此用字段而非枚举（例如限流不写日期，但服务端错误也可能已含日期）。
pub struct CheckinOutcome {
    pub account_id: String,
    pub date: Option<String>,
    pub cooldown: Option<CheckinCooldown>,
}

/// 账号当前是否处于冷却中；返回剩余截止时刻（UTC 秒）
pub fn is_cooling_down(account: &Account) -> Option<i64> {
    let cooldown = account.checkin_cooldown.as_ref()?;
    let until = clamp_to_today_end(cooldown.until);
    (until > chrono::Utc::now().timestamp()).then_some(until)
}

/// 清除指定原因的冷却；返回是否发生变更，供调用方决定是否落盘
///
/// 为什么泛化出 reason 参数：TraeWork 的 `credential_stale` 需要挂在「保存当前登录态」
/// 上清除，而 Token 刷新只该清 `auth_expired`——各入口自带语义，按 reason 精确清除
/// 才不会出现「保存登录态顺手解除了 9074 限流」这类越权。
pub fn clear_cooldown_reason(account: &mut Account, reason: &str) -> bool {
    let matched = account
        .checkin_cooldown
        .as_ref()
        .is_some_and(|c| c.reason == reason);
    if matched {
        account.checkin_cooldown = None;
    }
    matched
}

/// 清除 `auth_expired` 冷却（Token 刷新 / 重新登录 / 编辑账号后调用）
///
/// 仅清该 reason：刷新 Token 不该解除 9074 限流冷却。保留为薄包装以维持既有调用点。
pub fn clear_auth_cooldown(account: &mut Account) -> bool {
    clear_cooldown_reason(account, REASON_AUTH_EXPIRED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, NaiveTime, TimeZone};

    /// 明日时间戳必须被钳回「本地时区」当日 23:59:59——按 UTC 日界钳会钳到北京时间
    /// 07:59:59，「跨天必重试」直接破洞（这是本模块最容易写错的一处）
    #[test]
    fn clamp_pulls_future_until_back_to_local_day_end() {
        let tomorrow = chrono::Utc::now().timestamp() + 48 * 3600;
        let clamped = clamp_to_today_end(tomorrow);
        let expected = Local
            .from_local_datetime(
                &Local::now()
                    .date_naive()
                    .and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()),
            )
            .earliest()
            .unwrap()
            .timestamp();
        assert_eq!(clamped, expected);
    }

    /// `i64::MAX` 是「需人工介入」哨兵，无时钟语义，禁止参与任何时间换算
    #[test]
    fn clamp_passes_sentinel_through() {
        assert_eq!(clamp_to_today_end(i64::MAX), i64::MAX);
        assert_eq!(cooldown_for(CooldownTrigger::AuthExpired).until, i64::MAX);
    }

    /// 过去的冷却视为已结束（读取侧钳制让手改数据与时钟回拨都能自愈）
    #[test]
    fn expired_cooldown_is_not_cooling() {
        let mut account = test_account();
        account.checkin_cooldown = Some(CheckinCooldown {
            until: chrono::Utc::now().timestamp() - 1,
            reason: REASON_RATE_LIMITED.to_string(),
        });
        assert!(is_cooling_down(&account).is_none());
    }

    /// 策略表：9074 十分钟、HTTP 429 六十秒、1005 十二小时、401 哨兵、credential_stale 十二小时
    #[test]
    fn policy_table_matches_plan() {
        assert_eq!(CooldownTrigger::RateLimited.duration_secs(), 600);
        assert_eq!(CooldownTrigger::Http429.duration_secs(), 60);
        assert_eq!(CooldownTrigger::Http404.duration_secs(), 60);
        assert_eq!(CooldownTrigger::Transient.duration_secs(), 120);
        assert_eq!(CooldownTrigger::EntitlementDenied.duration_secs(), 12 * 3600);
        assert_eq!(CooldownTrigger::CredentialStale.duration_secs(), 12 * 3600);
        assert_eq!(CooldownTrigger::Http429.reason(), REASON_RATE_LIMITED);
        assert_eq!(CooldownTrigger::Http404.reason(), REASON_SERVER_ERROR);
        assert_eq!(CooldownTrigger::CredentialStale.reason(), REASON_CREDENTIAL_STALE);
        assert!(CooldownTrigger::RateLimited.retryable());
        assert!(!CooldownTrigger::EntitlementDenied.retryable());
        // TraeWork 凭据失效不可重试：本工具没有刷新链路，批次内重试必然白打
        assert!(!CooldownTrigger::CredentialStale.retryable());
        // 12h 冷却必须被钳到当日 23:59:59，而不是真的压上 12 小时
        assert_ne!(cooldown_for(CooldownTrigger::CredentialStale).until, i64::MAX);
    }

    /// 刷新 Token 只清 auth_expired，限流冷却必须保留
    #[test]
    fn clear_auth_cooldown_only_touches_auth_reason() {
        let mut account = test_account();
        account.checkin_cooldown = Some(CheckinCooldown {
            until: i64::MAX,
            reason: REASON_AUTH_EXPIRED.to_string(),
        });
        assert!(clear_auth_cooldown(&mut account));
        assert!(account.checkin_cooldown.is_none());

        account.checkin_cooldown = Some(CheckinCooldown {
            until: chrono::Utc::now().timestamp() + 600,
            reason: REASON_RATE_LIMITED.to_string(),
        });
        assert!(!clear_auth_cooldown(&mut account));
        assert!(account.checkin_cooldown.is_some());
    }

    /// 泛化清除必须精确按 reason 命中：「保存登录态」清 credential_stale 时
    /// 不得顺手解掉 auth_expired 哨兵（那会让需人工介入的账号被静默放行）
    #[test]
    fn clear_cooldown_reason_only_touches_matching_reason() {
        let mut account = test_account();
        account.checkin_cooldown = Some(CheckinCooldown {
            until: chrono::Utc::now().timestamp() + 600,
            reason: REASON_CREDENTIAL_STALE.to_string(),
        });
        assert!(clear_cooldown_reason(&mut account, REASON_CREDENTIAL_STALE));
        assert!(account.checkin_cooldown.is_none());

        account.checkin_cooldown = Some(CheckinCooldown {
            until: i64::MAX,
            reason: REASON_AUTH_EXPIRED.to_string(),
        });
        assert!(!clear_cooldown_reason(&mut account, REASON_CREDENTIAL_STALE));
        assert!(account.checkin_cooldown.is_some(), "哨兵不得被别的 reason 的清除动作碰掉");
        assert!(clear_cooldown_reason(&mut account, REASON_AUTH_EXPIRED));
        assert!(account.checkin_cooldown.is_none());
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
