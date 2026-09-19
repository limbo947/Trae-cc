// 账号简要信息
export interface AccountBrief {
  id: string;
  name: string;
  email: string;
  avatar_url: string;
  plan_type: string;
  is_active: boolean;
  created_at: number;
  machine_id: string | null;
  is_current: boolean; // 是否是当前 Trae IDE 正在使用的账号
  /** 今日是否已签到（后端按本地日期口径判定） */
  checked_in_today: boolean;
  /** 归属应用：traecode（Trae CN）或 traework（TRAE SOLO CN） */
  app: string;
  /** TraeWork uid；traecode 账号为 null */
  uid: string | null;
}

// 完整账号信息
export interface Account {
  id: string;
  name: string;
  email: string;
  avatar_url: string;
  cookies: string;
  jwt_token: string | null;
  token_expired_at: string | null;
  password?: string | null;
  user_id: string;
  tenant_id: string;
  region: string;
  plan_type: string;
  created_at: number;
  updated_at: number;
  is_active: boolean;
  machine_id: string | null;
  /** 签到设备号（X-Device-Id），与 machine_id 解耦 */
  device_id?: string | null;
  /** 签到冷却（跨批次记忆）：until 为 UTC 秒，reason 为机器可读码 */
  checkin_cooldown?: {
    until: number;
    reason: string;
  } | null;
  /** 归属应用：traecode（Trae CN）或 traework（TRAE SOLO CN） */
  app?: string;
  /** TraeWork uid */
  uid?: string | null;
  /** 快照槽名（默认等于 uid） */
  snapshot_slot?: string | null;
}

// ============ TraeWork（TRAE SOLO CN）============
//
// TraeWork 采用「登录态快照 / 恢复」而非改写登录态：其登录真源是 storage.json 与
// state.vscdb 双源，后者是带加密 secret storage 的 SQLite，写 JSON 覆盖不到。

/** uid 推导结果（置信度不足时 uid 为 null，前端不得猜测） */
export interface TraeworkUidEvidence {
  uid: string | null;
  confident: boolean;
  candidates: string[];
  reason: string;
}

/** 单个快照槽状态 */
export interface TraeworkSlotStatus {
  slot: string;
  exists: boolean;
  bak_exists: boolean;
  bytes: number;
  modified_at: number | null;
}

/** TraeWork 概览 */
export interface TraeworkOverview {
  current_slot: string | null;
  running: boolean;
  exe_path: string | null;
  snapshots: TraeworkSlotStatus[];
  orphan_slots: string[];
}

/** 单条执行步骤 */
export interface TraeworkStep {
  stage: string;
  status: "info" | "running" | "ok" | "warn" | "error" | "skip";
  message: string;
}

/** 保存 / 切换动作的结果 */
export interface TraeworkActionResult {
  message: string;
  steps: TraeworkStep[];
  account: AccountBrief | null;
}

/** 修复槽位结果 */
export interface TraeworkReconcile {
  message: string;
  steps: TraeworkStep[];
  /** 成功归位的 [原目录名, 纠正后的账号 id] */
  renamed: [string, string][];
  /** 未能自动处理的目录及原因 */
  skipped: string[];
  /** 修复后现存的槽位 */
  slots: string[];
  /** 清理白名单外历史文件释放的字节数 */
  freed_bytes: number;
}

// 使用量汇总
export interface UsageSummary {
  plan_type: string;
  reset_time: number;

  // Fast Request - 请求次数
  fast_request_used: number;
  fast_request_limit: number;
  fast_request_left: number;

  // Fast Request - 美元额度 (新账号 3 美元额度显示用)
  fast_dollar_used: number;
  fast_dollar_limit: number;
  fast_dollar_left: number;

  // Basic 额度 (基础 $3)
  basic_dollar_limit: number;
  basic_dollar_used: number;
  basic_dollar_left: number;

  // Bonus 额度 (奖励 $3)
  bonus_dollar_limit: number;
  bonus_dollar_used: number;
  bonus_dollar_left: number;

  // Extra Package
  extra_fast_request_used: number;
  extra_fast_request_limit: number;
  extra_fast_request_left: number;
  extra_expire_time: number;
  extra_package_name: string;

  // Slow Request
  slow_request_used: number;
  slow_request_limit: number;
  slow_request_left: number;

  // Advanced Model
  advanced_model_used: number;
  advanced_model_limit: number;
  advanced_model_left: number;

  // Autocomplete
  autocomplete_used: number;
  autocomplete_limit: number;
  autocomplete_left: number;

  // 是否是美元计费模式 (新账号)
  is_dollar_billing: boolean;

  // CN 版积分钟模型（v2 端点返回，国际版无此模式）
  is_credits_billing: boolean;
  credits_total: number;
  credits_used: number;
  credits_left: number;

  // CN 积分按适用产品拆分：通用积分（TraeCode/TraeWork 均可用）与 Work 专属积分（仅 TraeWork）
  credits_general_total: number;
  credits_general_used: number;
  credits_general_left: number;
  credits_work_total: number;
  credits_work_used: number;
  credits_work_left: number;
}

// 签到结果（rate_limited：服务端限流，今日未签到，稍后重试即可；
// cooldown：账号处于冷却期、本次未尝试，不是错误）
export interface CheckinResult {
  account_id: string;
  account_name: string;
  state: "ok" | "already" | "rate_limited" | "cooldown" | "failed";
  detail: string;
  /** 冷却截止时刻（UTC 秒）；i64::MAX 会超出 JS 安全整数，判定一律走 cooldown_reason */
  cooldown_until?: number | null;
  /** 冷却原因码：auth_expired / rate_limited / risk_control / server_error */
  cooldown_reason?: string | null;
}

// 使用事件
export interface UsageEvent {
  session_id: string;
  usage_time: number;
  mode: string;
  model_name: string;
  amount_float: number;
  cost_money_float: number;
  use_max_mode: boolean;
  product_type_list: number[];
  extra_info: {
    cache_read_token: number;
    cache_write_token: number;
    input_token: number;
    output_token: number;
  };
}

// 使用事件响应
export interface UsageEventsResponse {
  total: number;
  user_usage_group_by_sessions: UsageEvent[];
}

// API 错误
export interface ApiError {
  message: string;
}

export interface AppSettings {
  auto_refresh_enabled: boolean;
  privacy_auto_enable: boolean;
  auto_start_enabled: boolean;
}

// 用户统计数据
export interface UserStatisticData {
  UserID: string;
  RegisterDays: number;
  AiCnt365d: Record<string, number>;
  CodeAiAcceptCnt7d: number;
  CodeAiAcceptDiffLanguageCnt7d: Record<string, number>;
  CodeCompCnt7d: number;
  CodeCompDiffAgentCnt7d: Record<string, number>;
  CodeCompDiffModelCnt7d: Record<string, number>;
  IdeActiveDiffHourCnt7d: Record<string, number>;
  DataDate: string;
  IsIde: boolean;
}
