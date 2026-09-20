import type { UsageSummary } from "../types";
import "./AccountCheckinBadge.css";

interface AccountListItemProps {
  account: {
    id: string;
    name: string;
    email: string;
    avatar_url: string;
    plan_type: string;
    created_at: number;
  };
  usage: UsageSummary | null;
  selected: boolean;
  /** 今日签到状态（后端按本地日期判定，前端只负责渲染） */
  checkedInToday: boolean;
  onSelect: (id: string) => void;
  onContextMenu: (e: React.MouseEvent, id: string) => void;
}

export function AccountListItem({ account, usage, selected, checkedInToday, onSelect, onContextMenu }: AccountListItemProps) {
  const hasUsage = !!usage;
  
  // 根据计费模式显示不同的额度：CN 积分钟 > 美元计费 > 请求次数
  const isCreditsBilling = usage?.is_credits_billing ?? false;
  const isDollarBilling = usage?.is_dollar_billing ?? false;

  const totalUsed = isCreditsBilling
    ? (usage?.credits_used ?? 0)
    : isDollarBilling
    ? (usage?.fast_dollar_used ?? 0)
    : (usage ? usage.fast_request_used + usage.extra_fast_request_used : 0);
  const totalLimit = isCreditsBilling
    ? (usage?.credits_total ?? 0)
    : isDollarBilling
    ? (usage?.fast_dollar_limit ?? 0)
    : (usage ? usage.fast_request_limit + usage.extra_fast_request_limit : 0);
  const totalLeft = isCreditsBilling
    ? (usage?.credits_left ?? 0)
    : isDollarBilling
    ? (usage?.fast_dollar_left ?? 0)
    : (usage ? usage.fast_request_left + usage.extra_fast_request_left : 0);
  const usagePercent = totalLimit > 0 ? Math.round((totalUsed / totalLimit) * 100) : 0;

  // CN 积分按适用产品拆分：通用积分与 Work 专属积分
  const generalTotal = usage?.credits_general_total ?? 0;
  const generalLeft = usage?.credits_general_left ?? 0;
  const workTotal = usage?.credits_work_total ?? 0;
  const workLeft = usage?.credits_work_left ?? 0;

  const getUsageColor = () => {
    if (usagePercent >= 80) return "var(--danger)";
    if (usagePercent >= 50) return "var(--warning)";
    return "var(--success)";
  };

  const formatCreatedDate = (timestamp: number) => {
    if (!timestamp) return "-";
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffTime = Math.abs(now.getTime() - date.getTime());
    const diffDays = Math.ceil(diffTime / (1000 * 60 * 60 * 24));

    if (diffDays === 0) return "今天";
    if (diffDays === 1) return "昨天";
    if (diffDays < 7) return `${diffDays}天前`;
    if (diffDays < 30) return `${Math.floor(diffDays / 7)}周前`;
    if (diffDays < 365) return `${Math.floor(diffDays / 30)}个月前`;
    return `${Math.floor(diffDays / 365)}年前`;
  };

  const isTokenExpired = false; // TODO: 根据实际 token 过期时间判断

  // 额度耗尽信号：只有拿到用量后才判断（usage 为 null 时判断会误报）
  const quotaExhausted = hasUsage && totalLeft <= 0;

  return (
    <div
      className={`account-list-item ${selected ? "selected" : ""}`}
      onClick={(e) => {
        if (e.button !== 0) return;
        onSelect(account.id);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        onContextMenu(e, account.id);
      }}
    >
      <div className="list-item-checkbox" onClick={(e) => e.stopPropagation()}>
        <input
          type="checkbox"
          checked={selected}
          onChange={() => onSelect(account.id)}
        />
      </div>

      <div className="list-item-avatar">
        {account.avatar_url ? (
          <img src={account.avatar_url} alt={account.name} />
        ) : (
          <div className="avatar-placeholder">
            {(account.name || account.email).charAt(0).toUpperCase()}
          </div>
        )}
      </div>

      <div className="list-item-info">
        <div className="list-item-name-row">
          <span className="list-item-email">{account.name || account.email}</span>
          <span className={`checkin-tag ${checkedInToday ? "done" : "pending"}`}>
            {checkedInToday ? "已签到" : "未签到"}
          </span>
        </div>
        <span className="list-item-id">Trae 账号</span>
      </div>

      <div className="list-item-plan">
        <span className="plan-badge">{usage?.plan_type || account.plan_type || "Free"}</span>
        {usage && usage.extra_fast_request_limit > 0 && (
          <span className="extra-badge">礼包</span>
        )}
      </div>

      <div className="list-item-usage">
        {isDollarBilling ? (
          // 美元计费模式 - 紧凑美观显示
          <div className="usage-dollar-compact">
            <div className="usage-dollar-main">
              <span className="dollar-used">${totalUsed.toFixed(2)}</span>
              <span className="dollar-divider">/</span>
              <span className="dollar-limit">
                <span className="limit-basic">{usage?.basic_dollar_limit.toFixed(1)}</span>
                {(usage?.bonus_dollar_limit ?? 0) > 0 && (
                  <>
                    <span className="limit-plus">+</span>
                    <span className="limit-bonus">{usage?.bonus_dollar_limit.toFixed(1)}</span>
                  </>
                )}
              </span>
            </div>
            <div className="usage-bar-mini">
              <div
                className="usage-bar-fill-mini"
                style={{ width: `${Math.min(usagePercent, 100)}%`, background: getUsageColor() }}
              />
            </div>
            <div
              className={`dollar-left ${totalLeft <= 0 ? 'negative' : ''}`}
              title={quotaExhausted ? "额度已用尽：可升级专业版，或改用其他账号" : undefined}
            >
              {totalLeft < 0 ? '超支' : '剩'} ${Math.abs(totalLeft).toFixed(2)}
            </div>
          </div>
        ) : (
          // 普通模式
          <>
            <div className="usage-info">
              <span className="usage-text">
                <strong>{hasUsage ? Math.round(totalUsed) : "-"}</strong> / {hasUsage ? totalLimit : "-"}
              </span>
              <span
                className={`usage-left${quotaExhausted ? " exhausted" : ""}`}
                title={quotaExhausted ? "额度已用尽：可升级专业版，或改用其他账号" : undefined}
              >
                {quotaExhausted ? "额度已用尽" : `剩余 ${hasUsage ? Math.round(totalLeft) : "-"}`}
              </span>
            </div>
            <div className="usage-bar-mini">
              <div
                className="usage-bar-fill-mini"
                style={{ width: `${Math.min(usagePercent, 100)}%`, background: getUsageColor() }}
              />
            </div>
            {isCreditsBilling && (generalTotal > 0 || workTotal > 0) && (
              <div className="usage-split">
                {generalTotal > 0 && <span>通用 剩 {Math.round(generalLeft)}</span>}
                {workTotal > 0 && <span>Work 剩 {Math.round(workLeft)}</span>}
              </div>
            )}
          </>
        )}
      </div>

      <div className="list-item-reset">
        <span className="reset-label">添加时间</span>
        <span className="reset-date">{formatCreatedDate(account.created_at)}</span>
      </div>

      <div className="list-item-status">
        <span className={`status-dot ${isTokenExpired ? "expired" : "normal"}`}></span>
        <span>{isTokenExpired ? "过期" : "正常"}</span>
      </div>

      <div className="list-item-actions">
        <button
          className="action-btn"
          title="更多操作"
          onClick={(e) => {
            e.stopPropagation();
            onContextMenu(e, account.id);
          }}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor">
            <circle cx="12" cy="5" r="2"/>
            <circle cx="12" cy="12" r="2"/>
            <circle cx="12" cy="19" r="2"/>
          </svg>
        </button>
      </div>
    </div>
  );
}
