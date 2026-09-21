# AGENTS.md — Trae账号管理（trae-cc）

> 面向 AI 编码助手的工作指南：项目结构、构建方式、关键机制、编码约定与安全边界。
> 文中行数、命令数量等均取自对仓库的实测（2026-09）；改动代码前请先读完第 4、5、6 节。

## 1. 项目速览

- **名称**：Trae账号管理（应用标识 `com.hhj.trae-cc`），当前版本 **1.0.6**（三处同步：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`）
- **定位**：Windows 桌面工具，管理多个 Trae 账号——**TraeCode（Trae CN）** 侧：账号存储、一键切换（改写 IDE 登录态与机器码）、用量查询/统计图表、每日签到（手动 + 开机自动）、机器码管理、隐私模式写入；**TraeWork（TRAE SOLO CN）** 侧：登录态快照 / 恢复式切换、每日签到（手动 + 自动，凭据走快照解析）（2026-09 新增，见 §5.9）
- **架构**：Tauri 2 应用 = React 19 前端（`src/`）+ Rust 后端（`src-tauri/src/`）。前后端仅通过 `#[tauri::command]` 通信，共注册 **60 个命令**（本地 50 + TraeWork 10；TraeWork 命令定义在 `src-tauri/src/traework/commands.rs`，注册仍在 lib.rs）；前端统一走 `src/api.ts` 的 invoke 封装
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
- 无 lint 配置；根目录的 `check_quota*.mjs/.py` 是手动调试脚本，不是测试套件。Rust 侧有 `#[cfg(test)]` 单元测试（`device_id` / `checkin_guard` / `checkin` / `account_manager` / `device_reset` / `tc_crypto` / `traework::*`），用 `cargo test --lib` 运行（2026-09-20 实测 56 个全绿）。TraeWork 侧覆盖：槽位名路径穿越拒绝、进程名白名单、快照往返（边车清除 + 对称恢复）、`.bak` 轮转与回退、uid 证据链与并列时拒绝识别、当前账号标记 BOM 剥离、凭据解析三优先级、签到 app 分派（skipped 不落盘）与 `credential_stale` 冷却。
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
│   ├── pages/                  # Stats（用量图表）、About、settings/（设置页：Settings 容器 + 分区组件 + Settings.css）
│   ├── utils/                  # accountBackup.ts（账号库导入/导出的共享实现，设置页与添加弹窗共用一份）
│   └── components/             # AccountCard、AddAccountModal、DashboardWidgets、DetailModal、TraeworkPanel（TraeWork 面板 + 同名 CSS）等
├── doc/                        # 方案与调研文档（含 TraeWork账号切换计划.md）
├── src-tauri/
│   ├── build.rs                # 向上递归查找 .env，输出 cargo:rustc-env 注入编译期变量（当前无消费方）
│   ├── src/lib.rs              # 应用入口：49 个本地命令 + AppState + 浏览器登录/open_pricing 注入脚本（⚠ 有效约 1500 行，
│   │                           #   其中 build_browser_login_script 的 JS 字符串约 480 行，勿再继续膨胀）
│   ├── src/traework/           # TraeWork（TRAE SOLO CN）账号切换：profile（常量+快照白名单 21 项）/snapshot（备份恢复原语+对称恢复）/device（设备标识归一+注册表同步）/proc（三级关闭）/locate（exe 五级发现）/uid（证据链+置信度门槛+接口凭据）/credentials（凭据解析优先级）/commands（10 个命令）
│   ├── src/account/            # 账号域：account_manager.rs（CRUD/切换/刷新/签到落盘，⚠ 有效 1400 行）、device_identity.rs（设备标识的 user_id 级归属规则）、types.rs
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
| src-tauri/src/account/account_manager.rs | 1400 | ⚠ 超限（2026-09-21 复核；同日已把设备标识规则拆出到 device_identity.rs） |
| src-tauri/src/account/device_identity.rs | 145 | 2026-09-21 新增：设备标识的**归属规则**（`user_id` 键 / 稳定派生 / 跨应用对齐），纯函数 + 单测 |
| src/App.tsx | 约 1030 | ⚠ 超限（TraeWork 面板已拆成独立组件，勿再往此文件加页面逻辑） |
| src-tauri/src/api/trae_api.rs | 784 | 临近上限 |
| src/components/DashboardWidgets.tsx | 695 | 临近上限 |
| src-tauri/src/machine.rs | 665 | 尚可，但总行数 910 偏大 |
| src/components/DetailModal.tsx | 约 530 | 观察名单 |
| src/pages/settings/（Settings 容器 + 7 个分区组件 + shared.ts） | 容器约 130，其余 55–260 | 2026-09-20 从单文件 Settings.tsx（641 行）拆出，均已低于上限 |
| src-tauri/src/api/checkin.rs | 617 | 临近上限（含 TraeWork 凭据分派与 skipped 语义；再增需拆 checkin/ 子模块） |
| src-tauri/src/api/checkin_guard.rs / device_id.rs / device_reset.rs | 187 / 95 / 73 | 含单元测试 |
| src-tauri/src/traework/snapshot.rs | 571 | 含备份/恢复/对称清理/轮转/校验 + 归位改名 + 按白名单瘦身 |
| src-tauri/src/traework/mod.rs / uid.rs / commands.rs / device.rs / proc.rs / profile.rs / locate.rs / credentials.rs | 383 / 473 / 381 / 469 / 206 / 163 / 148 / 80（2026-09-21 复核） | device.rs 为 2026-09-21 新增的**设备标识层**（保存时按 `user_id` 对齐 + 切换时同步注册表 + 可注入的注册表出口，含单测）；credentials.rs 为积分与签到共用的凭据解析 |

**棘轮冻结线（只降不升，2026-09-21 起执行）**：超限文件以下表实测值为各自临时上限，任何修改后的有效行数**不得高于冻结值**；收敛后按新实测值更新本表（逐次收紧）。确需新增逻辑时优先入新文件，或在原文件内同步删减/拆出等量以上代码。

| 文件 | 冻结值（有效行） |
|---|---|
| src/App.css | ≤ 4240 |
| src-tauri/src/lib.rs | ≤ 1500 |
| src-tauri/src/account/account_manager.rs | ≤ 1400 |
| src/App.tsx | ≤ 1030 |

修改上述文件前先实测当前有效行作为基准：

```powershell
# 排除空行与行注释的有效行估算；块注释居多的文件（如 CSS）需人工再扣除块注释行
(Get-Content <文件> | Where-Object { $_ -match '\S' -and $_ -notmatch '^\s*(//|#|\*|/\*|<!--)' }).Count
```

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

**为什么上述步骤不清理 `Partitions\`（2026-09-20 实测结论，勿"补全"）**：`%APPDATA%\Trae CN\Partitions\` 下两个 profile（`trae-webview`、`icube-web-crawler-shared-session-v1.0`）是客户端**内置浏览器**的通用 Chromium profile。把两者的「origin + 键名」全量提取后，53 + 21 个键**全部**是用户访问过的站点（figma / volcengine / baidu / github / bilibili / 番茄小说 / 本地 localhost 开发服务）的 Cookie 与 Local Storage，来自 `trae.com.cn` 的键 **0 个**（`Network\Cookies` 里 trae 域名同样 0 命中）。它与账号、登录态、设备标识都无关，删它只会丢掉用户在内置浏览器里的登录态与浏览记录。**TraeWork 快照白名单包含这些路径不是"口径不一致"**：TraeWork 是快照/恢复（拷走再还原，为完整还原账号上下文），TraeCode 是就地删除——场景不同，口径本就该不同。
（注意：对该目录做 ASCII 子串检索会命中 `device` / `uid` / `token`，那是站点脚本字段名与 Chromium 的 `Trust Tokens` 文件造成的**误命中**。凡是判断「某目录是否承载某类数据」，必须拿到键名/域名级证据，不能凭子串下结论。）

### 5.3 Token 生命周期

- 优先 JWT：请求头为 `Authorization: Cloud-IDE-JWT <token>`；401 时用 Cookies 调 `GetUserToken` 刷新并回写账号。
- 查询用量入口为 lib.rs 的 `fetch_usage_for_account`（401 判断靠错误字符串包含 "401"，改动错误信息格式时注意）。
- API 端点（trae_api.rs）：本项目仅适配国内版 Trae CN，三个常量 `API_BASE_US` / `API_BASE_SG` / `API_BASE_UG` 统一指向 `api.trae.com.cn`（CN 版为单域名体系，国际版的分区域域名已弃用）；`machine.rs` 写 storage.json 时 host 亦统一为 `api.trae.com.cn`。
- **CN 版为积分钟模型（重要）**：`/trae/api/v1/pay/user_current_entitlement_list` 与 v1 `ide_user_ent_usage` 对 CN 账号恒返回 0（国际版美元/请求次数模型），真实额度只在 **v2 端点** `/trae/api/v2/pay/ide_user_ent_usage`，字段为 `usage_summary.{total_amount,consumed_amount}` 与各礼包 `quota.credits_limit` / `usage.credits_amount`。查询入口为 `api/cn_credits.rs`（`try_credits_usage` 在 `trae_api.rs` 中优先尝试，非积分钟或失败才回退旧解析）。改额度相关代码前先读 `cn_credits.rs`。
- **积分按适用产品拆分（通用 / Work 专属）**：官方规则为「通用积分（TraeCode 与 TraeWork 通用）」与「Work 专属积分（仅 TraeWork）」两类。接口不直接给分类，`cn_credits.rs` 按礼包 `entitlement_base_info.available_endpoint` 归类：`1` = Work 专属，其余（`0`/缺省）= 通用；判据是官方「老用户升级福利 = 2000 通用 + 2000 Work」「每月登录/每日签到 = 通用」与实测礼包 pid（208/221 → endpoint 0，209 → endpoint 1）的对应关系。前端在卡片/列表/详情三处分开显示，字段为 `credits_general_*` / `credits_work_*`。

### 5.4 浏览器登录（webview 凭据捕获）

`start_browser_login` 先用 `about:blank` 建 `trae-login` webview、清掉上一轮登录残留的 web 会话、再导航到 www.trae.com.cn/login（**顺序不可颠倒**：窗口一旦带着登录 URL 建起来，首次请求就已经把旧 Cookie 发出去了——这正是 2026-09-20 之前「打开登录窗口时还带着上次那个账号」的成因），随后注入 `build_browser_login_script` 的 JS：hook fetch/XHR 请求体与 `HTMLInputElement.prototype.value` setter、递归扫描 shadowRoot/iframe 捕获输入，自动点击 Cookie 同意条；凭据 POST 到 warp 起的 `127.0.0.1:随机端口/callback`。整体 300 秒超时，多路 oneshot（取消 / 窗口关闭）竞争取消；凭据脱敏后落日志。另有 `browser_auto_login.rs`（邮箱密码自动填充）与 `open_pricing`（清 Cookie → 写入账号 Cookie → 跳转 pricing 页）两个独立注入点，勿混淆。

清理由 `clear_login_webview_session` 负责：先 `clear_all_browsing_data`，再按域显式删一遍 Cookie（`ClearBrowsingData` 异步落盘，显式删除覆盖它尚未生效的窗口期）。两个 webview 都未指定 `data_directory`，共用 `%LOCALAPPDATA%\com.hhj.trae-cc\EBWebView`，因此**只删 trae 域 Cookie、不做全量**——购买窗口依赖「清 Cookie → 写入目标账号 Cookie → 跳转」这条链路，全量删会顺带抹掉它的其它站点状态。

### 5.5 已移除的能力（历史沿革）

快速注册 / 扫码领号功能已于 2026-09 整体删除（代理后端不可用），对应前端入口、`quick_register_backend.rs`、`custom_tempmail.rs`、`quick_register_simple.rs` 与 `quick_register` / `quick_register_with_custom_tempmail` 命令均已不存在。`AppSettings` 中的 `quick_register_show_window` / `api_key` / `custom_tempmail_config` 字段一并移除（旧 settings.json 里的残留字段会被 serde 静默忽略）。**新增账号只走浏览器登录 / 从 Trae 读取 / 导入三条路径，勿再引入注册链路。**

另有两个「从未被注册过」的命令：`add_account`（仅用 Cookies 添加）与 `update_cookies` 在 `lib.rs` 的 `generate_handler!` 里查无此名（2026-09-20 实测），而 `api.ts` 却为它们写了封装——属调用必失败的死封装，已删除。新增账号请用 `add_account_by_token` / `add_account_by_email` / 浏览器登录这三条已注册路径。

### 5.6 无头模式（--silent）

进程参数含 `--silent` 时不初始化 Tauri：隐藏控制台窗口（Windows）→ `handle_silent_start`（刷新所有账号 Token → 自动签到（方案B，见 5.7）→ 若 Trae 未运行且当前账号有 JWT，则把登录态写入 storage.json 保持同步）→ `process::exit(0)`。开机自启注册表项（HKCU Run，键名 `Trae账号管理`）即以 `--silent` 方式拉起。

### 5.7 每日签到（手动 + 方案B 自动）

- **接口**：`api.trae.cn` 的 `/trae/api/v2/ug/checkin_credits/status`（查今日状态）与 `/claim`（领取），请求体 `{}`，头为 JWT + UA `Trae/0.1.52` + `X-User-Region: CN`，实现见 `api/checkin.rs`。**`claim` 必须带 `X-Device-Id`（实测缺省返回 9004「order parameters incorrect」）**；实测 `api.trae.com.cn` 未提供该接口（claim 返回 404），代码里的双端点回退仅用于网络容错。
- **判定规则（勿改松）**：「已签到」判定须同时覆盖「已签到」与「已经签到」两种措辞——服务端 9095 原文为「当前设备今日已经签到」，而「已经签到」不含子串「已签到」，只匹配前者会误判为失败。签到按**设备维度**做每日去重（9095）：同一 X-Device-Id 当日已签即拒绝，提示「本设备今日已签到」而非失败。
- **设备号隔离（`Account.device_id`，完全独立于 `machine_id`）**：`X-Device-Id` 用 `api/device_id.rs` 派生的 16 位设备号，`machine_id` 不再参与签到（避免「换签到设备号」连带改写 IDE 机器身份）。`device_id` 派生一次即落盘（sha256 域前缀 `trae-cc:device-id:v1:` + seed），启动回填**只补空值、绝不无条件重算**；`resolve_device_id` 三层回退（落盘值 → user_id 派生 → 内部 id 派生），**禁止固定种子兜底**（多个无 user_id 账号会撞号互踢 9095）。**TraeWork 账号的种子为 `traework:{uid}`（`types.rs::traework_device_seed`，upsert 落盘与启动回填共用）**——前缀把「两条记录是否撞同一设备号」变成结构性保证，不依赖「traecode 的 user_id 与 TraeWork 的 uid 是否同域」这一未验证事实（代价：同一真实账号的两条记录各有一个设备号，第二次 claim 由账号级去重挡成 already，多一次白请求）。日志一律 `mask_device_id` 脱敏。右键菜单「重置设备标识」（`reset_account_device_id` 命令）换随机号 + 清冷却 + 清日期，用 `try_acquire` 防重入。
- **冷却状态机（`api/checkin_guard.rs`）**：失败按数值 `code`/HTTP 状态码优先分类（`classify_error`），子串仅兜底（数值优先避免「消息里恰好含数字」的误判白等）。策略表：9074→10min、HTTP 429/404→60s、5xx/网络→2min、1005 权益不足→12h、401/1001→`i64::MAX`(reason=auth_expired)、**TraeWork 凭据失效→12h(reason=credential_stale，见下条)**；9095/9004/活动未开放→不冷却。`until` 落盘与读取两侧都钳到**本地时区**当日 23:59:59（按 UTC 日界钳会钳到北京时间 07:59:59，「跨天必重试」破洞）；`i64::MAX` 是「需人工介入」哨兵、无时钟语义，前端按 `cooldown_reason` 渲染不读数值。Token 刷新/重新登录**仅清 auth_expired** 冷却。
- **并发防重入**：三入口（`checkin_account` / `checkin_all_accounts` / `auto_checkin`）共用 `checkin_guard::try_acquire`（进程级 `static`，覆盖 `--silent` 自建 manager 的场景），持锁到全部结束；持锁期间手动签到返回「签到进行中」是有意取舍。`--silent` 与 GUI 双进程仍可能并发，不做跨进程文件锁（最坏得到 9095/9074，且有冷却兜底）。
- **语义分层（勿破坏）**：冷却管「跨批次记忆」，批次内重试轮次管「批次内退避」。批次启动时**快照一次**已有冷却做准入、运行期间不重新准入；批次内失败**不落盘冷却**，全部轮次结束后 `apply_checkin_outcomes` **统一写盘一次**（成功/Already 写日期 + 失败项写冷却；同一账号重试成功时后一结果覆盖前一冷却）。
- **自动签到批次重试（仅 `auto_checkin_pending`）**：第 1 轮全量 → 收集 `rate_limited` 账号 → 睡 30s → 第 2 轮 → 睡 90s → 第 3 轮 → 结束；等待次数与账号数无关（最坏 ≈ 2 分钟）、仅存在可重试项才 sleep。手动入口**不重试**（单轮结束即落盘）。
- **自动签到（方案B）**：启动时（前端挂载后、以及 `--silent` 无头启动）调用 `auto_checkin` 命令，只处理 `last_checkin_date != 今天` 的账号；成功/已签到后写日期，避免一天多次开机重复请求；失败不写日期，下次启动自动重试。日期字段是 accounts.json 的兼容新增字段（`Account.last_checkin_date`）。
- **手动入口**：右键菜单「签到」（`checkin_account`）、工具栏「全部签到」（`checkin_all_accounts`，**只签 TraeCode**）、TraeWork 面板「签到 / 全部签到」（`checkin_account` / `traework_checkin_all`，只签 TraeWork）。冷却账号返回 `cooldown` 状态（info 提示、不进失败汇总；自动签到维持静默）。**防双签红线：`checkin_with_token` 中 claim 前必先查 status**（重置设备号后该账号今日已真签到会被短路为 Already），该顺序禁止改动、禁止绕过。签到不触碰 IDE 文件与机器码，是纯网络请求。
- **按 app 分派凭据（勿改成「非 traework 就走 cookies」的单边判断）**：`list_accounts_for_checkin` 已**不再**按 `is_traecode` 过滤（过滤条件只剩 is_active 与日期），app 过滤落在 `checkin.rs`（`checkin_scope(manager, app: Option<&str>)` 锁外 `retain`）。TraeWork 账号的凭据由 `resolve_traework_credentials` **批次级**解析（一次 `spawn_blocking` + 一次 `Ctx::from_env`，不持 manager 锁；复用 `traework/credentials.rs::resolve_token` 的「当前槽读现场 → 主槽 → .bak」优先级，与积分查询共用，防两处漂移）。**无可达凭据记 `Skipped`（`CheckinState::Skipped`）**：不是失败（不进失败汇总）、也不是冷却（`collect_outcomes` 对它不写日期不写冷却），detail 透传「切换到 X 并重新保存」的可操作提示。`Ctx::from_env` 失败属环境级异常：全部 TraeWork 账号按 Skipped 跳过，traecode 照常签到。
- **TraeWork 凭据失效 → `credential_stale`，绝不落 `auth_expired` 哨兵（阶段红线）**：TraeWork 账号 `cookies` 恒为空串，`refresh_token_via_cookies` 对它是恒空转，`process_account` 按 app 在刷新尝试之前就分派进 `traework_round`；401/1001 在其中被映射为 `CheckinError::CredentialStale`（12h、不可重试）。清除入口挂在「保存当前登录态」：`upsert_traework_account` 内调 `clear_cooldown_reason(account, REASON_CREDENTIAL_STALE)`（切换只是恢复旧凭据，重新保存才是新 token 落盘的时刻）。`i64::MAX` 哨兵无清除入口，落上去账号就永久卡死。

### 5.8 aha 层设备标识（`device_reset.rs`）

`%APPDATA%\Trae CN\aha\TinyStorage` 内含 `aha.device.device_id`（加密 blob）。`reset_aha_device_id` 定位 `tiny_storage_data` **子对象**后仅移除其内该键，保留同层 `aha_access_policy` / `aha_doctor_domain` / `aha_last_renderer_oom`（勿在顶层 remove——会静默落空）；写回原子化（临时文件 + rename），解析失败保持原样（best-effort，绝不让重置变成数据丢失）。**不能写「合法值」**：加密密钥不在我们掌握中，写入非法值会让客户端解密异常，删除永远比伪造安全。登录态与 aha 无关，**不需要重新登录**。`aha\Remote_State`（纯 feature flag）与 `ahanet\prefs\local_prefs.json`（纯 Chromium http_server_properties）无设备标识，不动。`Partitions\trae-webview\` 的 Cookie/Local Storage/IndexedDB **暂不纳入**——需先对比切换前后 mtime/大小确认有写入，无证据不盲删。

### 5.9 TraeWork（TRAE SOLO CN）快照式账号切换（2026-09 新增）

**为什么不能沿用 traecode 的「改写登录态」路线（本节的根因，勿推翻）**：traecode 的登录真源只有 `User/globalStorage/storage.json`，用 `tc_crypto.rs` 加密写入 `iCubeAuthInfo` 即可实现秒切（已实测生效）。TraeWork 是**双真源**——`storage.json` **加** `state.vscdb`，而后者是带加密 secret storage 的 SQLite：写 JSON 覆盖不到它，客户端会以 vscdb 为准，表现为「切换后仍要重新登录」。参考实现 [smart-open/TraeWorkAssistant](https://github.com/smart-open/TraeWorkAssistant) 全仓只有 `tc_decrypt`、**没有任何 icube 凭据加密写入路径**，他们试过之后选了快照。因此本模块只做「整组快照 / 覆盖」，**不要给 TraeWork 加 tc 写入**。

**数据落点**：快照根 `%APPDATA%\hhj\trae-cc\data\profiles_traework\<uid>\`（与账号库同一套 `ProjectDirs`）；当前账号标记 `profiles_traework\current_account.txt`；exe 路径 `%APPDATA%\hhj\trae-cc\config\traework_path.txt`。**当前账号以 `current_account.txt` 为真源，不写 `AccountStore.current_account_id`**——后者是 traecode 的「Trae IDE 当前账号」，两个应用可同时登录不同账号，共用会互相污染。

**快照白名单（`traework/profile.rs::SNAPSHOT_ITEMS`，21 项，改上游版本前先核对这里）**：`storage.json` / `state.vscdb` / `-.wal` / `-.shm` / `-.backup` / `machineid` / `aha\` / `Preferences` / `Local State` / `Local Storage\leveldb` / `Local Storage\config.db` / `Network\` / `Session Storage\`，外加两个 `Partitions\*` 目录的**会话子路径**（`Network` / `Local Storage` / `IndexedDB` / `Session Storage`，各 4 项）。

**`Partitions\*` 不得整目录拷贝（2026-09-20 实测收窄）**：整目录时这两项合计 **487MB**，其中 483MB 是 `Cache`/`Code Cache`/`DawnWebGPUCache`/`GPUCache` 等**可再生缓存**，真正承载会话的 `Network`/`Local Storage`/`IndexedDB` 不到 1MB；叠加 `.bak` 会让**每个账号**占约 1GB（收窄后约 12MB）。缓存丢失只影响首次加载速度，不影响登录态。

**刻意排除 `ModularData\`**（业务对话数据绑定账号与路径，跨账号替换会串数据）。`REQUIRED_ITEMS` 只有 storage.json 与 state.vscdb：其余缺失只是体验降级，不该触发回滚。

**白名单收窄不会自动给存量快照瘦身**：旧快照里仍躺着被排除的文件。`snapshot::prune_slot` 按白名单清理它们——判据是"恢复只读白名单路径"，故删它**不可能改变任何恢复结果**，只是丢掉不再需要的数据。由「修复槽位」触发。

**切换编排（`traework/mod.rs::switch_to`，顺序即正确性）**：① 预检快照存在性（**此时还没杀客户端**，缺失就立刻失败，不破坏用户当前状态）→ ② `proc::stop` 三级关闭 → ③ 现场备份到保留槽 `last` → ④ 覆盖恢复 → ⑤ `verify_restore` 校验，失败则用 `last` 回滚 + **把注册表设备标识一并退回 `last` 的值** + 启动 + 报错 → ⑥ 写 `current_account.txt` → ⑦ **`device::apply_registry` 同步注册表设备标识**（必须在启动客户端**之前**：客户端启动时就把它读走了）→ ⑧ 启动。`save_current_login` 同构（关客户端 → 备份到目标槽 → 槽名校验 → **`device::normalize_on_save` 按 `user_id` 对齐设备标识**（目标值由命令层从账号库取出后传入，编排层不持账号库锁）→ 写标记 → 启动），**保存成功后才把账号登记进账号库**（先登记后保存失败会留下「有账号无快照」的槽位，把失败推迟到更难解释的位置）。

**设备标识归属 `user_id`，不归属记录/app（`traework/device.rs` + `AccountManager::shared_machine_id`，2026-09-21 修正，勿退回）**：两个约束必须同时满足，且它们指向同一个答案——**不同真实账号 → 不同设备**（否则触发设备级限制「该设备绑定的账户数量已达上限」）；**同一真实账号 → 同一设备**（官方口径「同一台电脑同时登录 TraeCode 和 TraeWork 只算 1 台设备」，而账号库里同一账号本就有两条记录：`app` 不同、`user_id` 相同）。因此设备标识是 **`user_id` 的属性**：`AccountManager::shared_machine_id` 按 `user_id` 取值（**traecode 记录优先**——它一直是被写进注册表与 `machineid` 文件的那个值；无既有值则由 `derived_machine_id` 稳定派生，保证幂等、可反复回填），启动回填 `align_device_identities` 把同 `user_id` 的记录统一到同一值。

**为什么这是修正而不是初版**：初版按「每个 TraeWork 账号一套独立标识」实现，只解决了第一条约束——同一账号在 traecode 侧 `caf505a4…`、traework 侧 `65e414a1…` 时服务端把它当**两台设备**，正是账号级风控（`Login Abnormality`）的成因方向。「一台设备多账号」与「一个账号多设备」是两条**相反**的风控特征，只拆不合并等于拆东墙补西墙。

本模块两层：

- **保存时对齐**（`normalize_on_save`，目标值由命令层从账号库取出后传入）：把槽位与现场改成该 `user_id` 的目标值（`machineid` + telemetry 三件套，规则复用 `machine::telemetry_ids`，**不得在本模块另写一套**）。拿不到共享值（账号还没进库）时才退化为「撞号则生成新值」。**必须同时写槽位与现场**——只写一处会被下一次保存/切换把旧值铺回来，现象是「每次都提示已对齐，账号之间始终不一致」。`aha` 只删不写（加密 blob，复用 `device_reset::reset_aha_device_id`）。**槽位被 `verify_slot_name` 归位时，调用方传入的共享值必须丢弃**（它属于另一个账号，写下去比不对齐更糟）。
- **切换时写注册表**（`apply_registry`）：把槽位 `machineid` 写入 `MachineGuid` 并**读回验证**（`set` 返回成功 ≠ 生效，权限/策略会让它静默落空）。回滚路径必须写回 `last` 的值，否则现场已退回而注册表停在中途的失败目标上，本地两层标识互相矛盾。

注册表读写抽成 `RegistryAccess` trait 注入：写 `HKLM` 是系统级副作用，**测试绝不能碰真实注册表**，注入后「读不到标识→跳过」「写失败→只告警」「读回不一致→判失败」三条分支都能单测。撞号判定用 `Fingerprint::clashes_with`（**任一非空字段相同即算撞号**，偏安全方向），并**排除保留槽 `last`**（否则每个账号恒被判为撞号、每次保存都重算，反而制造「设备频繁轮换」这个风控强特征）。**边界：不做批量重置**——对齐只在用户主动点「保存当前登录态」时对该槽位生效；**风控冷却期内不要执行**（改设备身份可能加重处罚，见 §7 限制 12）。界面把「注册表 / 现场 / 各槽位」三处值一并列出（`traework_overview` 的 `registry_machine_id` / `live_machine_id` 与每槽位的 `machine_id` / `shares_device_with`），隔离是否生效**必须可见可对账**。

**【2026-09-21 实测更正】`telemetry.machineId` 并非 `sha256(machineid)`**（两组对照全对不上：算得 `84b55d30…` 而槽位里是 `0513f181…`），该字段由客户端自行维护，故改写它只是**尽力而为的兜底**、可能被客户端覆盖回自己的算法值——**判定隔离是否生效请看 `machineid` 与注册表这两层，不要拿 telemetry 说事**。附注：这也顺带解掉了 `doc/设备身份隔离调研.md` §4.1 遗留的未确证项。另：`Login Abnormality` 弹窗给出的申诉邮箱是 **`feedback@mail.trae.cn`**（国内），不是 `.ai`。

**三条不可省的工程约束**（每条都对应一次事故，见 doc/TraeWork账号切换计划.md §2.6）：

- **恢复前必须删 `state.vscdb-wal`/`-shm`**：强杀是常态，边车里的旧登录写入会被客户端启动时回放，把切换前账号「复活」——这是参考实现「切换后账号不变」的根因。
- **恢复必须对称**：槽位没有的项要删掉现场同名项，否则上一账号的残留（如 `state.vscdb.backup`）留在新账号现场。
- **优雅等待 8 秒（`GRACEFUL_WAIT_SECS`）不要下调，但要知道「强杀是常态」**：TraeWork 实测**每次都超时**（2026-09-20 三次真关闭全部走强杀，含运行已久的实例），与参考实现「每次切换都强杀」一致——所以日志/界面里的「优雅关闭超时，强制结束进程」**不是回归，不要按缺陷排查**。最可能原因是 Electron 窗口关闭后进程仍驻留（窗口关闭 ≠ 进程退出）。可接受的理由：强杀发生在**备份之前**（拷的是已落盘状态），`state.vscdb` 是 SQLite、恢复前还会删 `wal`/`shm`；唯一代价是快照可能少掉最后几秒的写入。`proc::post_wm_close` **已返回 `(匹配窗口数, 投递成功数)`**（2026-09-20 补诊断日志），超时提示按「投递成功 == 0」与否分岔，并额外打一条 warn 级日志 `matched=… posted=… pids=…`——拿 `app.log` 即可定性是「客户端关窗后仍驻留」还是「压根没有顶层窗口可投」，不必再靠猜。

其余要点：`code.lock` 恢复前须删（否则启动冲突）；覆盖槽位前先 `.bak` 单代轮转（防止「登错账号后保存」把好快照刷成错的且不可恢复）；`copy_item` 对文件也「先删后拷」（源被占用时不留旧文件冒充备份成功）；`ensure_slot_safe` 只放行 `[A-Za-z0-9_-]{1,64}`（槽位名会被拼进路径，这是唯一能从「换个账号」升级成「损坏系统」的入口）。

**uid 判定（`traework/uid.rs`）—— 2026-09-20 修正，勿退回**：**权威来源是解密 `iCubeAuthInfo://icube.cloudide` 后的 `userId`**（登录态本体，客户端自己就是读它判断"当前是谁"，用 `tc_crypto::decrypt_storage_value`）。明文 JSON 兼容旧客户端。

**`icube_gtm.users` 只是兜底，绝不可作为主判据**：它对"退出登录→换号登录"存在**滞后**。实测事故：用户存账号 A 再换账号 B 保存，两次都解析成同一 uid，第二次把 A 的快照整个覆盖（`.bak` 里才发现是另一个账号的 `userId`）。同理 `iCubeAuthInfo://usertag` 解密后是**以 uid 为键的累积表**（新旧账号都在），也不能判"当前"。`iCubeAuthInfo://icube-dc:<id>` 里的 `<id>` 是 OAuth 设备凭证 id（值是 EC P-256 私钥 PEM），**不是账号 uid**。TraeWork 在 `storage.json` 里**没有** `iCubeEntitlementInfo`（traecode 有）。

两道防线（配合上面那条才完整）：① **备份后自校验槽名**（`verify_slot_name`）——以快照内部解出的 userId 为最终裁决，不符即改名归位，判据不依赖任何外部字段时序；② **切换前不符即拒绝**，避免"点切换到 X 实际登录成 Y"。`account_id_from_storage()` / `slot_account_id()` 让"实时现场"与"快照内容"共用同一判据。

**命令（10 个，注册在 lib.rs，实现均在 `traework/commands.rs`）**：`traework_overview`（当前账号/进程/各槽快照状态/孤儿槽）、`traework_discover`（uid 判定）、`traework_credits`（单账号积分余额，凭据来源见下段）、`traework_save_current_login`、`traework_switch_account`、`traework_delete_snapshot`、`traework_remove_account`（删账号记录并连同磁盘快照，与「删除快照」的区别是后者只删文件、账号仍在列表里）、`traework_set_path`、`traework_scan_path`、**`traework_reconcile`（修复槽位：错位快照按真实账号改名归位 + 登记缺失账号 + 按白名单清理存量快照，目标槽位已存在时拒绝覆盖）**。另有本地命令 `traework_checkin_all`（lib.rs，转发 `checkin::checkin_all_traework`）与单账号复用的 `checkin_account`。全部把阻塞工作（`tasklist` / `std::fs`）放进 `spawn_blocking`，锁只在取账号信息时短暂持有。孤儿槽/错位槽的入口就是这个「修复槽位」按钮，没有别的路径。

**凭据与积分、签到（2026-09-20 新增，优先级逻辑收敛在 `traework/credentials.rs::resolve_token`）**：`iCubeAuthInfo://icube.cloudide` 解出的 JSON 除 `userId` 与展示字段外，还有 `token` / `refreshToken` / `expiredAt` / `refreshExpiredAt`。`traework/uid.rs` 已把「读文件 + 解 auth 密文」抽成 `auth_plaintext()`，供 `profile_from_storage`（身份）与 `credentials_from_storage`（凭据）共用——这两处此前各写一遍解密，`iCubeAuthInfo` 由明文改密文时就漂移过一次。**凭据只经 `TraeworkCredentials` 传递，且刻意不 derive `Serialize`**：`AccountProfile` 已经 derive 了它，凭据若挂上去，将来任何一次「顺手返回 profile」都会把 token 漏给前端；同理禁止落日志、禁止写进 accounts.json。取凭据的优先级是**当前账号读实时现场 → 否则读快照主槽 → 主槽缺失回退 `.bak`**（`credentials.rs::resolve_token`，积分查询与签到共用同一份实现）：客户端会用 refreshToken 换发新凭据并**只写回现场**，对当前账号读快照只会拿到一份必然过期的旧 token。实测该 token 被 CN 端点直接接受（`api.trae.cn/trae/api/v2/ug/checkin_credits/status` 200；**`claim` 亦实测生效**，2026-09-20 探针：头 A `Trae/0.1.52` 一次通过、无需参考实现的 VSCode UA；`api.trae.com.cn/trae/api/v2/pay/ide_user_ent_usage` 均 200），所以**积分显示与签到都不需要新的认证链路**。已知边界：access 只有 14 天，快照放久了必然过期——签到侧落 `credential_stale` 冷却（见 §5.7）、积分查询报鉴权失败，两者都提示「切过去重新保存」；用 `refreshToken` 主动续期的端点**未找到**（客户端 JS 已压缩）。界面只在账号名后显示「剩余 / 总额」，通用与 Work 专属的分类明细挂在 `title`。

**前端**：独立侧边栏页「TraeWork」（`src/components/TraeworkPanel.tsx` + 同名 CSS）。账号管理页/统计页/批量操作**只处理 traecode 账号**（`isTraeworkAccount` 过滤），两套切换机制相反，混在一个列表里会让用户按同一预期连点。**进度反馈刻意不用 Tauri 事件流**：本仓库此前没有任何 `emit` 用法，为单一功能引入事件通道会带来订阅时机/事件丢失一整套新问题；当前做法是进行中遮罩 + 已耗时秒表 + 结果里回传分步日志（`StepCollector`），已满足「明确进度、防连点」的原始诉求。

**traecode 侧准入判据**：`Account.is_traecode()` 保留在 `AccountManager::refresh_token`、`get_account_usage` 两处。签到侧已改为**按 app 分派**（TraeWork 走快照凭据、无凭据记 Skipped，见 §5.7），不再用 `is_traecode` 拒绝。**新增任何遍历全量账号的 traecode 链路时，必须先想清楚 TraeWork 账号该走哪条路（分派或过滤），不能让它们落到凭据为空的失败路径上。**

### 5.10 已注册但无 UI 入口的命令（清单，避免重复推导）

「某个命令到底有没有调用者」此前每次都要逐个 grep 确认，很费轮次。以下为 2026-09-20 逐个 grep 的实测结果，**改动时请同步更新本节**。

**仍无前端调用点（6 个）**：

| 命令 | 说明 |
|---|---|
| `set_machine_id` | 设置页只提供读取与随机重置，不提供写入任意值 |
| `bind_account_machine_id` | 账号级机器码绑定，目前仅后端流程使用 |
| `set_trae_machine_id` | 设置页只展示 Trae 的 `machineid`；该文件由切换账号 / 清除登录状态写入 |
| `export_accounts`（无路径版） | 导出统一走 `export_accounts_to_path`（配文件对话框） |
| `update_account_token` | 前端靠 `get_account_usage` 触发 Token 刷新与回写 |
| `download_and_run_installer` | 更新改走 `tauri-plugin-updater` 的 `check_update` / `install_update` |

**本轮（2026-09-20）接通的 8 个历史闲置命令**：`get_machine_id`、`reset_machine_id`、`clear_accounts`、`export_logs_cmd`、`clear_logs_cmd`、`get_log_file_path_cmd`、`check_update`、`install_update` —— 全部接进设置页「机器码 / 日志 / 数据与备份」或关于页。

**从未注册的死封装（已删）**：`add_account`、`update_cookies`。注意与上表不同——上表是「后端有、前端没调」，这两个是「前端有封装、后端根本没注册」，调用必失败。详见 §5.5。

其余命令均有前端调用点（含 `get_usage_events`，由 `UsageEvents.tsx` 调用；`cancel_browser_login`，由 `AddAccountModal` 调用）。

### 5.11 设置项必须能指出消费方（2026-09-20 起）

新增任何 `AppSettings` 字段时，必须能回答「哪一行读它」。此前设置页挂着两个死开关——`auto_refresh_enabled` 存得下却没有任何定时器读它、刷新间隔连字段都不存在——根因就是没有这条约束。当前字段与消费方的对应关系：

| 字段 | 消费方 |
|---|---|
| `auto_refresh_enabled` + `refresh_interval` | `App.tsx` 的定时刷新 `useEffect`（窗口不可见时跳过、`switchInProgressRef` 置位期间跳过——切换是唯一持锁做网络请求的路径） |
| `auto_checkin_enabled` | `App.tsx` 自动签到 `useEffect` 的准入条件。**只作用于 GUI 启动路径**，`--silent` 无头模式保持「总是尝试」（那边没有界面，签到静默失败不影响用户） |
| `privacy_auto_enable` | `switch_account` 命令层（lib.rs），决定是否走「启动 IDE → 写隐私模式 → 二次重启」 |
| `auto_start_enabled` | `update_settings` 写 HKCU Run；启动时用落盘值重写一次（注册表写失败可自愈，故允许降级为日志警告） |
| `theme` | `ThemeSwitcher`（经 `App.tsx` 透传到 `Sidebar`）。存储已从 `localStorage` 迁入 settings.json；`theme` 为 `null` 表示从未设置过，前端据此从旧的 `trae_theme_v1` 迁移一次 |

### 5.12 窗口几何持久化（`src/window_state.rs`，2026-09-20 自研）

记录主窗口的尺寸/位置/最大化状态，落点 `%APPDATA%\hhj\trae-cc\config\window_state.json`（与 settings.json 同目录）。两个接入点都在 lib.rs：`setup` 里 `restore`（先恢复再 `show`）、`CloseRequested` 里 `save`（紧接着 `api.prevent_close()` + `process::exit(0)`）。

**不要改回 `tauri-plugin-window-state`（已试过并移除，根因如下）**：
- 插件的落盘挂在 `RunEvent::Exit` 上，而本应用主窗口关闭时是 `std::process::exit(0)` 直接终止进程 —— `Exit` 事件**永不触发**，自动保存等于不存在；
- 手动调它的 `save_window_state()` 试过一版：只返回一个被 `let _` 吞掉的 `Result`，磁盘上始终没有状态文件，也**没有任何观测点**能说明失败在哪一步（实测 `%APPDATA%\com.hhj.trae-cc` 目录都没被创建）。不把功能建立在查不出原因的第三方路径上。

自研实现的两条关键约定（都有单测覆盖）：
- **最大化时只翻转 `maximized` 标志，不覆盖尺寸/位置**：最大化状态下 `outer_position`/`inner_size` 报的是屏幕尺寸，原样存下来会让用户取消最大化后得到一个满屏大小的窗口；首次运行即最大化时存 0 尺寸表示「只恢复最大化，不动尺寸/位置」。
- **位置只在落在某个显示器内时才恢复**（`position_visible` 按窗口左上角判定，标题栏在屏外等于窗口丢了）：外接屏拔掉后旧位置可能指向不存在的屏幕；拿不到显示器列表时放行（宁可按记录恢复，也不要因一次枚举失败丢位置）。尺寸不受此限制，照常恢复。

坐标为物理像素（`outer_position` / `inner_size`），跨 DPI 缩放变化时按物理尺寸恢复，属已知取舍。验证方式（无需手拖窗口）：预写一份特征几何到该 json → 启动 → 用 Win32 `GetWindowRect` 量窗口矩形比对；或先跑一次再关闭，核对文件里的数值与实际几何一致。

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
12. **TraeWork 设备标识已按 `user_id` 补齐（跨应用统一），存量不一致需逐个重新保存**（2026-09-21 两次重写本条：上一版的「`machineid` 随快照走 = 一账号一设备」与「切 TraeWork 不改注册表 `MachineGuid`」均被实测推翻；本轮再修正为「设备标识归属 `user_id`，不归属记录/app」）。对齐只在用户主动点「保存当前登录态」时对该槽位生效，**刻意不做批量重置**（边界见 §5.9）；存量账号若仍显示「设备标识共用」，切到该账号重新保存一次即可。固有限制：① 注册表 `MachineGuid` 是**全局单值**，反映「最近一次切换的账号」——同一真实账号跨应用共用同一值（正是本轮要的效果），但**两个不同账号仍会互相覆盖**，两应用同时使用时无法同时成立，属方案固有约束而非缺陷；② 写 `HKLM` 需管理员权限，非管理员时该层降级为 `warn`（界面显示注册表与现场不一致）；③ **风控冷却期内不要执行对齐**——账号被标记（`Login Abnormality`）时改设备身份可能加重处罚。**仍未提供**「重置 TraeWork 机器码」入口：改标识的正当入口是「保存当前登录态」的按 `user_id` 对齐，不是手动轮换。TraeWork 是否有隐私模式键仍未调研。\n13. **TraeWork 存量快照不会自动瘦身**：白名单收窄只影响新快照，旧快照里仍躺着被排除的缓存（实测约 487MB/份，叠加 `.bak` 翻倍）。需用户点一次「修复槽位」才清理——刻意不做自动清理，因为「删除快照内容」这种破坏性动作不应在用户没察觉时发生。\n14. **uid 兜底路径仍有滞后风险**：若上游改了 auth 字段格式导致解密失败，判定会退回 `icube_gtm.users`，而该字段对「换号登录」滞后（正是 2026-09-20 覆盖事故的成因）。此时「备份后槽名自校验」会用同一个退化判据，防线失效。**判断信号**：面板「识别结果」里来源不是「已解密登录态确认」而是「来自 icube_gtm.users」时即处于该状态，此时新增账号前建议先核对客户端里实际登录的是谁。

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
