# 每日签到功能移植计划（traework2api → trae-cc）

> 调研对象：`d:\2api\traework2api`（Go，SOLO 反代项目，含一次性签到工具 `cmd/signin` 与每日定时签到 `internal/scheduler`）
> 目标项目：trae-cc 1.0.5（Tauri 2 + React 19 + Rust）
> 结论先行：**可以移植，且改动量小**。签到仅需账号已有的 JWT，不依赖机器码改写、不触碰 IDE 文件，是纯网络请求功能。

---

## 1. traework2api 签到机制拆解

### 1.1 API 端点（实测值，来自 `internal/upstream/constants.go`）

| 用途 | 方法 | 地址 | 请求体 |
|---|---|---|---|
| 查询签到状态 | POST | `https://api.trae.cn/trae/api/v2/ug/checkin_credits/status` | `{}` |
| 执行签到 | POST | `https://api.trae.cn/trae/api/v2/ug/checkin_credits/claim` | `{}` |

status 响应结构：

```json
{ "checked_in": false, "credits": 0, "enable": true }
```

- `checked_in`：今日是否已签
- `enable`：签到活动是否对该账号开放（false 时不要再调 claim）
- 签到收益：**每日 +200（会员）/ +150（免费）积分**（见 `docs/RESEARCH.md` §3）

### 1.2 请求头（`internal/upstream/headers.go` 的 `UgHeaders`）

```
Content-Type: application/json
Accept: application/json
User-Agent: Trae/0.1.52
Authorization: Cloud-IDE-JWT <accessToken>
X-User-Region: CN
X-Device-Id: <deviceId>   ← 可选，为空则不发送
```

**关键点：只需 JWT，不需要 Cookies，不需要签名。** 这正是 trae-cc 账号库里已有的东西。

### 1.3 签到流程（`cmd/signin/main.go`）

```
对每个账号：
  1. token 临期（2h 内）→ 先刷新
  2. 调 status
     ├─ 报错含「已签到 / already check」→ 记 ALREADY（不误判 429/5xx，见 §1.4）
     ├─ checked_in=true              → 记 ALREADY
     ├─ enable=false                 → 记 FAIL（活动未开放）
     └─ 否则调 claim → 成功记 OK
  3. 顺手查一次积分余额（ide_user_ent_usage）
```

另有 `internal/scheduler/scheduler.go`：每日固定整点（默认 9:00）自动对全部账号跑同一流程，签到成功后重新查积分。

### 1.4 已签到判定与错误码（容易踩的坑）

- **判定只认无歧义标记**：错误串含「已签到」、"already check"、"already checked" 才视为已签。`cmd/signin/isalready_test.go` 专门测试过：`429 checkin rate limited`、`code=400` 等**不能**误判为已签。
- 高峰时段 claim 常返回 **`9074 当前参与用户太多，请稍后再试`**——这是限流不是失败，应隔段时间重试（traework2api 有专门的 `checkin_retry.sh` 思路）。
- `1001` = token 失效，需重新登录刷新。

### 1.5 域名差异（移植时唯一需要实测的点）

| 项目 | 签到/额度域名 |
|---|---|
| traework2api | `api.trae.cn`（UgHost） |
| trae-cc 现状 | 全部统一 `api.trae.com.cn` |

两者都是 CN 体系域名，但签到端点在 trae-cc 里尚未验证过。**计划按「先试 `api.trae.cn`，失败回退 `api.trae.com.cn`」的双端点策略实现**——trae-cc 的 `trae_api.rs` 本就有多端点回退的先例（`get_user_info_by_token`）。

---

## 2. 移植到 trae-cc 的可行性判断

| 依赖项 | traework2api 用法 | trae-cc 现状 | 结论 |
|---|---|---|---|
| JWT token | `Authorization: Cloud-IDE-JWT` | `Account.jwt_token` 已有，401 时走 cookies `GetUserToken` 刷新 | ✅ 直接可用 |
| X-Device-Id | 可选头 | `Account` 无此字段 | ✅ 不发即可 |
| X-User-Region: CN | 固定值 | 项目仅适配 CN 版 | ✅ 固定写死 |
| 签到后查积分 | `ide_user_ent_usage` | `cn_credits.rs` 已实现同端点解析 | ✅ 直接复用 |
| token 刷新 | ExchangeToken（refreshToken 轮换） | trae-cc 无 refreshToken 字段，走 cookies 刷新 | ⚠️ 沿用现有 `manager.refresh_token` 路径，不引入 ExchangeToken |
| 每日定时 | Go scheduler | 无常驻后端；但已有 `--silent` 无头模式 + 开机自启 | ⚠️ 见 §3.3 方案 |

**结论：功能完全可移植，无阻塞项。** 不引入新 Rust 依赖（reqwest/serde_json 已有），符合 OPTIMIZATION_PLAN 的体积控制方向。

---

## 3. 实现计划

### 3.1 后端：新增 `src-tauri/src/api/checkin.rs`（新模块，避免撑爆 `trae_api.rs` 的 758 行）

```
fetch_checkin_status(client, token) -> CheckinStatus { checked_in, credits, enable }
claim_checkin(client, token)       -> Result<()>
```

- 头拼装复用 `build_headers_token_only` 的思路，但补 `X-User-Region: CN`、UA 改为 `Trae/0.1.52`（UG 系端点认这个 UA，浏览器 UA 未必放行——`UgHeaders` 实测值）。
- 双端点回退：先 `https://api.trae.cn`，网络/404 失败再试 `api.trae.com.cn`。
- 已签到判定函数单独写（照搬 `isAlready` 逻辑：只认「已签到 / already check」），**不要**用「错误串含 checkin」这种模糊匹配。

### 3.2 新增命令（按 AGENTS.md 三步 checklist）

| 命令 | 说明 | invoke 封装 |
|---|---|---|
| `checkin_account(account_id)` | 单账号：token 过期先刷新 → status → claim → 返回最新积分 | `invokeNetwork` |
| `checkin_all_accounts()` | 批量：遍历活跃账号逐个执行，返回每账号结果列表（OK/ALREADY/FAIL + 剩余积分） | `invokeNetwork` |

- 锁纪律：遵循「取锁读 → 无锁网络 → 取锁写」三段式，参照 `get_account_usage`。
- 401 处理：沿用现有错误串匹配约定，触发 cookies `GetUserToken` 刷新后重试一次。
- `src/types/index.ts` 加 `CheckinResult { accountId, status: "ok"|"already"|"failed", detail, creditsLeft? }`。

### 3.3 前端：账号卡片 + 批量操作

- `AccountCard` 操作区加「签到」按钮（单账号触发 `checkin_account`，完成后原地刷新额度显示）。
- 主页面批量操作栏加「全部签到」（调 `checkin_all_accounts`，结果用现有弹窗/通知样式汇总展示）。
- 样式追加到组件级 CSS，**不动** `App.css`（已 5275 行严重超限）。

### 3.4（可选，二期）每日自动签到

traework2api 是常驻进程所以能定时；trae-cc 是桌面工具，两个低成本方案：

- **方案 A（推荐）**：挂到现有 `--silent` 无头模式——开机自启拉起时，刷新 token 后顺手对全部账号执行一次签到。零新增机制，缺点是「一天开几次机就签几次」（无害，已签会返回 ALREADY）。
- 方案 B：主程序启动时检查「今日是否已签」（在 accounts.json 加一个 `last_checkin_date` 兼容字段），未签则后台静默执行。

二期再定，一期只做手动触发。

### 3.5 验证与边界

- **验证方式**：本地起 `npm run tauri dev`，用真实账号点「签到」，观察：首次 OK、重复点击 ALREADY、额度刷新（会员 +200 / 免费 +150）。**不要用脚本批量打 claim 接口压测**（9074 限流在前，且可能触发风控）。
- 日志脱敏：只记账号 id/昵称与结果状态，token 不落日志。
- 高峰期 claim 返回 9074 时前端提示「签到人数过多，请稍后重试」，不标为账号异常。
- 遵循仓库免责声明边界：签到为 Trae 官方公开的每日福利活动，本功能只是代用户对其自有账号执行，不扩展批量绕过用途。

---

## 4. 改动文件清单（预估）

| 文件 | 动作 | 预估有效行 |
|---|---|---|
| `src-tauri/src/api/checkin.rs` | 新建 | ~150 |
| `src-tauri/src/api/mod.rs` | 加 `pub mod checkin;` | +1 |
| `src-tauri/src/lib.rs` | 2 个命令 + 注册（lib.rs 已 1475 行超限，命令主体逻辑下沉到 checkin.rs，lib.rs 只留薄壳） | +40 |
| `src/api.ts` | 2 个封装 | +20 |
| `src/types/index.ts` | `CheckinResult` 类型 | +10 |
| `src/components/AccountCard.tsx` | 签到按钮 | +20 |
| `src/App.tsx` | 「全部签到」入口 + 结果展示 | +40 |
| `src/types/errorCodes.ts` | 9074 等错误码映射 | +5 |

无新增依赖，无账号存储格式变更（一期）。
