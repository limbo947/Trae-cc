import { invoke } from "@tauri-apps/api/core";
import type { Account, AccountBrief, AppPaths, AppSettings, CheckinResult, RuntimeStatus, UpdateInfo, UsageSummary, UsageEventsResponse, UserStatisticData, TraeworkOverview, TraeworkUidEvidence, TraeworkActionResult, TraeworkReconcile, TraeIdeReadOutcome } from "./types";

function checkNetwork() {
  if (typeof navigator !== 'undefined' && !navigator.onLine) {
    throw new Error("网络连接已断开，请检查网络设置");
  }
}

async function invokeNetwork<T>(cmd: string, args?: any): Promise<T> {
  checkNetwork();
  return invoke(cmd, args);
}

// 添加账号（通过 Token，可选 Cookies）
export async function addAccountByToken(token: string, cookies?: string): Promise<Account> {
  return invokeNetwork("add_account_by_token", { token, cookies });
}

// 添加账号（通过邮箱密码登录）
export async function addAccountByEmail(email: string, password: string): Promise<Account> {
  return invokeNetwork("add_account_by_email", { email, password });
}

export async function startBrowserLogin(): Promise<void> {
  return invokeNetwork("start_browser_login");
}

export async function finishBrowserLogin(): Promise<Account> {
  return invokeNetwork("finish_browser_login");
}

export async function cancelBrowserLogin(): Promise<void> {
  return invoke("cancel_browser_login");
}

// 浏览器自动登录
export async function browserAutoLogin(email: string, password: string): Promise<Account> {
  return invokeNetwork("browser_auto_login_command", { email, password });
}

// 下载并运行更新安装包（Windows: .msi）
export async function downloadAndRunInstaller(url: string): Promise<string> {
  return invokeNetwork("download_and_run_installer", { url });
}

// 删除账号
export async function removeAccount(accountId: string): Promise<void> {
  return invoke("remove_account", { accountId });
}

// 获取所有账号
export async function getAccounts(): Promise<AccountBrief[]> {
  return invoke("get_accounts");
}

// 获取单个账号详情（包含 token）
export async function getAccount(accountId: string): Promise<Account> {
  return invoke("get_account", { accountId });
}

// 设置活跃账号
export async function setActiveAccount(
  accountId: string,
  options?: { force?: boolean }
): Promise<void> {
  return invoke("switch_account", { accountId, force: options?.force });
}

// 切换账号（设置活跃账号并更新机器码）
export async function switchAccount(
  accountId: string,
  options?: { force?: boolean }
): Promise<void> {
  return invoke("switch_account", { accountId, force: options?.force });
}

// 获取账号使用量
export async function getAccountUsage(accountId: string): Promise<UsageSummary> {
  return invokeNetwork("get_account_usage", { accountId });
}

// 更新账号 Token
export async function updateAccountToken(accountId: string, token: string): Promise<UsageSummary> {
  return invokeNetwork("update_account_token", { accountId, token });
}

// 刷新 Token
export async function refreshToken(accountId: string): Promise<void> {
  return invokeNetwork("refresh_token", { accountId });
}

export async function refreshTokenWithPassword(accountId: string, password: string): Promise<void> {
  return invokeNetwork("refresh_token_with_password", { accountId, password });
}

export async function loginAccountWithEmail(
  accountId: string,
  email: string,
  password: string
): Promise<UsageSummary> {
  return invokeNetwork("login_account_with_email", { accountId, email, password });
}

export async function updateAccountProfile(
  accountId: string,
  updates: { email?: string | null; password?: string | null }
): Promise<Account> {
  return invokeNetwork("update_account_profile", {
    accountId,
    email: updates.email ?? null,
    password: updates.password ?? null,
  });
}

// 导出账号
export async function exportAccounts(): Promise<string> {
  return invoke("export_accounts");
}

export async function exportAccountsToPath(path: string): Promise<void> {
  return invoke("export_accounts_to_path", { path });
}

// 导入账号
export async function importAccounts(data: string): Promise<number> {
  return invoke("import_accounts", { data });
}

export async function clearAccounts(): Promise<number> {
  return invoke("clear_accounts");
}

export async function getSettings(): Promise<AppSettings> {
  return invoke("get_settings");
}

export async function updateSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke("update_settings", { settings });
}

// 获取使用事件
export async function getUsageEvents(
  accountId: string,
  startTime: number,
  endTime: number,
  pageNum: number = 1,
  pageSize: number = 20
): Promise<UsageEventsResponse> {
  return invokeNetwork("get_usage_events", {
    accountId,
    startTime,
    endTime,
    pageNum,
    pageSize
  });
}

// 从 Trae IDE 读取当前登录账号
// 返回状态而非可空账号：null 无法区分「本机没登录」与「账号已在列表中」
export async function readTraeAccount(): Promise<TraeIdeReadOutcome> {
  return invoke("read_trae_account");
}

// ============ 机器码相关 API ============

// 获取当前系统机器码
export async function getMachineId(): Promise<string> {
  return invoke("get_machine_id");
}

// 重置系统机器码（生成新的随机机器码）
export async function resetMachineId(): Promise<string> {
  return invoke("reset_machine_id");
}

// 设置系统机器码为指定值
export async function setMachineId(machineId: string): Promise<void> {
  return invoke("set_machine_id", { machineId });
}

// 绑定账号机器码（保存当前系统机器码到账号）
export async function bindAccountMachineId(accountId: string): Promise<string> {
  return invoke("bind_account_machine_id", { accountId });
}

// ============ Trae IDE 机器码相关 API ============

// 获取 Trae IDE 的机器码
export async function getTraeMachineId(): Promise<string> {
  return invoke("get_trae_machine_id");
}

// 设置 Trae IDE 的机器码
export async function setTraeMachineId(machineId: string): Promise<void> {
  return invoke("set_trae_machine_id", { machineId });
}

// 清除 Trae IDE 登录状态（让 IDE 变成全新安装状态）
export async function clearTraeLoginState(): Promise<void> {
  return invoke("clear_trae_login_state");
}

// ============ Trae IDE 路径相关 API ============

// 获取保存的 Trae IDE 路径
export async function getTraePath(): Promise<string> {
  return invoke("get_trae_path");
}

// 设置 Trae IDE 路径
export async function setTraePath(path: string): Promise<void> {
  return invoke("set_trae_path", { path });
}

// 自动扫描 Trae IDE 路径
export async function scanTraePath(): Promise<string> {
  return invoke("scan_trae_path");
}

// ============ 礼包相关 API ============

// 获取用户统计数据
export async function getUserStatistics(accountId: string): Promise<UserStatisticData> {
  return invokeNetwork("get_user_statistics", { accountId });
}

// ============ 每日签到 API ============

// 单账号签到（手动）
export async function checkinAccount(accountId: string): Promise<CheckinResult> {
  return invokeNetwork("checkin_account", { accountId });
}

// 全部账号签到（手动，只签 TraeCode）
export async function checkinAllAccounts(): Promise<CheckinResult[]> {
  return invokeNetwork("checkin_all_accounts");
}

// 全部 TraeWork 账号签到（手动，TraeWork 面板按钮；凭据按快照解析，无凭据返回 skipped）
export async function traeworkCheckinAll(): Promise<CheckinResult[]> {
  return invokeNetwork("traework_checkin_all");
}

// 自动签到（方案B：仅今日未签到的账号，启动时静默调用）
export async function autoCheckin(): Promise<CheckinResult[]> {
  return invokeNetwork("auto_checkin");
}

// 重置单账号签到设备号（9095 自救：本地命令，不走联网检查）
export async function resetAccountDeviceId(accountId: string): Promise<void> {
  return invoke("reset_account_device_id", { accountId });
}

// 打开购买页面（内置浏览器，携带账号 Cookies）
export async function openPricing(accountId: string): Promise<void> {
  return invokeNetwork("open_pricing", { accountId });
}

// ============ 日志相关 API ============

// 获取最近日志
export async function getLogs(count: number): Promise<string[]> {
  return invoke("get_logs", { count });
}

// 导出日志
export async function exportLogs(path: string): Promise<void> {
  return invoke("export_logs_cmd", { path });
}

// 清空日志
export async function clearLogs(): Promise<void> {
  return invoke("clear_logs_cmd");
}

// 获取日志文件路径
export async function getLogFilePath(): Promise<string> {
  return invoke("get_log_file_path_cmd");
}

// 获取应用数据路径（账号库 / 设置 / 日志 / TraeWork 快照）
// 纯本地读路径，不走联网检查——断网时设置页的数据分组仍应可见
export async function getAppPaths(): Promise<AppPaths> {
  return invoke("get_app_paths");
}

// ============ 运行时状态 ============

// 进程状态（设置页状态面板）。纯本地系统调用，不走联网检查。
export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  return invoke("get_runtime_status");
}

// ============ 更新 ============

// 检查更新（返回 null 表示已是最新）
export async function checkUpdate(): Promise<UpdateInfo | null> {
  return invokeNetwork("check_update");
}

// 下载并安装更新（成功后由 updater 重启应用）
export async function installUpdate(): Promise<void> {
  return invokeNetwork("install_update");
}

// ============ TraeWork（TRAE SOLO CN）API ============
//
// TraeWork 走「登录态快照 / 恢复」，与 TraeCode 的切换机制完全不同，命令彼此独立。
// 这些命令都会杀/启客户端进程，单次耗时数十秒，前端必须给出进行中反馈并禁用重复触发。

// 概览：当前账号、进程状态、各槽位快照状态
export async function traeworkOverview(): Promise<TraeworkOverview> {
  return invoke("traework_overview");
}

// 识别当前 TraeWork 登录账号（证据链推导，可能置信度不足）
export async function traeworkDiscover(): Promise<TraeworkUidEvidence> {
  return invoke("traework_discover");
}

// TraeWork 账号积分余额
// 与 get_account_usage 是两条独立链路：它会现读快照（或当前账号的实时登录态）解出凭据，
// 而不是读账号库里存的 JWT/Cookies——TraeWork 账号那两项恒为空
export async function traeworkCredits(accountId: string): Promise<UsageSummary> {
  return invokeNetwork("traework_credits", { accountId });
}

// 保存当前登录态（关客户端 → 快照 → 登记账号 → 重启客户端）
export async function traeworkSaveCurrentLogin(
  uid?: string,
  name?: string,
): Promise<TraeworkActionResult> {
  return invoke("traework_save_current_login", { uid, name });
}

// 切换到指定 TraeWork 账号
export async function traeworkSwitchAccount(accountId: string): Promise<TraeworkActionResult> {
  return invoke("traework_switch_account", { accountId });
}

// 删除某账号的快照，返回释放的字节数
export async function traeworkDeleteSnapshot(accountId: string): Promise<number> {
  return invoke("traework_delete_snapshot", { accountId });
}

// 移除 TraeWork 账号：删账号记录并一并删掉磁盘快照（与「删除快照」不同，后者只删文件、
// 账号仍留在列表里变成「无快照」条目，而 TraeWork 账号在账号管理页不可见、没有别的删除入口）
export async function traeworkRemoveAccount(accountId: string): Promise<TraeworkActionResult> {
  return invoke("traework_remove_account", { accountId });
}

// 设置 TraeWork 可执行文件路径
export async function traeworkSetPath(path: string): Promise<string> {
  return invoke("traework_set_path", { path });
}

// 自动扫描 TraeWork 可执行文件路径
export async function traeworkScanPath(): Promise<string | null> {
  return invoke("traework_scan_path");
}

// 修复槽位：把内容与目录名不符的快照按其中记录的真实账号 id 改名归位，并登记缺失账号
export async function traeworkReconcile(): Promise<TraeworkReconcile> {
  return invoke("traework_reconcile");
}
