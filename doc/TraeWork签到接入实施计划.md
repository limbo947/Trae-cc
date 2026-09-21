# TraeWork 账号签到接入实施计划

> 依据：`AGENTS.md` §5.7（签到）/ §5.9（TraeWork）；`doc/TraeWork账号切换计划.md`；
> `doc/签到系统优化实施计划.md`；参考实现 3.x 对照结论（`smart-open/TraeWorkAssistant` main 分支）。
> 结论先行：**不需要移植任何签到代码**——本仓库 `api/checkin.rs` 与参考实现 `tasks/trae_checkin.rs` 是
> 同一份协议的两份实现（同一对 `api.trae.cn/trae/api/v2/ug/checkin_credits/{status,claim}`，
> 同一套 9074 / 1005 / 9004 / 9095 / 401 语义，同样「先 status 再 claim」）。本计划做的是
> **给 TraeWork 接一条凭据取值分派**，而不是搬协议。

## 全局纪律（沿用既有实施计划）

- 注释解释「为什么」，不复述「做什么」；Token / Cookie / 设备号一律不落日志（设备号按 `mask_device_id`）。
- 新逻辑进新模块；`account_manager.rs`（有效 1160 行，已超 800 上限）**只允许删行、不加行**，
  唯二例外为本计划点名的两处：阶段 2 任务 2 删 `is_traecode` filter（净删行）、阶段 3 任务 4/5 在
  `upsert_traework_account` 内的冷却清除 + 设备号落盘块（合计净增约 10 行，豁免理由见该两任务）。
- 编译验证：`cargo check`（工作目录 `src-tauri`）+ `npx tsc --noEmit`（项目根）。
- 打包验证必须 `npm run tauri build`（裸 `cargo build --release` 产出开发态二进制，禁止用于验证）。
- 新增命令三步 checklist：`#[tauri::command]` + `generate_handler!` 注册 → `api.ts` 封装 → `types/index.ts` 同步类型。
- 阶段 1–3 全部落在既有模块上，**每阶段结束跑一次 `cargo test --lib`**（当前 48 绿）。

## 事实基线（已实测，勿再作为假设推翻）

| 事实 | 出处 |
|---|---|
| TraeWork `storage.json` 的 `iCubeAuthInfo://icube.cloudide` 密文内含可用 `token`（access 14 天 / refresh 180 天） | 既有调研归档；`uid.rs` `credentials_from_storage` |
| 该 token 被 CN `.../checkin_credits/status` **直接接受**（HTTP 200，`checked_in:false`，`credits:150`） | 2026-09-20 只读探针 |
| `claim` **未实测**（会消耗该账号当日 150+50 积分） | 同上 |
| `Ctx::from_env()` 只依赖 `APPDATA` 与 `ProjectDirs`，**不依赖 Tauri** → `--silent` 无头模式可用 | `traework/profile.rs:74` |
| 签到设备号 `device_id` 对 TraeWork 恒为 `None`，运行时按 `user_id`(=uid) 派生 | `types.rs:163`、`device_id.rs:49` |
| 前端 TraeWork 账号被 `accounts` 过滤出列表与批量操作，签到入口够不着 | `App.tsx:957` |

---

## 阶段 0：前置实测（不通过则不进入后续阶段）

**目标**：确认 `claim` 对 TraeWork 凭据真的生效，并确认请求头形态（UA 是最可能的不一致点）。

### 任务

1. 写一个**临时探针**（跑完即删，不进版本库；沿用 2026-09-20 只读探针的做法）：
   - 用 `uid::slot_credentials(&ctx, "<已保存的槽>")` 解出 token（**探的是当前槽时必须读
     `uid::live_storage(&ctx)` 现场**——快照里可能是旧凭据；只有非当前槽才读快照，
     优先级语义与 `commands.rs:158-167` 一致）；
   - 先打 `status` 记录余额（`credits` / `extra_credits`），再打 `claim`，最后再打一次 `status` **对照增量**；
   - 打印 HTTP 状态 + 响应正文（正文里没有凭据，可直接打印）。
2. 若 `claim` 返回异常码，做**单变量对照**（每次只改一项，失败不叠加变量）：
   - 头 A（现状）：`User-Agent: Trae/0.1.52`（`checkin.rs:222`）；
   - 头 B（参考实现）：`User-Agent: VSCode 1.107.1 (TRAE SOLO CN)` + `x-market-client-id: VSCode 1.107.1`。
     **参考实现用的是 B**，我们用 A 能过 `status` 但 `claim` 未验证——这是本阶段唯一真正的不确定点。
3. 结论（含原始响应）回写本文件「实测记录」段与既有调研归档。

### 验收

- 拿到 `claim` 的成功/失败原文，并明确「头 A 可用 / 需切头 B / 两者皆不可用」。
- 不通过（例如恒 1005 权益不足）时**停下并上报**，不进入阶段 1。

### 回滚

删除探针文件即可，无生产代码改动。

---

## 阶段 1：抽凭据解析为公共函数（无行为变化的纯重构）

**目标**：把「按槽位取 TraeWork 凭据」从命令闭包里提出来，供积分与签到共用，消除两份实现的漂移风险。

### 任务

1. 新建 `src-tauri/src/traework/credentials.rs`（预估 70–90 有效行）：
   - `pub fn resolve_token(ctx: &Ctx, slot: &str) -> Result<String, String>`：
     优先级 **当前槽读实时现场 → 主槽快照 → `.bak`**（与 `commands.rs:158-167` 完全一致，语义照搬）。
   - 为什么单独成模块而不是塞进 `uid.rs`：本函数依赖 `snapshot::read_current_slot`（编排层），
     而 `uid.rs` 目前只依赖 `profile::Ctx`；塞进去会让解析层反向依赖快照层。
   - 为什么必须抽出来：这段优先级逻辑现在写在 `traework_credits` 的 `spawn_blocking` 闭包里，
     签到侧够不着——重写一遍必然与积分侧漂移（历史上 `auth_plaintext` 就漂移过一次）。
2. `traework/commands.rs`：`traework_credits` 的闭包体替换为调用它，**错误文案保持不变**
   （「快照里没有可用凭据…请切换到 X 并重新保存」是用户唯一可操作的提示）。
3. 单元测试（用 `traework::test_support::fake_ctx`）：
   - 当前槽 → 读实时现场；
   - 非当前槽 → 读主槽快照；
   - 主槽缺失、`.bak` 存在 → 读 `.bak`；
   - 三处都无 token → `Err` 且文案含「重新保存」。

### 验收

- `cargo test --lib` 全绿（48 + 新增 4）。
- GUI 实测 `traework_credits` 行为不变（面板积分标签仍显示，不出现「积分 未知」）。
- `traework/mod.rs` 加 `pub mod credentials;`。

### 回滚

独立新模块 + 一处调用替换，还原 `commands.rs` 即可。

---

## 阶段 2：签到链路按 app 分派凭据（核心改动）

**目标**：TraeWork 账号进入签到批次，凭据走快照解析；无凭据时明确跳过而不是报失败。

### 任务

1. `src-tauri/src/api/checkin.rs`：
   - **批次级解析凭据**：新增 `async fn resolve_traework_tokens(accounts: &[Account]) -> HashMap<String, String>`，
     内部 `tauri::async_runtime::spawn_blocking` + `Ctx::from_env()` **一次**，只处理 `app == traework` 的账号。
     - 为什么批次级而不是账号级：读文件是阻塞 IO，账号级要每个账号一次 `spawn_blocking`；
       且批次期间的 30/90s sleep 不会改变快照内容，重试轮直接复用同一份结果即可。
     - 锁纪律：整个解析在**不持 `account_manager` 锁**的前提下完成（全仓「不持锁做 IO」约定）。
     - `Ctx::from_env()` 失败策略：**不报错、不阻断批次**——全部 TraeWork 账号按无凭据记
       `Skipped`（detail 注明环境原因），traecode 账号照常进行。from_env 只依赖 APPDATA 与
       ProjectDirs，失败属环境级异常，不该让 traecode 的既有签到跟着一起失败。
   - `run_round(manager, accounts, tokens: &HashMap<String, String>)` 扩展签名；
     `run_round` 的三处调用点（`checkin_one_account` / `checkin_all` / `auto_checkin_pending`）同步。
   - `process_account(client, manager, account, token: Option<&str>)` 分派：
     - `traework` → 用传入 token；**无 token 记 `Skipped`，直接返回，不发任何请求**；
     - `traecode` → 现状不动（`jwt_token` → 空则 cookies 刷新）。
   - `CheckinError` 新增变体 `Skip(String)`：`trigger()` 返回 `None`、`state()` 返回 `Skipped`、`retryable()` 为 `false`。
     - 为什么新增 `CheckinState::Skipped` 而不是复用 `Failed`：这不是失败（不该进失败汇总、不该弹红字提示），
       也不是 `Cooldown`（没有冷却语义，不该显示冷却徽标）。参考实现同样用 `skip_reasons` 单列。
     - `collect_outcomes` 中 `Skipped` 与 `Cooldown` 一样**不产生落盘项**（既不写日期也不写冷却）。
2. `src-tauri/src/account/account_manager.rs:1587-1598`：`list_accounts_for_checkin` **删掉**
   `.filter(|a| a.is_traecode())` 一行（含其注释），注释改为「过滤条件只剩 is_active 与日期」。
   - 这是本阶段唯一允许在超限文件里的改动，且是**净删行**。
3. `src-tauri/src/api/checkin.rs:575-579`：删掉单账号入口的 `is_traecode` 拒绝，
   改为「凭据解析失败 → 返回 `Skipped` 结果」（返回 `Ok`，不是 `Err`——它不是一个错误，是一次有理由的跳过）。
4. 批次范围参数化（为阶段 4 的入口铺路）：把 `checkin_all` 的主体抽成
   `async fn checkin_scope(manager, app: Option<&str>)`；
   - **app 过滤的落点固定在 `checkin.rs`**：`list_accounts_for_checkin` 删掉 `is_traecode` 后返回
     全部 app，由 `checkin_scope` 在锁外按 `app` 参数 `retain`（`None` 不过滤）。
     不给 `list_accounts_for_checkin` 加 app 参数——那会向超限的 `account_manager.rs` 加行，
     而过滤是纯调用方语义，留在 `checkin.rs` 零成本。
   - `checkin_all` 传 `Some(APP_TRAECODE)` —— **保持 TraeCode 工具栏「全部签到」语义不变**（只签 TraeCode）；
   - 新增 `checkin_all_traework` 传 `Some(APP_TRAEWORK)`（阶段 4 注册命令）；
   - `auto_checkin_pending` 传 `None`（是否纳入待拍板，见末节）。

### 验收

- **TraeCode 行为零变化**：手动签到、全部签到、自动签到、冷却、批次重试全部与改动前一致（对照实测）。
- TraeWork 账号在 `checkin_scope(Some("traework"))` 下：有凭据 → 正常走 status→claim；无凭据 → `skipped`，
  且 `accounts.json` 中**既不写 `last_checkin_date` 也不写 `checkin_cooldown`**。
- 日志中不出现 token / cookie（只出现账号名、uid、脱敏设备号）。

### 回滚

恢复 `is_traecode` 两处（1 行 filter + 1 段拒绝）+ 还原 `run_round` 签名。
改动集中在 `checkin.rs` 与 `account_manager.rs` 各一处，无数据格式变更。

### 风险

- **R1**：分派若写成「非穷尽」判断（如 `if app == traework {...} else { 走 cookies }`），
  TraeWork 账号会落到 cookies 刷新路径 → 一屏 `auth_expired`。缓解：用 `match` 显式穷尽两个 app 常量。
- **R2**：`checkin.rs` 当前有效 490 行，本阶段预计 +90~120 → **约 580–610 行**，仍在 700 行预警线内；
  若后续继续加（阶段 3 的冷却分派），需按仓库约定拆出 `checkin/` 子模块。
- **R3（合入约束，高危）**：本阶段落地后、阶段 3 落地前，TraeWork 过期凭据会走
  `AuthExpired → auth_expired(i64::MAX)` 永久哨兵——`i64::MAX` 无清除入口，自动签到准入会把该账号
  永久拦死，用户切过去重新保存也救不回。**阶段 2 与阶段 3 必须同批合入、禁止单独发布阶段 2**
  （等价做法：本阶段先在 `process_account` 内联 CredentialStale 映射，阶段 3 再补清除入口等其余部分）。
- **R4**：凭据解析发生在批次持锁期间（`try_acquire` 之后、`run_round` 之前），槽位极多时会拉长持锁
  窗口。缓解：解析在 `spawn_blocking` 中执行（不占 async 调度），且文件读通常 <1s；
  锁纪律优先于时长优化，不做预解析缓存。

---

## 阶段 3：冷却语义（TraeWork 专用 reason）+ 设备号落盘

**目标**：TraeWork 的凭据失效**不能**落在 `auth_expired` 的 `i64::MAX` 哨兵上——那会让该账号在冷却表里
永久卡死，用户切过去重新保存也救不回来。

### 任务

1. `src-tauri/src/api/checkin_guard.rs`：
   - 新增 `pub const REASON_CREDENTIAL_STALE: &str = "credential_stale";`
   - 新增 `CooldownTrigger::CredentialStale`：`duration_secs() = 12 * 3600`（与 `EntitlementDenied` 同级），
     `reason() = REASON_CREDENTIAL_STALE`，`retryable() = false`。
     - 为什么不用 `i64::MAX`：`i64::MAX` 的语义是「需人工介入」且**无清除入口**；TraeWork 的人工介入
       是一个用户能完成的具体动作（切过去 + 保存登录态），冷却必须能被那个动作清掉。
     - 为什么是 12h 而不是 60s：access 过期后**本工具无法续期**（refreshToken 端点未找到），
       短冷却只会让批次反复白打请求；12h 与「等用户下次想起来切换」的节奏相符，且被钳到当日 23:59:59。
2. `checkin.rs`：`process_account` 中，`account.app == traework` 且错误为 `AuthExpired` 时，
   改映射为 `CredentialStale`（不重试、不刷新——TraeWork 没有可用的刷新链路）。
   - **映射点在 cookies 刷新尝试之前**：`refresh_token_via_cookies` 对 traework 账号是恒空转
     （cookies 恒为空串），必须按 app 先拦截，否则会先白跑一次刷新才落到映射。
3. `checkin_guard.rs`：把 `clear_auth_cooldown` 泛化为
   `pub fn clear_cooldown_reason(account: &mut Account, reason: &str) -> bool`，
   原函数保留为 `clear_cooldown_reason(account, REASON_AUTH_EXPIRED)` 的包装（不改既有调用点）。
4. 清除入口挂在「保存登录态」的落盘路径上：**在 `upsert_traework_account` 内部**、既有字段补齐
   逻辑旁，对该账号调 `clear_cooldown_reason(&mut account, REASON_CREDENTIAL_STALE)`，
   有变更时随 upsert 既有的 `save_store()` 一并落盘（不新增落盘调用点，属全局纪律声明的净增块之一）。
   - 为什么不放在 `commands.rs` 事后补：命令层拿到的是不可变快照，再清冷却就要第二次取锁 +
     第二个落盘方法——把「登记 + 清冷却」收敛进一次 upsert 的原子里更简单。
   - 为什么挂在「保存登录态」而不是「切换」：切换只是把旧凭据恢复到现场，**新凭据是客户端续期后才产生的**；
     用户按提示重新「保存当前登录态」才是真正把新 token 写进快照的那一步。
5. 设备号落盘（`device_id` 目前对 TraeWork 恒为 `None`）：
   - 在 `upsert_traework_account` 里，`device_id` 为空时写入
     `device_id::derive_device_id(&format!("{APP_TRAEWORK}:{uid}"))`；`Account::new_traework` 不动
     （upsert 是唯一登记入口，避免两处派生）。备选落点 `types.rs::new_traework` 被否决：
     存量账号不会重走构造函数，落盘覆盖不到它们。
   - **启动回填（必做，堵种子不一致）**：`AccountManager::new` 既有的 device_id 启动回填只处理
     traecode 侧；本任务给 `app == traework && device_id 为空` 的账号按**同一**
     `derive_device_id("traework:{uid}")` 前缀种子派生落盘。不做会留下洞：`resolve_device_id`
     的运行时回退种子是裸 `user_id`（types.rs:152，TraeWork 账号的 `user_id` 就是 uid），与落盘
     种子不同——同一账号「落盘前」与「重新保存落盘后」是两个设备号；且只走 upsert 落盘意味着
     从不重存登录态的存量账号永远裸 uid 回退。回填让全量账号收敛到唯一种子，口径与既有
     「启动回填只补空值、绝不无条件重算」约定一致。
   - **为什么加 app 域前缀**：不加则与 traecode 侧共用同一份种子空间，「traecode 的 `user_id` 与
     TraeWork 的 `uid` 是否同一数值域」这个**未验证事实**就会决定行为（同域 → 两条记录撞同一设备号 →
     第二条被服务端设备去重报 9095「本设备今日已签到」，文案误导）。加前缀把这件事变成结构性保证，
     **不需要先实测**。
   - 代价（如实记录）：同一真实账号的两条记录会拿到两个设备号，第二次 claim 由**账号级**去重挡成
     `already` —— 多一次无副作用的白请求，换来的是「不依赖未验证事实」。

### 验收

- 用一份**过期**的槽位凭据跑签到：结果为 `failed` + `cooldown_reason = "credential_stale"`（**不是** `auth_expired`），
  冷却为 12h 且被钳到当日 23:59:59（读 `accounts.json` 验证）。
- 对该账号执行「保存当前登录态」后，`checkin_cooldown` 被清空。
- `accounts.json` 中 TraeWork 账号出现落盘的 16 位 `device_id`，重启后不变；与任一 traecode 账号设备号不同。
- **重启即回填**：升级后从未重存登录态的存量 TraeWork 账号，一次启动后 `device_id` 也落盘，
  且等于 `derive_device_id("traework:{uid}")`（不是裸 uid 运行时回退值）。

### 回滚

新增的 reason 是纯增量；设备号字段仍是 `#[serde(default)]`，清空即回到运行时派生。

---

## 阶段 4：前端入口（TraeworkPanel）

**目标**：让 TraeWork 的签到在界面上可达，并如实渲染 `skipped`。

### 任务

1. `src-tauri/src/lib.rs`：新增
   `#[tauri::command] async fn traework_checkin_all(...)`（调 `checkin::checkin_all_traework`）
   并注册进 `generate_handler!`（命令数 59 → 60）。
   - 单账号签到**不新增命令**：复用已注册的 `checkin_account`（它按 id 直调，不经过列表过滤）。
2. `src/api.ts`：新增 `traeworkCheckinAll(): Promise<CheckinResult[]>`（走 `invokeNetwork`）。
3. 前端类型与分支（`state` 是联合类型但代码用 if/else，**`tsc` 不会报漏**，必须逐个手工补）：
   - `src/types/index.ts:206`：联合类型加 `"skipped"`；`:210` 的 `cooldown_reason` 注释加 `credential_stale`；
   - `src/App.tsx:48-70` `describeCooldown`：加 `case "credential_stale"` → 「凭据已过期，请切换到该账号并重新保存登录态」；
   - **把 `describeCooldown` 上移到共享模块**（如 `src/utils/checkinDisplay.ts`）：面板侧（任务 4）
     渲染签到结果同样要用这份文案，留在 `App.tsx` 作私有函数面板够不着；上移后 `App.tsx` 改为 import；
   - `src/App.tsx:448-456`（单账号结果 toast）：`skipped` 按 `info` 展示 `detail`（不是 warning）；
   - `src/App.tsx:512-520`（批量汇总）：`skipped` 单独计数并写进汇总文案，**不计入 `failed`**。
4. `src/components/TraeworkPanel.tsx`：
   - 每张账号卡片加「签到」按钮 → `api.checkinAccount(account.id)`；用既有的 `inFlight` 遮罩防连点
     （但**不要**复用 `runAction`：它假定返回 `TraeworkActionResult`，签到返回的是 `CheckinResult`）；
   - 面板顶部加「全部签到」→ `api.traeworkCheckinAll()`；
   - 账号名旁渲染「今日已签」标记，数据源是已有字段 `AccountBrief.checked_in_today`（TraeworkPanel 收到的
     `accounts` 是全量，无需新增后端字段）；**徽标直接按阶段 5 任务 2 的目标形态做**（`.card-status`
     规格），不挂旧 `.traework-tag` pill——避免阶段 5 再来一轮改名；
   - **面板侧结果渲染**：单账号签到按状态出 toast（`ok/already` 成功、`skipped` info、`cooldown` info
     用共享 `describeCooldown`、`failed/rate_limited` warning）；「全部签到」结束后做逐状态计数汇总
     （成功 / 已签 / 跳过 N / 冷却 N / 失败 N，`skipped` 单独计数、不计入失败），汇总逻辑与
     `App.tsx:512-520` 的批量汇总同构，可连同 `describeCooldown` 一起下沉到共享模块；
   - 结束后 `await onAccountsChanged()` 刷新徽标（与其它动作一致）。

### 验收

- 面板上对 TraeWork 账号点「签到」：成功 → toast 成功并出现「今日已签」；无凭据 → info 提示且**不出现冷却徽标**。
- TraeCode 页面的「全部签到」按钮**不处理** TraeWork 账号（`checkin_all` 仍为 `Some(APP_TRAECODE)`）。
- `npx tsc --noEmit` + `read_lints` 零错误。

### 回滚

前端为纯增量；后端新命令可单独从 `generate_handler!` 摘除。

---

## 阶段 5：UI 视觉体系统一（TraeCode 账号页 ↔ TraeWork 面板）

**目标**：两页共用一套**组件形态**（容器、行、徽标、按钮、浮层五类），变量全部落到 token 层语义名；
允许有意的可见变化（遮罩半透明化、容器圆角 6→8、标题 20→16、行去盒化），禁止无意的漂移。
本阶段**纯视觉，零逻辑改动**。

### 现状取证（2026-09-20 实测 + 代码级核对，勿凭印象）

| 维度 | TraeCode 账号页 | TraeWork 面板 | 实测结论 |
|---|---|---|---|
| 样式落点 | `trae-components.css` 重塑 `.account-card` 等；`trae-pages.css` 覆盖其他页 | `components/TraeworkPanel.css`（299 行）独立自足，`trae-*.css` 里**无任何 `.traework-*` 规则** | 需迁入设计层 |
| 尺寸变量 | `--spacer-*` / `--radius-*` 新名 | `--space-*`（App.css:71-76）与 `--radius/--radius-sm/--radius-md` 旧名 | **数值逐一相等**（space 4/8/12/16/24 = spacer）；`--radius/--radius-sm/--radius-md/--transition-fast` 已被 `trae-tokens.css` 的 `:root` 同名覆盖（6/4/8px、0.12s，tokens 在 main.tsx 后加载生效）——任务 1 是**改名**不是对齐 |
| 容器圆角 | `.account-card` 显式 `--radius-8`（8px） | `.traework-card` 用 `var(--radius)` = **6px**（被 tokens 收敛后） | **真实差异：6px vs 8px，截图可见**，靠任务 2 对齐 |
| 表面语义 | `--bg-base-secondary`、`--border-neutral-l1` | `--bg-card` / `--bg-secondary` / `--border-light` / `--bg-input`（旧别名） | 旧别名被 tokens 重定义，**亮暗两套值已全部相等，颜色无差异、语义漂移**；归一化按新名 |
| 行形态 | `.account-list-item`：无独立底色、`border-bottom` 分隔、hover `--bg-overlay-l1` | `.traework-item`：「盒中行」——独立底色 + 全边框 + 圆角（截图里行套在小卡片里） | **两页最大形态差异**，任务 2 去盒化 |
| 徽标 | `.card-status`（`--radius-4`、11px、status surface 底色）、`.tag`（`--radius-2`、11px） | `.traework-tag`：999px pill、12px | pill 取消，分两规格对齐 |
| 主按钮 | 设计层主按钮：height 32px、padding 0 14px、`--bg-brand` | `.traework-primary`：padding 9px 18px、`--gradient-accent`（tokens 已降级纯色） | 颜色已跟随，差异在尺寸 / padding / hover |
| 次级按钮 | `.header-btn` 等：height 30px、`--bg-overlay-l1` 底、hover `--bg-overlay-l2` | `.traework-mini-btn`：padding 5px 12px、`--bg-input` 底、hover `--bg-hover` | hover 语义不同，归并 |
| 标题层级 | `.page-title`：16px/600 + 3px 品牌竖条（全站页标题） | `h2` 20px 粗体、无竖条 | **对齐目标是 `.page-title`**；账号页 `.toolbar` 没有标题（旧版写「toolbar 标题层级」是误引用） |
| 遮罩 | `.modal-overlay`：`--scrim`（亮 0.32 / 暗 0.55 半透明）、无 blur | `.traework-overlay`：`--glass-bg`（**实色**：亮 `#ffffff` / 暗 `#222427`）+ `blur(6px)` | 现状是实色遮罩；改 `--scrim` 后变半透明，是**有意可见变化** |

### 任务 1：变量归位（纯改名，零视觉变化）

`TraeworkPanel.css` 全量替换旧变量名。**颜色映射表（已逐条核对 `trae-tokens.css` 亮/暗两套值，照表执行）**：

| 现状 | 映射到 | 值核对 |
|---|---|---|
| `--bg-card`、`--bg-secondary` | `--bg-base-secondary` | 亮/暗均同值 ✓ |
| `--border-light` | `--border-neutral-l1` | 同值 ✓ |
| `--border` | `--border-neutral-l2` | 同值 ✓ |
| `--bg-input`（mini-btn 底） | `--bg-base-default` | 亮 #fff / 暗 #1a1b1d，同值 ✓ |
| `--bg-hover` | `--bg-overlay-l2` | 本就是 token 派生别名 ✓ |
| `--accent-bg` / `--success-bg` / `--warning-bg` / `--danger-bg` | 对应 `--status-*-surface-l1` | 同族同值 ✓ |
| `--gradient-accent` | `--bg-brand` | tokens 已降级纯色 ✓ |
| `--text-inverse` | `--text-onbrand` | 暗色下 tokens 已改深字，换名语义准确 ✓ |
| `--space-*` | `--spacer-4/8/12/16/24/32`（**数值名**，tokens 没有 xs/sm/md/lg/xl 后缀） | 数值逐一相等 ✓ |
| `--radius` / `--radius-sm` / `--radius-md` | `--radius-6` / `--radius-4` / `--radius-8` | 已被 tokens 同名覆盖，等值 ✓ |
| `box-shadow: var(--shadow-sm)` | 删除该声明 | tokens 已定义 `none`，本就无阴影 ✓ |
| `--transition-*` | 同名保留 | tokens 已覆盖 ✓ |

- **验收**：改前/改后亮、暗各截一张，逐像素无差异（本任务做得到——值已全部相等；**出现差异即映射错误**）。
- R2 预防：逐条按上表执行，不产出「看起来要改、其实已经相等」的无意义 diff。

### 任务 2：组件形态对齐（允许四处有意可见变化）

1. **标题**：`h2 20px` → `.page-title` 规格（16px/600 + 3px 品牌色竖条，写法抄 `trae-components.css:151-163`）；
   副标题保持 13px muted。可见变化①。
2. **容器**：`.traework-card` / `.traework-list` / `.traework-steps` 对齐 `.account-card`：`--radius-8`、
   `--spacer-16` padding、`--border-neutral-l1`。可见变化②（圆角 6→8）。
3. **行去盒化**：`.traework-item` 改 `.account-list-item` 语义——去独立底色与全边框，改
   `border-bottom: 1px solid var(--border-neutral-l1)` 分隔 + `:hover { background: var(--bg-overlay-l1) }`，
   末行去底边框。可见变化③（两页截图中最大的形态差异）。
4. **徽标分两规格**：状态类（客户端运行中 / 当前 / 凭据警示 / 阶段 4 的「今日已签」）用 `.card-status`
   规格（`--radius-4`、11px、对应 status surface 底色 + 同色文字）；信息类（积分 / 体积 / 可回退代）用
   `.tag` 规格（`--radius-2`、11px）。999px pill 取消。
5. **按钮归并**：`.traework-primary` → 设计层主按钮（height 32px、padding 0 14px、`--bg-brand` 底、
   hover `--bg-brand-hover`、disabled `--bg-brand-disabled`），保留「每页一个主 CTA」原则；
   `.traework-mini-btn` → 次级按钮（height 30px、padding 0 12px、`--bg-overlay-l1` 底、
   hover `--bg-overlay-l2`、`--border-neutral-l1` 边框），`.danger` 变体用 `--status-error-default` 文字 +
   `--status-error-surface-l1` hover。
6. **className 去前缀**：以上五类直接复用设计层类名（`page-title` / `account-list-item` / `card-status` /
   `tag` / `header-btn`），`TraeworkPanel.tsx` 同步改；组件级只保留真·面板布局类
   （`.traework-panel` / `.traework-row` / `.traework-label` / `.traework-actions` 等），
   **不保留两套并存的死样式**。
- **验收**：两页并排截图，容器圆角/边框/内间距、徽标、按钮尺寸一致；亮暗两套各过一遍；
  明确接受四处可见变化（标题 20→16、圆角 6→8、行去盒化、徽标 pill→微圆角）。

### 任务 3：浮层归并

- `.traework-overlay`：`background: var(--glass-bg)` + `backdrop-filter: blur(6px)` →
  `background: var(--scrim)`（亮 `rgba(23,23,23,0.32)` / 暗 `rgba(0,0,0,0.55)`），删 blur 与 `--glass-bg` 引用。
  - **不能只删 `backdrop-filter`**：现状 `--glass-bg` 是实色，改 `--scrim` 后是半透明——视觉由
    「实色+模糊」变「半透明」，是**有意的**；遮罩厚度靠 `--scrim` 透明度保证，
    职责（阻塞数十秒、挡住点击）不变。
- `.traework-overlay-card` 对齐 `.modal-content`：`--bg-base-secondary`、`--radius-8`、`--shadow-lg`、
  边框 `--border-neutral-l2`。
- **z-index 核对（补）**：遮罩现 z-index 60；执行时读 App.css 核对遮罩高于 `.app-main`、
  **低于 Toast 容器**（签到失败提示不能被遮罩盖住），核对结果写进验收记录。
- **验收**：遮罩仍完全阻挡点击、三行内容（标题/计时/提示）完整；触发一次签到失败后
  Toast 在遮罩之上可见；亮暗各验一次。

### 约束

- `App.css` 仍严重超限（约 4240 有效行），**本阶段不往它加任何规则**；归并样式进 `trae-components.css`。
- **分流决策前置**：`trae-components.css` 现 758 行，任务 2/3 归并预计 +60~90 行 → 逼近/超过 800 上限。
  先把归并规则写进 `trae-components.css`；若超 800，面板特有部分（`.traework-row` / `.traework-label` 等）
  拆到新建 `trae-traework.css`，挂进 `main.tsx` 样式层末尾（`trae-pages.css` 之后）。
- 本阶段不动「按主题切换品牌色」的既有设计（`trae-tokens.css:7-9`：暗色 = TraeCode 绿、亮色 = TraeWork 靛蓝）；
  若要改这个，见「待拍板项」第 5 条。

### 回滚

纯 CSS + className，还原对应文件即可；无数据格式、无接口、无命令变更。

### 风险

- **R1**：变量映射错语义 → 暗色下一片同色。缓解：本版映射表已逐条核对两套值，照表执行 + 双主题截图对照。
- **R2**：「已经相等」的项（如 `--shadow-sm`）已在上表标注，避免无意义 diff 掩盖真正的差异
  （容器圆角、行形态、标题层级、遮罩这四项才是真差异）。
- **R3**：统一过程容易顺手改功能文案/间距语义。本阶段验收只认视觉，不接受夹带逻辑改动。

---

## 阶段 6：收尾（文档 / 测试 / 打包）

### 任务

1. `AGENTS.md`：
   - §5.7 更新：签到不再按 app 拒绝、准入改为「凭据可达性」、新增 `skipped` 状态与 `credential_stale` 冷却、
     设备号对 TraeWork 的 app 域前缀与启动回填；
   - §5.9 补「凭据与签到」段；
   - §1 命令数 59 → 60；文件规模表更新 `checkin.rs` / 新增 `traework/credentials.rs` 有效行数；
   - 顺手修正过时数字：§3 的 `cargo test --lib` 实测数 47 → 48。
2. 版本与发布文档（本计划含用户可见变更，走仓库惯例）：
   - 版本 bump：1.0.5 → 1.0.6，三处同步（`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`）；
   - `CHANGELOG.md` + `RELEASE_NOTES.md` 增补：TraeWork 签到（面板手动/批量入口、自动签到纳入）、
     `skipped` 状态与 `credential_stale` 冷却自救路径、面板视觉与设计层统一。
3. 单元测试补齐（`cargo test --lib`）：
   - `credentials.rs` 四条路径（阶段 1）；
   - `checkin.rs` 的 app 分派：TraeWork 无凭据 → `Skipped` 且 `collect_outcomes` 产出空（不落日期不落冷却）；
   - `checkin_guard.rs`：`CredentialStale` 的时长/reason/`retryable=false`；`clear_cooldown_reason` 只清指定 reason；
   - `account_manager.rs`：TraeWork 账号启动回填 `device_id` 用 `traework:{uid}` 前缀种子、只补空值、
     不重算既有值（含与 traecode 账号设备号不同域的断言）。
4. 端到端验证清单：
   - `cargo test --lib`（48 + 新增全绿）、`cargo check`（零新增警告）、`npx tsc --noEmit`、`read_lints`；
   - `npm run tauri build` 成功（产物约 7.66MB 起，判断标准是「是否少了约 0.37MB」）；
   - 手工回归：TraeCode 手动/全部/自动签到各一次；TraeWork 有凭据与无凭据各一次；
     `--silent` 启动（`traework` 是否纳入见待拍板项）后检查 `accounts.json` 与日志；
   - 视觉核对（阶段 5）：暗色与亮色两套主题下，TraeCode 账号页与 TraeWork 面板各截一张并排对照，
     核对卡片圆角/边框/内间距、工具栏按钮尺寸、徽标形态、遮罩层一致性。

---

## 待拍板项（未定，勿擅自实现）

| # | 决策点 | 建议 | 理由 / 风险 |
|---|---|---|---|
| 1 | `auto_checkin`（方案B，启动时静默）是否纳入 TraeWork | **建议纳入** | 否则用户每天要手点一次；风险是凭据过期的账号会在冷却表里留 `credential_stale`，属预期行为（有清除入口）。注：GUI 启动路径受 `auto_checkin_enabled` 设置门控（lib.rs:54-57），纳入后 TraeWork 自动受同一开关管；`--silent` 无头路径直接调 `auto_checkin_pending`，不受该开关门控（既有行为，本计划不改变） |
| 2 | TraeCode 工具栏「全部签到」是否连带签 TraeWork | **建议不连带** | 按钮语义应单一：用户按 TraeCode 的按钮时预期只处理 TraeCode；TraeWork 用面板自己的按钮 |
| 3 | 同一真实账号的 traecode / traework 两条记录是否按 `user_id` 归并成一次 | **建议先不做** | 需要先实测两者 `user_id` 是否同域；不做只有「一次白请求」，无正确性问题 |
| 4 | TraeWork 是否在 `--silent` 无头模式签到 | 跟随 1 | 无头模式没有界面，`skipped` 只进日志（`Ctx::from_env` 已验证可用） |
| 5 | 「视觉风格统一」统一到哪一层（阶段 5） | **建议只统一组件形态与尺寸尺度**，不动「按主题切换品牌色」 | 现状是暗色 = TraeCode 绿 / 亮色 = TraeWork 靛蓝（`trae-tokens.css:7-9`）。若要求 TraeWork 面板**恒定**使用 TraeWork（亮）配色，需给面板加局部 token 覆盖，属另一件事，且会与用户的全局主题设置冲突 |

## 明确不做

- **不引入 schtasks 计划任务或常驻 tick 调度器**：现有「启动时一次 + 批次内 `[30,90]` 重试」已覆盖主要场景，
  引入常驻 tick 要额外处理与单实例 / `--silent` 双进程的关系（参考实现为此付出了「无停机机制」的代价）。
- **不引入 MITM 代理抓真实设备指纹**：我们已有可控的派生设备号；TraeWork 客户端的设备标识在 aha 层，
  引入代理是另一个量级的工程。
- **不给 TraeWork 做 Token 刷新**：refreshToken 续期端点未找到（客户端 JS 已压缩）；
  过期一律提示「切过去重新保存登录态」，并由阶段 3 的清除入口配合。
- **不改 `machine_id` 语义**：签到设备号与 IDE 机器身份继续彻底解耦（`device_id.rs` 的既有约定）。

## 已知边界（如实记录，不当作已解决）

- GUI 与 `--silent` 双进程仍可能并发签到；最坏得到 9095/9074，且有冷却兜底（既有边界，本计划不改变）。
- 阶段 0 未通过（`claim` 对 TraeWork 凭据不生效）时，本计划整体搁置，只保留阶段 1 的重构价值。
  阶段 1 是纯重构、不依赖阶段 0 结论，可与阶段 0 并行或先行落地，不受此约束影响。

---

## 实测记录

> 阶段 0 执行后在此追加：探针日期、使用的槽位、两套请求头各自的 HTTP 状态与响应码、
> `status` 前后余额差、以及「头 A / 头 B」的最终结论。

### 阶段 0：claim 探针（2026-09-20，头 A 直接通过，未启用头 B 对照）

- **槽位**：当前槽 `371900****779`（`current_account.txt` 指向，凭据经 `credentials::resolve_token` 读实时现场——该函数为阶段 1 抽取的公共实现，与探针同批落地）。
- **设备号**：按阶段 3 计划的同款种子派生 `derive_device_id("traework:{uid}")`（脱敏 `4126…91`）。
- **status（claim 前，头 A）**：HTTP 200 `{"checked_in":false,"code":0,"credits":150,"did_checked_in":false,"enable":true,"extra_credits":50,"message":"success"}`。
- **claim（头 A）**：HTTP 200 `{"code":0,"message":"success"}`——**头 A 一次通过**，无需头 B 对照。
- **status（claim 后，头 A）**：HTTP 200 `checked_in` 与 `did_checked_in` 均翻转为 `true`，余额字段（`credits`/`extra_credits`）不回显增量（增量语义见 claim 响应，status 只报当前权益配置）。
- **结论：头 A（`User-Agent: Trae/0.1.52`）可用**，签到链路无需引入参考实现的 `VSCode` UA 与 `x-market-client-id`；`claim` 对 TraeWork 凭据真实生效，阶段 1–6 全部放行。附带收获：该账号当日 +50 签到积分已实际到账（探针即完成当日签到）。
- 探针文件（`traework/probe.rs` + `mod.rs` 挂载行）已按计划删除，不进版本库。

### 阶段 1–6 实施记录（2026-09-20，一次完成，阶段 2+3 同批合入）

- **实现偏差说明**：计划阶段 2 的 `CheckinError::Skip(String)` 变体未新增——跳过路径由
  `TraeworkCredential::{Token, Skip}` 枚举直接折算成 `Skipped` 结果（`skipped_round`），
  避免产生一个无调用方的死变体；「skipped 不落盘、非失败、无冷却」的语义与计划一致。
- **待拍板项落定**：#1 自动签到纳入 TraeWork（用户确认，`auto_checkin_pending` 全量、
  不按 app 过滤）；#2 不连带；#3 不归并；#5 只统一组件形态。
- **阶段 5 z-index 核对**（计划补条）：`.traework-overlay` z-index 60 ＞ `.app-main`（未设，
  常规流），＜ Toast 容器 9999（App.css）——签到失败等 toast 在遮罩之上可见 ✓。
- **分流决策结果**：`trae-components.css` 实测 646 行（计划写 758 系过时数字），归并后
  664 行，未超 800，无需拆出 `trae-traework.css`。
- **自动化验证**：`cargo test --lib` 56 全绿（48 → 56，新增 credentials 4 条、guard 1 条、
  checkin 2 条、account_manager 1 条）；`cargo check` 零警告；`npx tsc --noEmit` 零错误；
  `npm run tauri build` 成功，产物 **7.67MB**（完整内嵌前端，唯一警告为已知的
  `linker_messages` 假警报）。
- **待人工回归**（需 GUI，无法自动化）：TraeCode 手动/全部/自动签到各一次；TraeWork 面板
  有凭据与无凭据各签一次（无凭据账号可临时改 `snapshot_slot` 制造）；亮/暗两套主题下
  TraeCode 账号页与 TraeWork 面板并排视觉对照。

### 阶段 5 实施后翻车与修复（2026-09-20，用户截图报障）

- **翻车 1（行被压成 40px 窄列，整行竖排）**：按任务 2 第 6 条给 `.traework-item` 挂了设计层类
  `account-list-item`，但 **App.css:4065 的同名类是 TraeCode 列表视图专用的 8 列 grid**
  （`display:grid; grid-template-columns: 40px 48px 1fr …`），trae-components 的覆盖规则不设
  display，grid 生效把行塞进第一列。**教训：「复用设计层类名」前必须全仓 grep 该类名的全部
  定义**——设计层同名类未必只有形态那一条。修复：TSX 去掉该类名，行形态（padding/border-bottom/
  hover）直接写进面板 CSS。
- **翻车 2（间距全部塌缩为 0）**：本计划任务 1 的映射表把间距目标写成 `--spacer-xs/sm/md/lg/xl`，
  实际 tokens 定义的是**数值名** `--spacer-4/8/12/16/24/32`——未定义变量使 gap/padding 全部
  失效。用户已手改数值名，映射表已更正。
- **翻车 3（徽标文字竖排折行）**：设计层 `.tag` / `.card-status` 未设 `white-space: nowrap`
  （TraeCode 侧徽标内容短，暴露不出），面板的「积分 483 / 6100」带空格即折行。已在
  `.traework-item-title` 作用域内补 nowrap。

### 完成度审查（2026-09-20 复审，代码逐文件比对）

- **发现并已修复（1 处真 bug）**：`TraeworkPanel.css` 曾把间距映射写成档位别名
  `--spacer-xs/sm/md/lg/xl`，而 `trae-tokens.css` 只有数字档（`--spacer-4/8/12/16/24/32`）——
  全库零定义的变量会让对应 gap/padding 声明被静默丢弃（面板布局塌陷）。已全部改为数字名（15 处），
  这正是任务 1 验收「出现差异即映射错误」预警的情形；CSS 无类型检查，`tauri build` 不会暴露它，
  靠双主题截图验收兜住。
- **复核通过**：阶段 1 四路径测试与错误文案逐字保留；阶段 2 分派显式按 app（注释载明 R1 理由）、
  `from_env` 失败策略、retain 落点、重试轮复用批次凭据；阶段 3 映射在 cookies 刷新之前（结构性）、
  `clear_cooldown_reason` 在 upsert 内、启动回填同前缀种子；阶段 4 `describeCooldown`/`summarizeCheckin`
  已下沉共享、徽标按 `.card-status`/`.tag` 两规格、签到与长动作共用 `inFlight` 闸门；阶段 5 容器
  radius-8、行去盒化、`--scrim` 遮罩、z-index 60＜Toast 9999 均落实；阶段 6 版本 1.0.6 三处同步、
  AGENTS/CHANGELOG/RELEASE_NOTES 已更新（测试数 56、命令数 60、checkin.rs 617 行临近上限已标注）。
- **合理偏差（已在实施记录声明）**：`CheckinError::Skip(String)` 未新增，改由
  `TraeworkCredential::{Token, Skip}` 折算——语义等价且无死变体。
- **剩余风险**：仅「待人工回归」四项 GUI 验证未做；审查者侧 `cargo test --lib`（56 绿）、
  `tsc --noEmit`（0 错）已复跑确认。
