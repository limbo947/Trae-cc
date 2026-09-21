//! 窗口大小/位置持久化（自研实现，不用 `tauri-plugin-window-state`）。
//!
//! 为什么自研：
//! - 该插件的落盘挂在 `RunEvent::Exit`，而本应用主窗口关闭时会 `std::process::exit(0)`
//!   直接终止进程，`Exit` 事件永远不会触发——插件的自动保存等于不存在；
//! - 手动调它的 `save_window_state()` 试过一版：只返回一个被吞掉的 `Result`，
//!   磁盘上始终没有状态文件，也没有任何观测点能说明失败在哪一步。把功能建立在
//!   「查不出原因的第三方路径」上不可接受。
//!
//! 本模块只有两件事：**读窗口 → 写 JSON**、**读 JSON → 恢复窗口**，每一步的错误
//! 都如实返回给调用方（调用方落日志），落点与 `settings.json` 同目录，
//! 与仓库既有约定一致。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{PhysicalPosition, PhysicalSize, Runtime, Window};

/// 窗口几何（物理像素）
///
/// 为什么用物理像素而不是逻辑像素：`outer_position` / `inner_size` 本就是物理量，
/// 直接存取少一层换算；代价是用户在两次启动之间改了系统缩放比例时，窗口会按物理
/// 尺寸恢复（视觉上略大/略小），这是刻意的取舍——换成逻辑像素需要引入缩放因子
/// 判定与除法，收益不抵复杂度。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// 上次关闭时是否处于最大化
    #[serde(default)]
    pub maximized: bool,
}

/// 状态文件路径（与应用 settings.json 同目录）
fn state_path() -> Result<PathBuf, String> {
    let proj_dirs = directories::ProjectDirs::from("com", "hhj", "trae-cc")
        .ok_or_else(|| "无法获取应用配置目录".to_string())?;
    let dir = proj_dirs.config_dir();
    std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    Ok(dir.join("window_state.json"))
}

/// 读状态文件；缺失/非法一律返回 None（首启或用户手工改坏都应按「无记录」处理）
fn read_at(path: &Path) -> Option<WindowGeometry> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()
}

/// 保存当前窗口几何
///
/// **最大化时保留上一次的非最大化几何**：最大化状态下 `outer_position` / `inner_size`
/// 报的是屏幕尺寸，原样存下来会让用户下次取消最大化后得到一个满屏大小的窗口，
/// 而不是他最大化之前那个尺寸。所以最大化只翻转标志位。
pub fn save<R: Runtime>(window: &Window<R>) -> Result<(), String> {
    let path = state_path()?;
    let previous = read_at(&path);
    let maximized = window.is_maximized().unwrap_or(false);

    let geometry = if maximized {
        match previous {
            Some(g) => WindowGeometry {
                maximized: true,
                ..g
            },
            // 首次运行就最大化：拿不到「最大化之前」的尺寸，存全 0 表示「只恢复最大化，
            // 不动尺寸/位置」（restore 对 0 尺寸与 0,0 位置都有明确的跳过分支）
            None => WindowGeometry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                maximized: true,
            },
        }
    } else {
        let position = window
            .outer_position()
            .map_err(|e| format!("读取窗口位置失败: {e}"))?;
        let size = window
            .inner_size()
            .map_err(|e| format!("读取窗口尺寸失败: {e}"))?;
        WindowGeometry {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            maximized: false,
        }
    };

    let json = serde_json::to_string_pretty(&geometry)
        .map_err(|e| format!("序列化窗口状态失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写入窗口状态失败: {e}"))
}

/// 恢复上次的窗口几何；无记录（首次启动）时静默跳过
pub fn restore<R: Runtime>(window: &Window<R>) -> Result<(), String> {
    let path = state_path()?;
    let Some(geometry) = read_at(&path) else {
        return Ok(());
    };

    if geometry.width > 0 && geometry.height > 0 {
        window
            .set_size(PhysicalSize::new(geometry.width, geometry.height))
            .map_err(|e| format!("恢复窗口尺寸失败: {e}"))?;
    }

    // 位置只在「落在某个显示器上」时才恢复：外接屏拔掉后，上次保存的位置可能
    // 指向一块已不存在的屏幕，硬恢复会让窗口出现在用户看不见的地方
    let monitors = monitor_rects(window);
    if (geometry.x != 0 || geometry.y != 0)
        && position_visible(geometry.x, geometry.y, &monitors)
    {
        window
            .set_position(PhysicalPosition::new(geometry.x, geometry.y))
            .map_err(|e| format!("恢复窗口位置失败: {e}"))?;
    }

    if geometry.maximized {
        window
            .maximize()
            .map_err(|e| format!("恢复最大化状态失败: {e}"))?;
    }
    Ok(())
}

/// 显示器矩形列表（x, y, width, height）；拿不到时返回空表
fn monitor_rects<R: Runtime>(window: &Window<R>) -> Vec<(i32, i32, u32, u32)> {
    window
        .available_monitors()
        .map(|monitors| {
            monitors
                .iter()
                .map(|m| {
                    let p = m.position();
                    let s = m.size();
                    (p.x, p.y, s.width, s.height)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `(x, y)` 是否落在任一显示器内
///
/// 判据用**窗口左上角**而不是整窗相交：左上角决定标题栏在哪儿——标题栏在屏外时
/// 用户根本抓不到窗口，等于窗口丢了。
/// `monitors` 为空（拿不到显示器信息）时返回 true：宁可按记录恢复，也不要因为
/// 一次枚举失败把用户的位置丢掉。
pub fn position_visible(x: i32, y: i32, monitors: &[(i32, i32, u32, u32)]) -> bool {
    if monitors.is_empty() {
        return true;
    }
    monitors.iter().any(|&(mx, my, mw, mh)| {
        x >= mx && x < mx + mw as i32 && y >= my && y < my + mh as i32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 位置落在显示器内才算可见（左上角判定）
    #[test]
    fn 位置可见性按显示器矩形判定() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 1920, 1080)];
        assert!(position_visible(0, 0, &monitors));
        assert!(position_visible(1919, 1079, &monitors));
        // 第二块屏（横向拼接）
        assert!(position_visible(2000, 100, &monitors));
        // 右边界外 / 左边界外 / 下方
        assert!(!position_visible(3840, 100, &monitors));
        assert!(!position_visible(-10, 100, &monitors));
        assert!(!position_visible(100, 1080, &monitors));
    }

    /// 拿不到显示器信息时不拦位置恢复（宁可按记录恢复，也不要因一次枚举失败丢位置）
    #[test]
    fn 无显示器信息时放行() {
        assert!(position_visible(-9999, -9999, &[]));
    }

    /// 状态文件往返：写出去的内容必须能原样读回（含最大化标志）
    #[test]
    fn 状态文件往返() {
        let dir = std::env::temp_dir().join(format!(
            "trae-cc-window-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("window_state.json");

        let original = WindowGeometry {
            x: 120,
            y: 80,
            width: 1000,
            height: 700,
            maximized: true,
        };
        std::fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        assert_eq!(read_at(&path), Some(original));

        // 缺失 / 非法 JSON / 带 BOM 的旧文件都按「无记录」处理，不得 panic
        let missing = dir.join("nope.json");
        assert_eq!(read_at(&missing), None);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_at(&path), None);
        std::fs::write(&path, format!("\u{feff}{}", serde_json::to_string(&original).unwrap()))
            .unwrap();
        assert_eq!(read_at(&path), Some(original));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
