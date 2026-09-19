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
use super::{reconcile, save_current_login, switch_to, ReconcileReport};
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
            .map(|s| snapshot::slot_status(&ctx, s))
            .collect();
        let orphan_slots = snapshot::list_slots(&ctx)
            .into_iter()
            .filter(|s| !slots.contains(s) && s != super::LAST_SLOT)
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

    let (slot, steps, copied) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(String, Vec<StepLog>, usize), String> {
            let ctx = Ctx::from_env()?;
            let slot = match explicit {
                Some(u) => u,
                None => {
                    let evidence = uid::discover(&ctx);
                    if !evidence.confident {
                        return Err(evidence.reason);
                    }
                    evidence.uid.ok_or_else(|| "未识别到 TraeWork 登录账号".to_string())?
                }
            };
            let sink = StepCollector::new();
            let copied = save_current_login(&ctx, &slot, &sink)?;
            Ok((slot, sink.steps(), copied))
        })
        .await
        .map_err(|e| err(format!("保存登录态任务异常: {e}")))?
        .map_err(err)?;

    // 落库（长耗时的杀进程/拷贝已完成，此处只做一次极短的写盘）
    let brief = {
        let mut manager = state.account_manager.lock().await;
        let account = manager
            .upsert_traework_account(&slot, name)
            .map_err(ApiError::from)?;
        AccountBrief::from_account(&account, false)
    };

    Ok(TraeworkActionResult {
        message: format!("已保存账号 {slot} 的登录态（{copied} 项）"),
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
    let (steps, restored) = tauri::async_runtime::spawn_blocking(
        move || -> Result<(Vec<StepLog>, usize), String> {
            let ctx = Ctx::from_env()?;
            let sink = StepCollector::new();
            let restored = switch_to(&ctx, &slot_for_task, &sink)?;
            Ok((sink.steps(), restored))
        },
    )
    .await
    .map_err(|e| err(format!("切换任务异常: {e}")))?
    .map_err(err)?;

    Ok(TraeworkActionResult {
        message: format!("已切换到 TraeWork 账号 {slot}（{restored} 项已恢复）"),
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
    {
        let mut manager = state.account_manager.lock().await;
        for slot in &report.slots {
            match manager.upsert_traework_account(slot, None) {
                Ok(_) => registered += 1,
                // 单个槽位登记失败不该让整个修复失败：归位动作已完成且不可回退
                Err(e) => log::warn!("登记槽位 {slot} 失败: {e}"),
            }
        }
    }

    let freed_mb = report.freed_bytes as f64 / 1048576.0;
    let head = if report.renamed.is_empty() {
        "未发现错位的槽位".to_string()
    } else {
        format!("已归位 {} 个错位槽位", report.renamed.len())
    };
    let message = format!(
        "{head}，现有 {registered} 个槽位已登记，清理释放 {freed_mb:.1} MB"
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
