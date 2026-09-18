//! CN 版积分额度查询
//!
//! 为什么单独成模块：`trae_api.rs` 已临近单文件 800 行上限，而 CN 版的积分模型
//! 与国际版的美元/请求次数模型字段完全不同，独立出来便于维护。

use anyhow::{anyhow, Result};
use reqwest::header::HeaderMap;
use serde_json::json;

use super::types::{CreditsEntitlementResponse, UsageSummary};

/// v2 端点路径：CN 版积分钟模型专用
/// （v1 与 `user_current_entitlement_list` 对 CN 账号恒返回 0，不可用）
const CREDITS_USAGE_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";

/// 查询 CN 版积分额度
///
/// 调用方传入已构造好的鉴权头，避免与本模块重复实现 header 拼装逻辑。
pub async fn fetch_credits_usage(
    client: &reqwest::Client,
    headers: HeaderMap,
    api_base: &str,
) -> Result<UsageSummary> {
    let url = format!("{}{}", api_base, CREDITS_USAGE_PATH);

    let resp = client
        .post(&url)
        .headers(headers)
        .json(&json!({ "require_usage": true }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!("积分额度查询失败: HTTP {}", resp.status()));
    }

    let text = resp.text().await?;
    parse_credits_to_summary(&text)
}

/// 解析 v2 响应为前端展示用的 `UsageSummary`
///
/// 总额与已用量直接取 `usage_summary`（服务端已聚合各礼包），
/// 礼包列表用于补充：重置时间、礼包到期时间，以及按 `available_endpoint`
/// 拆分的「通用积分 / Work 专属积分」两类明细。
pub fn parse_credits_to_summary(response_text: &str) -> Result<UsageSummary> {
    let resp: CreditsEntitlementResponse = serde_json::from_str(response_text)
        .map_err(|e| anyhow!("积分额度响应解析失败: {}", e))?;

    let mut summary = UsageSummary::default();
    summary.is_credits_billing = resp.is_credits_billing;

    if let Some(us) = &resp.usage_summary {
        summary.credits_total = us.total_amount;
        summary.credits_used = us.consumed_amount;
        summary.credits_left = us.total_amount - us.consumed_amount;
    }

    for pack in &resp.user_entitlement_pack_list {
        let base = &pack.entitlement_base_info;
        match base.quota.as_ref().map(|q| q.credits_limit) {
            Some(limit) if limit > 0 => {
                // 按适用产品归类：endpoint 1 = Work 专属积分，其余（0/缺省）= 通用积分
                let used = pack.usage.as_ref().map(|u| u.credits_amount).unwrap_or(0.0);
                if base.available_endpoint == 1 {
                    summary.credits_work_total += limit as f64;
                    summary.credits_work_used += used;
                } else {
                    summary.credits_general_total += limit as f64;
                    summary.credits_general_used += used;
                }
            }
            Some(limit) if limit < 0 => {
                // -1 表示不限量：暂不计入任何一类，聚合值仍以 usage_summary 为准
                log::warn!("CN 积分额度出现不限量礼包（credits_limit={}），未计入分类明细", limit);
            }
            _ => {}
        }

        if base.product_type == 0 {
            // 主套餐（Free/Pro）：决定重置时间与套餐类型
            summary.plan_type = if base.product_id == 0 {
                "Free".to_string()
            } else {
                "Pro".to_string()
            };
            summary.reset_time = base.end_time;
        } else if base.end_time > summary.extra_expire_time {
            // 赠送礼包可能有多条，取最晚到期时间展示（顺序不保证，不能取"最后一条"）
            summary.extra_expire_time = base.end_time;
            summary.extra_package_name = pack.display_desc.clone();
        }
    }

    summary.credits_general_left = summary.credits_general_total - summary.credits_general_used;
    summary.credits_work_left = summary.credits_work_total - summary.credits_work_used;

    log::info!(
        "CN 积分额度 - total: {}, used: {}, left: {}, 通用: {} / {}, Work: {} / {}, packs: {}",
        summary.credits_total,
        summary.credits_used,
        summary.credits_left,
        summary.credits_general_left,
        summary.credits_general_total,
        summary.credits_work_left,
        summary.credits_work_total,
        resp.user_entitlement_pack_list.len()
    );

    Ok(summary)
}