//! CN 版每日签到
//!
//! 为什么单独成模块：签到虽与额度同属 UG 体系，但请求头（UA `Trae/0.1.52` +
//! `X-User-Region: CN`）、失败语义（HTTP 200 + 业务 code）与「已签到」判定规则都不同，
//! 且 `trae_api.rs` 已临近单文件 800 行上限。
//!
//! 行为约定（移植自 traework2api，均为实测结论）：
//! - 鉴权只需 JWT（`claim` 另需 `X-Device-Id`，用独立派生的**签到设备号**，与 `machine_id`
//!   解耦），不需要 Cookies 与签名，不触碰 IDE 文件
//! - 「已签到」判定要同时覆盖「已签到」与「已经签到」两种措辞：服务端 9095 原文是
//!   「当前设备今日已经签到」，「已经签到」不含子串「已签到」，只匹配前者会误判为失败
//! - 服务端对签到做**设备维度**的每日去重（9095）：同一 `X-Device-Id` 当日已签即拒绝
//! - `9074 当前参与用户太多` 是账号级限流不是失败：记为 `rate_limited` 并进冷却
//!   （策略表见 `checkin_guard`）；`1001`/401 token 失效，用 Cookies 刷新后重试一次
//! - 并发保护：三入口共用 `checkin_guard::try_acquire` 互斥，防连点/双进程重复打服务端
//!
//! 语义分层（勿破坏）：冷却管**跨批次记忆**（准入时快照一次），重试轮次管**批次内退避**。
//! 批次运行期间产生的失败只进内存待重试集合、不落盘冷却，全部轮次结束后统一写盘一次。

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use crate::account::{Account, AccountManager, CheckinCooldown};
use crate::api::checkin_guard::{self, CheckinOutcome, CooldownTrigger};
use crate::api::device_id;
use crate::api::TraeApiClient;

/// 签到端点域名：优先 `api.trae.cn`（traework2api 实测值），失败回退 `api.trae.com.cn`
const CHECKIN_HOSTS: [&str; 2] = ["https://api.trae.cn", "https://api.trae.com.cn"];
const CHECKIN_STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CHECKIN_CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";

/// 批次内重试的等待时长（第 2 轮前 / 第 3 轮前）；次数固定为 2 次，与账号数无关
const RETRY_WAITS_SECS: [u64; 2] = [30, 90];

/// 签到结果状态（序列化给前端展示）
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CheckinState {
    /// 本次签到成功
    Ok,
    /// 今日已签到（含手动/自动已签过）
    Already,
    /// 服务端限流（9074 等）：今日尚未签到，稍后重试即可
    RateLimited,
    /// 本次未尝试（账号处于冷却期）：不是错误，不该进失败汇总
    Cooldown,
    /// 签到失败（活动未开放、凭据失效等）
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckinResult {
    pub account_id: String,
    pub account_name: String,
    pub state: CheckinState,
    pub detail: String,
    /// 冷却截止时刻（UTC 秒）；`i64::MAX` 超出 JS 安全整数范围、JSON 反序列化后等值判断
    /// 不可靠，前端一律按 `cooldown_reason` 渲染，不读该数值
    pub cooldown_until: Option<i64>,
    /// 冷却原因码（auth_expired / rate_limited / risk_control / server_error）
    pub cooldown_reason: Option<String>,
}

impl CheckinResult {
    fn new(account: &Account, state: CheckinState, detail: String, cooldown: Option<CheckinCooldown>) -> Self {
        Self {
            account_id: account.id.clone(),
            account_name: account.name.clone(),
            state,
            detail,
            cooldown_until: cooldown.as_ref().map(|c| c.until),
            cooldown_reason: cooldown.map(|c| c.reason),
        }
    }

    fn plain(account: &Account, state: CheckinState, detail: String) -> Self {
        Self::new(account, state, detail, None)
    }
}

/// 签到状态响应（`checkin_credits/status`）
#[derive(Debug, Clone, Deserialize)]
struct CheckinStatusResponse {
    #[serde(default)]
    checked_in: bool,
    #[serde(default)]
    enable: bool,
    #[serde(default)]
    credits: i64,
    #[serde(default)]
    extra_credits: i64,
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// 业务响应（claim 成功可能为空响应，失败为 HTTP 200 + code/message）
#[derive(Debug, Clone, Deserialize)]
struct CheckinBizResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// 单次请求的内部错误；每个变体都对应策略表里的一个冷却触发源（`Other` 除外）
enum CheckinError {
    /// 401 / 1001：Token 失效，刷新一次后仍失败则需重新登录
    AuthExpired,
    /// 9074 / HTTP 429：限流，稍后重试即可
    RateLimited(CooldownTrigger),
    /// 网络超时 / 5xx / 404：暂时性故障，可重试
    Transient(CooldownTrigger),
    /// 1005 权益不足：账号不满足活动条件，长冷却且不重试
    EntitlementDenied,
    /// 9004 设备号缺失或非法：设备号隔离后不应再出现，出现即回归信号（直报失败）
    DeviceInvalid(String),
    /// 其它失败：无冷却、不重试
    Other(String),
}

impl CheckinError {
    /// 该错误对应的冷却触发源；`None` 表示不冷却
    fn trigger(&self) -> Option<CooldownTrigger> {
        match self {
            Self::AuthExpired => Some(CooldownTrigger::AuthExpired),
            Self::RateLimited(t) | Self::Transient(t) => Some(*t),
            Self::EntitlementDenied => Some(CooldownTrigger::EntitlementDenied),
            Self::DeviceInvalid(_) | Self::Other(_) => None,
        }
    }

    /// 展示状态：限流单独成类（不计失败），其余都是失败
    fn state(&self) -> CheckinState {
        match self {
            Self::RateLimited(_) => CheckinState::RateLimited,
            _ => CheckinState::Failed,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::AuthExpired => "Token 已失效，请重新登录".to_string(),
            Self::RateLimited(_) => "签到人数过多，请稍后重试".to_string(),
            Self::Transient(_) => "网络或服务端暂时不可用".to_string(),
            Self::EntitlementDenied => "账号权益不足，无法签到".to_string(),
            Self::DeviceInvalid(detail) => detail.clone(),
            Self::Other(detail) => detail.clone(),
        }
    }

    /// 是否值得在批次内重试
    fn retryable(&self) -> bool {
        self.trigger().is_some_and(CooldownTrigger::retryable)
    }
}

/// 批次内单个账号的处理结果
struct RoundResult {
    checkin: CheckinResult,
    retryable: bool,
}

/// 按数值 `code` / HTTP 状态码 / 文本三级优先分类
///
/// 为什么数值优先于子串：引入冷却后子串误判的代价从「报错文案不准」变成「白等 10 分钟」。
/// 原实现用 `contains("401")` / `contains("9074")`，消息里恰好含这些数字就误判。
/// 数值 code 与 HTTP 状态由各解析点透传；子串仅在拿不到 code 时兜底。
fn classify_error(code: Option<i64>, http_status: Option<u16>, text: &str) -> CheckinError {
    if let Some(code) = code {
        match code {
            9074 => return CheckinError::RateLimited(CooldownTrigger::RateLimited),
            1001 => return CheckinError::AuthExpired,
            1005 => return CheckinError::EntitlementDenied,
            9004 => return CheckinError::DeviceInvalid(text.to_string()),
            _ => {}
        }
    }
    if let Some(status) = http_status {
        match status {
            401 => return CheckinError::AuthExpired,
            429 => return CheckinError::RateLimited(CooldownTrigger::Http429),
            404 => return CheckinError::Transient(CooldownTrigger::Http404),
            500..=599 => return CheckinError::Transient(CooldownTrigger::Transient),
            _ => {}
        }
    }
    // 子串兜底：仅在解析不到 code / 状态码时走到
    let lower = text.to_lowercase();
    if text.contains("1001")
        || text.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("token is invalid")
    {
        return CheckinError::AuthExpired;
    }
    if text.contains("9074")
        || text.contains("使用人数太多")
        || lower.contains("rate limit")
        || lower.contains("too many")
    {
        return CheckinError::RateLimited(CooldownTrigger::RateLimited);
    }
    if text.contains("9004") || lower.contains("order parameters") {
        return CheckinError::DeviceInvalid(text.to_string());
    }
    CheckinError::Other(text.to_string())
}

fn build_headers(token: &str, device_id: Option<&str>) -> Result<header::HeaderMap> {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse()?);
    headers.insert(header::ACCEPT, "application/json".parse()?);
    // UG 系端点认 Trae 客户端 UA，浏览器 UA 未必放行（traework2api 实测）
    headers.insert(header::USER_AGENT, "Trae/0.1.52".parse()?);
    headers.insert("X-User-Region", "CN".parse()?);
    // claim 必须带 X-Device-Id：实测缺省时返回 9004「order parameters incorrect」，
    // 带上设备号后才会走到正常业务（9074 限流/成功）。status 不校验该头。
    if let Some(id) = device_id.filter(|id| !id.is_empty()) {
        headers.insert("X-Device-Id", id.parse()?);
    }
    let auth = header::HeaderValue::from_bytes(format!("Cloud-IDE-JWT {}", token).as_bytes())
        .map_err(|e| anyhow!("Token 格式错误: {}", e))?;
    headers.insert(header::AUTHORIZATION, auth);
    Ok(headers)
}

/// 无歧义判定「今日已签到」——只认明确的已签标记（含大小写变体）
///
/// 除「已签到 / already check」外必须包含「已经签到」：服务端 9095 的原文是
/// 「当前设备今日已经签到，请明日再来哦～」，其中「已经签到」**不含**子串「已签到」，
/// 只匹配前者会把 9095 误判为失败（实测踩过）。
pub fn is_already_checked_in(text: &str) -> bool {
    let lower = text.to_lowercase();
    text.contains("已签到")
        || text.contains("已经签到")
        || lower.contains("already check")
        || lower.contains("already checked")
}

/// 服务端按「设备」维度的每日去重：同一 X-Device-Id 当日签过即拒绝（实测 9095）。
/// 与「账号已签到」不同——该账号当日并未拿到积分，但换设备号前重试无意义。
fn is_device_checked_in(text: &str) -> bool {
    text.contains("9095") || text.contains("当前设备")
}

/// 发送签到请求，双端点回退（返回 HTTP 状态码与响应文本）
async fn post_with_fallback(
    client: &reqwest::Client,
    token: &str,
    device_id: Option<&str>,
    path: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let mut last_error = anyhow!("所有签到端点都失败");

    for host in CHECKIN_HOSTS {
        let url = format!("{}{}", host, path);
        match client
            .post(&url)
            .headers(build_headers(token, device_id)?)
            .json(&json!({}))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Ok((status, text));
            }
            Err(e) => {
                log::warn!("签到端点 {} 请求失败: {}", host, e);
                last_error = anyhow!("签到请求失败: {}", e);
            }
        }
    }

    Err(last_error)
}

/// 单账号签到：status → claim
///
/// **claim 前必先查 status 是防双签红线**：重置设备号后若该账号今日已真签到成功，
/// status 的 `checked_in=true` 会把流程短路为 Already，避免同日二次 claim。
/// 该顺序禁止改动、禁止绕过（持久化数据无法区分「今日 9095 未拿积分」与「今日真签到成功」）。
async fn checkin_with_token(
    client: &reqwest::Client,
    token: &str,
    device_id: Option<&str>,
) -> std::result::Result<(CheckinState, String), CheckinError> {
    let (status, text) = post_with_fallback(client, token, device_id, CHECKIN_STATUS_PATH)
        .await
        .map_err(|_| CheckinError::Transient(CooldownTrigger::Transient))?;

    if !status.is_success() {
        let detail = format!("签到状态查询失败: HTTP {} {}", status, text);
        return Err(classify_error(None, Some(status.as_u16()), &detail));
    }

    let status_resp: CheckinStatusResponse = serde_json::from_str(&text)
        .map_err(|e| CheckinError::Other(format!("签到状态解析失败: {}", e)))?;

    if status_resp.code != 0 && !status_resp.message.is_empty() {
        let detail = format!("{} {}", status_resp.code, status_resp.message);
        if is_already_checked_in(&detail) {
            return Ok((CheckinState::Already, "今日已签到".to_string()));
        }
        return Err(classify_error(Some(status_resp.code), None, &detail));
    }

    if status_resp.checked_in {
        return Ok((CheckinState::Already, "今日已签到".to_string()));
    }

    if !status_resp.enable {
        // 活动未开放是稳定状态：不冷却、不写日期（追认现状，Failed 直报）
        return Ok((CheckinState::Failed, "签到活动未对该账号开放".to_string()));
    }

    // 执行签到
    let (claim_status, claim_text) = post_with_fallback(client, token, device_id, CHECKIN_CLAIM_PATH)
        .await
        .map_err(|_| CheckinError::Transient(CooldownTrigger::Transient))?;

    if !claim_status.is_success() {
        let detail = format!("签到失败: HTTP {} {}", claim_status, claim_text);
        return Err(classify_error(None, Some(claim_status.as_u16()), &detail));
    }

    if !claim_text.trim().is_empty() {
        match serde_json::from_str::<CheckinBizResponse>(&claim_text) {
            Ok(biz) => {
                if biz.code != 0 {
                    let detail = format!("{} {}", biz.code, biz.message);
                    // 设备维度去重优先于通用「已签到」：该账号今日其实没拿到积分，
                    // 但同一设备号当日已签，提示要区分开，否则用户会误以为积分已到账
                    if is_device_checked_in(&detail) {
                        return Ok((CheckinState::Already, "本设备今日已签到，请明日再试".to_string()));
                    }
                    if is_already_checked_in(&detail) {
                        return Ok((CheckinState::Already, "今日已签到".to_string()));
                    }
                    return Err(classify_error(Some(biz.code), None, &detail));
                }
            }
            Err(_) => {
                // 成功响应按约定是空体或 code=0 的 JSON；非 JSON 正文按异常处理，
                // 但保留「已签到 / success」这类明确的成功标记
                if is_already_checked_in(&claim_text) {
                    return Ok((CheckinState::Already, "今日已签到".to_string()));
                }
                if !claim_text.to_lowercase().contains("success") {
                    return Err(classify_error(None, None, &claim_text));
                }
            }
        }
    }

    let detail = if status_resp.credits > 0 && status_resp.extra_credits > 0 {
        format!("签到成功，获得 {} 积分（额外 +{}）", status_resp.credits, status_resp.extra_credits)
    } else if status_resp.credits + status_resp.extra_credits > 0 {
        format!("签到成功，获得 {} 积分", status_resp.credits + status_resp.extra_credits)
    } else {
        "签到成功".to_string()
    };
    Ok((CheckinState::Ok, detail))
}

/// 用 Cookies 刷新 Token（取锁读 → 无锁网络 → 取锁写，遵守仓库锁纪律）
async fn refresh_token_via_cookies(manager: &Mutex<AccountManager>, account_id: &str) -> Option<String> {
    let cookies = {
        let guard = manager.lock().await;
        guard.get_account(account_id).ok()?.cookies
    };
    if cookies.trim().is_empty() {
        return None;
    }

    let mut client = TraeApiClient::new(&cookies).ok()?;
    let token_result = client.get_user_token().await.ok()?;

    {
        let mut guard = manager.lock().await;
        guard
            .save_refreshed_token(account_id, &token_result.token, &token_result.expired_at)
            .ok()?;
    }
    Some(token_result.token)
}

/// 把内部错误折算成对外的批次结果（含冷却字段）
fn error_round(account: &Account, err: &CheckinError) -> RoundResult {
    let cooldown = err.trigger().map(checkin_guard::cooldown_for);
    RoundResult {
        checkin: CheckinResult::new(account, err.state(), err.message(), cooldown),
        retryable: err.retryable(),
    }
}

/// 处理单个账号：token 失效时先用 Cookies 刷新再重试一次
async fn process_account(
    client: &reqwest::Client,
    manager: &Mutex<AccountManager>,
    account: &Account,
) -> RoundResult {
    let mut token = account.jwt_token.clone().filter(|t| !t.trim().is_empty());
    if token.is_none() {
        token = refresh_token_via_cookies(manager, &account.id).await;
    }

    let Some(mut current_token) = token else {
        // 既无 Token 也无可用 Cookies：等价于「需重新登录」，走 auth_expired 冷却
        return error_round(account, &CheckinError::AuthExpired);
    };
    // claim 必须带设备号：用独立派生的签到设备号，与 IDE 机器码解耦（见 device_id 模块）
    let device_id_str = device_id::resolve_device_id(account);

    let outcome = checkin_with_token(client, &current_token, Some(device_id_str.as_str())).await;
    let err = match outcome {
        Ok((state, detail)) => {
            return RoundResult {
                checkin: CheckinResult::plain(account, state, detail),
                retryable: false,
            }
        }
        Err(err) => err,
    };

    // 仅 Token 失效才刷新重试（与改动前一致）；其余类别直接按策略表落冷却
    if !matches!(err, CheckinError::AuthExpired) {
        return error_round(account, &err);
    }

    match refresh_token_via_cookies(manager, &account.id).await {
        Some(new_token) => {
            current_token = new_token;
            match checkin_with_token(client, &current_token, Some(device_id_str.as_str())).await {
                Ok((state, detail)) => RoundResult {
                    checkin: CheckinResult::plain(account, state, detail),
                    retryable: false,
                },
                Err(second_err) => error_round(account, &second_err),
            }
        }
        None => error_round(account, &CheckinError::AuthExpired),
    }
}

/// 执行一轮签到，返回结果与「本轮仍可重试」的账号 ID
async fn run_round(
    manager: &Mutex<AccountManager>,
    accounts: Vec<Account>,
) -> Result<(Vec<CheckinResult>, HashSet<String>)> {
    if accounts.is_empty() {
        return Ok((Vec::new(), HashSet::new()));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| anyhow!("创建签到请求客户端失败: {}", e))?;

    let mut results = Vec::with_capacity(accounts.len());
    let mut retryable_ids = HashSet::new();
    for account in accounts {
        let round = process_account(&client, manager, &account).await;
        if round.retryable {
            retryable_ids.insert(account.id.clone());
        }
        log::info!(
            "签到 - {}（{}）[设备 {}]: {:?} {}",
            account.name,
            account.email,
            device_id::mask_device_id(&device_id::resolve_device_id(&account)),
            round.checkin.state,
            round.checkin.detail
        );
        results.push(round.checkin);
    }
    Ok((results, retryable_ids))
}

/// 把结果折算成一次性落盘的 `CheckinOutcome` 集合
///
/// 同一账号可能出现在多个重试轮次里，取**最后一次**结果（重试成功要覆盖首轮冷却）。
/// `Cooldown`（未尝试）不产生落盘项；普通失败（无冷却）也不产生。
fn collect_outcomes(results: &[CheckinResult], today: &str) -> Vec<CheckinOutcome> {
    let mut latest: HashMap<&str, &CheckinResult> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for r in results {
        if latest.insert(r.account_id.as_str(), r).is_none() {
            order.push(r.account_id.as_str());
        }
    }

    order
        .into_iter()
        .filter_map(|id| {
            let r = latest[id];
            match r.state {
                CheckinState::Ok | CheckinState::Already => Some(CheckinOutcome {
                    account_id: id.to_string(),
                    date: Some(today.to_string()),
                    cooldown: None,
                }),
                CheckinState::RateLimited | CheckinState::Failed => {
                    r.cooldown_reason.as_ref().map(|reason| CheckinOutcome {
                        account_id: id.to_string(),
                        date: None,
                        cooldown: Some(CheckinCooldown {
                            until: r.cooldown_until.unwrap_or(i64::MAX),
                            reason: reason.clone(),
                        }),
                    })
                }
                CheckinState::Cooldown => None,
            }
        })
        .collect()
}

/// 批次启动时**快照一次**已有冷却做准入；返回（冷却结果, 待请求账号）
///
/// 为什么只快照一次：批次运行期间产生的失败不落盘、也不重新准入，否则 10 分钟的 9074
/// 冷却会把 30 秒后的重试轮直接拦死（见模块头部「语义分层」）。
fn snapshot_cooldowns(accounts: &[Account]) -> (Vec<CheckinResult>, Vec<Account>) {
    let mut cooldown_results = Vec::new();
    let mut pending = Vec::new();
    for account in accounts {
        match checkin_guard::is_cooling_down(account) {
            Some(until) => {
                let reason = account
                    .checkin_cooldown
                    .as_ref()
                    .map_or(String::new(), |c| c.reason.clone());
                cooldown_results.push(CheckinResult::new(
                    account,
                    CheckinState::Cooldown,
                    "账号处于冷却期，本次未尝试".to_string(),
                    Some(CheckinCooldown { until, reason }),
                ));
            }
            None => pending.push(account.clone()),
        }
    }
    (cooldown_results, pending)
}

/// 统一写盘一批结果（成功/Already 写日期，失败项写冷却）
async fn persist_outcomes(manager: &Mutex<AccountManager>, outcomes: Vec<CheckinOutcome>) {
    if outcomes.is_empty() {
        return;
    }
    let mut guard = manager.lock().await;
    if let Err(e) = guard.apply_checkin_outcomes(&outcomes) {
        log::warn!("签到结果写盘失败: {}", e);
    }
}

/// 单账号签到（手动触发，右键菜单）——防重入 + 冷却准入，单轮结束即落盘
pub async fn checkin_one_account(manager: &Mutex<AccountManager>, account_id: &str) -> Result<CheckinResult> {
    let _guard = checkin_guard::try_acquire().ok_or_else(|| anyhow!("签到进行中，请稍候再试"))?;

    let account = {
        let guard = manager.lock().await;
        guard.get_account(account_id)?
    };

    let (mut results, pending) = snapshot_cooldowns(std::slice::from_ref(&account));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (round_results, _) = run_round(manager, pending).await?;
    results.extend(round_results);

    persist_outcomes(manager, collect_outcomes(&results, &today)).await;
    Ok(results.pop().expect("单账号签到必然产生一条结果"))
}

/// 全部账号签到（手动触发，工具栏按钮）——防重入 + 冷却准入，单轮不重试
pub async fn checkin_all(manager: &Mutex<AccountManager>) -> Result<Vec<CheckinResult>> {
    let _guard = checkin_guard::try_acquire().ok_or_else(|| anyhow!("签到进行中，请稍候再试"))?;

    let accounts = {
        let guard = manager.lock().await;
        guard.list_accounts_for_checkin("", false)
    };

    let (mut results, pending) = snapshot_cooldowns(&accounts);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (round_results, _) = run_round(manager, pending).await?;
    results.extend(round_results);

    persist_outcomes(manager, collect_outcomes(&results, &today)).await;
    Ok(results)
}

/// 自动签到（方案B）：仅今日未签到账号；批次级重试轮次，全部轮次结束后统一落盘一次
///
/// 为什么批次级轮次而非账号级重试：账号级最坏耗时随账号数线性增长（5 个限流账号 ≈ 10 分钟）
/// 且全程持锁；批次级把等待次数固定为 2 次，最坏 ≈ 2 分钟、与账号数无关。
/// 仅当存在可重试账号才 sleep（正常全成功零额外等待）。
pub async fn auto_checkin_pending(manager: &Mutex<AccountManager>) -> Result<Vec<CheckinResult>> {
    let _guard = checkin_guard::try_acquire().ok_or_else(|| anyhow!("签到进行中，请稍候再试"))?;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let pending_raw = {
        let guard = manager.lock().await;
        guard.list_accounts_for_checkin(&today, true)
    };
    if pending_raw.is_empty() {
        return Ok(Vec::new());
    }

    log::info!("自动签到：{} 个账号今日未签到，开始静默执行", pending_raw.len());

    // 冷却准入快照：冷却中的账号本次不请求，其落盘冷却原样保留
    let (mut all_results, pending) = snapshot_cooldowns(&pending_raw);
    let (mut round_results, mut retryable_ids) = run_round(manager, pending).await?;
    all_results.append(&mut round_results);

    for (index, sleep_secs) in RETRY_WAITS_SECS.iter().enumerate() {
        if retryable_ids.is_empty() {
            break;
        }
        log::info!(
            "自动签到：第 {} 轮，可重试 {} 个账号，等待 {}s",
            index + 2,
            retryable_ids.len(),
            sleep_secs
        );
        tokio::time::sleep(std::time::Duration::from_secs(*sleep_secs)).await;

        let retry_accounts: Vec<Account> = {
            let guard = manager.lock().await;
            retryable_ids.iter().filter_map(|id| guard.get_account(id).ok()).collect()
        };
        let (mut round_results, new_retryable) = run_round(manager, retry_accounts).await?;
        retryable_ids = new_retryable;
        all_results.append(&mut round_results);
    }

    persist_outcomes(manager, collect_outcomes(&all_results, &today)).await;
    Ok(all_results)
}
