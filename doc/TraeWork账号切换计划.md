# TraeWork（TRAE SOLO CN）账号切换计划

> 调研对象：本机 TRAE SOLO CN `1.107.1`（iCube 内核 `2.3.85573`），实测日期 2026-09-19
> 参考实现：[smart-open/TraeWorkAssistant](https://github.com/smart-open/TraeWorkAssistant)（MIT，Tauri 2 + React + Rust）
> 目标项目：trae-cc（本文按「在 trae-cc 内扩展 TraeWork 支持」设计；若要独立成项目，第 4 节模块划分可整体搬走）
> 结论先行：**TraeWork 必须走「登录态快照 / 恢复」，不能沿用 trae-cc 现有对 traecode 的「改写登录态」方案。**
> 原因是 TraeWork 的登录真源是 `storage.json` 与 `state.vscdb` **双源**，而 `state.vscdb` 是带加密 secret storage 的 SQLite，无法靠写 JSON 伪造。

---

## 1. 本机 TraeWork 文件系统调研

### 1.1 基础事实（实测）

| 项 | 值 |
|---|---|
| 应用名 | TRAE SOLO CN（用户口语称 TraeWork） |
| 安装目录 | `D:\TRAE SOLO CN\`（非默认 `%LOCALAPPDATA%\Programs`，自定义安装） |
| 主进程 | `TRAE SOLO CN.exe`（Electron 多进程，实测 30+ 子进程） |
| 数据目录 | `%APPDATA%\TRAE SOLO CN` |
| 应用版本 | `resources/app/package.json` → `1.107.1` |
| 内核版本 | `storage.json` → `iCubeLastVersion` = `2.3.85573` |
| 单实例锁 | `code.lock`（**当前不存在**——客户端退出时会清理，可作「是否运行」的辅助判据） |

> 注意与 traecode 的区别：traecode 是 `%APPDATA%\Trae CN` + `Trae CN.exe`，两者**完全独立**，互不影响。此前会话中已确认这两个是不同应用，不要混淆路径。

### 1.2 登录态相关文件清单

按「是否承载登录身份」分类（这是快照白名单的选型依据）：

| 路径（相对数据目录） | 内容 | 是否登录态 | 本机实测 |
|---|---|---|---|
| `User\globalStorage\storage.json` | 认证信息 + 设备标识 + 遥测 | **是（真源之一）** | 11.7 KB |
| `User\globalStorage\state.vscdb` | 登录令牌数据库（secret storage） | **是（真源之二）** | 1216 KB |
| `User\globalStorage\state.vscdb.backup` | 主库备份 | **是** | 1216 KB |
| `User\globalStorage\state.vscdb-wal` / `-shm` | SQLite WAL 边车 | **是（易被漏掉）** | 运行时存在 |
| `machineid` | 机器标识（UUID） | **是（风控维度）** | 36 B，`65e414a1-…` |
| `aha\TinyStorage` | aha 层设备标识（加密 blob） | **是（风控维度）** | 1.1 KB |
| `aha\Remote_State` | feature flag | 否 | 2.2 KB |
| `Local State` | Chromium 加密密钥等 | 弱相关 | 434 B |
| `Preferences` | 客户端偏好 | 否 | 3517 B |
| `Local Storage\config.db` | 应用级配置（非 leveldb） | 弱相关 | — |
| `Local Storage\*.ldb` / `.log` | leveldb，web 侧 KV | **是（web 登录缓存）** | 多个 |
| `Network\Cookies` | Cookie | **是** | — |
| `Network\Cookies-journal` | Cookie 日志 | **是** | — |
| `Session Storage\` | 会话存储 | 弱相关 | — |
| `Partitions\trae-webview\` | 独立 profile（含自己的 Network / Local Storage / IndexedDB） | **是（未验证）** | 存在 |
| `Partitions\icube-web-crawler-shared-session-v1.0\` | 爬虫共享会话 | **是（未验证）** | 存在 |
| `ahanet\prefs\local_prefs.json` | Chromium http_server_properties | 否 | 8.1 KB |
| `ModularData\database.db` | 对话/业务数据（**绑定账号与路径**） | 业务数据，非登录态 | — |
| `Workspaces\` / `solo-lite\` | 工作区与缩略图 | 否 | — |

### 1.3 `storage.json` 键级分析（21 个键，实测）

```
telemetry.machineId / telemetry.sqmId / telemetry.devDeviceId   ← 设备标识三件套
has_device_id_updated_to_aha                                     ← 设备号已迁移到 aha 的标志（true）
iCubeAuthInfo://icube.cloudide                                   ← ★ 登录凭据（tc 密文，2484 字符）
iCubeAuthInfo://usertag                                          ← tc 密文，224 字符
iCubeAuthInfo://icube-dc:4179714300984316                        ← ★ 设备私钥（tc 密文，776 字符）
iCubeServerData://icube.cloudide                                 ← 明文 JSON（4459 字符，权益/套餐）
icube_gtm                                                        ← {"users":{"168695880747001":{…}}} ★ uid 线索
iCubeLastVersion / iCubeInstallAction / iCubeNativeAppFirstStart  ← 安装与版本标记
theme / themeBackground / windowSplash / windowsState / …         ← 界面状态
backupWorkspaces / profileAssociations / solo-lite.windows.trayNoticeShown
```

三个 `tc` 密文键的格式（此前已验证）：`base64( [6B 头 74 63 05 10 00 00] [32B 随机数] [AES-128-CBC 密文] )`，明文前 64 字节是 SHA-512 摘要。trae-cc 现已具备**加密写入**能力（`src-tauri/src/tc_crypto.rs`），但这只解决 `storage.json` 一侧。

两个关键细节：

- **`iCubeAuthInfo://icube-dc:<deviceId>` 的 `<deviceId>` 不是账号 uid**，而是 OAuth 设备凭证 id，值里是 EC P-256 私钥 PEM（DeviceProof 签名用）。参考项目明确标注「不能入池」——把它当账号 id 会得到错误的账号列表。
- **`iCubeEntitlementInfo://icube.cloudide` 在 TraeWork 中不存在**（traecode 有）。跨应用写 storage.json 时不能照搬 traecode 的键集合。

### 1.4 与 traecode 的差异对照

| 维度 | traecode（`Trae CN`） | TraeWork（`TRAE SOLO CN`） |
|---|---|---|
| 数据目录 | `%APPDATA%\Trae CN` | `%APPDATA%\TRAE SOLO CN` |
| 进程名 | `Trae CN.exe` | `TRAE SOLO CN.exe` |
| `iCubeEntitlementInfo` | 有（明文） | **无** |
| 设备私钥键 | 有（`icube-dc:*`） | 有 |
| `solo-lite.*` 系列键 | 无 | 有 |
| 登录态能否纯写 JSON 伪造 | **能**（实测已修复并生效） | **不能**（双源，vscdb 不可伪造） |
| 磁盘占用（登录态相关） | ~1.5 MB | **~2.5 MB+**（含两个 1.2 MB 的 vscdb） |

**这张表就是本方案的分水岭**：traecode 能靠 `tc_crypto` 写 `storage.json` 实现秒切；TraeWork 不行。

---

## 2. 参考实现拆解（TraeWorkAssistant）

### 2.1 整体路线：整目录快照替换

参考项目在 `src-tauri/src/switcher/` 下做了表驱动设计（5 应用 × 3 布局），TraeWork 属 `Layout::Icube`，与 Trae 共用同一套快照管线。核心思路：

```
切到账号 X：
  1. 关掉客户端
  2. 把「当前现场」备份到 profiles/last          ← 安全回退槽
  3. 把 profiles/<X> 的快照覆盖回现场
  4. 校验恢复结果，失败则用 last 回滚
  5. 启动客户端
```

**全仓没有任何 `iCubeAuthInfo` 的加密写入路径**（`icube_auth.rs` 只有 `tc_decrypt`），也没有直接改写登录态的代码——他们试过之后选了快照。这本身是很强的信号。

### 2.2 快照白名单（14 项，`switcher/icube.rs`）

```
User\globalStorage\storage.json
User\globalStorage\state.vscdb
User\globalStorage\state.vscdb-wal        ← 边车必须随主库
User\globalStorage\state.vscdb-shm
User\globalStorage\state.vscdb.backup
machineid
aha\                                       ← 整个目录
Preferences
Local State
Local Storage\leveldb\
Local Storage\config.db
Network\
Partitions\trae-webview\
Partitions\icube-web-crawler-shared-session-v1.0\
Session Storage\
```

这份白名单与本机 §1.2 的实测清单高度吻合，**可以直接作为 trae-cc 的起点**（`Partitions\*` 两项参考项目自己也标注了未完全验证，建议先纳入、出问题再摘）。

### 2.3 切换编排（`switcher/mod.rs::switch_flow`）

| 步 | 动作 | 关键点 |
|---|---|---|
| 1 | `action_gate().try_lock()` | 全局串行，进行中直接拒绝（防连点） |
| 2 | 入参校验 | uid 必填 + 路径安全校验 |
| 3 | `Session::new` | 由 `profile_for` 决定数据目录/快照目录/布局 |
| 4 | **预检快照是否存在** | 缺失即 fatal——**此时还没杀客户端**（不破坏用户当前状态） |
| 5 | `proc::stop_app` | 关客户端 |
| 6 | `backup_current(sess, "last")` | 现场先存 last 槽，留退路 |
| 7 | 防误覆盖守卫 | 读 `current_account.txt`，仅当「预期 uid == 记录的当前 uid」才回写账号槽，否则 warn 跳过 |
| 8 | `restore_profile(uid)` | 先抽 vscdb 全局键 → 恢复快照 → 合并回写全局键 |
| 9 | **恢复后校验** | 恢复项数 ≤0 或缺 `storage.json`/`state.vscdb` → 回滚 last + 启动 + fatal |
| 10 | `set_current_account` → `start_app` | 写标记 + 启动 |

「先预检、后杀进程」和「恢复后校验 + 回滚」是两条很值得抄的工程约束。

### 2.4 账号 uid 从哪来（`commands/accounts.rs`）

TraeWork 的自动发现流程：

1. 读 `%APPDATA%\TRAE SOLO CN\User\globalStorage\storage.json`
2. 主线索：`icube_gtm.users` 的**键名**（本机实测为 `168695880747001`）——权重最高
3. 交叉证据：`state.vscdb` 的 `ItemTable` 里 `solo.mobile.allowControl` 的 per-uid `updatedTime`、`solo-lite-mode-state-map-<uid>`、`:user:<uid>` 前缀键
4. 与桥标记 `current_account.txt` + `meta.json` 比对，证据更新则信证据
5. `uid_confident == false` 时**拒绝入池**（宁可少识别，不可错识别）

这条「多证据 + 置信度门槛」的做法，正好可以解决 trae-cc 里账号 `email` 为空时的识别问题（本机账号库中两个账号的 `email` 都为空，见 §3.3）。

### 2.5 进程关闭三级（`switcher/proc.rs`）

```
① 精确映像名匹配（注意：sysinfo 返回的名字带 .exe，要先剥后缀——他们曾因没剥导致「永远匹配不上、从不关客户端」）
② PostMessageW(WM_CLOSE) 对每个顶层窗口发关闭消息，等 graceful_wait_secs
③ 仍未退出 → kill_all，再等 ≤5s
```

`graceful_wait_secs` 对 TraeWork 设的是 **8 秒**（Trae/Doubao 同为 8，WB/CB 为 5）。

### 2.6 已踩过的坑（比代码更值钱）

| # | 坑 | 后果 | 对策 |
|---|---|---|---|
| 1 | 优雅等待只给 3 秒 | 3 秒恒超时 → 每次强杀 → vscdb WAL 残留 | 提到 8 秒 |
| 2 | 恢复前不删 `state.vscdb-wal/-shm` | 客户端启动回放旧 WAL，**旧账号复活**（"切换后账号不变"的根因） | 恢复前先删边车 |
| 3 | 不删 `code.lock` | 启动冲突 | 恢复前删除 |
| 4 | 覆盖快照无保护 | 拷贝中断即永久丢快照 | 覆盖前把旧槽挪成 `.bak`（单代回滚） |
| 5 | 恢复只做「覆盖」不做「删除」 | 槽位缺的文件保留现场残留（如旧账号的 `state.vscdb.backup`） | **对称恢复**：槽位没有的项，删掉现场同名项 |
| 6 | exe 发现把「运行中进程」放最高优先级 | 残留/错误的 `Trae CN.exe` 被优先采用，启动错应用 | 运行中进程降为第 5 级回退，且必须过 `exe_names` 白名单防串台 |

---

## 3. 方案选型

### 3.1 三个候选

**方案 A：登录态快照 / 恢复（参考项目路线）**

- 做法：每账号一份白名单快照，切换时整组替换
- 优点：覆盖 `state.vscdb` 等全部登录态，**已被他人实测验证**；机器码与 aha 随快照走，天然做到「一账号一设备」；不需要理解任何加密格式
- 缺点：磁盘占用（每账号约 3–8 MB，需实测）；切换必须关客户端（约 10–20 秒）；首次录入必须先登录一次

**方案 B：`tc` 加密写入 `storage.json`（trae-cc 现有路线）**

- 做法：复用 `tc_crypto.rs`，把新账号凭据加密写进 `storage.json`
- 优点：秒切、无需关客户端、零额外磁盘
- 缺点：**`state.vscdb` 覆盖不到**。TraeWork 的 token 同时存在 vscdb 的 secret storage 里，客户端可能以 vscdb 为准 → 极可能表现为「切换后仍要重新登录」，与 traecode 的修复效果不可类比
- 结论：**风险过高，不建议作为主方案**；可作为「快照恢复后补写 storage.json」的辅助手段

**方案 C：快照为主 + 加密写入兜底**

- 以 A 为主干，在恢复完成后用 `tc_crypto` 把 `storage.json` 里的 `iCubeAuthInfo` 重写为账号库中的最新 token（解决「快照里的 token 已过期、而账号库里有刷新后的 token」）
- 这是 A 的自然增强，**推荐作为最终形态**，但应放在 A 跑通之后再做

### 3.2 推荐结论

**先做 A，验证通过后再叠加 C。** 不建议尝试 B。

理由收敛成一句话：**TraeWork 的登录态不是「一个 JSON 文件」，而是一组带加密数据库的状态**——凡是试图只改写其中一个文件的方案，都要额外证明另一个不会覆盖它，而快照方案不需要证明任何东西。

### 3.3 与 trae-cc 现有资产的复用关系

| trae-cc 现有资产 | 能否复用 | 说明 |
|---|---|---|
| `AccountManager` / `accounts.json` | **可复用** | 需给 `Account` 加 `app` 字段区分应用 |
| 前端账号卡片 / 列表 / 详情 | **可复用** | 需加应用标签与「快照存在性」状态 |
| `machine.rs` 的进程 kill/open | **部分复用** | 进程名、路径需按应用参数化 |
| `tc_crypto.rs` | **可复用（阶段 C）** | 目前只有加密写入 |
| `switch_trae_account` 的「清目录 + 写登录态」 | **不可复用** | 语义与快照方案相反 |
| `privacy.rs` | 不适用 | TraeWork 未调研隐私模式键 |

另外注意：本机账号库里两个账号的 `email` 字段都是空的（`user_id` 也是空），说明现有账号主要靠 `machine_id` 与内部 id 区分。TraeWork 接入后建议**以 uid 作为主键**（§2.4 的证据链），不要沿用空 email。

---

## 4. 实施计划

### 阶段 1：数据模型与存储

1. `src/types/index.ts` + `src-tauri/src/account/types.rs`：`Account` 增加
   - `app: String`（`"traecode"` / `"traework"`，默认 `"traecode"`，保证旧数据兼容）
   - `uid: Option<String>`（TraeWork 的 Cloud-IDE uid）
   - `snapshot_slot: Option<String>`（快照槽名，默认等于 uid）
2. 快照根目录：`%APPDATA%\hhj\trae-cc\data\profiles_traework\<uid>\`
3. 当前账号标记：`%APPDATA%\hhj\trae-cc\data\profiles_traework\current_account.txt`
4. **兼容性红线**：`accounts.json` 只做增量字段，`serde(default)` 兜底，禁止改已有字段语义

### 阶段 2：Rust 后端模块（新增 `src-tauri/src/traework/`，每个文件控制在 800 行内）

| 文件 | 职责 | 预估有效行 |
|---|---|---|
| `mod.rs` | 编排（对应参考项目 `switch_flow` 的 10 步）+ 串行锁 | ~250 |
| `profile.rs` | TraeWork 档案常量（数据目录/进程名/exe 名/等待秒数） | ~80 |
| `snapshot.rs` | 白名单定义 + 备份/恢复/对称删除/`.bak` 轮转 | ~300 |
| `locate.rs` | exe 发现（六级，运行中进程降到第 5 级） | ~180 |
| `uid.rs` | uid 证据链推导 + 置信度门槛 | ~200 |
| `proc.rs` | 三级关闭 + 分离启动 | ~200 |

设计约束：
- **不复制** `machine.rs` 里现有的 `switch_trae_account`（它已 155 行重复清理逻辑待重构），新模块独立，避免把技术债搬过来
- 编排层对外只暴露一个 `switch_to(uid)`，锁的粒度、回滚策略都收在里面
- 所有失败路径都要落到 `anyhow` + `ApiError`，且**回滚必须执行**（参考项目把「回滚 last」写进了 fatal 路径）

### 阶段 3：命令层（`lib.rs` 注册 + `api.ts` 封装 + `types/index.ts` 同步）

| 命令 | 作用 |
|---|---|
| `traework_discover_accounts` | 读现场登录态，推导 uid，返回可录入账号（含置信度） |
| `traework_save_current_login` | 「保存当前登录态」→ 关客户端 → 快照到 `profiles_traework/<uid>` → 启动 |
| `traework_switch_account` | 切到指定 uid（完整 10 步编排） |
| `traework_snapshot_status` | 列出各槽位存在性/大小/时间，供前端展示 |
| `traework_delete_snapshot` | 删除某槽位（释放磁盘） |

按 AGENTS.md 的「新增命令 checklist」三步走，缺一不可。

### 阶段 4：前端

- 账号卡片/列表加**应用标签**（TraeCode / TraeWork），支持按应用筛选
- TraeWork 账号卡片增加「快照」状态（存在 / 缺失 / 大小）
- 切换按钮：切换期间禁用 + 进度提示（TraeWork 切换需 10–20 秒，**必须给明确进度**，否则用户会像上次 traecode 那样连点）
- 设置页增加「TraeWork 路径」（自定义安装如 `D:\TRAE SOLO CN`，不能只靠默认候选路径）

### 阶段 5：验证

见第 6 节验收清单。**注意**：按仓库安全边界，改注册表/杀进程的验证动作只在你自己机器上手动触发，不由 AI 代为执行。

---

## 5. 风险与边界

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| 1 | `Partitions\trae-webview\` 与 `icube-web-crawler-shared-session-v1.0\` 未验证是否含登录态 | 漏拷 → 切换后仍需登录；误拷 → 快照体积翻倍 | 先纳入白名单跑通；若体积不可接受，再按「切换前后 mtime/大小是否变化」判断取舍（**无证据不盲删**，与 trae-cc 现有 `Partitions` 结论一致） |
| 2 | 快照里的 token 过期 | 切过去仍要重新登录 | 阶段 C：恢复后用账号库中的新 token 重写 `storage.json` |
| 3 | 每账号 3–8 MB 磁盘占用 | 10 个账号约 30–80 MB | 前端展示占用 + 提供删除；只快照白名单，不做整目录拷贝 |
| 4 | TraeWork 与 traecode 的 aha/设备标识互相污染 | 风控关联 | 两应用数据目录天然隔离，只需保证各自 `machine_id` 独立管理 |
| 5 | 上游版本升级导致白名单失效 | 切换后要重新登录 | 白名单集中在 `profile.rs` 一处，便于版本变更时核对 |
| 6 | 用户误以为「切换失败」而连点 | 重复杀进程 | 全局串行锁 + 前端进度 + 明确失败原因 |

**明确不做的事**（与 trae-cc 现有边界一致）：

- 不批量重置设备标识、不做账号倒卖用途
- 不在 AI 侧执行杀进程/改注册表来「验证」
- 不改 TraeWork 的 `ModularData`（业务数据绑定账号与路径，跨账号替换会串数据）

---

## 6. 验收清单

跑通标准（全部满足才算完成）：

- [ ] 账号 A 登录 TraeWork → 「保存当前登录态」→ 快照生成且白名单 15 项齐全（项数差异见 §8.2）
- [ ] 账号 B 登录 → 保存 → 从 B 切回 A：**TraeWork 直接进入已登录状态，不出现登录页**
- [ ] A → B → A 连续来回切 3 轮，每轮均免登录
- [ ] 切换后 `state.vscdb-wal/-shm` 不残留旧账号数据（旧账号不复活）
- [ ] 两账号的 `machineid` 与 `aha\TinyStorage` 各自独立且稳定（不因切换而变化）
- [ ] 切换过程有进度反馈，命令不长时间无响应（吸取 traecode 的教训）
- [ ] 快照体积实测值记录到本文档（替换 §5 风险 3 的估算）
- [ ] `accounts.json` 旧数据（无 `app` 字段）加载正常，不丢账号

---

## 7. 遗留问题（需实测确认，不要凭推测填）

1. `Partitions\*` 两个目录到底是否承载登录态 —— 需对比切换前后 mtime/大小
2. 单账号快照的真实体积 —— 需实际生成一次快照后 `du`
3. TraeWork 是否有类似 traecode 的「隐私模式」键 —— 本次未调研
4. 参考项目 `current_cloud_uid_hybrid` 的 vscdb 证据在**本机**是否成立（本机 `icube_gtm.users` 只有一个 uid，需双账号实测才能验证证据链的区分度）
5. `solo-lite` 与 `ModularData` 是否需要「跨账号保留」（工作区列表这类非登录态数据，理想是切换时保留，需单独设计）

---

## 附：关键结论速查

```
TraeWork 数据目录   %APPDATA%\TRAE SOLO CN
TraeWork 进程       TRAE SOLO CN.exe
TraeWork 安装       D:\TRAE SOLO CN（自定义路径，需支持手动配置）
登录真源            storage.json + state.vscdb（双源）
storage.json 凭据键 iCubeAuthInfo://icube.cloudide（tc 密文）
设备私钥键          iCubeAuthInfo://icube-dc:<deviceId>（≠ 账号 uid，不可入池）
uid 线索            icube_gtm.users 键名 + vscdb 证据
推荐方案            整目录快照 / 恢复（15 项白名单）
绝对不要           只写 storage.json 就以为切好了
```

---

## 8. 实施状态（2026-09-19 落地）

**决策落定**：① TraeWork 支持**在 trae-cc 内扩展**（不另起项目）；② 技术路线按参考项目
`smart-open/TraeWorkAssistant` 走**快照 / 恢复**；③ 用户已实测 TraeCode 侧「切换后免登录」
通过（`tc_crypto` 加密写入路线收工，不再调整）。本节记录实际实现与上文计划的差异，
**上文未标注的偏差以本节为准**。

### 8.1 代码落点

| 计划中的模块 | 实际实现 |
|---|---|
| `traework/mod.rs` | 编排层：`save_current_login` / `switch_to`（预检 → 停止 → last 备份 → 恢复 → 校验回滚 → 标记 → 启动）、`StepStatus`/`ProgressSink`/`StepCollector`、全局串行 `try_lock` 闸门 |
| `traework/profile.rs` | 常量 + 15 项白名单 + `REQUIRED_ITEMS` + `Ctx`（把「数据目录 / 快照根」显式化为参数，使破坏性操作可在临时目录上测试）+ `ensure_slot_safe` |
| `traework/snapshot.rs` | `copy_item` / `rotate_bak` / `resolve_slot` / `backup` / `restore` / `verify_restore` / `slot_status` / `delete_slot` / `list_slots` / 当前账号标记读写 |
| `traework/proc.rs` | 三级关闭（`EnumWindows`+`WM_CLOSE` → `taskkill /F` → 等待）+ 启动；进程枚举用 `tasklist`（**未引入 `sysinfo`**，复用仓库既有手段） |
| `traework/locate.rs` | 五级发现：已保存路径 → 常见候选 → 卸载注册表 → 运行中进程（PowerShell）。**运行中进程刻意排在最后**（参考实现放第一位曾导致启动错的应用） |
| `traework/uid.rs` | `icube_gtm.users` 键名 + vscdb per-uid 键名计数消歧 + 置信度门槛 |
| `traework/commands.rs` | 7 个命令（定义在此、注册在 lib.rs，避免继续撑大 `lib.rs`） |
| 前端 | `src/components/TraeworkPanel.tsx` + 同名 CSS，侧边栏独立页「TraeWork」 |

### 8.2 与计划的差异（有意为之）

1. **白名单 14 → 15 项**：拆出 `User\globalStorage\state.vscdb.backup` 单列（原计划把它含在 `state.vscdb` 的语境里）。对照本机实测的 21 个 storage.json 键后确认无遗漏。
2. **当前账号真源用 `current_account.txt`，不写 `AccountStore.current_account_id`**：后者是 TraeCode 的「Trae IDE 当前账号」，两个应用可同时登录不同账号，共用会互相污染。
3. **进度反馈不用 Tauri 事件流**：本仓库此前零 `emit` 用法。改为「进行中遮罩 + 已耗时秒表 + 结果回传分步日志」。计划要求的「必须给明确进度、防连点」已满足，事件流留作将来有第二个订阅方时再引入。
4. **阶段 3 兜底未实现**（恢复后用新 token 重写 `storage.json`）：TraeWork 侧凭据只存在于 vscdb 与密文里，当前没有可信的 token 来源可用。已记入 AGENTS.md §7 限制 9。
5. **TraeWork 账号不进 TraeCode 链路**：`is_traecode()` 判据加到签到列表、单账号签到、Token 刷新、额度查询四处；账号管理页/统计页/批量操作只处理 TraeCode 账号。

### 8.3 验证情况

**已通过（AI 侧，静态与单元层）**：

- `cargo test --lib`：**36 个测试全绿**（其中 23 个为本次新增，覆盖槽位名路径穿越拒绝、进程名白名单、快照往返含边车清除与对称恢复、`.bak` 轮转与回退、零项恢复拦截、uid 证据链与并列拒绝、BOM 剥离）
- `cargo check --lib --tests`：零警告
- `npx tsc --noEmit`：零错误
- `npm run tauri build`：成功，产物 **7.59 MB**（Tauri CLI 构建、已内嵌前端；`#[warn(linker_messages)]` 为 MSVC 已知噪音，非错误）
- 旧 `accounts.json`（无 `app` 字段）兼容：由 `#[serde(default = "default_app")]` 落为 `traecode`

**待用户实测（AI 不代为执行杀进程动作）**：即第 6 节验收清单中带 🔬 标记的各项，尤其是
「B 切回 A 后不出现登录页」与「连续来回切 3 轮」——这两项是方案成立与否的判据。

### 8.4 第 7 节遗留问题的状态

1. `Partitions\*` 是否承载登录态 —— **仍未实测**；已按计划先纳入白名单。
2. 单账号快照真实体积 —— **仍未实测**（需生成一次快照）；界面已展示体积，实测后回填上文 §5 风险 3。
3. TraeWork 隐私模式键 —— **确认未调研**，TraeWork 不做隐私模式。
4. vscdb 证据链区分度 —— **单账号已验证，双账号未实测**；多账号时若提示「置信度不足」属设计内保守行为。
5. `solo-lite` / `ModularData` 跨账号保留 —— **未实现**，仍在明确不做之列。
