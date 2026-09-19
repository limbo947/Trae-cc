# AGENTS.md — Trae账号管理（trae-cc）

> 面向 AI 编码助手的工作指南：项目结构、构建方式、关键机制、编码约定与安全边界。
> 文中行数、命令数量等均取自对仓库的实测（2026-09）；改动代码前请先读完第 4、5、6 节。

## 1. 项目速览

- **名称**：Trae账号管理（应用标识 `com.hhj.trae-cc`），当前版本 **1.0.5**（三处同步：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`）
- **定位**：Windows 桌面工具，管理多个 Trae 账号——**TraeCode（Trae CN）** 侧：账号存储、一键切换（改写 IDE 登录态与机器码）、用量查询/统计图表、每日签到（手动 + 开机自动）、机器码管理、隐私模式写入；**TraeWork（TRAE SOLO CN）** 侧：登录态快照 / 恢复式切换（2026-09 新增，见 §5.9）
- **架构**：Tauri 2 应用 = React 19 前端（`src/`）+ Rust 后端（`src-tauri/src/`）。前后端仅通过 `#[tauri::command]` 通信，共注册 **55 个命令**（其中 8 个 TraeWork 命令定义在 `src-tauri/src/traework/commands.rs`，注册仍在 lib.rs）；前端统一走 `src/api.ts` 的 invoke 封装
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

- **构建期环境变量注入**：`src-tauri/build.rs` 会从当前目录起向上递归查找 `.env`，逐行输出 `cargo:rustc-env`（Rust 侧用 `option_env!` 读取，前端经 Vite 读取）。随快速注册功能删除，该机制目前没有任何消费方，保留仅为后续需要编译期注入时备用。
- `tauri.conf.json` 中 `bundle.active = false`：默认构建不产出安装包，需要时临时开启，**勿提交开启状态**。
- 无 lint 配置；根目录的 `check_quota*.mjs/.py` 是手动调试脚本，不是测试套件。Rust 侧有 `#[cfg(test)]` 单元测试（`device_id` / `checkin_guard` / `device_reset` / `tc_crypto` / `traework::*`），用 `cargo test --lib` 运行（2026-09 实测 36 个全绿）。TraeWork 侧覆盖：槽位名路径穿越拒绝、进程名白名单、快照往返（边车清除 + 对称恢复）、`.bak` 轮转与回退、uid 证据链与并列时拒绝识别、当前账号标记 BOM 剥离。
- 修改注册表 MachineGuid 需管理员权限，调试相关功能时请以管理员身份运行（非管理员时写入静默失败，见 5.2）。
- **构建必须走 Tauri CLI，不要用裸 `cargo build --release`**：裸 cargo 拿不到 CLI 注入的环境变量，产出的是「开发模式」二进制——运行时去连 Vite 开发服务器 `localhost:1420` 而非加载内嵌前端，界面报 `ERR_CONNECTION_REFUSED`「无法访问此页面」。识别特征：体积少约 0.37MB（正是内嵌前端资源大小），且应用日志停在 `Initializing account manager...` 之后无任何记录（前端未加载）。正确产物约 **7.62MB**（2026-09-20 加入 TraeWork 槽位修复后实测值；此前 7.59MB 为 TraeWork 初版、7.49MB 为移除快速注册时的值；随功能增减会变，判断标准是「是否少了约 0.37MB」而非绝对值）。另注意 `tauri build` 会出现 `#[warn(linker_messages)]`（MSVC `link.exe` 的「正在创建库 …dll.lib 和对象 ….exp」经 stdout 被当警告），**非错误**，`cargo check` 为零警告不代表构建有问题。
- 链接阶段偶发失败（`link.exe` 退出码 `0xc0000142`，DLL 初始化失败，多见于系统资源紧张或杀软干扰）时，库本身已编译成功，直接重跑同一条构建命令即可通过。

## 4. 目录结构与核心文件

```
trae-cc/
├── src/                        # React 前端
│   ├── App.tsx                 # 主页面：账号列表（卡片/列表）、批量操作、弹窗编排（⚠ 有效约 1005 行）
│   ├── api.ts                  # 所有 invoke 调用的 TS 封装（改动命令签名时同步更新）
│   ├── types/index.ts          # Account / UsageSummary / AppSettings 等共享类型
│   ├── hooks/useThemeColors.ts # 主题色 Hook
│   ├── App.css                 # ⚠ 全部样式，严重超限（2026-09 已清除尾部扫码领号弹窗死样式，760 行），新增样式优先放组件级 CSS
│   ├── pages/                  # Stats（用量图表）、Settings、About
│   └── components/             # AccountCard、AddAccountModal、DashboardWidgets、DetailModal、TraeworkPanel（TraeWork 面板 + 同名 CSS）等
├── doc/                        # 方案与调研文档（含 TraeWork账号切换计划.md）
├── src-tauri/
│   ├── build.rs                # 向上递归查找 .env，输出 cargo:rustc-env 注入编译期变量（当前无消费方）
│   ├── src/lib.rs              # 应用入口：47 个本地命令 + AppState + 浏览器登录/open_pricing 注入脚本（⚠ 有效约 1500 行，
│   │                           #   其中 build_browser_login_script 的 JS 字符串约 480 行，勿再继续膨胀）
│   ├── src/traework/           # TraeWork（TRAE SOLO CN）账号切换：profile（常量+白名单 15 项）/snapshot（备份恢复原语+对称恢复）/proc（三级关闭）/locate（exe 五级发现）/uid（证据链+置信度门槛）/commands（7 个命令）
│   ├── src/account/            # 账号域：account_manager.rs（CRUD/切换/刷新/签到落盘，⚠ 有效 1160 行）、types.rs
│   ├── src/api/                # Trae API 客户端：trae_api.rs（双认证 + 多端点回退，有效 784 行，临近上限）、cn_credits.rs（CN 积分钟额度查询/解析 + 通用/Work 拆分）、checkin.rs（每日签到：状态/领取 + 判定 + 冷却准入 + 重试轮次）、checkin_guard.rs（冷却策略表 + 防重入）、device_id.rs（签到设备号派生/脱敏）、types.rs
│   ├── src/machine.rs          # 机器码（注册表 MachineGuid）、Trae 路径扫描、进程 kill/open、写 storage.json（有效 665 行）
│   ├── src/device_reset.rs     # aha 层设备标识重置（aha/TinyStorage 的 aha.device.device_id，原子写回）
│   ├── src/privacy.rs          # 向 state.vscdb 的 ItemTable 读写 icube.privacy.mode
│   ├── src/browser_auto_login.rs  # 邮箱密码自动填充登录 webview
│   ├── src/logger.rs / autostart.rs  # 日志（fern，5MB×5 轮转）与开机自启（--silent 无头模式）
│   └── tauri.conf.json         # 窗口配置、updater 端点与签名公钥
```

**文件规模现状**（有效代码行，即不含空行/注释，与第 6 节 800 行上限同一口径）：

| 文件 | 有效行 | 状态 |
|---|---|---|
| src/App.css | 约 4240 | ⚠ 严重超限 |
| src-tauri/src/lib.rs | 约 1500 | ⚠ 超限 |
| src-tauri/src/account/account_manager.rs | 1160 | ⚠ 超限 |
| src/App.tsx | 约 1030 | ⚠ 超限（TraeWork 面板已拆成独立组件，勿再往此文件加页面逻辑） |
| src-tauri/src/api/trae_api.rs | 784 | 临近上限 |
| src/components/DashboardWidgets.tsx | 695 | 临近上限 |
| src-tauri/src/machine.rs | 665 | 尚可，但总行数 910 偏大 |
| src/components/DetailModal.tsx / Settings.tsx | 约 530 / 596 | 观察名单 |
| src-tauri/src/api/checkin.rs | 490 | 尚可（含新增冷却/重试轮次） |
| src-tauri/src/api/checkin_guard.rs / device_id.rs / device_reset.rs | 158 / 95 / 73 | 新增模块（含单元测试） |
| src-tauri/src/traework/snapshot.rs | 517 | 新增，含备份/恢复/对称清理/轮转/校验 + 归位改名 + 按白名单瘦身 |
| src-tauri/src/traework/mod.rs / uid.rs / commands.rs / proc.rs / profile.rs / locate.rs | 258 / 248 / 228 / 167 / 163 / 148 | 新增模块，均远低于上限 |

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

### 5.5 已移除的能力（历史沿革）

快速注册 / 扫码领号功能已于 2026-09 整体删除（代理后端不可用），对应前端入口、`quick_register_backend.rs`、`custom_tempmail.rs`、`quick_register_simple.rs` 与 `quick_register` / `quick_register_with_custom_tempmail` 命令均已不存在。`AppSettings` 中的 `quick_register_show_window` / `api_key` / `custom_tempmail_config` 字段一并移除（旧 settings.json 里的残留字段会被 serde 静默忽略）。**新增账号只走浏览器登录 / 从 Trae 读取 / 导入三条路径，勿再引入注册链路。**

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

### 5.9 TraeWork（TRAE SOLO CN）快照式账号切换（2026-09 新增）

**为什么不能沿用 traecode 的「改写登录态」路线（本节的根因，勿推翻）**：traecode 的登录真源只有 `User/globalStorage/storage.json`，用 `tc_crypto.rs` 加密写入 `iCubeAuthInfo` 即可实现秒切（已实测生效）。TraeWork 是**双真源**——`storage.json` **加** `state.vscdb`，而后者是带加密 secret storage 的 SQLite：写 JSON 覆盖不到它，客户端会以 vscdb 为准，表现为「切换后仍要重新登录」。参考实现 [smart-open/TraeWorkAssistant](https://github.com/smart-open/TraeWorkAssistant) 全仓只有 `tc_decrypt`、**没有任何 icube 凭据加密写入路径**，他们试过之后选了快照。因此本模块只做「整组快照 / 覆盖」，**不要给 TraeWork 加 tc 写入**。

**数据落点**：快照根 `%APPDATA%\hhj\trae-cc\data\profiles_traework\<uid>\`（与账号库同一套 `ProjectDirs`）；当前账号标记 `profiles_traework\current_account.txt`；exe 路径 `%APPDATA%\hhj\trae-cc\config\traework_path.txt`。**当前账号以 `current_account.txt` 为真源，不写 `AccountStore.current_account_id`**——后者是 traecode 的「Trae IDE 当前账号」，两个应用可同时登录不同账号，共用会互相污染。

**快照白名单（`traework/profile.rs::SNAPSHOT_ITEMS`，21 项，改上游版本前先核对这里）**：`storage.json` / `state.vscdb` / `-.wal` / `-.shm` / `-.backup` / `machineid` / `aha\` / `Preferences` / `Local State` / `Local Storage\leveldb` / `Local Storage\config.db` / `Network\` / `Session Storage\`，外加两个 `Partitions\*` 目录的**会话子路径**（`Network` / `Local Storage` / `IndexedDB` / `Session Storage`，各 4 项）。

**`Partitions\*` 不得整目录拷贝（2026-09-20 实测收窄）**：整目录时这两项合计 **487MB**，其中 483MB 是 `Cache`/`Code Cache`/`DawnWebGPUCache`/`GPUCache` 等**可再生缓存**，真正承载会话的 `Network`/`Local Storage`/`IndexedDB` 不到 1MB；叠加 `.bak` 会让**每个账号**占约 1GB（收窄后约 12MB）。缓存丢失只影响首次加载速度，不影响登录态。

**刻意排除 `ModularData\`**（业务对话数据绑定账号与路径，跨账号替换会串数据）。`REQUIRED_ITEMS` 只有 storage.json 与 state.vscdb：其余缺失只是体验降级，不该触发回滚。

**白名单收窄不会自动给存量快照瘦身**：旧快照里仍躺着被排除的文件。`snapshot::prune_slot` 按白名单清理它们——判据是"恢复只读白名单路径"，故删它**不可能改变任何恢复结果**，只是丢掉不再需要的数据。由「修复槽位」触发。

**切换编排（`traework/mod.rs::switch_to`，顺序即正确性）**：① 预检快照存在性（**此时还没杀客户端**，缺失就立刻失败，不破坏用户当前状态）→ ② `proc::stop` 三级关闭 → ③ 现场备份到保留槽 `last` → ④ 覆盖恢复 → ⑤ `verify_restore` 校验，失败则用 `last` 回滚 + 启动 + 报错 → ⑥ 写 `current_account.txt` → ⑦ 启动。`save_current_login` 同构（关客户端 → 备份到目标槽 → 写标记 → 启动），**保存成功后才把账号登记进账号库**（先登记后保存失败会留下「有账号无快照」的槽位，把失败推迟到更难解释的位置）。

**三条不可省的工程约束**（每条都对应一次事故，见 doc/TraeWork账号切换计划.md §2.6）：

- **恢复前必须删 `state.vscdb-wal`/`-shm`**：强杀是常态，边车里的旧登录写入会被客户端启动时回放，把切换前账号「复活」——这是参考实现「切换后账号不变」的根因。
- **恢复必须对称**：槽位没有的项要删掉现场同名项，否则上一账号的残留（如 `state.vscdb.backup`）留在新账号现场。
- **优雅等待 8 秒（`GRACEFUL_WAIT_SECS`）不要下调，但要知道「强杀是常态」**：TraeWork 实测**每次都超时**（2026-09-20 三次真关闭全部走强杀，含运行已久的实例），与参考实现「每次切换都强杀」一致——所以日志/界面里的「优雅关闭超时，强制结束进程」**不是回归，不要按缺陷排查**。最可能原因是 Electron 窗口关闭后进程仍驻留（窗口关闭 ≠ 进程退出）。可接受的理由：强杀发生在**备份之前**（拷的是已落盘状态），`state.vscdb` 是 SQLite、恢复前还会删 `wal`/`shm`；唯一代价是快照可能少掉最后几秒的写入。`proc::post_wm_close` **已返回 `(匹配窗口数, 投递成功数)`**（2026-09-20 补诊断日志），超时提示按「投递成功 == 0」与否分岔，并额外打一条 warn 级日志 `matched=… posted=… pids=…`——拿 `app.log` 即可定性是「客户端关窗后仍驻留」还是「压根没有顶层窗口可投」，不必再靠猜。

其余要点：`code.lock` 恢复前须删（否则启动冲突）；覆盖槽位前先 `.bak` 单代轮转（防止「登错账号后保存」把好快照刷成错的且不可恢复）；`copy_item` 对文件也「先删后拷」（源被占用时不留旧文件冒充备份成功）；`ensure_slot_safe` 只放行 `[A-Za-z0-9_-]{1,64}`（槽位名会被拼进路径，这是唯一能从「换个账号」升级成「损坏系统」的入口）。

**uid 判定（`traework/uid.rs`）—— 2026-09-20 修正，勿退回**：**权威来源是解密 `iCubeAuthInfo://icube.cloudide` 后的 `userId`**（登录态本体，客户端自己就是读它判断"当前是谁"，用 `tc_crypto::decrypt_storage_value`）。明文 JSON 兼容旧客户端。

**`icube_gtm.users` 只是兜底，绝不可作为主判据**：它对"退出登录→换号登录"存在**滞后**。实测事故：用户存账号 A 再换账号 B 保存，两次都解析成同一 uid，第二次把 A 的快照整个覆盖（`.bak` 里才发现是另一个账号的 `userId`）。同理 `iCubeAuthInfo://usertag` 解密后是**以 uid 为键的累积表**（新旧账号都在），也不能判"当前"。`iCubeAuthInfo://icube-dc:<id>` 里的 `<id>` 是 OAuth 设备凭证 id（值是 EC P-256 私钥 PEM），**不是账号 uid**。TraeWork 在 `storage.json` 里**没有** `iCubeEntitlementInfo`（traecode 有）。

两道防线（配合上面那条才完整）：① **备份后自校验槽名**（`verify_slot_name`）——以快照内部解出的 userId 为最终裁决，不符即改名归位，判据不依赖任何外部字段时序；② **切换前不符即拒绝**，避免"点切换到 X 实际登录成 Y"。`account_id_from_storage()` / `slot_account_id()` 让"实时现场"与"快照内容"共用同一判据。

**命令（8 个，注册在 lib.rs，实现均在 `traework/commands.rs`）**：`traework_overview`（当前账号/进程/各槽快照状态/孤儿槽）、`traework_discover`（uid 判定）、`traework_save_current_login`、`traework_switch_account`、`traework_delete_snapshot`、`traework_set_path`、`traework_scan_path`、**`traework_reconcile`（修复槽位：错位快照按真实账号改名归位 + 登记缺失账号 + 按白名单清理存量快照，目标槽位已存在时拒绝覆盖）**。全部把阻塞工作（`tasklist` / `std::fs`）放进 `spawn_blocking`，锁只在取账号信息时短暂持有。孤儿槽/错位槽的入口就是这个「修复槽位」按钮，没有别的路径。

**前端**：独立侧边栏页「TraeWork」（`src/components/TraeworkPanel.tsx` + 同名 CSS）。账号管理页/统计页/批量操作**只处理 traecode 账号**（`isTraeworkAccount` 过滤），两套切换机制相反，混在一个列表里会让用户按同一预期连点。**进度反馈刻意不用 Tauri 事件流**：本仓库此前没有任何 `emit` 用法，为单一功能引入事件通道会带来订阅时机/事件丢失一整套新问题；当前做法是进行中遮罩 + 已耗时秒表 + 结果里回传分步日志（`StepCollector`），已满足「明确进度、防连点」的原始诉求。

**traecode 侧准入判据**：`Account.is_traecode()` 已加到 `list_accounts_for_checkin`、`checkin_one_account`、`AccountManager::refresh_token`、`get_account_usage` 四处。**新增任何遍历全量账号的 traecode 链路时，必须同样先过滤**，否则 TraeWork 账号会产生一串误导性失败。

## 6. 编码约定

- **注释**：新增/修改函数时写函数级注释，解释「为什么这样做」（动机与权衡），而非复述「做了什么」。
- **文件规模**：单文件有效代码（不含空行/注释）≤ 800 行；预计超 700 行即应拆分。超限与临近文件清单见第 4 节表格，**新增逻辑优先放入对应领域模块，不要继续往这些文件里加**。
- **复用优先**：新功能先检查 `account/`、`api/`、`machine.rs` 中是否已有可复用函数，再考虑新模块。
- **新增命令 checklist**（三步缺一不可）：
  1. Rust 侧写 `#[tauri::command]` 并在 lib.rs 的 `tauri::generate_handler!` 列表注册。命令定义可以放在领域模块里（如 `traework/commands.rs`），注册时写路径限定名 `traework::commands::xxx` 即可——**命令清单必须集中可见，实现不必挤进 lib.rs**（该文件已超限）；
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
- **错误处理**：Rust 错误统一经 `anyhow` → `ApiError { message }` 返回前端。
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
9. **TraeWork 快照里的 token 可能过期**：切换恢复的是「保存那一刻的登录态」，若期间账号在别处重新登录导致 token 失效，切过去仍要重新登录。计划中的兜底（恢复后用账号库新 token 重写 `storage.json`）**尚未实现**——需要先有一条可信的 token 来源，TraeWork 侧的凭据目前只存在于 vscdb 与密文里。
10. **TraeWork 快照体积未实测**：白名单含 `state.vscdb`（本机 1.2MB × 2）与两个 `Partitions\*` 目录，单槽预估 3–8MB；`doc/TraeWork账号切换计划.md` §5 风险 3 的估算**待首次真实快照生成后替换**。`Partitions\*` 两项参考实现自己也标注未完全验证，先纳入跑通，若体积不可接受再按「切换前后 mtime/大小是否变化」取舍。
11. **TraeWork 的 uid 证据链只在本机单账号场景验证过**：`icube_gtm.users` 多账号并存时的消歧（vscdb per-uid 键名计数）已写但**未在真实双账号环境实测**——单测只覆盖了构造数据。多账号场景下若出现「置信度不足」提示，属设计内的保守行为，不是 bug。
12. **TraeWork 无隐私模式与机器码联动**：未调研 TraeWork 是否有类似 traecode 的隐私模式键；`machineid` 随快照走（一账号一设备），因此**不提供**「重置 TraeWork 机器码」入口。切 TraeWork 不改注册表 MachineGuid（那是 traecode 的行为）。\n13. **TraeWork 存量快照不会自动瘦身**：白名单收窄只影响新快照，旧快照里仍躺着被排除的缓存（实测约 487MB/份，叠加 `.bak` 翻倍）。需用户点一次「修复槽位」才清理——刻意不做自动清理，因为「删除快照内容」这种破坏性动作不应在用户没察觉时发生。\n14. **uid 兜底路径仍有滞后风险**：若上游改了 auth 字段格式导致解密失败，判定会退回 `icube_gtm.users`，而该字段对「换号登录」滞后（正是 2026-09-20 覆盖事故的成因）。此时「备份后槽名自校验」会用同一个退化判据，防线失效。**判断信号**：面板「识别结果」里来源不是「已解密登录态确认」而是「来自 icube_gtm.users」时即处于该状态，此时新增账号前建议先核对客户端里实际登录的是谁。

## 8. 安全与边界（不要做的事）

- 不要提交真实 `.env` 或任何密钥；不要改动 updater 的 `pubkey` 与端点，除非用户明确要求。
- 不要删除/重写 `accounts.json`、`settings.json` 的兼容字段；导入导出格式变更需保持向后兼容（注意：`settings.json` 里已删除字段的残留值会被 `#[serde(default)]` 静默忽略，属预期行为）。
- 不要执行会改本机注册表、杀进程的操作来「验证」代码——这些副作用仅供最终用户在其机器上触发。
- 注入脚本（`build_browser_login_script`）只在 trae.com.cn 域名下工作，修改时保持域名检查与凭据脱敏逻辑。
- 本项目仅适配国内版：TraeCode 侧为 `%APPDATA%\Trae CN` + `Trae CN.exe`；TraeWork 侧为 `%APPDATA%\TRAE SOLO CN` + `TRAE SOLO CN.exe`。**两者完全独立，路径/进程名/命令不得互相复用**（最坏后果是用一个应用的身份去清另一个应用的数据目录），也不要引入国际版（trae.ai）的路径或域名。
- **不要给 TraeWork 增加「加密写入 storage.json」的切换路径**：其登录真源是双源，写 JSON 覆盖不到 `state.vscdb`，只会得到「切换后仍要重新登录」。快照方案是经过参考实现试错后的选择，不要退回。
- 不要改 TraeWork 的 `ModularData`（业务对话数据绑定账号与路径，跨账号替换会串数据）；不要在缺少 `.bak` 轮转保护的情况下覆盖槽位（用户登错账号时会把好快照永久刷坏）。
- TraeWork 的快照槽位名只允许 `[A-Za-z0-9_-]`：它来自 `storage.json` 推导并会被拼进文件路径，放宽就是路径穿越。
- README.md 面向最终用户，仅保留技术性内容；技术细节以本文件（AGENTS.md）为准，不要向 README 或其他技术文档追加营销、推广内容。
- 本项目为个人学习研究用途的工具；协助时遵循仓库 README 的免责声明边界，不扩展用于倒卖或批量绕过授权的用途。

## 9. 提交规范

- 遵循 [Conventional Commits](https://www.conventionalcommits.org/)：`feat:` / `fix:` / `refactor:` / `docs:` / `chore:` 等前缀（仓库历史中也有带中文 scope 的写法，如 `feat(配置):`）。
- 版本号同时维护 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处；发布后同步 `CHANGELOG.md`（Keep a Changelog 格式。注意：当前 CHANGELOG 停在 1.0.4，已滞后于三处版本号的 1.0.5）。
