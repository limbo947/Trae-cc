# traework2api 端点与数据字典

> 调研对象：`d:\2api\traework2api`（Go，TRAE SOLO CN 的 OpenAI 兼容反代）
> 整理时间：2026-09-18。所有端点、字段均取自源码实测注释（`internal/upstream/constants.go` 标注「来自实测，禁止改动」）。
> 文档分两部分：**A. 上游 TRAE 端点**（项目作为客户端去调的）与 **B. 本地服务端点**（项目自己暴露的）。

---

## A. 上游 TRAE 端点（7 个 API + 1 个登录页）

涉及 4 个 Host（`internal/upstream/constants.go`）：

| Host 常量 | 值 | 用途 |
|---|---|---|
| AgentHost | `https://trae-api-cn.mchost.guru` | SOLO 对话 / 模型表 |
| UgHost | `https://api.trae.cn` | 签到 / 积分额度 |
| OAuthHost | `https://api.trae.com.cn` | 换 token / 用户信息 |
| ConsoleHost | `https://www.trae.cn` | 登录页 |

固定参数：`ClientID=en1oxy7wnw8j9n`、`AppID=6eefa01c-1036-4c7e-9ca5-d891f63bfcd8`、`IdeVersion=0.1.52`（版本号控制模型解锁，0.1.43 请求 glm-5.3 会报 4001）。

### A1. SOLO 对话 —— 核心通道

```
POST https://trae-api-cn.mchost.guru/api/agent/v3/llm_utils_chat
```

- **请求头（SOLOHeaders，全部实测必须）**：`Authorization: Cloud-IDE-JWT <token>` + `X-Cloudide-Token` + `X-Ide-Token`（三者同为 token）+ `X-Uid` + `X-App-Id` + `X-Ide-Version: 0.1.52` + `X-Ide-Version-Code: 20260811` + `X-Device-Type: windows` + `X-OS-Version` + `X-Device-Brand: 83DG` + `X-Machine-Id` / `X-Device-Id` + `Request-Traffic-Type: prod`
- **请求体**：OpenAI 风格 messages + `model=<config_name>`，关键参数 `function: "solo_work_lite"`（实测其他值如 work/solo/work_lite 均无效）
- **返回**：自定义 SSE 流（非标准 OpenAI 格式），事件序列：

| event | data 内容 |
|---|---|
| `metadata` | `session_id`、`prompt_completion_id` 等会话元数据 |
| `timing_cost` | 各阶段耗时（如 `llm_raw_chat_v2`） |
| `output` ×N | **核心内容**：`response`（正文增量）、`reasoning_content`（思考链增量）、`tool_calls` |
| `extra_info` | reasoning_content 完整版 |
| `token_usage` | `prompt_tokens` / `completion_tokens` / `total_tokens` / `reasoning_tokens` |
| `notify_usage` | **计费结构**：`billing_mode: "credits"` + `cn_credits_remain_info: {ide_credits, work_credits}` ← SOLO 通道真实剩余额度在这里 |
| `done` | `finish_reason: "stop"` |
| `error` | 流内业务错误（code + msg），如 1005 权益不足 |

### A2. 模型表

```
POST https://trae-api-cn.mchost.guru/api/ide/v1/get_detail_param
```

- 请求体：`{"function":"solo_work_lite","config_names":null,"need_prompt":false,"poly_prompt":true,...}`
- **返回**：`config_info_list[]`，每项含 `config_name`（模型 ID，如 glm-5.3）、`display_config.display_name`、`model_detail_list[].model_name`。0.1.52 版本返回约 35 个配置，列表随版本动态更新

### A3. 换 Token（refreshToken 轮换）

```
POST https://api.trae.com.cn/cloudide/api/v3/trae/oauth/ExchangeToken
```

- 请求体：`{ClientID, RefreshToken, ClientSecret: "-", UserID: ""}`（头无签名，仅 UA）
- **返回**：`Result.{Token, TokenExpireAt, TokenExpireDuration, RefreshToken, RefreshExpireAt}`
- 注意：`TokenExpireAt` 是**毫秒**时间戳（需归一化为秒，>1e12 判定）；每次调用**轮换 refreshToken**，成功后必须落盘新值

### A4. 用户信息

```
POST https://api.trae.com.cn/cloudide/api/v3/trae/GetUserInfo
```

- 请求体：`{ReqSource: "IDE", IDEVersion: "0.1.52"}` + 头 `X-Cloudide-Token: <token>`
- **返回**：`Result.{UserID, ScreenName, EnterpriseID}` —— uid、昵称、企业 ID

### A5. 签到状态

```
POST https://api.trae.cn/trae/api/v2/ug/checkin_credits/status
```

- 请求体 `{}`；头（UgHeaders）：`Authorization: Cloud-IDE-JWT` + `X-User-Region: CN` + UA `Trae/0.1.52`（+ 可选 `X-Device-Id`）
- **返回**：`{checked_in, code, credits, extra_credits, did_checked_in, enable, message}` —— 今日是否已签 / 签到可得积分（`credits` 会员 200、免费 150；实测免费号 `credits=150` 另有 `extra_credits=50` 加赠）/ 活动是否开放

### A6. 执行签到

```
POST https://api.trae.cn/trae/api/v2/ug/checkin_credits/claim
```

- 请求体 `{}`，头同 A5
- **返回**：成功空响应（对已签到账号重复 claim 也返回 `{"code":0,"message":"success"}`）；签到奖励：**会员 +200 / 免费用户 +150**（官方技术支持确认，均为通用积分，TRAE Code + Work 通用，有效期 31 天）。高峰常报 `9074 当前参与用户太多，请稍后再试`（限流，稍后重试即可；实测为账号级——同一时刻不同账号结果可能不同）；**按设备每日去重**：同一 `X-Device-Id` 当日已签返回 `9095 当前设备今日已经签到，请明日再来哦～`；缺 `X-Device-Id` 返回 `9004`。「已签到」判定须同时覆盖「已签到」与「已经签到」（后者不含前者子串）

### A7. 积分额度

```
POST https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage
```

- 请求体 `{}`，头同 A5
- **返回**：`user_entitlement_pack_list[]`，每项 `entitlement_base_info.quota.credits_limit`（额度上限）+ `usage.credits_amount`（已用，浮点）
- 聚合方式：limit/used/remain 分别求和，`remain = limit - used`
- ⚠️ **实测坑**：该端点聚合的是全部 entitlement 包（含 work 包），`remain=2000` 可能是 work_credits，**不代表 SOLO 通道可用额度**；SOLO 真实额度要看 A1 对话流里的 `notify_usage.cn_credits_remain_info.ide_credits`

### A8. 登录页（浏览器）

```
GET https://www.trae.cn/authorization?...参数集
```

- 关键参数：`client_id`、`auth_callback_url`（强制 `127.0.0.1` 回调）、`machine_id` / `device_id`（hex32，需与落盘凭证同一对）、`login_trace_id`（由 machine+device 派生 hex16，回调时回传用于关联）、`x_app_version` 等
- 登录成功后 302 到回调地址，query 携带：`refreshToken`、`userInfo`（JSON：`UserID/ScreenName/TenantID`，中文昵称双重 URL 编码需二次解码）、`userJwt`（JSON：`Token/RefreshToken/TokenExpireAt` 兜底）、`loginTraceID`

---

## B. 本地服务端点（默认 `:7864`，登录回调 `:18080`）

鉴权：写操作与 `/v1/*` 需 `Authorization: Bearer $TW2A_API_KEY`（常量时间比较）；`/admin` 读接口与 `/healthz` 无鉴权（本地面板定位）。

### B1. OpenAI 兼容对话

```
POST /v1/chat/completions
```

- 标准 OpenAI 请求体（`model` 支持 config_name、`__dev` 后缀自动剥离、`auto`/空走默认模型、下划线命名宽松归一化）；`stream: true/false`
- 内部行为：池内挑号（积分降序）→ token 临期自动 ExchangeToken → 转发 A1 → SSE 转 OpenAI 格式；1005 冷却 12h、429 冷却 60s、session 失效禁用，单请求最多轮换 3 个账号
- **返回**：标准 OpenAI chat.completion（流式为 `data:` 块）；全账号不可用时 503 `no_healthy_account`
- 请求体上限 8MB（413）

### B2. 模型列表

```
GET /v1/models
```

- **返回**：OpenAI `{"object":"list","data":[...]}`，每项 `{id, object:"model", owned_by:"trae-solo", context_length}`
- 数据源：优先动态拉 A2（缓存 1h，失败负缓存 5min），失败回退内置静态表（32 个 config_name）

### B3. 账号池状态

```
GET /status
```

- **返回**：`{accounts: [pool.Status...]}` —— 每账号 uid、昵称、启停、冷却/禁用状态与原因、积分、连续错误计数

### B4. 健康检查

```
GET /healthz
```

- **返回**：纯文本 `ok`

### B5. Web 管理面板

```
GET /admin
```

- **返回**：`go:embed` 内嵌的单页 HTML（深色面板，60s 自动刷新）：账号卡片（昵称/uid/剩余积分大字/总量/已用/权益包数/签到徽章/冷却禁用标记）、账号管理操作、Web 登录入口、导入框

### B6. 全账号实时额度 + 签到状态

```
GET /admin/api/credits
```

- **返回**：`accounts[]`，每项 `{uid, nickname, remain, limit, used, packs, checked_in, checkin_credits, checkin_enable, cooling, disabled, error?}`
- 内部多账号**并发**拉取 A7 + A5 聚合

### B7. 账号列表（脱敏）

```
GET /admin/api/accounts
```

- **返回**：`accounts[]`，每项 `{uid, nickname, enterprise_id, enabled, disabled, cooling, reason, credits, err_count, expires_at, expired_soon, machine_id(前8位), device_id(前8位), has_auth}`

### B8. 导入账号 🔒

```
POST /admin/api/accounts/import
```

- body 三选一：`callback_url`（TRAE 回调链接，走 ParseCallback → ExchangeToken → GetUserInfo 全流程）、`json`（嵌套形 `{"auth":..,"account":..}` 或扁平形凭证）、可附 `machine_id`/`device_id` 覆盖
- **返回**：`{uid, nickname, action: "created"|"updated", needs_check}`；落盘 `auths/trae-{uid}.json`（原子写 0600）+ 池热加载，**无需重启**

### B9. 删除账号 🔒

```
DELETE /admin/api/accounts/{uid}
```

- 删池条目 + 删凭证文件（幂等）；**返回** `{uid, deleted: true}`

### B10. 改账号（软开关 / 昵称）🔒

```
PATCH /admin/api/accounts/{uid}
```

- body `{enabled?: bool, nickname?: string}`；token 字段一律拒绝修改
- **返回**：最新 pool.Status

### B11. 手动刷新 Token 🔒

```
POST /admin/api/accounts/{uid}/refresh
```

- 强制 ExchangeToken + 落盘；session 失效自动硬禁用
- **返回**：`{uid, expires_at, status}`

### B12. 凭证 JSON 预览（严格脱敏）

```
GET /admin/api/accounts/{uid}/json
```

- **返回**：`{uid, nickname, enterprise_id, domain, api_host, machine_id(前8位), device_id(前8位), access_token(前12字符+长度), refresh_token(同), expires_at, file_path}` —— 完整 token 绝不返回

### B13. Web 登录闭环 🔒（start/cancel）

```
POST /admin/api/login              → {login_url, pending_id, callback_url}
POST /admin/api/login/cancel       → {pending_id, canceled}
```

- start 生成随机 machine/device id + 登录 URL（A8），建 10 分钟 TTL 的内存 pending 态

### B14. 登录结果轮询

```
GET /admin/api/login/result?pending_id=...
```

- **返回**：`{pending_id, state: pending|success|failed|canceled, uid?, nickname?, error?}`

### B15. TRAE 回调落点（:18080，无鉴权）

```
GET /authorize?refreshToken=...&userInfo={...}&userJwt={...}&loginTraceID=...
```

- TRAE 浏览器 302 落点（不带 Bearer 故无需鉴权）：解析回调 → 用 `loginTraceID` 反查 pending 取回登录时的 machine/device id 对 → ExchangeToken → GetUserInfo → 原子落盘 → 池热加载 → 标记 pending 成功
- **返回**：浏览器可见的极简 HTML（成功/失败提示，3 秒后尝试自动关窗），非 JSON

---

## 附：错误码速查（实测，RESEARCH.md §5）

| code | 含义 | 项目内处理 |
|---|---|---|
| 1001 | 认证失败（token 失效） | 换 refreshToken 重登 |
| 1005 | plan 权益不足 | 账号长冷却 12h |
| 4001 | 参数无效（模型/版本不匹配） | 升级 IdeVersion 或换模型 |
| 4008 | ide_credits 配额耗尽 | 等每日重置 / 签到 |
| 4011 | 请求频率超限 | 等限流窗口 |
| 9074 | 当前参与用户太多（限流） | 稍后重试，不计失败 |
| 9095 | 当前设备今日已经签到 | 设备维度去重，当日无法再签（换账号也不行） |
| 9004 | 订单参数错误 | `claim` 缺 `X-Device-Id` 头 |
| 429 | 软限流 | 账号短冷却 60s |

## 附：对 trae-cc 的可复用点

- **已采用**：A5/A6 签到（见 [SIGNIN_FEATURE_PLAN.md](SIGNIN_FEATURE_PLAN.md)）、A7 积分（`cn_credits.rs` 已实现同端点）
- **可参考**：A3 ExchangeToken 的 refreshToken 轮换机制（trae-cc 目前无 refreshToken 字段，走 cookies 刷新）；A1 的 `notify_usage` 里 `ide_credits` 才是 SOLO 通道真实额度（若未来 trae-cc 要做更精准的额度展示可关注）；B 侧面板的 token 脱敏展示（前缀+长度）与原子写盘实践
