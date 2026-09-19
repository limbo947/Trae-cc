//! TraeWork（TRAE SOLO CN）应用档案：数据目录、进程名、可执行文件候选与快照白名单。
//!
//! 为什么不复用 `machine.rs` 里的 traecode 常量：两者的数据目录、进程名、登录态载体
//! 完全不同，且切换语义相反（traecode 是「清目录 + 写登录态」，TraeWork 是「整组快照
//! 覆盖」）。混在一处迟早串台——最坏情况是用 TraeWork 的路径去清 traecode 的目录。
//!
//! **白名单集中在本文件是刻意的**：上游版本升级导致登录态布局变化时，只需核对这一个
//! 地方（见 doc/TraeWork账号切换计划.md §5 风险 5）。

use std::path::PathBuf;

/// 主进程映像名白名单（**不含 .exe**，匹配前统一剥后缀，对齐 `tasklist` 的输出差异）
///
/// 为什么包含 "TRAE SOLO"：本机实测为 `TRAE SOLO CN.exe`，但同名系列存在不带 CN 的
/// 发行版；漏掉会让「切换前关不掉客户端」进而把运行中的现场当成快照备份下来。
pub const PROC_NAMES: &[&str] = &["TRAE SOLO CN", "TRAE SOLO"];

/// 可执行文件名白名单（自定义安装路径校验用；防把别的应用 exe 写进配置）
pub const EXE_NAMES: &[&str] = &["TRAE SOLO CN.exe", "TRAE SOLO.exe"];

/// 优雅关闭等待秒数
///
/// 为什么是 8 秒而不是更短的 3 秒：参考实现实测 3 秒恒超时 → 每次切换都走强杀 →
/// `state.vscdb-wal` 残留未 checkpoint 的登录写入 → 恢复后被客户端回放导致旧账号复活。
/// 这个值直接决定「切换是否真的生效」，不要下调。
pub const GRACEFUL_WAIT_SECS: u64 = 8;

/// 强杀后等待完全退出的秒数（等待文件句柄释放，避免紧接着拷贝拿到半截文件）
pub const KILL_WAIT_SECS: u64 = 5;

/// TraeWork 数据目录：`%APPDATA%\TRAE SOLO CN`
///
/// 刻意不接受自定义：这是客户端自己决定的路径（非安装路径），改不了也不该改。
pub fn data_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| "无法获取 APPDATA 环境变量，无法定位 TraeWork 数据目录".to_string())?;
    Ok(PathBuf::from(appdata).join("TRAE SOLO CN"))
}

/// 快照根目录：与应用账号库共用同一套 `ProjectDirs`，避免出现第三个数据根
///（`%APPDATA%\hhj\trae-cc\data\profiles_traework`）
pub fn profiles_dir() -> Result<PathBuf, String> {
    let proj_dirs = directories::ProjectDirs::from("com", "hhj", "trae-cc")
        .ok_or_else(|| "无法获取应用数据目录".to_string())?;
    let dir = proj_dirs.data_dir().join("profiles_traework");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建快照目录失败: {e}"))?;
    Ok(dir)
}

/// 已保存的 TraeWork 可执行文件路径（配置目录，与 traecode 的 `trae_path.txt` 同级）
pub fn saved_exe_path_file() -> Result<PathBuf, String> {
    let proj_dirs = directories::ProjectDirs::from("com", "hhj", "trae-cc")
        .ok_or_else(|| "无法获取应用配置目录".to_string())?;
    let dir = proj_dirs.config_dir();
    std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    Ok(dir.join("traework_path.txt"))
}

/// 路径上下文：把「TraeWork 数据目录 + 快照根目录」显式化为参数
///
/// 为什么不让每个函数各自去读环境变量：备份/恢复是**破坏性**操作（整组文件被覆盖或
/// 删除），必须能在测试里指向临时目录验证；否则这段代码要么碰真实 `%APPDATA%`（一旦
/// 出错就是用户登录态全丢），要么根本没法测。显式传参是这两条中唯一可接受的选项。
#[derive(Debug, Clone)]
pub struct Ctx {
    /// TraeWork 实际数据目录
    pub data_dir: PathBuf,
    /// 快照槽根目录
    pub profiles_dir: PathBuf,
}

impl Ctx {
    /// 从真实环境构造（生产路径）
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            data_dir: data_dir()?,
            profiles_dir: profiles_dir()?,
        })
    }

    /// 构造测试用上下文（只指向临时目录，绝不触碰真实应用数据）
    #[cfg(test)]
    pub fn for_test(data_dir: PathBuf, profiles_dir: PathBuf) -> Self {
        Self {
            data_dir,
            profiles_dir,
        }
    }

    /// 主槽目录
    pub fn slot_dir(&self, slot: &str) -> Result<PathBuf, String> {
        ensure_slot_safe(slot)?;
        Ok(self.profiles_dir.join(slot))
    }

    /// 回退槽（`.bak`）目录
    ///
    /// 为什么显式拼 `.bak` 而不用 `with_extension`：槽位名理论上可含点号，
    /// `with_extension` 会把点号后的部分当扩展名替换掉，得到错误的目录名。
    pub fn bak_dir(&self, slot: &str) -> Result<PathBuf, String> {
        ensure_slot_safe(slot)?;
        Ok(self.profiles_dir.join(format!("{slot}.bak")))
    }

    /// 按目录名直达（允许 `.bak` 后缀）
    ///
    /// 为什么单独开一个入口：`.bak` 里的点号不在槽位白名单字符集内，直接走 `slot_dir` 会被
    /// 拦下——但修复流程必须能读回退代的内容（被错误覆盖时，真正的上一代账号只存在于
    /// `.bak`）。这里对**基础名**做校验，后缀由本函数显式剥离，防穿越强度不降。
    pub fn dir_by_name(&self, dir_name: &str) -> Result<PathBuf, String> {
        let base = dir_name.strip_suffix(".bak").unwrap_or(dir_name);
        ensure_slot_safe(base)?;
        Ok(self.profiles_dir.join(dir_name))
    }

    /// 当前账号标记文件
    pub fn current_account_file(&self) -> PathBuf {
        self.profiles_dir.join("current_account.txt")
    }
}

/// 槽位名安全校验
///
/// 为什么必须校验：槽位名来自 `storage.json` 推导出的 uid，而槽位名会被拼进文件路径。
/// 一旦上游写入异常（或被构造的 storage.json）出现 `..` 或路径分隔符，恢复操作就会
/// 越出快照根目录去覆盖任意位置——这是本模块唯一能从「换个账号」升级成「损坏系统」的
/// 入口，所以按白名单字符集收口。
pub fn ensure_slot_safe(slot: &str) -> Result<(), String> {
    let s = slot.trim();
    if s.is_empty() {
        return Err("槽位名为空".to_string());
    }
    if s.len() > 64 {
        return Err(format!("槽位名过长（最多 64 字符）: {s}"));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("槽位名含非法字符（仅允许 A-Z a-z 0-9 _ -）: {s}"));
    }
    Ok(())
}

/// 快照白名单（相对数据目录）
///
/// 来源：参考实现 `switcher/icube.rs` 的 14 项，已与本机 TraeWork 实测清单（计划文档
/// §1.2）逐项对照过。语义分三类：
///
/// - **登录真源**：`storage.json`（设备标识/遥测/认证）+ `state.vscdb` 及其 `.backup`
///   + `-wal`/`-shm` 边车。边车必须随主库一起走——强杀是常态，最新登录写入可能尚未
///   checkpoint 进主库，漏拷即丢数据。
/// - **设备与风控身份**：`machineid`、`aha\`。随快照走才能做到「一账号一设备」。
/// - **Chromium 侧会话**：`Network\`（Cookie）、`Local Storage\`、`Session Storage\`、
///   `Partitions\*` 的**会话子路径**（见下方注释：已按实测收窄，不再整目录拷贝）。
///
/// 刻意不纳入 `ModularData\`：那是绑定账号与路径的业务数据（对话），跨账号替换会串数据。
pub const SNAPSHOT_ITEMS: &[&str] = &[
    r"User\globalStorage\storage.json",
    r"User\globalStorage\state.vscdb",
    r"User\globalStorage\state.vscdb-wal",
    r"User\globalStorage\state.vscdb-shm",
    r"User\globalStorage\state.vscdb.backup",
    r"machineid",
    r"aha",
    r"Preferences",
    r"Local State",
    r"Local Storage\leveldb",
    r"Local Storage\config.db",
    r"Network",
    // Partitions\* 只取承载会话的子路径，**不整目录**。
    // 2026-09-19 实测：整目录时这两项合计 487MB，其中 483MB 是可再生缓存
    // （trae-webview：Cache 329.6 + Code Cache 52.3；icube-web-crawler：Cache 68.8 +
    // Code Cache 25.6 + DawnWebGPUCache 5.6 + GPUCache 1.6），而真正与登录有关的
    // Network / Local Storage / IndexedDB 合计不到 1MB。再加上 `.bak` 单代回退，
    // 整目录方案会让**每个账号**占约 1GB，收窄后约 12MB。
    // 缓存丢失只影响首次加载速度，不影响登录态——这是「有证据才动白名单」的取舍。
    r"Partitions\trae-webview\Network",
    r"Partitions\trae-webview\Local Storage",
    r"Partitions\trae-webview\IndexedDB",
    r"Partitions\trae-webview\Session Storage",
    r"Partitions\icube-web-crawler-shared-session-v1.0\Network",
    r"Partitions\icube-web-crawler-shared-session-v1.0\Local Storage",
    r"Partitions\icube-web-crawler-shared-session-v1.0\IndexedDB",
    r"Partitions\icube-web-crawler-shared-session-v1.0\Session Storage",
    r"Session Storage",
];

/// 恢复后校验的必需项（缺任一即视为「切了个寂寞」，必须回滚）
///
/// 为什么单独列而不是复用白名单前两项：这两个是登录态的最小充分集，缺了必然要重新
/// 登录；其余项缺失只是体验降级，不该触发回滚。
pub const REQUIRED_ITEMS: &[&str] = &[
    r"User\globalStorage\storage.json",
    r"User\globalStorage\state.vscdb",
];

/// 检查 `.exe` 是否属于 TraeWork 白名单（防把别的应用写进配置）
pub fn exe_matches(path: &std::path::Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => EXE_NAMES.iter().any(|e| e.eq_ignore_ascii_case(name)),
        None => false,
    }
}

/// 剥离映像名尾部的 `.exe`（大小写不敏感）
///
/// 为什么需要：`tasklist` 在不同 Windows 语言/版本下可能给出带或不带后缀的形态，
/// 直接与不带后缀的白名单比较会恒不命中 → 进程枚举恒空 → **从不关闭客户端**却在运行中
/// 覆盖快照（参考实现踩过这个坑，症状是「切换不生效」）。
pub fn strip_exe_suffix(name: &str) -> &str {
    if name.len() > 4 {
        if let Some(base) = name.get(..name.len() - 4) {
            if name[base.len()..].eq_ignore_ascii_case(".exe") {
                return base;
            }
        }
    }
    name
}

/// 进程名是否命中白名单（大小写不敏感，先剥 `.exe`）
pub fn proc_name_matches(name: &str) -> bool {
    let base = strip_exe_suffix(name.trim());
    PROC_NAMES.iter().any(|n| base.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 槽位名拒绝路径穿越与非法字符() {
        assert!(ensure_slot_safe("168695880747001").is_ok());
        assert!(ensure_slot_safe("uid_with-dash").is_ok());
        assert!(ensure_slot_safe("../evil").is_err());
        assert!(ensure_slot_safe(r"..\evil").is_err());
        assert!(ensure_slot_safe(r"a\b").is_err());
        assert!(ensure_slot_safe("a/b").is_err());
        assert!(ensure_slot_safe("").is_err());
        assert!(ensure_slot_safe(&"x".repeat(65)).is_err());
        // 中文/空格不在白名单字符集内
        assert!(ensure_slot_safe("槽位 A").is_err());
    }

    #[test]
    fn 进程名匹配剥exe后缀且大小写不敏感() {
        assert!(proc_name_matches("TRAE SOLO CN.exe"));
        assert!(proc_name_matches("trae solo cn.EXE"));
        assert!(proc_name_matches("TRAE SOLO CN"));
        assert!(proc_name_matches("TRAE SOLO.exe"));
        // traecode 的进程名不得命中（防串台：关错应用会把 traecode 一起杀掉）
        assert!(!proc_name_matches("Trae CN.exe"));
        assert!(!proc_name_matches("Trae.exe"));
        assert!(!proc_name_matches("ai-work-assistant.exe"));
    }

    #[test]
    fn exe白名单拒绝其他应用() {
        assert!(exe_matches(std::path::Path::new(r"D:\TRAE SOLO CN\TRAE SOLO CN.exe")));
        assert!(exe_matches(std::path::Path::new(r"D:\x\trae solo cn.EXE")));
        assert!(!exe_matches(std::path::Path::new(r"D:\x\Trae CN.exe")));
        assert!(!exe_matches(std::path::Path::new(r"D:\x\Doubao.exe")));
    }

    #[test]
    fn 白名单与必需项自洽() {
        // 必需项必须真的在白名单里，否则「恢复后校验」会要求一个从不备份的文件
        for req in REQUIRED_ITEMS {
            assert!(SNAPSHOT_ITEMS.contains(req), "必需项 {req} 不在白名单内");
        }
        assert!(SNAPSHOT_ITEMS.len() >= 14);
    }
}
