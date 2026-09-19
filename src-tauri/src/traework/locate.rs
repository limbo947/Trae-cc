//! TraeWork 可执行文件定位。
//!
//! 为什么需要多级（而不是像 traecode 那样只查配置 + 常见路径）：本机 TraeWork 是
//! **自定义安装**在 `D:\TRAE SOLO CN`，不在任何默认候选目录里。只靠默认路径会让
//! 「切换成功但启动不了客户端」，用户看到的是「切换没反应」。
//!
//! 级次顺序（高 → 低）：
//! 1. 用户显式配置（`traework_path.txt`）——最高优先级，用户说在哪就是哪；
//! 2. 常见安装目录候选；
//! 3. 卸载注册表 `InstallLocation`；
//! 4. 运行中进程的 `Path`（PowerShell 兜底，最慢且依赖 PowerShell 可用）。
//!
//! **刻意不把「运行中进程」放在首位**：参考实现把进程回退放第一位导致残留/错误的同名
//! 进程被优先采用、启动了错的应用（计划文档 §2.6 坑 6）。任何级次的结果都必须先过
//! `profile::exe_matches` 白名单，防串台到 traecode。

use std::path::{Path, PathBuf};

use super::profile;

/// 定位 TraeWork 可执行文件；失败返回面向用户的提示
pub fn find_exe() -> Result<PathBuf, String> {
    if let Some(p) = saved_path() {
        return Ok(p);
    }
    if let Some(p) = from_candidates() {
        return Ok(p);
    }
    if let Some(p) = from_registry() {
        return Ok(p);
    }
    if let Some(p) = from_running_process() {
        return Ok(p);
    }
    Err(
        "未找到 TraeWork 安装路径，请在设置中手动指定 TRAE SOLO CN.exe（本机自定义安装时常见）"
            .to_string(),
    )
}

/// 读取已保存的路径（不存在/已失效/不在白名单则视为未配置）
pub fn saved_path() -> Option<PathBuf> {
    let file = profile::saved_exe_path_file().ok()?;
    let raw = std::fs::read_to_string(file).ok()?;
    let trimmed = raw.trim().trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.exists() && profile::exe_matches(&path) {
        Some(path)
    } else {
        None
    }
}

/// 保存用户指定的路径（必须过 exe 白名单，防止把别的应用写进配置）
pub fn save_path(path: &str) -> Result<(), String> {
    let p = PathBuf::from(path.trim());
    if !p.exists() {
        return Err(format!("指定的路径不存在：{}", p.display()));
    }
    if !profile::exe_matches(&p) {
        return Err(format!(
            "请选择 TraeWork 的可执行文件（{}）",
            profile::EXE_NAMES.join(" 或 ")
        ));
    }
    std::fs::write(profile::saved_exe_path_file()?, p.to_string_lossy().to_string())
        .map_err(|e| format!("保存路径失败: {e}"))
}

/// 常见安装位置候选
fn from_candidates() -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let program_files = std::env::var("ProgramFiles").unwrap_or_default();
    let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();

    let mut candidates: Vec<PathBuf> = Vec::new();
    for exe in profile::EXE_NAMES {
        candidates.push(PathBuf::from(&local).join("Programs").join("TRAE SOLO CN").join(exe));
        candidates.push(PathBuf::from(&local).join("Programs").join("TRAE SOLO").join(exe));
        candidates.push(PathBuf::from(&local).join("TRAE SOLO CN").join(exe));
        candidates.push(PathBuf::from(&program_files).join("TRAE SOLO CN").join(exe));
        candidates.push(PathBuf::from(&program_files_x86).join("TRAE SOLO CN").join(exe));
        // 自定义盘符安装：无法穷举，只覆盖最常见的 D 盘两种布局
        candidates.push(PathBuf::from(r"D:\TRAE SOLO CN").join(exe));
        candidates.push(PathBuf::from(r"D:\Programs\TRAE SOLO CN").join(exe));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// 从卸载注册表项取 `InstallLocation`
///
/// 为什么按 `TRAE SOLO` 而非 `TraeWork` 搜：注册表里的 DisplayName 是发行名
///（本机为 TRAE SOLO CN），搜用户口语名必然落空。
#[cfg(target_os = "windows")]
fn from_registry() -> Option<PathBuf> {
    let roots = [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];
    for root in roots {
        let Ok(output) = super::command_no_window("reg")
            .args(["query", root, "/s", "/f", "TRAE SOLO", "/k"])
            .output()
        else {
            continue;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if !line.contains("InstallLocation") {
                continue;
            }
            let Some(value) = line.split_whitespace().last() else {
                continue;
            };
            for exe in profile::EXE_NAMES {
                let candidate = Path::new(value).join(exe);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn from_registry() -> Option<PathBuf> {
    None
}

/// 从运行中的进程取 `Path`（PowerShell 兜底）
#[cfg(target_os = "windows")]
fn from_running_process() -> Option<PathBuf> {
    let name = profile::PROC_NAMES.first()?;
    let script = format!(
        "(Get-Process -Name '{name}' -ErrorAction SilentlyContinue | Where-Object {{ $_.Path }} | Select-Object -First 1 -ExpandProperty Path)"
    );
    let output = super::command_no_window("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if profile::exe_matches(&path) {
        Some(path)
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
fn from_running_process() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 保存路径拒绝非白名单exe() {
        let ctx_path = profile::saved_exe_path_file().expect("配置目录可创建");
        let backup = std::fs::read_to_string(&ctx_path).ok();
        let _ = std::fs::remove_file(&ctx_path);

        // 不存在的路径
        assert!(save_path(r"D:\No\Such\TRAE SOLO CN.exe").is_err());
        // 存在的路径但不是白名单 exe（用本测试自身可执行文件当替身）
        let self_exe = std::env::current_exe().expect("当前进程路径");
        assert!(save_path(&self_exe.to_string_lossy()).is_err());
        // 未配置时 saved_path 为 None
        assert!(saved_path().is_none());

        // 还原现场，避免污染开发者本机配置
        match backup {
            Some(content) => {
                let _ = std::fs::write(&ctx_path, content);
            }
            None => {
                let _ = std::fs::remove_file(&ctx_path);
            }
        }
    }
}
