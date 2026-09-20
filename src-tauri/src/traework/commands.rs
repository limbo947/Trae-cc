//! TraeWork 的 Tauri 命令层。
//!
//! 为什么命令实现放在这里而不是 `lib.rs`：`lib.rs` 已远超单文件 800 行的约束，而本
//! 模块的命令是自洽的一整套（7 个）。注册仍在 `lib.rs` 的 `generate_handler!` 里显式
//! 列出——Tauri 允许写路径限定名，命令清单保持「一处可见」的既有约定。
//!
//! 三层职责边界（不要混）：
//! - 命令层：入参校验、锁的获取与释放（**绝不持锁做文件操作或网络请求**）、阻塞任务调度；
//! - 编排层（`mod.rs`）：流程顺序与回滚；
//! - 原语层（`snapshot` / `proc` / `locate` / `uid`）：单步动作。
//!
//! 所有涉及杀进程 / 拷文件的动作都放进 `spawn_blocking`：`std::fs` 与 `tasklist` 是阻塞
//! 调用，直接在 async 命令里执行会占住 tokio 工作线程，把并行查询额度的请求一起卡住。

use tauri::State;

use super::profile::Ctx;
use super::{locate, proc, snapshot, uid, StepCollector, StepLog};
use super::{reconcile, save_current_login, switch_to, ReconcileReport, SaveOutcome};
use crate::account::{AccountBrief, APP_TRAEWORK};
use crate::{ApiError, AppState};

/// 构造 `ApiError`（`ApiError::from` 只吃 `anyhow::Error`，本模块的错误都是面向用户的
/// 中文字符串，直接构造比先包一层 anyhow 再解构更直白）
fn err(msg: impl Into<String>) -> ApiError {
    ApiError {
        message: msg.into(),
    }
}

#[derive(serde::Serialize)]
pub struct TraeworkActionResult {
    /// 面向用户的结果描述
    pub message: String,
    /// 执行过程的分步记录（前端「详情」里展开）
    pub steps: Vec<StepLog>,
    /// 涉及账号（保存登录态时为新落库的账号）
    pub account: Option<AccountBrief>,
}

/// TraeWork 概览
#[derive(serde::Serialize)]
pub struct TraeworkOverview {
    /// 当前已生效的槽位（来自快照目录的 current_account.txt，非账号库字段）
    pub current_slot: Option<String>,
    /// 客户端是否在运行
    pub running: bool,
    /// 可执行文件路径（未配置/未找到为 None）
    pub exe_path: Option<String>,
    /// 各槽位快照状态（账号槽 + 保留槽 last）
    pub snapshots: Vec<snapshot::SlotStatus>,
    /// 磁盘上有快照但账号库里没有对应账号的孤儿槽（导出/删除账号后可能残留）
    pub orphan_slots: Vec<String>,
}

/// 槽位状态 + 快照内的凭据到期时间
///
/// 为什么在这里合并而不是让 `slot_status` 自己解析：`snapshot` 只认文件系统，解析登录态
/// 内容属 `uid` 层（分层见 mod.rs 头部）。合并只多一次文件读取，却换来了两层各自纯净。
///
/// 为什么面板需要它：快照能不能长期用，取决于快照里那份凭据的到期时间——实测 access 寿命
/// 14 天、refresh 记录 180 天，放着不用的槽位会先过 access、再过 refresh，用户看不到这个
/// 期限就只能靠"切换失败"来发现。
fn slot_status_with_credential(ctx: &Ctx, slot: &str) -> snapshot::SlotStatus {
    let mut status = snapshot::slot_status(ctx, slot);
    if let Some(profile) = uid::slot_profile(ctx, slot) {
        status.expired_at = profile.expired_at;
        status.refresh_expired_at = profile.refresh_expired_at;
    }
    status
}

/// TraeWork 概览：当前账号、进程状态、各槽位快照体积
#[tauri::command]
pub async fn traework_overview(state: State<'_, AppState>) -> Result<TraeworkOverview, ApiError> {
    // 短锁：只取槽位名清单，文件系统统计放到锁外
    let slots: Vec<String> = {
        let manager = state.account_manager.lock().await;
        manager
            .get_accounts()
            .into_iter()
            .filter(|a| a.app == APP_TRAEWORK)
            .filter_map(|a| a.uid)
            .collect()
    };

    tauri::async_runtime::spawn_blocking(move || -> Result<TraeworkOverview, String> {
        let ctx = Ctx::from_env()?;
        let mut all_slots = slots.clone();
        all_slots.push(super::LAST_SLOT.to_string());
        let snapshots = all_slots
            .iter()
            .map(|s| slot_status_with_credential(&ctx, s))
            .collect();
        // 用账号槽清单（已剔除保留槽 last）而不是全目录：last 由上面显式加入 snapshots
        // 展示回退状态，但它不是「未登记账号」，不该混进孤儿槽提示里
        let orphan_slots = snapshot::list_account_slots(&ctx)
            .into_iter()
            .filter(|s| !slots.contains(s))
            .collect();

        Ok(TraeworkOverview {
            current_slot: snapshot::read_current_slot(&ctx),
            running: proc::is_running(),
            exe_path: locate::find_exe().ok().map(|p| p.to_string_lossy().to_string()),
            snapshots,
            orphan_slots,
        })
    })
    .await
    .map_err(|e| err(format!("TraeWork 概览查询异常: {e}")))?
    .map_err(err)
}

/// 读取当前 TraeWork 登录账号（证据链推导，可能置信度不足）
#[tauri::command]
pub async fn traework_discover() -> Result<uid::UidEvidence, ApiError> {
    tauri::async_runtime::spawn_blocking(move || -> Result<uid::UidEvidence, String> {
        let ctx = Ctx::from_env()?;
        Ok(uid::discover(&ctx))
    })
    .await
    .map_err(|e| err(format!("TraeWork 账号识别异常: {e}")))?
    .map_err(err)
}

/// 保存当前登录态到快照，并把账号登记到账号库
///
/// 为什么保存成功后才登记账号：登记意味着前端会出现一条可切换的记录。若先登记后保存，
/// 保存失败就会留下一个「有账号但没有快照」的槽位，用户点切换时才报错——把失败推迟到
/// 更晚、更难解释的位置。
#[tauri::command]
pub async fn traework_save_current_login(
    uid: Option<String>,
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<TraeworkActionResult, ApiError> {
    let explicit = uid
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    // 显式传入的名字优先（当前前端不传，保留给将来的重命名入口）
    let explicit_name = name.filter(|n| !n.trim().is_empty());

    let (outcome, steps) = tauri::async_runtime::spawn_blocking(
        move || -> Result<(SaveOutcome, Vec<StepLog>), String> {
            let ctx = Ctx::from_env()?;
            let slot = match explicit {
                Some(u) => u,
                None => {
                    let evidence = uid::discover(&ctx);
                    if !evidence.confident {
                        return Err(evidence.reason);
                    }
                    evidence
                        .uid
                        .ok_or_else(|| "未识别到 TraeWork 登录账号".to_string())?
                }
            };
            let sink = StepCollector::new();
            let outcome = save_current_login(&ctx, &slot, &sink)?;
            Ok((outcome, sink.steps()))
        },
    )
    .await
    .map_err(|e| err(format!("保存登录态任务异常: {e}")))?
    .map_err(err)?;

    // 账号名优先取登录态里解出的用户名——那才是用户在客户端里看到的名字。
    // 取不到时**传 None**（而非 uid）交给 upsert 兜底：upsert 对 None 是"不改名"，
    // 传 uid 会把用户可能已手工改过的名字打回去
    let display = explicit_name.or_else(|| outcome.profile.as_ref().map(|p| p.display_name()));
    let slot = outcome.slot.clone();

    // 落库（长耗时的杀进程/拷贝已完成，此处只做一次极短的写盘）
    let brief = {
        let mut manager = state.account_manager.lock().await;
        let account = manager
            .upsert_traework_account(&slot, display)
            .map_err(ApiError::from)?;
        AccountBrief::from_account(&account, false)
    };

    Ok(TraeworkActionResult {
        message: format!("已保存账号 {slot} 的登录态（{} 项）", outcome.copied),
        steps,
        account: Some(brief),
    })
}

/// 切换到指定 TraeWork 账号（按账号 id）
#[tauri::command]
pub async fn traework_switch_account(
    account_id: String,
    state: State<'_, AppState>,
) -> Result<TraeworkActionResult, ApiError> {
    let slot = {
        let manager = state.account_manager.lock().await;
        let account = manager.get_account(&account_id).map_err(ApiError::from)?;
        if account.app != APP_TRAEWORK {
            return Err(err("该账号不是 TraeWork 账号，请使用「切换账号」操作 TraeCode 账号"));
        }
        account
            .slot()
            .map(|s| s.to_string())
            .ok_or_else(|| err("该 TraeWork 账号缺少 uid，无法定位快照"))?
    };

    let slot_for_task = slot.clone();
    let (steps, restored, expired_before) = tauri::async_runtime::spawn_blocking(
        move || -> Result<(Vec<StepLog>, usize, bool), String> {
            let ctx = Ctx::from_env()?;
            let sink = StepCollector::new();
            // 凭据状态必须在**切换前**读：切完之后现场已被覆盖，读到的是客户端续期后的新值
            let expired_before = uid::slot_profile(&ctx, &slot_for_task)
                .is_some_and(|p| p.access_expired());
            let restored = switch_to(&ctx, &slot_for_task, &sink)?;
            Ok((sink.steps(), restored, expired_before))
        },
    )
    .await
    .map_err(|e| err(format!("切换任务异常: {e}")))?
    .map_err(err)?;

    // 为什么要提示重新保存：客户端启动时会用快照里的 refreshToken 换发新凭据并写回**现场**，
    // 而快照文件不会跟着更新；refreshToken 若是轮换型，槽位里那份随即作废，该槽位下次
    // （access 过期后）再切回就要求重新登录。凭据寿命实测见 uid.rs 的字段注释。
    let expired_note = if expired_before {
        "；快照的 access 凭据此前已过期，本次由客户端自动续期"
    } else {
        ""
    };
    Ok(TraeworkActionResult {
        message: format!(
            "已切换到 TraeWork 账号 {slot}（{restored} 项已恢复）{expired_note}。建议再点一次「保存当前登录态」，把客户端换发后的凭据写回该槽位"
        ),
        steps,
        account: None,
    })
}

/// 删除某账号的快照（释放磁盘），返回释放的字节数
#[tauri::command]
pub async fn traework_delete_snapshot(
    account_id: String,
    state: State<'_, AppState>,
) -> Result<u64, ApiError> {
    let slot = {
        let manager = state.account_manager.lock().await;
        let account = manager.get_account(&account_id).map_err(ApiError::from)?;
        account
            .slot()
            .map(|s| s.to_string())
            .ok_or_else(|| err("该 TraeWork 账号缺少 uid，无法定位快照"))?
    };

    tauri::async_runtime::spawn_blocking(move || -> Result<u64, String> {
        let ctx = Ctx::from_env()?;
        snapshot::delete_slot(&ctx, &slot)
    })
    .await
    .map_err(|e| err(format!("删除快照任务异常: {e}")))?
    .map_err(err)
}

/// 移除 TraeWork 账号：删账号记录，并一并删掉磁盘快照（含回退代）
///
/// 为什么必须连快照一起删：账号管理页过滤掉 TraeWork 账号，本面板是它唯一的出口；只删记录
/// 会留下一个孤儿槽——面板底部会把它报成「未登记账号的快照」，下次点「修复槽位」还会按槽位
/// 把它重新登记回账号库，用户看到的就是「删了又回来」。
///
/// 为什么先删文件再删记录：倒过来的话，删记录成功而文件删除失败，同样会留下孤儿槽（即上面
/// 那个残局）。当前顺序下任一失败都只是「什么都没发生」，报错重试即可。
#[tauri::command]
pub async fn traework_remove_account(
    account_id: String,
    state: State<'_, AppState>,
) -> Result<TraeworkActionResult, ApiError> {
    let slot = {
        let manager = state.account_manager.lock().await;
        let account = manager.get_account(&account_id).map_err(ApiError::from)?;
        if account.app != APP_TRAEWORK {
            return Err(err("该账号不是 TraeWork 账号，请使用「删除账号」操作 TraeCode 账号"));
        }
        account
            .slot()
            .map(|s| s.to_string())
            .ok_or_else(|| err("该 TraeWork 账号缺少 uid，无法定位快照"))?
    };

    let slot_for_task = slot.clone();
    let freed = tauri::async_runtime::spawn_blocking(move || -> Result<u64, String> {
        let ctx = Ctx::from_env()?;
        let freed = snapshot::delete_slot(&ctx, &slot_for_task)?;
        // 删掉的正好是被标记为「当前账号」的槽位时，标记必须清掉：留着会让面板把当前账号显示成
        // 一个已不存在的 uid，且没有任何入口能纠正它
        if snapshot::read_current_slot(&ctx).as_deref() == Some(slot_for_task.as_str()) {
            snapshot::clear_current_slot(&ctx)?;
        }
        Ok(freed)
    })
    .await
    .map_err(|e| err(format!("移除账号任务异常: {e}")))?
    .map_err(err)?;

    // 记录删除只可能失败于「账号不存在」，而上面刚读到过它；真失败时快照已删，如实告知
    {
        let mut manager = state.account_manager.lock().await;
        manager.remove_account(&account_id).map_err(ApiError::from)?;
    }

    Ok(TraeworkActionResult {
        message: format!(
            "已移除 TraeWork 账号 {slot}（{:.1} MB 快照已释放）",
            freed as f64 / 1048576.0
        ),
        steps: Vec::new(),
        account: None,
    })
}

/// 设置 TraeWork 可执行文件路径（必须过 exe 白名单）
#[tauri::command]
pub async fn traework_set_path(path: String) -> Result<String, ApiError> {
    locate::save_path(&path).map_err(err)?;
    Ok(path)
}

/// 自动扫描 TraeWork 可执行文件路径
#[tauri::command]
pub async fn traework_scan_path() -> Result<Option<String>, ApiError> {
    Ok(locate::find_exe()
        .ok()
        .map(|p| p.to_string_lossy().to_string()))
}

/// 修复结果
#[derive(serde::Serialize)]
pub struct TraeworkReconcile {
    pub message: String,
    pub steps: Vec<StepLog>,
    pub renamed: Vec<(String, String)>,
    pub skipped: Vec<String>,
    pub slots: Vec<String>,
    /// 清理白名单外历史文件释放的字节数
    pub freed_bytes: u64,
}

/// 修复槽位：把内容与目录名不符的快照按真实账号 id 归位，并把槽位登记进账号库
///
/// 为什么登记也放在这里：错位快照归位后，账号库里往往缺这条记录（历史事故中它是被漏掉的
/// 那一个账号），不登记则 UI 看不到、也无法切换——修复就成了半截活。
#[tauri::command]
pub async fn traework_reconcile(state: State<'_, AppState>) -> Result<TraeworkReconcile, ApiError> {
    let (report, steps) = tauri::async_runtime::spawn_blocking(
        move || -> Result<(ReconcileReport, Vec<StepLog>), String> {
            let ctx = Ctx::from_env()?;
            let sink = StepCollector::new();
            let report = reconcile(&ctx, &sink)?;
            Ok((report, sink.steps()))
        },
    )
    .await
    .map_err(|e| err(format!("修复任务异常: {e}")))?
    .map_err(err)?;

    let mut registered = 0usize;
    let mut named = 0usize;
    let mut purged = 0usize;
    {
        let mut manager = state.account_manager.lock().await;

        // 先清掉保留槽的历史误登记。修复流程早期版本会把 `last` 目录按账号 upsert 进库，
        // 于是列表里长出一条「名字取的是另一个账号、点切换必被拒绝」的假账号；而修复流程
        // 本就是它唯一的产生源，所以在同一处顺手收尾（否则用户只能靠手工改 accounts.json）。
        let bogus: Vec<String> = manager
            .get_accounts()
            .into_iter()
            .filter(|a| a.app == APP_TRAEWORK && a.uid.as_deref().is_some_and(snapshot::is_reserved_slot))
            .map(|a| a.id)
            .collect();
        for id in bogus {
            match manager.remove_account(&id) {
                Ok(()) => purged += 1,
                // 与下面的登记同理：单个失败不该让整个修复失败，归位动作已经做完且不可回退
                Err(e) => log::warn!("清理保留槽误登记账号 {id} 失败: {e}"),
            }
        }

        for slot in &report.slots {
            // 用登录态里解出的用户名回填。存量账号记录的 name 是当初拿 uid 顶上的，
            // 不回填的话用户得把每个账号重新登录保存一遍才能看到可读名字
            let name = report
                .identities
                .iter()
                .find(|i| &i.slot == slot)
                .and_then(|i| i.username.clone());
            if name.is_some() {
                named += 1;
            }
            match manager.upsert_traework_account(slot, name) {
                Ok(_) => registered += 1,
                // 单个槽位登记失败不该让整个修复失败：归位动作已完成且不可回退
                Err(e) => log::warn!("登记槽位 {slot} 失败: {e}"),
            }
        }
    }

    let freed_mb = report.freed_bytes as f64 / 1048576.0;
    // 摘要必须把 skipped 算进去：只说 renamed 会出现「未发现错位的槽位」与下面的
    // 「有 N 项未自动处理」同时弹出的自相矛盾文案（2026-09-20 实测截图）
    let head = match (report.renamed.len(), report.skipped.len()) {
        (0, 0) => "未发现错位的槽位".to_string(),
        (0, n) => format!("有 {n} 项无法自动归位"),
        (r, 0) => format!("已归位 {r} 个错位槽位"),
        (r, n) => format!("已归位 {r} 个错位槽位，另有 {n} 项无法自动归位"),
    };
    let purged_note = if purged > 0 {
        format!("清理 {purged} 条误登记的保留槽账号，")
    } else {
        String::new()
    };
    let message = format!(
        "{head}，现有 {registered} 个槽位已登记（{named} 个补上了用户名），{purged_note}清理释放 {freed_mb:.1} MB"
    );

    Ok(TraeworkReconcile {
        message,
        steps,
        renamed: report.renamed,
        skipped: report.skipped,
        slots: report.slots,
        freed_bytes: report.freed_bytes,
    })
}
