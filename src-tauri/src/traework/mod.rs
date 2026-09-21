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
pub mod credentials;
pub mod device;
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
pub fn save_current_login(
    ctx: &Ctx,
    slot: &str,
    sink: &dyn ProgressSink,
    // 该账号（按 `user_id`）的目标设备标识，由命令层从账号库取出后传入。
    // 为什么不让本层自己去取：编排层不持 `AccountManager` 锁（分层见 `mod.rs` 头部），
    // 而设备标识的权威在账号库（`AccountManager::shared_machine_id`）
    shared_machine_id: Option<&str>,
) -> Result<SaveOutcome, String> {
    let _guard = action_gate()
        .try_lock()
        .map_err(|_| "已有 TraeWork 操作进行中，请稍后再试".to_string())?;

    profile::ensure_slot_safe(slot)?;
    // 保留槽不接受保存：`last` 是切换流程专用的「切换前现场」滚动备份，写进去会顶掉用户
    // 唯一的回退保底；而槽名来自 storage.json 的 `userId`，是被构造的值也能落进这里
    if snapshot::is_reserved_slot(slot) {
        return Err(format!("{slot} 是保留槽名，不能作为账号槽位"));
    }
    sink.step("init", StepStatus::Info, &format!("开始保存账号 {slot} 的登录态"));

    proc::stop(sink)?;
    let copied = snapshot::backup(ctx, slot, sink)?;

    // 槽名自校验：快照内解密出的账号必须就是槽名。这是 2026-09-19 事故的防线——
    // 当时 uid 来源（icube_gtm.users）滞后，导致「按 A 保存、实际存的是 B」，
    // 而 B 的第二次保存又把 A 的快照整个覆盖掉。校验以快照内部证据为最终裁决，
    // 不依赖任何外部字段的时序，因此能兜住上游格式/时序的漂移。
    let requested_slot = slot.to_string();
    let slot = verify_slot_name(ctx, slot, sink)?;

    // 槽位被归位（uid 来源滞后）时，调用方传入的设备标识属于**另一个账号**，必须丢弃——
    // 否则会把 A 的设备标识写到 B 的快照与现场上，比不做对齐更糟
    let shared_machine_id = if slot == requested_slot {
        shared_machine_id
    } else {
        sink.step(
            "device",
            StepStatus::Warn,
            "槽位已归位到另一个账号，本次不使用账号库中的设备标识",
        );
        None
    };

    // 设备标识对齐（必须在客户端仍处于关闭状态时做：写入现场的文件会被运行中的客户端回写覆盖）。
    // 为什么放在槽名校验之后：对齐要同时改槽位快照与现场，槽名定下来才知道改的是哪一个槽位
    let device = device::normalize_on_save(ctx, &slot, sink, shared_machine_id);

    // 标记只在保存成功后写：槽位里确实有这份现场，标记才不会指向不存在的快照
    snapshot::write_current_slot(ctx, &slot)?;
    sink.step("mark", StepStatus::Info, &format!("当前 TraeWork 账号标记为 {slot}"));

    // 展示信息（用户名等）顺手取回：调用方要用它给账号记录命名，
    // 否则界面上只剩一串 uid，用户无法分辨哪个是哪个
    let profile = uid::slot_profile(ctx, &slot);
    if let Some(p) = &profile {
        sink.step(
            "mark",
            StepStatus::Info,
            &format!("账号展示名：{}", p.display_name()),
        );
    }

    let exe = locate::find_exe()?;
    proc::start(&exe, sink)?;
    Ok(SaveOutcome {
        slot,
        copied,
        profile,
        device,
    })
}

/// 保存登录态的结果
///
/// 为什么把槽位回传：`verify_slot_name` 可能纠正槽名（uid 来源滞后时就会发生），调用方若
/// 仍用自己传入的那个槽名去写账号记录，就会把账记到错误的槽上——这正是最初事故的形状。
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    /// 实际落盘的槽位（可能与入参不同）
    pub slot: String,
    /// 成功拷贝的条目数
    pub copied: usize,
    /// 账号展示信息（用户名/脱敏手机/头像），解不出时为 None
    pub profile: Option<uid::AccountProfile>,
    /// 设备标识归一结果（撞号时已给该账号换上一套独立标识，见 `device` 模块）
    pub device: device::NormalizeOutcome,
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

/// 槽位的账号展示信息（供调用方回填账号名）
///
/// 为什么修复时要一并解出：存量账号记录的 `name` 是当初拿 uid 顶上的（那时还没有解析
/// 展示字段的能力）。不回填的话，用户必须把每个账号重新登录保存一遍才能看到用户名——
/// 而数据（用户名）本来就在快照里躺着。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SlotIdentity {
    pub slot: String,
    pub username: Option<String>,
    pub mobile: Option<String>,
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
    /// 各槽位解出的账号展示信息（用于回填账号名）
    pub identities: Vec<SlotIdentity>,
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
        // 保留槽（`last` / `last.bak`）不参与归位：它的目录名按设计就不等于账号 id
        // （内容是「切换前现场」的滚动备份），用「名实是否相符」去判必然命中。而它的内容
        // 几乎总是某个已登记账号的快照，于是会稳定走 skipped 分支——每次修复都刷两条假告警，
        // 把真正的槽位冲突淹掉（2026-09-20 实测）。它照旧参与下面的白名单瘦身。
        if snapshot::is_reserved_slot(&base) {
            continue;
        }
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
    let mut identities: Vec<SlotIdentity> = Vec::new();
    for slot in snapshot::list_slots(ctx) {
        freed_bytes += snapshot::prune_slot(ctx, &slot, sink).unwrap_or(0);
        // 顺手解出展示信息：调用方要用它把账号库里「名字=uid」的记录回填成真实用户名，
        // 让存量账号无需重新登录就能显示可读名称
        if let Some(p) = uid::slot_profile(ctx, &slot) {
            identities.push(SlotIdentity {
                slot: slot.clone(),
                username: p.username,
                mobile: p.mobile,
            });
        }
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
        // 用账号槽清单而不是全目录：调用方要拿它登记账号，混进保留槽会造出假账号
        slots: snapshot::list_account_slots(ctx),
        freed_bytes,
        identities,
    })
}

/// 把设备标识同步结果落到日志
///
/// 为什么单独抽：正常路径与回滚路径都要记这条账，关心的信息也相同。用户可见的反馈由
/// `apply_registry` 内部的步骤承担，这里补的是**持久化对账依据**——app.log 里能查到
/// 「这次切换到底把注册表改成了什么」，而不必去翻瞬时的界面提示。
fn log_registry_outcome(outcome: &device::RegistryOutcome) {
    match outcome {
        device::RegistryOutcome::Applied { value, changed } => log::info!(
            "[traework] 注册表设备标识同步完成（{}，{}）",
            device::short(value),
            if *changed { "已变更" } else { "原本一致" }
        ),
        device::RegistryOutcome::Skipped(reason) => {
            log::warn!("[traework] 注册表设备标识同步跳过: {reason}")
        }
        device::RegistryOutcome::Failed(reason) => {
            log::warn!("[traework] 注册表设备标识同步失败: {reason}")
        }
    }
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

    // 保留槽不是账号槽：它的内容是「切换前现场」的滚动备份。不显式拦下的话，会掉进下面的
    // 「名实相符」校验，报出「请先点修复槽位」——而修复本来就不该归位它，用户照做也修不好
    // （2026-09-20 实测：账号库里曾被误登记出一条 last，点切换即陷入这个死循环）。
    if snapshot::is_reserved_slot(slot) {
        return Err(format!(
            "{slot} 是「切换前现场」的保留备份槽，不是账号槽位，无法切换；请切换到具体账号"
        ));
    }

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
            // 回滚必须把设备标识一起退回去：现场已经回到「切换前」，注册表若停在中途那个
            // 失败目标值上，本地两层标识就互相矛盾——比不写更糟（等于凭空换了设备身份）
            let rolled_back =
                device::apply_registry(ctx, LAST_SLOT, sink, &device::SystemRegistry);
            log_registry_outcome(&rolled_back);
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
    // 注册表设备标识必须在**启动客户端之前**同步：客户端启动时就会读走该值，
    // 晚一步写等于这次切换仍然带着上一个账号的设备身份跑起来
    let registry_outcome = device::apply_registry(ctx, slot, sink, &device::SystemRegistry);
    log_registry_outcome(&registry_outcome);
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

#[cfg(test)]
mod tests {
    use super::test_support::{fake_ctx, QuietSink};
    use super::*;

    /// 在槽目录里造一份能解出 `uid` 的 storage.json（tc 密文格式，与真实客户端一致）
    fn write_slot_storage(ctx: &Ctx, dir_name: &str, uid: &str) {
        let plain = format!(r#"{{"userId":"{uid}","account":{{"username":"u{uid}"}}}}"#);
        let enc = crate::tc_crypto::encrypt_storage_value(&plain).unwrap();
        let dir = ctx
            .profiles_dir
            .join(dir_name)
            .join("User")
            .join("globalStorage");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("storage.json"),
            format!(r#"{{"iCubeAuthInfo://icube.cloudide":"{enc}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn 修复只归位真错位槽且不把保留槽当账号() {
        let ctx = fake_ctx("reconcile-reserved");
        let sink = QuietSink;
        write_slot_storage(&ctx, "168695880747001", "168695880747001");
        // 保留槽 last：内容必然是「切换前的那个账号」，而该账号自己的主槽也存在——
        // 这是每次切换后的恒定状态，绝不能被判成「错位」或登记成账号
        write_slot_storage(&ctx, LAST_SLOT, "3031811986829834");
        write_slot_storage(&ctx, "3031811986829834", "3031811986829834");
        // 真错位：目录名与内容都不对（上一代备份里压的其实是另一个账号）
        write_slot_storage(&ctx, "1111111111111111.bak", "2222222222222222");

        let report = reconcile(&ctx, &sink).unwrap();

        assert!(ctx.profiles_dir.join(LAST_SLOT).exists(), "保留槽不得被改名");
        assert!(
            !report.slots.iter().any(|s| s == LAST_SLOT),
            "保留槽不得出现在账号槽清单里（调用方会拿它登记账号）"
        );
        assert_eq!(
            report.renamed,
            vec![(
                "1111111111111111.bak".to_string(),
                "2222222222222222".to_string()
            )]
        );
        assert!(ctx.profiles_dir.join("2222222222222222").exists());
        assert!(
            report.skipped.is_empty(),
            "保留槽造成的假告警必须消失：{:?}",
            report.skipped
        );
        // 切到保留槽要给出明确原因，不能掉进「请先点修复槽位」的死循环。
        // 与上面的断言同处一个用例：两者都经全局串行闸门，分开写会互相抢锁而随机失败
        let err = switch_to(&ctx, LAST_SLOT, &QuietSink).unwrap_err();
        assert!(err.contains("保留备份槽"), "{err}");
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }
}
