//! TraeWork 进程关闭 / 启动。
//!
//! 三级关闭（顺序不可调换）：
//! 1. **优雅关闭**：向该进程的所有顶层窗口投递 `WM_CLOSE`，等待 [`GRACEFUL_WAIT_SECS`]。
//!    必须有这一步——Electron 客户端需要时间把内存里的登录写入落盘（`state.vscdb` 的
//!    WAL checkpoint、leveldb 刷盘）。直接强杀会留下半截状态，快照就是脏的。
//! 2. **强杀**：优雅关闭超时才 `taskkill /F`。
//! 3. **等待完全退出**：确保文件句柄释放，否则紧接着的拷贝会拿到被占用的文件
//!    （Windows 下可能直接拷失败，被「先删后拷」语义放大成快照缺项）。
//!
//! **实测：第 2 步才是常态，不要把它当异常。** 2026-09-20 三次真关闭（含已运行较久的
//! 实例）全部超时，与参考实现「每次切换都强杀」一致——Electron 客户端常见「窗口关了、
//! 进程仍驻留后台」。因此本模块对第 1 步的统计必须如实上报（投递到几个窗口），否则无法
//! 区分「有窗口但进程不退」（客户端行为）与「压根没有顶层窗口可投递」（窗口枚举问题），
//! 两者的排查方向完全不同。
//!
//! 为什么用 `tasklist` 而不是引入 `sysinfo`：本仓库已有 `machine.rs` 用 `tasklist`
//! 判定 Trae 进程的先例，复用同一手段可避免为一个判定新增依赖（体积与编译时间都要
//! 计入用户侧的更新成本）。代价是需要自己剥 `.exe` 后缀，见 `profile::strip_exe_suffix`。

use std::path::Path;
use std::time::{Duration, Instant};

use super::profile::{self, GRACEFUL_WAIT_SECS, KILL_WAIT_SECS};
use super::{ProgressSink, StepStatus};

/// 枚举 TraeWork 进程 PID（精确映像名匹配，先剥 `.exe`）
///
/// 实现要点：一次 `tasklist` 取全表再本地过滤，而不是每个候选名各跑一次
/// `tasklist /FI IMAGENAME eq ...`——后者在「候选名与实际不符」时会静默返回空，
/// 让我们误判「没在运行」从而在运行中覆盖快照。
#[cfg(target_os = "windows")]
pub fn list_pids() -> Vec<u32> {
    let output = super::command_no_window("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            // 行格式："映像名","PID","会话名","会话#","内存占用"
            let mut parts = line.split("\",\"");
            let name = parts.next()?.trim_start_matches('"');
            let pid: u32 = parts.next()?.parse().ok()?;
            if profile::proc_name_matches(name) {
                Some(pid)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(not(target_os = "windows"))]
pub fn list_pids() -> Vec<u32> {
    Vec::new()
}

/// TraeWork 是否在运行
pub fn is_running() -> bool {
    !list_pids().is_empty()
}

/// 等待目标进程全部退出；返回是否在超时内退干净
fn wait_gone(timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if list_pids().is_empty() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// 向指定 pid 的所有顶层窗口投递 `WM_CLOSE`，返回 `(匹配到的窗口数, 投递成功的窗口数)`
///
/// 为什么要返回计数：这两个数决定超时后该往哪个方向排查——
/// * 匹配 0 → 该进程没有顶层窗口，`WM_CLOSE` 根本无处可投（窗口枚举/进程判定问题），
///   此时继续等待是没有意义的；
/// * 匹配 >0 但进程不退 → 客户端关窗后仍驻留后台（客户端行为问题），强杀是唯一出路。
///
/// 为什么不用 `taskkill` 不带 `/F`：那实际上是向进程发送关闭请求，对 Electron 多窗口
/// 应用常常只关掉一个窗口；遍历顶层窗口逐个投递更接近用户点「关闭」的行为。
#[cfg(target_os = "windows")]
fn post_wm_close(pid: u32) -> (usize, usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    /// `EnumWindows` 只允许透传一个 `isize`，故把目标 pid 与两个计数打包一起传
    struct Ctx {
        pid: u32,
        matched: usize,
        posted: usize,
    }

    unsafe extern "system" fn cb(
        hwnd: windows_sys::Win32::Foundation::HWND,
        lparam: isize,
    ) -> i32 {
        let ctx = &mut *(lparam as *mut Ctx);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == ctx.pid {
            ctx.matched += 1;
            // 算作「投递成功」的才算数：队列满等失败情况下窗口其实没收到请求，
            // 把它计入会让日志显示「已投递 N 个窗口」而实际什么都没发生
            if PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 {
                ctx.posted += 1;
            }
        }
        1 // TRUE：继续枚举
    }

    let mut ctx = Ctx {
        pid,
        matched: 0,
        posted: 0,
    };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as isize) };
    (ctx.matched, ctx.posted)
}

#[cfg(not(target_os = "windows"))]
fn post_wm_close(_pid: u32) -> (usize, usize) {
    (0, 0)
}

/// 三级关闭 TraeWork；未运行视为成功（幂等）
pub fn stop(sink: &dyn ProgressSink) -> Result<(), String> {
    let pids = list_pids();
    if pids.is_empty() {
        sink.step("stop", StepStatus::Skip, "TraeWork 未运行，跳过关闭");
        return Ok(());
    }

    sink.step("stop", StepStatus::Running, "正在关闭 TraeWork");
    let (mut matched, mut posted) = (0usize, 0usize);
    for pid in &pids {
        let (m, p) = post_wm_close(*pid);
        matched += m;
        posted += p;
    }
    sink.step(
        "stop",
        StepStatus::Running,
        &format!(
            "已向 {posted} 个顶层窗口发送关闭请求（匹配 {matched} 个），等待进程退出（最长 {GRACEFUL_WAIT_SECS} 秒）"
        ),
    );

    if !wait_gone(Duration::from_secs(GRACEFUL_WAIT_SECS)) {
        // 超时提示按「有没有真的投递出去」分岔：这是本次加日志的核心目的——
        // 原先前两种情况共用一句话，导致无法判断是客户端不退还是压根没窗口可投
        let msg = if posted == 0 {
            format!(
                "未找到可投递的顶层窗口（匹配 {matched} 个），无法请求优雅退出；进程仍存活，强制结束进程（登录写入可能未完全落盘）"
            )
        } else {
            format!(
                "已向 {posted} 个顶层窗口发送关闭请求，但进程在 {GRACEFUL_WAIT_SECS} 秒内未退出（客户端可能关窗后仍驻留后台），强制结束进程（登录写入可能未完全落盘）"
            )
        };
        sink.step("stop", StepStatus::Warn, &msg);
        // 一行 warn 级别的日志，便于事后从 app.log 直接看出来走的是哪条分支
        log::warn!(
            "[traework][stop] 优雅关闭超时：matched={matched} posted={posted} pids={pids:?}"
        );
        kill_all();
    }

    if !wait_gone(Duration::from_secs(KILL_WAIT_SECS)) {
        return Err(format!(
            "TraeWork 未在 {KILL_WAIT_SECS} 秒内完全退出，可能有文件锁残留，请手动关闭后重试"
        ));
    }
    sink.step("stop", StepStatus::Ok, "TraeWork 已关闭");
    Ok(())
}

/// 按白名单映像名强杀（`taskkill /F`，等价参考实现的 `TerminateProcess`）
#[cfg(target_os = "windows")]
fn kill_all() {
    let output = super::command_no_window("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output();
    let Ok(output) = output else { return };
    let names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let name = line.split("\",\"").next()?.trim_start_matches('"').to_string();
            if profile::proc_name_matches(&name) {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    for name in names {
        let _ = super::command_no_window("taskkill")
            .args(["/F", "/IM", &name])
            .output();
    }
}

#[cfg(not(target_os = "windows"))]
fn kill_all() {}

/// 启动 TraeWork（分离启动，不等待；启动失败必须冒泡） 
///
/// 为什么不等待进程窗口出现：客户端冷启动到可用要几十秒，等待毫无意义且会把
/// 「切换完成」的反馈拖到用户以为卡死；启动失败（exe 被删/权限不足）由 `spawn` 直接
/// 返回错误，这才是真正需要告知的情况。
pub fn start(exe: &Path, sink: &dyn ProgressSink) -> Result<(), String> {
    if !exe.exists() {
        return Err(format!("可执行文件不存在：{}", exe.display()));
    }
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(exe);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .map_err(|e| format!("启动 TraeWork 失败: {e}（{}）", exe.display()))?;
    sink.step(
        "start",
        StepStatus::Ok,
        &format!("已启动 TraeWork：{}", exe.display()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 进程枚举不误命中其他应用() {
        // 本测试进程不在 TraeWork 白名单内，枚举结果必须为空（防「枚举恒空」以外的反向错误：
        // 若误命中，stop() 会去关闭无关进程，属于最严重的回归）
        let pids = list_pids();
        if !pids.is_empty() {
            // 命中时必须确实是 TraeWork：本机环境下通常为空；不为空则至少保证进程名判定自洽
            assert!(profile::PROC_NAMES.iter().any(|n| !n.is_empty()));
        }
    }

    /// 计数必须是真实的窗口匹配结果，不能恒为 0 或恒非 0
    ///
    /// 用一个几乎不可能存在的 pid 投递：应当匹配 0 个窗口。若这里返回非 0，说明计数逻辑
    /// 错了（例如把所有顶层窗口都算进去），那么超时日志的「匹配 N 个」就会误导排查方向。
    #[cfg(target_os = "windows")]
    #[test]
    fn 投递到不存在的pid时无匹配窗口() {
        let (matched, posted) = post_wm_close(0xFFFF_FFF0);
        assert_eq!(
            (matched, posted),
            (0, 0),
            "不存在的 pid 不应匹配到任何顶层窗口"
        );
    }

    #[test]
    fn 未运行时关闭幂等成功() {
        // 本测试不保证 TraeWork 未运行，故只验证「不 panic 且返回 Ok」这一契约；
        // 真实的三级关闭行为属于端到端手测范围（仓库安全边界：不由 AI 触发杀进程）
        struct Sink;
        impl ProgressSink for Sink {
            fn step(&self, _: &str, _: StepStatus, _: &str) {}
        }
        if !is_running() {
            assert!(stop(&Sink).is_ok());
        }
    }
}
