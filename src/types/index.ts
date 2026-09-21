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

/** 「从 Trae IDE 读取账号」的结果状态 */
export type TraeIdeReadStatus = "added" | "updated" | "exists" | "no_login";

/**
 * 「从 Trae IDE 读取账号」的结果。
 * message 由后端统一撰写：同一状态只应有一份文案，前端再拼一遍必然与后端口径漂移
 */
export interface TraeIdeReadOutcome {
  status: TraeIdeReadStatus;
  /** 涉及到的账号（added/updated/exists 时给出，用于直接刷新列表项） */
  account: Account | null;
  message: string;
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
  /** 展示名（登录态里的用户名；取不到为 null）。仅用于显示，不可用于判定身份 */
  display_name: string | null;
}

/** 单个快照槽状态 */
export interface TraeworkSlotStatus {
  slot: string;
  exists: boolean;
  bak_exists: boolean;
  bytes: number;
  modified_at: number | null;
  /**
   * 快照内凭据的到期时刻（RFC3339；解不出为 null）。
   *
   * 两者语义不同，别混用：access 过期只意味着切过去时客户端要静默续期一次，真正决定
   * 「该槽位必须重新登录」的是 refresh——所以警示口径按 refresh_expired_at 判定。
   */
  expired_at: string | null;
  refresh_expired_at: string | null;
  /**
   * 该槽位快照内的设备标识（`machineid` 文件，全量 UUID；展示时取短形）。null = 读不到。
   *
   * 「一账号一设备」是否真的成立，看的就是这个值在各槽位间是否互不相同——原来它只是
   * 一句设计假设，界面上无从验证。
   */
  machine_id: string | null;
  /**
   * 与本槽位设备标识**相同**的其它账号槽位（空数组 = 已隔离）。
   *
   * 非空意味着这些账号在 Trae 看来是同一台设备，正是设备维度限制的成因；
   * 切到该账号重新「保存当前登录态」即可生成独立标识。
   */
  shares_device_with: string[];
}

/** TraeWork 概览 */
export interface TraeworkOverview {
  current_slot: string | null;
  running: boolean;
  exe_path: string | null;
  snapshots: TraeworkSlotStatus[];
  orphan_slots: string[];
  /** 现场（客户端正在使用的那一份）的机器标识 */
  live_machine_id: string | null;
  /** 系统注册表 `MachineGuid`——设备标识真正被写进去、由客户端读取的那一层 */
  registry_machine_id: string | null;
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
// cooldown：账号处于冷却期、本次未尝试，不是错误；
// skipped：TraeWork 账号无可达凭据、本次未尝试，不是失败也没有冷却语义）
export interface CheckinResult {
  account_id: string;
  account_name: string;
  state: "ok" | "already" | "rate_limited" | "cooldown" | "skipped" | "failed";
  detail: string;
  /** 冷却截止时刻（UTC 秒）；i64::MAX 会超出 JS 安全整数，判定一律走 cooldown_reason */
  cooldown_until?: number | null;
  /** 冷却原因码：auth_expired / rate_limited / risk_control / server_error / credential_stale */
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
  /** 用量自动刷新的间隔（分钟）；0 视为不刷新 */
  refresh_interval: number;
  privacy_auto_enable: boolean;
  auto_start_enabled: boolean;
  /** 启动时静默自动签到（`--silent` 无头模式不受此开关影响） */
  auto_checkin_enabled: boolean;
  /**
   * 界面主题：`light` / `dark`。
   * null 表示从未设置过（老版本只存 localStorage），前端据此决定是否迁移。
   */
  theme: string | null;
  /**
   * TraeCode 账号页视图偏好：`grid`（卡片）/ `list`（列表）。
   * null 表示从未设置过（老配置），前端回落为 `grid`。
   */
  view_mode: string | null;
}

/**
 * 应用数据路径。四项均为「文件或目录的绝对路径」，可直接交给 revealItemInDir 定位。
 * 取不到的项为 null（例如 ProjectDirs 在该系统上不可用），前端按「未找到」渲染，
 * 不要让整组因为一项缺失而消失。
 */
export interface AppPaths {
  /** 账号库 accounts.json */
  accounts: string | null;
  /** 设置 settings.json */
  settings: string | null;
  /** 日志 app.log */
  logs: string | null;
  /** TraeWork 快照根目录 */
  traework_profiles: string | null;
}

/**
 * 运行时进程状态（后端 `get_runtime_status`）。
 *
 * 注意：`trae_running` 在非 Windows/macOS 上是常量 false（`machine.rs` 的占位实现），
 * 前端不应据此断言「未运行」——本项目只面向 Windows，该分支实际不会走到。
 */
export interface RuntimeStatus {
  trae_running: boolean;
  traework_running: boolean;
}

/**
 * 更新检查结果（后端 `check_update` 返回；没有新版本时为 null）。
 * 字段直接对应 tauri-plugin-updater 的 Update，body/date 可能缺失。
 */
export interface UpdateInfo {
  version: string;
  current_version: string;
  body: string | null;
  date: string | null;
}

/**
 * Toast 回调签名，与 `App.tsx` 的 `addToast` 对齐。
 * 放在共享类型里而不是某个页面的内部文件：设置页分区与 utils 下的导出/导入
 * 都要用它，各自复制一份会在改签名时漏掉一处。
 */
export type ToastFn = (
  type: "success" | "error" | "warning" | "info",
  message: string,
  duration?: number
) => void;

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
