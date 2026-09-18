# AGENTS.md — Trae账号管理（trae-cc）

> 面向 AI 编码助手的工作指南：项目结构、构建方式、关键机制、编码约定与安全边界。
> 文中行数、命令数量等均取自对仓库的实测（2026-09）；改动代码前请先读完第 4、5、6 节。

## 1. 项目速览

- **名称**：Trae账号管理（应用标识 `com.hhj.trae-cc`），当前版本 **1.0.5**（三处同步：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`）
- **定位**：Windows 桌面工具，管理多个 Trae IDE 账号——账号存储、一键切换（改写 IDE 登录态与机器码）、用量查询/统计图表、每日签到（手动 + 开机自动）、机器码管理、快速注册、隐私模式写入
- **架构**：Tauri 2 应用 = React 19 前端（`src/`）+ Rust 后端（`src-tauri/src/`）。前后端仅通过 `#[tauri::command]` 通信，共注册 **53 个命令**（lib.rs 49 + quick_register_backend 4）；前端统一走 `src/api.ts` 的 invoke 封装
- **目标平台**：仅 Windows 10/11 为完整实现（大量 `cfg(windows)` 分支）。例外：`autostart.rs` 的 macOS 分支是真实实现（LaunchAgents），其余 macOS/Linux 代码仅为可编译桩

## 2. 技术栈与关键依赖

| 层 | 技术 |
|---|---|
| 前端 | React 19、TypeScript ~5.8、Vite 7、recharts（图表）、纯 CSS（无组件库） |
| 后端 | Rust 2021、Tauri 2.2、tokio、reqwest（cookies/json）、rusqlite（读写 Trae 的 state.vscdb）、winreg + windows-sys（注册表/窗口/进程）、warp（本地登录回调服务器）、fern + log（日志）、directories（数据目录定位）、uuid、chrono |
| 插件 | tauri-plugin-opener / dialog / updater / log |
| 窗口能力 | `capabilities/default.json`（main 窗口）+ `capabilities/login.json`（trae-login 窗口，仅允许 close/set-focus）；新增 webview 窗口需同步配置 capability |

## 3. 构建与运行

```bash
npm install                # 安装前端依赖
npm run tauri dev          # 开发模式（Vite devUrl: http://localhost:1420）
npm run tauri build        # 生产构建（先 tsc && vite build，再 cargo build）
```

- **环境变量注入机制**：快速注册后端配置经 `VITE_QUICK_REGISTER_API_BASE` / `VITE_APP_ID` / `VITE_APP_SECRET` 三个变量注入。`src-tauri/build.rs` 会从当前目录起向上递归查找 `.env` 并逐行输出 `cargo:rustc-env`，Rust 侧用 `option_env!` 读取，前端经 Vite 读取。复制 `.env.example` 为 `.env` 后填写。
- 未配置时 `quick_register_backend.rs` 使用代码内兜底值（含默认 API Base `https://hhxyyq.online` 与硬编码 APP_SECRET）。**该兜底密钥已在仓库内明文，勿当成机密，但也勿再扩散。**
- `tauri.conf.json` 中 `bundle.active = false`：默认构建不产出安装包，需要时临时开启，**勿提交开启状态**。
- 无 lint 配置；根目录的 `check_quota*.mjs/.py`、`test_trae_signup.mjs`、`inspect_signup.cjs` 是手动调试脚本，不是测试套件。Rust 侧有少量 `#[cfg(test)]` 单元测试（`device_id` / `checkin_guard` / `device_reset`，覆盖设备号派生与脱敏、冷却策略表与本地日界钳制、aha 嵌套键外科删除），用 `cargo test --lib` 运行。
- 修改注册表 MachineGuid 需管理员权限，调试相关功能时请以管理员身份运行（非管理员时写入静默失败，见 5.2）。
- **构建必须走 Tauri CLI，不要用裸 `cargo build --release`**：裸 cargo 拿不到 CLI 注入的环境变量，产出的是「开发模式」二进制——运行时去连 Vite 开发服务器 `localhost:1420` 而非加载内嵌前端，界面报 `ERR_CONNECTION_REFUSED`「无法访问此页面」。识别特征：体积少约 0.37MB（正是内嵌前端资源大小，7.56MB → 7.19MB），且应用日志停在 `Initializing account manager...` 之后无任何记录（前端未加载）。正确产物约 **7.56MB**（1.0.5 + 签到优化后实测值；随功能增长会变，判断标准是「是否少了约 0.37MB」而非绝对值）。
- 链接阶段偶发失败（`link.exe` 退出码 `0xc0000142`，DLL 初始化失败，多见于系统资源紧张或杀软干扰）时，库本身已编译成功，直接重跑同一条构建命令即可通过。

## 4. 目录结构与核心文件

```
trae-cc/
├── src/                        # React 前端
│   ├── App.tsx                 # 主页面：账号列表（卡片/列表）、批量操作、弹窗编排（⚠ 有效 952 行）
│   ├── api.ts                  # 所有 invoke 调用的 TS 封装（改动命令签名时同步更新）；顶部另有 checkApiConfig() 读取 Vite 环境变量校验快速注册配置
│   ├── types/index.ts          # Account / UsageSummary / AppSettings 等共享类型
│   ├── types/errorCodes.ts     # 后端错误码 → 前端提示映射
│   ├── hooks/useThemeColors.ts # 主题色 Hook
│   ├── App.css                 # ⚠ 全部样式，有效 5275 行，严重超限，新增样式优先放组件级 CSS
│   ├── pages/                  # Stats（用量图表）、Settings、About
│   └── components/             # AccountCard、AddAccountModal、QuickRegisterModal、DashboardWidgets、DetailModal 等
├── src-tauri/
│   ├── build.rs                # 向上递归查找 .env，输出 cargo:rustc-env 注入编译期变量
│   ├── src/lib.rs              # 应用入口：49 个本地命令 + AppState + 浏览器登录/open_pricing 注入脚本（⚠ 有效 1532 行，
│   │                           #   其中 build_browser_login_script 的 JS 字符串约 480 行，勿再继续膨胀）
│   ├── src/account/            # 账号域：account_manager.rs（CRUD/切换/刷新/签到落盘，⚠ 有效 1160 行）、types.rs
│   ├── src/api/                # Trae API 客户端：trae_api.rs（双认证 + 多端点回退，有效 784 行，临近上限）、cn_credits.rs（CN 积分钟额度查询/解析 + 通用/Work 拆分）、checkin.rs（每日签到：状态/领取 + 判定 + 冷却准入 + 重试轮次）、checkin_guard.rs（冷却策略表 + 防重入）、device_id.rs（签到设备号派生/脱敏）、types.rs
│   ├── src/machine.rs          # 机器码（注册表 MachineGuid）、Trae 路径扫描、进程 kill/open、写 storage.json（有效 665 行）
│   ├── src/device_reset.rs     # aha 层设备标识重置（aha/TinyStorage 的 aha.device.device_id，原子写回）
│   ├── src/privacy.rs          # 向 state.vscdb 的 ItemTable 读写 icube.privacy.mode
│   ├── src/browser_auto_login.rs  # 邮箱密码自动填充登录 webview
│   ├── src/quick_register_backend.rs # 现行快速注册（代理自建后端 API，4 个命令，Result<T, String> 风格）
│   ├── src/custom_tempmail.rs  # 自定义临时邮箱客户端（代码已完成，但对应命令仍是「开发中」桩，未接线）
│   ├── src/quick_register_simple.rs  # ⚠ 旧版注册，626 行死代码；lib.rs 中 `mod quick_register_simple;` 已注释，不参与编译，勿引用
│   ├── src/logger.rs / autostart.rs  # 日志（fern，5MB×5 轮转）与开机自启（--silent 无头模式）
│   └── tauri.conf.json         # 窗口配置、updater 端点与签名公钥
└── .env.example                # 快速注册 API 配置模板
```

**文件规模现状**（有效代码行，即不含空行/注释，与第 6 节 800 行上限同一口径）：

| 文件 | 有效行 | 状态 |
|---|---|---|
| src/App.css | 5275 | ⚠ 严重超限 |
| src-tauri/src/lib.rs | 1532 | ⚠ 超限 |
| src-tauri/src/account/account_manager.rs | 1160 | ⚠ 超限 |
| src/App.tsx | 1040 | ⚠ 超限 |
| src-tauri/src/api/trae_api.rs | 784 | 临近上限 |
| src/components/DashboardWidgets.tsx | 695 | 临近上限 |
| src-tauri/src/machine.rs | 665 | 尚可，但总行数 910 偏大 |
| src-tauri/src/api/checkin.rs | 490 | 尚可（含新增冷却/重试轮次） |
| src/components/QuickRegisterModal.tsx / Settings.tsx | 642 / 627 | 观察名单 |
| src-tauri/src/api/checkin_guard.rs / device_id.rs / device_reset.rs | 158 / 95 / 73 | 新增模块（含单元测试） |

## 5. 关键机制（改动前必读）

### 5.1 数据存储路径

| 数据 | 位置（Windows） | 代码 |
|---|---|---|
| 账号库 accounts.json | `%APPDATA%\hhj\trae-cc\data\accounts.json` | `AccountManager::get_data_path` |
| 设置 settings.json | `%APPDATA%\hhj\trae-cc\config\settings.json`（注意：与 accounts.json **不同目录**） | lib.rs `get_settings_path` |
| 日志 app.log | `%LOCALAPPDATA%\HHJ\TraeCC\data\logs\app.log`（注意：logger.rs 用的是另一套 `ProjectDirs("com","HHJ","TraeCC")`，大小写与连字符不同） | logger.rs |

- 账号与设置由 `AccountManager` 统一管理。**密码目前明文存储于 accounts.json**，涉及持久化的改动需知悉此现状；敏感信息禁止落日志。
- 每个账号都有 `machine_id` 字段：`AccountManager::new` 启动时为缺失账号自动补随机 UUID；`bind_account_machine_id` 则把**当前系统** MachineGuid 绑定到账号。

### 5.2 切换账号流程（时序敏感，历史上修过竞争与 telemetry 覆盖两类 bug，改动需谨慎回归）

**命令层**（lib.rs `switch_account`）：

1. 持 `account_manager` 锁调用 `manager.switch_account()`（仅此段持锁，内部含 Token 刷新的网络请求——**有意为之的原子性设计，是"网络请求不持锁"规则的唯一例外**）；
2. 锁外根据 `settings.privacy_auto_enable` 决定后续：启动 Trae → 等 state.vscdb 生成（200ms 轮询，最长 30 秒）→ `privacy.rs` 写入隐私模式（写后读回验证）→ 二次重启 Trae；未启用隐私模式则直接启动 Trae。

**manager 层**（account_manager.rs）：校验非当前账号 → 检查 Token 过期（过期且有 Cookies 则调 `GetUserToken` 刷新并落盘）→ 组装 `TraeLoginInfo` → `machine::switch_trae_account(info, account.machine_id, auto_start=false)` → 用账号 `machine_id` 调 `set_machine_guid` 更新注册表（**失败静默忽略**，因需管理员权限）→ 更新 active/current 账号指针。

**machine 层**（machine.rs `switch_trae_account`，步骤编号即代码注释顺序）：杀 Trae → **⓪a `device_reset::reset_aha_device_id` 清除 aha 层设备标识（必须在 kill 之后，否则被运行中的进程回写覆盖；best-effort 不阻断）** → ①写 `machineid` 文件 → ②删 `state.vscdb` → ③删 `state.vscdb.backup` → ④删 `Local State` → ⑤删 `IndexedDB` → ⑥删 `Local Storage` → ⑦删 `Session Storage` → ⑧删 `Network/Cookies` → ⑨删 `Network/Cookies-journal` → ⑩重建 `User/globalStorage/storage.json`：先移除 4 个 iCube* 旧登录键，再重写 telemetry 三件套（key 为 `telemetry.devDeviceId` / `telemetry.machineId` / `telemetry.sqmId`，其中 machineId 由 machineid 文件内容派生）→ ⑪`write_trae_login_info` 写入新登录信息 → ⑫按需启动 Trae。

`clear_trae_login_state` 不先 `kill_trae`，因此**运行中执行时跳过 aha 重置并告警**（不新增 kill 行为，避免改变其既有语义）。aha 层细节见 §5.8。

### 5.3 Token 生命周期

- 优先 JWT：请求头为 `Authorization: Cloud-IDE-JWT <token>`；401 时用 Cookies 调 `GetUserToken` 刷新并回写账号。
- 查询用量入口为 lib.rs 的 `fetch_usage_for_account`（401 判断靠错误字符串包含 "401"，改动错误信息格式时注意）。
- API 端点（trae_api.rs）：本项目仅适配国内版 Trae CN，三个常量 `API_BASE_US` / `API_BASE_SG` / `API_BASE_UG` 统一指向 `api.trae.com.cn`（CN 版为单域名体系，国际版的分区域域名已弃用）；`machine.rs` 写 storage.json 时 host 亦统一为 `api.trae.com.cn`。
- **CN 版为积分钟模型（重要）**：`/trae/api/v1/pay/user_current_entitlement_list` 与 v1 `ide_user_ent_usage` 对 CN 账号恒返回 0（国际版美元/请求次数模型），真实额度只在 **v2 端点** `/trae/api/v2/pay/ide_user_ent_usage`，字段为 `usage_summary.{total_amount,consumed_amount}` 与各礼包 `quota.credits_limit` / `usage.credits_amount`。查询入口为 `api/cn_credits.rs`（`try_credits_usage` 在 `trae_api.rs` 中优先尝试，非积分钟或失败才回退旧解析）。改额度相关代码前先读 `cn_credits.rs`。
- **积分按适用产品拆分（通用 / Work 专属）**：官方规则为「通用积分（TraeCode 与 TraeWork 通用）」与「Work 专属积分（仅 TraeWork）」两类。接口不直接给分类，`cn_credits.rs` 按礼包 `entitlement_base_info.available_endpoint` 归类：`1` = Work 专属，其余（`0`/缺省）= 通用；判据是官方「老用户升级福利 = 2000 通用 + 2000 Work」「每月登录/每日签到 = 通用」与实测礼包 pid（208/221 → endpoint 0，209 → endpoint 1）的对应关系。前端在卡片/列表/详情三处分开显示，字段为 `credits_general_*` / `credits_work_*`。

### 5.4 浏览器登录（webview 凭据捕获）

`start_browser_login` 打开指向 www.trae.com.cn/login 的 `trae-login` webview 并注入 `build_browser_login_script` 的 JS：hook fetch/XHR 请求体与 `HTMLInputElement.prototype.value` setter、递归扫描 shadowRoot/iframe 捕获输入，自动点击 Cookie 同意条；凭据 POST 到 warp 起的 `127.0.0.1:随机端口/callback`。整体 300 秒超时，多路 oneshot（取消 / 窗口关闭）竞争取消；凭据脱敏后落日志。另有 `browser_auto_login.rs`（邮箱密码自动填充）与 `open_pricing`（清 Cookie → 写入账号 Cookie → 跳转 pricing 页）两个独立注入点，勿混淆。

### 5.5 快速注册三条路径（状态不同，勿混淆）

| 路径 | 状态 |
|---|---|
| `quick_register` 命令 | 已禁用，直接返回错误提示 |
| `quick_register_with_custom_tempmail` 命令 | 「开发中」桩（`custom_tempmail.rs` 客户端代码已写完但未接线） |
| `quick_register_backend.rs` 4 个命令（create_task / get_status / claim_resource / get_stats） | **现行可用**；前端 `api.ts` 的 `pollTaskVerification` 以 3 秒间隔轮询，驱动「建任务 → 扫码验证 → 领取账号」 |

### 5.6 无头模式（--silent）

进程参数含 `--silent` 时不初始化 Tauri：隐藏控制台窗口（Windows）→ `handle_silent_start`（刷新所有账号 Token → 自动签到（方案B，见 5.7）→ 若 Trae 未运行且当前账号有 JWT，则把登录态写入 storage.json 保持同步）→ `process::exit(0)`。开机自启注册表项（HKCU Run，键名 `Trae账号管理`）即以 `--silent` 方式拉起。

### 5.7 每日签到（手动 + 方案B 自动）

- **接口**：`api.trae.cn` 的 `/trae/api/v2/ug/checkin_credits/status`（查今日状态）与 `/claim`（领取），请求体 `{}`，头为 JWT + UA `Trae/0.1.52` + `X-User-Region: CN`，实现见 `api/checkin.rs`。**`claim` 必须带 `X-Device-Id`（实测缺省返回 9004「order parameters incorrect」）**；实测 `api.trae.com.cn` 未提供该接口（claim 返回 404），代码里的双端点回退仅用于网络容错。
- **判定规则（勿改松）**：「已签到」判定须同时覆盖「已签到」与「已经签到」两种措辞——服务端 9095 原文为「当前设备今日已经签到」，而「已经签到」不含子串「已签到」，只匹配前者会误判为失败。签到按**设备维度**做每日去重（9095）：同一 X-Device-Id 当日已签即拒绝，提示「本设备今日已签到」而非失败。
- **设备号隔离（`Account.device_id`，完全独立于 `machine_id`）**：`X-Device-Id` 用 `api/device_id.rs` 派生的 16 位设备号，`machine_id` 不再参与签到（避免「换签到设备号」连带改写 IDE 机器身份）。`device_id` 派生一次即落盘（sha256 域前缀 `trae-cc:device-id:v1:` + seed），启动回填**只补空值、绝不无条件重算**；`resolve_device_id` 三层回退（落盘值 → user_id 派生 → 内部 id 派生），**禁止固定种子兜底**（多个无 user_id 账号会撞号互踢 9095）。日志一律 `mask_device_id` 脱敏。右键菜单「重置设备标识」（`reset_account_device_id` 命令）换随机号 + 清冷却 + 清日期，用 `try_acquire` 防重入。
- **冷却状态机（`api/checkin_guard.rs`）**：失败按数值 `code`/HTTP 状态码优先分类（`classify_error`），子串仅兜底（数值优先避免「消息里恰好含数字」的误判白等）。策略表：9074→10min、HTTP 429/404→60s、5xx/网络→2min、1005 权益不足→12h、401/1001→`i64::MAX`(reason=auth_expired)；9095/9004/活动未开放→不冷却。`until` 落盘与读取两侧都钳到**本地时区**当日 23:59:59（按 UTC 日界钳会钳到北京时间 07:59:59，「跨天必重试」破洞）；`i64::MAX` 是「需人工介入」哨兵、无时钟语义，前端按 `cooldown_reason` 渲染不读数值。Token 刷新/重新登录**仅清 auth_expired** 冷却。
- **并发防重入**：三入口（`checkin_account` / `checkin_all_accounts` / `auto_checkin`）共用 `checkin_guard::try_acquire`（进程级 `static`，覆盖 `--silent` 自建 manager 的场景），持锁到全部结束；持锁期间手动签到返回「签到进行中」是有意取舍。`--silent` 与 GUI 双进程仍可能并发，不做跨进程文件锁（最坏得到 9095/9074，且有冷却兜底）。
- **语义分层（勿破坏）**：冷却管「跨批次记忆」，批次内重试轮次管「批次内退避」。批次启动时**快照一次**已有冷却做准入、运行期间不重新准入；批次内失败**不落盘冷却**，全部轮次结束后 `apply_checkin_outcomes` **统一写盘一次**（成功/Already 写日期 + 失败项写冷却；同一账号重试成功时后一结果覆盖前一冷却）。
- **自动签到批次重试（仅 `auto_checkin_pending`）**：第 1 轮全量 → 收集 `rate_limited` 账号 → 睡 30s → 第 2 轮 → 睡 90s → 第 3 轮 → 结束；等待次数与账号数无关（最坏 ≈ 2 分钟）、仅存在可重试项才 sleep。手动入口**不重试**（单轮结束即落盘）。
- **自动签到（方案B）**：启动时（前端挂载后、以及 `--silent` 无头启动）调用 `auto_checkin` 命令，只处理 `last_checkin_date != 今天` 的账号；成功/已签到后写日期，避免一天多次开机重复请求；失败不写日期，下次启动自动重试。日期字段是 accounts.json 的兼容新增字段（`Account.last_checkin_date`）。
- **手动入口**：右键菜单「签到」（`checkin_account`）、工具栏「全部签到」（`checkin_all_accounts`）。冷却账号返回 `cooldown` 状态（info 提示、不进失败汇总；自动签到维持静默）。**防双签红线：`checkin_with_token` 中 claim 前必先查 status**（重置设备号后该账号今日已真签到会被短路为 Already），该顺序禁止改动、禁止绕过。签到不触碰 IDE 文件与机器码，是纯网络请求。

### 5.8 aha 层设备标识（`device_reset.rs`）

`%APPDATA%\Trae CN\aha\TinyStorage` 内含 `aha.device.device_id`（加密 blob）。`reset_aha_device_id` 定位 `tiny_storage_data` **子对象**后仅移除其内该键，保留同层 `aha_access_policy` / `aha_doctor_domain` / `aha_last_renderer_oom`（勿在顶层 remove——会静默落空）；写回原子化（临时文件 + rename），解析失败保持原样（best-effort，绝不让重置变成数据丢失）。**不能写「合法值」**：加密密钥不在我们掌握中，写入非法值会让客户端解密异常，删除永远比伪造安全。登录态与 aha 无关，**不需要重新登录**。`aha\Remote_State`（纯 feature flag）与 `ahanet\prefs\local_prefs.json`（纯 Chromium http_server_properties）无设备标识，不动。`Partitions\trae-webview\` 的 Cookie/Local Storage/IndexedDB **暂不纳入**——需先对比切换前后 mtime/大小确认有写入，无证据不盲删。

## 6. 编码约定

- **注释**：新增/修改函数时写函数级注释，解释「为什么这样做」（动机与权衡），而非复述「做了什么」。
- **文件规模**：单文件有效代码（不含空行/注释）≤ 800 行；预计超 700 行即应拆分。超限与临近文件清单见第 4 节表格，**新增逻辑优先放入对应领域模块，不要继续往这些文件里加**。
- **复用优先**：新功能先检查 `account/`、`api/`、`machine.rs` 中是否已有可复用函数，再考虑新模块。
- **新增命令 checklist**（三步缺一不可）：
  1. Rust 侧写 `#[tauri::command]` 并在 lib.rs 的 `tauri::generate_handler!` 列表注册；
  2. `src/api.ts` 加 TS 封装（联网命令用 `invokeNetwork`——会先检查 `navigator.onLine`；本地命令用 `invoke`）；
  3. `src/types/index.ts` 同步共享类型。

  最小示例：

  ```rust
  // src-tauri/src/lib.rs
  #[tauri::command]
  async fn my_command(state: State<'_, AppState>) -> Result<MyData> { /* ... */ }
  // 并在 generate_handler![ ... my_command, ] 中注册
  ```

  ```ts
  // src/api.ts
  export async function myCommand(): Promise<MyData> {
    return invokeNetwork("my_command");
  }
  ```

- **已知陷阱（api.ts 死/重复封装，勿调用）**：
  - `addAccount()` → 后端从未注册 `add_account` 命令；添加账号请用 `addAccountByToken` / `addAccountByEmail`。
  - `updateCookies()` → 调用未注册的 `update_cookies` 命令，同为死封装。
  - `setActiveAccount()` 与 `switchAccount()` 是同一命令 `switch_account` 的重复封装，新代码统一用 `switchAccount`。
- **Rust 并发**：共享状态走 `AppState` 的 `tokio::Mutex`，网络请求不得持锁，参考 `get_account_usage` 的三段式写法（取锁读 → 无锁网络 → 取锁写）。**唯一有意的例外是 `switch_account` 持锁段**（见 5.2），不要把它当范本，也不要在持锁区新增更多网络调用。
- **错误处理**：Rust 错误统一经 `anyhow` → `ApiError { message }` 返回前端；`quick_register_backend.rs` 历史原因用 `Result<T, String>`，新命令请用 `ApiError` 路线。
- **平台分支**：Windows 为真实实现，其他平台仅保留可编译的桩（`autostart.rs` 的 macOS 分支除外）。
- **日志**：用 `log::info!/error!`；密码、Token、Cookies 等敏感信息禁止落日志（参考登录回调中的脱敏处理）。

## 7. 已知限制（已确认但本次不修）

1. **GUI 与 `--silent` 双进程写 `accounts.json`**：GUI 内存里的 store 会在下次 `save_store()` 覆盖 silent 进程刚写的 `last_checkin_date`/冷却。已用「批次单次写盘」缩小窗口，但不做读-改-写合并。
2. **子串匹配分类仍可能误判**：数值 `code`/HTTP 状态码优先已把主要路径覆盖，子串只在解析失败时兜底；消息里恰好含「9074」「401」这类数字的极端情况仍可能误判，代价是白等一次冷却。
3. **自动签到被 busy 跳过时 `autoCheckinRanRef` 已置位**（App.tsx），该会话不再重试；可接受。
4. **`machine.rs` 的 `switch_trae_account` 与 `clear_trae_login_state` 有约 155 行重复清理逻辑**待重构（超出「优化签到系统」范围且风险高）。
5. **`--silent` 进程寿命变长**：批次重试轮 + 单请求 20s 超时使无头进程从秒级延长到最坏约 2 分钟（仅当存在可重试账号时发生）。开机自启场景可接受，属有意行为变化。
6. **导出/导入原样携带 `device_id`**：账号导入第二台机器后两边同号，同日签到必有一边 9095。这是「同账号同设备身份」的有意语义，不是 bug，勿按缺陷修复。`checkin_cooldown` 同样随导出携带，跨机保留无害。
7. **每账号独立设备号使 9095 从常态拦截变为近乎绝迹（有意的行为变化，非副作用）**：设备号按 `user_id` 派生后，「一台电脑一天只能签一个账号」的设备维度限制在本工具内被结构性改变。与「不做批量重置」边界声明并存的前提是：设备号静态不旋转、重置仅限单账号自救。连带风险：同一 IP 下多设备号每日签到可能促使服务端把风控升级到账号/IP 维度，届时 9074 冷却策略需要重估。
8. **`Partitions\trae-webview\` 的设备标识清理待验证**：该目录有独立于默认 profile 的 `Network\Cookies` / `Local Storage` / `IndexedDB`，而 `open_pricing` 正是「清 Cookie → 写账号 Cookie → 跳转」的注入点。需先对比切换前后 mtime/大小确认确有写入，再决定是否纳入，**无证据不盲删**。

## 8. 安全与边界（不要做的事）

- 不要提交真实 `.env` 或快速注册密钥；不要改动 updater 的 `pubkey` 与端点，除非用户明确要求。
- 不要删除/重写 `accounts.json`、`settings.json` 的兼容字段；导入导出格式变更需保持向后兼容。
- 不要执行会改本机注册表、杀进程的操作来「验证」代码——这些副作用仅供最终用户在其机器上触发。
- 注入脚本（`build_browser_login_script`）只在 trae.com.cn 域名下工作，修改时保持域名检查与凭据脱敏逻辑。
- 本项目仅适配国内版 Trae CN：数据目录 `%APPDATA%\Trae CN`、进程名 `Trae CN.exe`、安装扫描路径与 API 域名均已按 CN 版硬编码，不要再引入国际版（trae.ai）的路径或域名。
- README.md 面向最终用户，仅保留技术性内容；技术细节以本文件（AGENTS.md）为准，不要向 README 或其他技术文档追加营销、推广内容。
- 本项目为个人学习研究用途的工具；协助时遵循仓库 README 的免责声明边界，不扩展用于倒卖或批量绕过授权的用途。

## 9. 提交规范

- 遵循 [Conventional Commits](https://www.conventionalcommits.org/)：`feat:` / `fix:` / `refactor:` / `docs:` / `chore:` 等前缀（仓库历史中也有带中文 scope 的写法，如 `feat(配置):`）。
- 版本号同时维护 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处；发布后同步 `CHANGELOG.md`（Keep a Changelog 格式。注意：当前 CHANGELOG 停在 1.0.4，已滞后于三处版本号的 1.0.5）。
