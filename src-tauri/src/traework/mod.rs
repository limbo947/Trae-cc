//! TraeWork（TRAE SOLO CN）账号切换：编排层。
//!
//! 为什么 TraeWork 必须走「快照 / 恢复」而不是 traecode 那套「改写登录态」：
//! traecode 的登录真源只有 `storage.json`，用 `tc_crypto` 加密写入即可秒切；TraeWork 的
//! 登录真源是 `storage.json` **加** `state.vscdb` 双源，而后者是带加密 secret storage 的
//! SQLite——写 JSON 覆盖不到它，客户端会以 vscdb 为准，表现为「切换后仍要重新登录」。
//! 快照方案不需要证明「另一个真源不会覆盖它」，因为两个都被整体替换了。
//!
//! 编排契约（对齐参考实现 `switcher/mod.rs::switch_flow`，每条都由一次事故换来）：
//! - **先预检快照存在性，再关客户端**：快照缺失时立刻失败，不破坏用户当前状态；
//! - **切换前把现场备份到 `last` 槽**：任何失败都能回滚到「切换前」，而不是回滚到「未知」；
//! - **恢复后校验 + 失败回滚**：0 项恢复或必需项缺失时，用 `last` 回滚并如实报错；
//! - **全局串行锁**：进行中直接拒绝（防连点导致重复杀进程 / 交错覆盖快照）。
//!
//! 进度反馈：本模块把每一步记进 [`StepCollector`] 并随命令结果返回。**刻意不引入
//! Tauri 事件流**：本仓库当前没有任何 `emit` 用法，为这一个功能新增事件通道会带来
//! 「前端订阅时机 / 事件丢失」一整套新问题，而前端只需在等待期间展示「进行中 + 已耗时」
//! 即可满足「明确进度、防连点」的原始诉求。

pub mod commands;
pub mod locate;
pub mod proc;
pub mod profile;
pub mod snapshot;
pub mod uid;

use std::sync::Mutex;

use profile::Ctx;

/// 步骤状态（语义对齐参考实现，前端按值着色）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// 一般信息
    Info,
    /// 已开始但尚未结束（如「已发送关闭请求，等待退出」）——前端应显示为进行中而非完成
    Running,
    Ok,
    Warn,
    Error,
    Skip,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Info => "info",
            StepStatus::Running => "running",
            StepStatus::Ok => "ok",
            StepStatus::Warn => "warn",
            StepStatus::Error => "error",
            StepStatus::Skip => "skip",
        }
    }
}

/// 单条进度（随命令结果返回给前端展示）
#[derive(Debug, Clone, serde::Serialize)]
pub struct StepLog {
    pub stage: String,
    pub status: String,
    pub message: String,
}

/// 进度出口：模块内所有耗时动作都应经此汇报，便于「事后从日志复盘为什么切换失败」
pub trait ProgressSink: Send + Sync {
    fn step(&self, stage: &str, status: StepStatus, message: &str);
}

/// 收集型出口：同时落日志与内存（命令结束后回传前端）
#[derive(Default)]
pub struct StepCollector {
    steps: Mutex<Vec<StepLog>>,
}

impl StepCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn steps(&self) -> Vec<StepLog> {
        self.steps.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl ProgressSink for StepCollector {
    fn step(&self, stage: &str, status: StepStatus, message: &str) {
        // 这里只会有路径/账号 id/失败原因，不含 Token 或 Cookie，可安全落盘
        log::info!("[traework][{stage}][{}] {message}", status.as_str());
        if let Ok(mut steps) = self.steps.lock() {
            steps.push(StepLog {
                stage: stage.to_string(),
                status: status.as_str().to_string(),
                message: message.to_string(),
            });
        }
    }
}

/// 无窗口命令行（避免在 GUI 应用里闪出黑框）
#[cfg(target_os = "windows")]
pub(crate) fn command_no_window(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = std::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn command_no_window(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

/// 「现场备份」槽位名（保留名，不参与账号列表）
pub const LAST_SLOT: &str = "last";

/// 全局串行互斥：同一时刻只允许一个保存/切换动作
///
/// 为什么用 `try_lock` 而不是排队等待：连点两次「切换」时，第二个请求排队执行是有害的
/// ——它会在第一个刚写完现场之后又完整跑一遍杀进程 + 覆盖，把用户明确的一次操作放大成
/// 两次；直接拒绝并提示「已有操作进行中」才能让用户知道发生了什么。
fn action_gate() -> &'static Mutex<()> {
    static GATE: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
}

/// 保存当前登录态：关客户端 → 快照到槽位 → 标记当前账号 → 重新启动
///
/// 为什么要关客户端：Electron 在运行时持有 leveldb / vscdb 的文件句柄，运行中拷贝
/// 会静默缺文件（快照「看起来成功」但恢复后登录态不全）。这是「保存」也必须关客户端的
/// 唯一原因，不是保守。
pub fn save_current_login(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<usize, String> {
    let _guard = action_gate()
        .try_lock()
        .map_err(|_| "已有 TraeWork 操作进行中，请稍后再试".to_string())?;

    profile::ensure_slot_safe(slot)?;
    sink.step("init", StepStatus::Info, &format!("开始保存账号 {slot} 的登录态"));

    proc::stop(sink)?;
    let copied = snapshot::backup(ctx, slot, sink)?;

    // 槽名自校验：快照内解密出的账号必须就是槽名。这是 2026-09-19 事故的防线——
    // 当时 uid 来源（icube_gtm.users）滞后，导致「按 A 保存、实际存的是 B」，
    // 而 B 的第二次保存又把 A 的快照整个覆盖掉。校验以快照内部证据为最终裁决，
    // 不依赖任何外部字段的时序，因此能兜住上游格式/时序的漂移。
    let slot = verify_slot_name(ctx, slot, sink)?;

    // 标记只在保存成功后写：槽位里确实有这份现场，标记才不会指向不存在的快照
    snapshot::write_current_slot(ctx, &slot)?;
    sink.step("mark", StepStatus::Info, &format!("当前 TraeWork 账号标记为 {slot}"));

    let exe = locate::find_exe()?;
    proc::start(&exe, sink)?;
    Ok(copied)
}

/// 校验 `槽名 == 快照内解出的账号 id`；不符则把快照改名归位，返回纠正后的槽名
///
/// 读不出账号 id 时**不阻断**（可能是旧客户端明文格式或上游布局变化），只告警——
/// 校验的价值在于兜住已知的错位场景，不应把「读不到附加信息」升级成「无法保存」。
fn verify_slot_name(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<String, String> {
    let Some(actual) = uid::slot_account_id(ctx, slot) else {
        sink.step(
            "verify",
            StepStatus::Warn,
            "无法从快照内解出账号 id，跳过槽名校验",
        );
        return Ok(slot.to_string());
    };
    if actual == slot {
        sink.step(
            "verify",
            StepStatus::Ok,
            &format!("槽名校验通过：快照记录的账号就是 {slot}"),
        );
        return Ok(slot.to_string());
    }
    sink.step(
        "verify",
        StepStatus::Warn,
        &format!("快照记录的账号是 {actual}，与槽名 {slot} 不符（uid 来源滞后），正在改名为 {actual}"),
    );
    snapshot::rename_slot(ctx, slot, &actual)?;
    sink.step(
        "verify",
        StepStatus::Ok,
        &format!("已把快照归位到槽位 {actual}"),
    );
    Ok(actual)
}

/// 修复报告
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconcileReport {
    /// 成功归位的：(原目录名, 纠正后的账号 id)
    pub renamed: Vec<(String, String)>,
    /// 未能自动处理的目录及原因
    pub skipped: Vec<String>,
    /// 修复后现存的槽位（去重）
    pub slots: Vec<String>,
    /// 清理白名单外历史文件释放的总字节数
    pub freed_bytes: u64,
}

/// 整理快照目录：把「内容与目录名不符」的快照按内部真实账号 id 改名归位
///
/// 为什么需要它：错位一旦发生，正确的数据往往压在 `.bak` 里（实测正是如此），
/// 而 `.bak` 不参与正常列表与切换。没有这个入口，用户只能重新登录该账号再存一次，
/// 明明数据还在却要重来——也会让 `.bak` 永远留在磁盘上占空间。
pub fn reconcile(ctx: &Ctx, sink: &dyn ProgressSink) -> Result<ReconcileReport, String> {
    let _guard = action_gate()
        .try_lock()
        .map_err(|_| "已有 TraeWork 操作进行中，请稍后再试".to_string())?;

    let mut renamed: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for (base, is_bak) in snapshot::list_all_dirs(ctx) {
        let dir_name = if is_bak {
            format!("{base}.bak")
        } else {
            base.clone()
        };
        let Some(actual) = uid::slot_account_id(ctx, &dir_name) else {
            skipped.push(format!("{dir_name}：解不出账号 id（可能是空目录或上游改了格式）"));
            continue;
        };
        if actual == base {
            continue; // 名实相符，无需处理
        }
        if ctx.slot_dir(&actual)?.exists() {
            // 目标已被占用：两个目录都可能是有效登录态，交由用户决定，绝不覆盖
            skipped.push(format!(
                "{dir_name}：实际账号是 {actual}，但该槽位已存在，未自动改动"
            ));
            continue;
        }
        snapshot::rename_slot(ctx, &dir_name, &actual)?;
        sink.step(
            "repair",
            StepStatus::Ok,
            &format!("{dir_name} → {actual}"),
        );
        renamed.push((dir_name, actual));
    }

    // 归位之后再清理：先保证每个目录都挂在正确的账号名下，再按白名单瘦身，
    // 这样清理报告里的槽位名与用户看到的一致
    let mut freed_bytes = 0u64;
    for slot in snapshot::list_slots(ctx) {
        freed_bytes += snapshot::prune_slot(ctx, &slot, sink).unwrap_or(0);
    }
    for (_, actual) in &renamed {
        // 归位后的目标是主槽，已被上面遍历覆盖；这里只兜底其对应的回退代
        let bak = format!("{actual}.bak");
        if ctx.bak_dir(actual).map(|p| p.exists()).unwrap_or(false) {
            freed_bytes += snapshot::prune_slot(ctx, &bak, sink).unwrap_or(0);
        }
    }

    Ok(ReconcileReport {
        renamed,
        skipped,
        slots: snapshot::list_slots(ctx),
        freed_bytes,
    })
}

/// 切换到指定槽位（完整编排，含预检、回滚与恢复后校验）
///
/// 返回成功恢复的条目数。失败时保证：客户端已按「切换前」或「切换后」的确定状态启动过，
/// 不存在「恢复了一半」的中间态。
pub fn switch_to(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<usize, String> {
    let _guard = action_gate()
        .try_lock()
        .map_err(|_| "已有 TraeWork 操作进行中，请稍后再试".to_string())?;

    profile::ensure_slot_safe(slot)?;

    // 预检放在关客户端之前：快照缺失是最常见的失败原因，此时不该动用户正在用的客户端
    let has_main = ctx.slot_dir(slot)?.exists();
    let has_bak = ctx.bak_dir(slot)?.exists();
    if !has_main && !has_bak {
        return Err(format!(
            "账号 {slot} 还没有快照，请先用该账号登录 TraeWork 并点击「保存当前登录态」"
        ));
    }

    // 槽名与快照内容不符时拒绝切换：否则用户点「切换到 X」实际登录的是 Y。
    // 这是「以错误账号启动客户端」的唯一入口，宁可拒绝并要求先修复，也不能默默切错。
    if let Some(actual) = uid::slot_account_id(ctx, slot) {
        if actual != slot {
            let msg = format!(
                "槽位 {slot} 内实际是账号 {actual} 的登录态（快照与槽名不符），已拒绝切换以免以错误账号登录。请先点面板上的「修复槽位」后重试。"
            );
            sink.step("fatal", StepStatus::Error, &msg);
            return Err(msg);
        }
    }

    sink.step("init", StepStatus::Info, &format!("开始切换到账号 {slot}"));
    proc::stop(sink)?;

    // 现场先存 last 槽：这是「切换失败也能回到原样」的唯一保底
    snapshot::backup(ctx, LAST_SLOT, sink)?;

    let restored = snapshot::restore(ctx, slot, sink)?;

    if let Err(missing) = snapshot::verify_restore(ctx, restored) {
        sink.step(
            "restore",
            StepStatus::Warn,
            &format!(
                "目标快照无效（{}），正在从 last 槽回滚到切换前状态",
                missing.join("；")
            ),
        );
        // 回滚失败就不再叠加错误：原始原因（快照无效）才是用户需要看到的
        if let Ok(n) = snapshot::restore(ctx, LAST_SLOT, sink) {
            if let Ok(exe) = locate::find_exe() {
                let _ = proc::start(&exe, sink);
            }
            let _ = n;
        }
        let msg = format!(
            "账号 {slot} 的快照无效（{}），已回滚到切换前的状态。请登录该账号后重新「保存当前登录态」；若仍然报错，可能是 TraeWork 新版改了登录态布局",
            missing.join("；")
        );
        // 失败也必须留下一条 error 级步骤：命令返回的错误字符串会被前端的 toast 顶掉，
        // 而「详情」里的步骤记录是用户事后复盘（以及我们排查）唯一能拿到的东西
        sink.step("fatal", StepStatus::Error, &msg);
        return Err(msg);
    }

    snapshot::write_current_slot(ctx, slot)?;
    let exe = locate::find_exe()?;
    proc::start(&exe, sink)?;
    Ok(restored)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::path::PathBuf;

    /// 不产生任何输出的出口（测试里不关心进度内容）
    pub struct QuietSink;

    impl ProgressSink for QuietSink {
        fn step(&self, _: &str, _: StepStatus, _: &str) {}
    }

    /// 造一个完全隔离的临时上下文
    ///
    /// 用「进程号 + 纳秒」做后缀：`cargo test` 默认并行执行，固定名字会让并发的两个测试
    /// 互相删除对方目录（Windows 上表现为随机的「文件不存在」失败）。
    pub fn fake_ctx(tag: &str) -> Ctx {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base: PathBuf = std::env::temp_dir().join(format!(
            "traework-test-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let data = base.join("data");
        let profiles = base.join("profiles");
        std::fs::create_dir_all(&data).expect("创建临时数据目录");
        std::fs::create_dir_all(&profiles).expect("创建临时快照目录");
        Ctx::for_test(data, profiles)
    }
}
