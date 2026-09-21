//! TraeWork 快照的备份 / 恢复原语。
//!
//! 本文件是整个 TraeWork 支持的地基：TraeWork 的登录真源是 `storage.json` +
//! `state.vscdb` **双源**，而 `state.vscdb` 是带加密 secret storage 的 SQLite，无法靠
//! 写 JSON 伪造（traecode 那套「改登录态」在这里无效）。因此只能整组快照 / 覆盖。
//!
//! 设计约束（每条都对应参考实现踩过的一个坑，见 doc/TraeWork账号切换计划.md §2.6）：
//! 1. 覆盖槽位前先做 `.bak` 单代轮转——拷贝中断（断电/被杀）不至于永久丢上一份快照；
//! 2. 恢复前必须删 `state.vscdb-wal`/`-shm`——否则客户端启动回放旧 WAL，**旧账号复活**；
//! 3. 恢复必须**对称**：槽位没有的项要把现场同名项删掉，而不是只覆盖槽位有的项，
//!    否则上一个账号的残留（如 `state.vscdb.backup`）会留在新账号现场；
//! 4. 单个条目拷贝失败即整体失败（不做「部分成功也算成功」），偏安全方向。
//!
//! 所有破坏性函数都接收 [`Ctx`] 而非自行读环境变量：这是让它们在临时目录上可测的
//! 唯一方式（理由见 `profile::Ctx` 的注释）。

use std::path::{Path, PathBuf};

use super::profile::{self, Ctx};
use super::{ProgressSink, StepStatus};

/// 快照槽状态（供前端展示「快照存在性 / 体积 / 时间」）
#[derive(Debug, Clone, serde::Serialize)]
pub struct SlotStatus {
    /// 槽位名（= TraeWork uid）
    pub slot: String,
    /// 主槽是否存在
    pub exists: bool,
    /// 回退槽 `.bak` 是否存在（主槽损坏时的最后退路）
    pub bak_exists: bool,
    /// 体积（字节）；主槽缺失时取 `.bak` 的体积
    pub bytes: u64,
    /// 主槽最后修改时间（Unix 秒）
    pub modified_at: Option<i64>,
    /// 快照内凭据的到期时刻（access，RFC3339 原样；解不出为 None）
    ///
    /// 为什么由**命令层**填充而不是 `slot_status` 自己填：本模块只认文件系统，解析登录态
    /// 内容属 `uid` 层；让两个平级原语互相依赖只为省一次文件读取并不划算（见 mod.rs 分层说明）
    pub expired_at: Option<String>,
    /// 快照内凭据的到期时刻（refresh）——**它才是「该槽位还能否免登录」的判据**
    pub refresh_expired_at: Option<String>,
    /// 该槽位快照内的设备标识（`machineid`）；读不到为 None
    ///
    /// 为什么由**命令层**填充：需跨槽位比较才能判断「是否与别的账号撞号」，属调用方视角
    /// （与上面两个凭据时间同理，本模块只认文件系统）
    pub machine_id: Option<String>,
    /// 与本槽位设备标识**相同**的其它账号槽位（空 = 已隔离）
    ///
    /// 为什么要有这个字段：TraeWork 原有实现假定「设备标识随快照走 = 天然一账号一设备」，
    /// 2026-09-21 实测否定（多个账号槽共用同一 machineid）。不把结果暴露出来，用户就只能
    /// 靠「又被风控了」来推测隔离有没有生效——这正是本次要消灭的不可验证状态。
    pub shares_device_with: Vec<String>,
}

/// 递归求目录/文件体积（求值失败按 0 计——体积只用于展示，不该阻断任何流程）
pub fn path_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(|e| e.ok())
            .map(|e| path_size(&e.path()))
            .sum()
    } else {
        meta.len()
    }
}

/// 拷贝单个条目（文件/目录自适应），返回是否成功
///
/// 为什么文件也「先删后拷」：源被占用导致拷贝失败时，若保留旧目标文件，备份会「看起来
/// 成功」但内容其实是上一代的——参考实现踩过这个坑（快照留陈旧 cookie，恢复后登录态
/// 错乱）。先删后拷让失败表现为「目标不存在」，调用方能立刻察觉。
pub fn copy_item(src: &Path, dst: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(src) else {
        return false;
    };
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ok = if meta.is_dir() {
        let _ = std::fs::remove_dir_all(dst);
        dir_copy_recursive(src, dst).is_ok()
    } else {
        let _ = std::fs::remove_file(dst);
        std::fs::copy(src, dst).is_ok()
    };
    ok && dst.exists()
}

/// 递归目录拷贝：任一文件失败即整体失败（不做静默跳过）
fn dir_copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            dir_copy_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// `.bak` 单代轮转：覆盖槽位前把现有快照整体挪到 `<slot>.bak`（上一代直接淘汰）
///
/// 为什么需要：「保存当前登录态」依赖用户此刻真的登录着目标账号。万一用户登错账号或
/// 客户端处于未登录态，这次保存会把错误内容刷进该槽**且不可恢复**；有 `.bak` 后任何
/// 一次覆盖都能回退一代。
pub fn rotate_bak(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<(), String> {
    let dest = ctx.slot_dir(slot)?;
    if !dest.exists() {
        return Ok(());
    }
    let bak = ctx.bak_dir(slot)?;
    let _ = std::fs::remove_dir_all(&bak);
    match std::fs::rename(&dest, &bak) {
        Ok(()) => {
            sink.step(
                "backup",
                StepStatus::Info,
                &format!("原快照已备份为 {slot}.bak（可回退一代）"),
            );
            Ok(())
        }
        Err(e) => {
            // 挪移失败不阻断（可能被占用）：退化为直接覆盖，但必须如实告知
            sink.step(
                "backup",
                StepStatus::Warn,
                &format!("旧快照挪移失败（将直接覆盖，本次无可回退代）: {e}"),
            );
            Ok(())
        }
    }
}

/// 解析恢复源：主槽缺失时回退 `.bak`
///
/// 返回 `(实际快照路径, 是否使用了回退槽)`。两者皆缺即报错——此时**绝不能**退化成
/// 「跳过恢复」，那会让客户端带着上一个账号的登录态启动，用户看到的是「切换成功了但
/// 账号没变」，比明确报错危险得多。
pub fn resolve_slot(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<(PathBuf, bool), String> {
    let main = ctx.slot_dir(slot)?;
    if main.exists() {
        return Ok((main, false));
    }
    let bak = ctx.bak_dir(slot)?;
    if bak.exists() {
        sink.step(
            "restore",
            StepStatus::Warn,
            &format!("账号 {slot} 主快照缺失，回退使用上一代备份（{slot}.bak）"),
        );
        return Ok((bak, true));
    }
    Err(format!(
        "目标账号 {slot} 无快照，请先用该账号登录 TraeWork 并点击「保存当前登录态」"
    ))
}

/// 备份当前现场到指定槽位，返回成功拷贝的条目数
pub fn backup(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<usize, String> {
    if !ctx.data_dir.exists() {
        return Err(format!(
            "TraeWork 数据目录不存在（{}），请先启动一次 TraeWork 并登录",
            ctx.data_dir.display()
        ));
    }
    rotate_bak(ctx, slot, sink)?;
    let dest = ctx.slot_dir(slot)?;
    std::fs::create_dir_all(&dest).map_err(|e| format!("创建快照目录失败: {e}"))?;

    let mut copied = 0usize;
    let mut failed: Vec<&str> = Vec::new();
    for item in profile::SNAPSHOT_ITEMS {
        if copy_item(&ctx.data_dir.join(item), &dest.join(item)) {
            copied += 1;
        } else if ctx.data_dir.join(item).exists() {
            // 「源不存在」是常态（如首次登录前没有 state.vscdb.backup），不算失败；
            // 只有「源存在但拷不过来」才是真问题。是否致命交给恢复后校验判定。
            failed.push(item);
        }
    }
    if !failed.is_empty() {
        sink.step(
            "backup",
            StepStatus::Warn,
            &format!("以下条目存在但拷贝失败：{}", failed.join("、")),
        );
    }
    sink.step(
        "backup",
        StepStatus::Ok,
        &format!("已备份当前登录态到槽位 {slot}（{copied} 项）"),
    );
    Ok(copied)
}

/// 用指定槽位覆盖当前现场，返回成功恢复的条目数
pub fn restore(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<usize, String> {
    let (src, used_bak) = resolve_slot(ctx, slot, sink)?;
    let dest = &ctx.data_dir;
    std::fs::create_dir_all(dest).map_err(|e| format!("创建数据目录失败: {e}"))?;

    // 单实例锁：残留会让客户端启动时判定「已有实例」而拒绝启动
    let _ = std::fs::remove_file(dest.join("code.lock"));

    // 关键顺序：先删 WAL/SHM 边车。SQLite 在 WAL 模式下打开恢复后的主库时会回放现场
    // 残留的旧 WAL，把切换前账号的登录写入重新灌进新库——这就是「切换后账号不变」的根因。
    let gs = dest.join("User").join("globalStorage");
    for stale in ["state.vscdb-wal", "state.vscdb-shm"] {
        let _ = std::fs::remove_file(gs.join(stale));
    }

    let mut restored = 0usize;
    for item in profile::SNAPSHOT_ITEMS {
        let src_item = src.join(item);
        let dst_item = dest.join(item);
        if src_item.exists() {
            if copy_item(&src_item, &dst_item) {
                restored += 1;
            }
        } else if dst_item.is_dir() {
            let _ = std::fs::remove_dir_all(&dst_item);
        } else if dst_item.exists() {
            let _ = std::fs::remove_file(&dst_item);
        }
    }

    sink.step(
        "restore",
        StepStatus::Ok,
        &format!(
            "已恢复账号 {slot} 的登录态（{restored} 项{}）",
            if used_bak { "，来源为回退槽" } else { "" }
        ),
    );
    Ok(restored)
}

/// 恢复后校验：返回缺失项清单（空 = 通过）
///
/// 为什么两重判定都要做：`restored == 0` 说明快照空/损坏；必需项缺失说明副本在但
/// 布局漂移（上游改了登录态文件位置）。两种情况启动后都只会「切了个寂寞」，
/// 必须回滚而不是让用户自己发现。
pub fn verify_restore(ctx: &Ctx, restored: usize) -> Result<(), Vec<String>> {
    if restored == 0 {
        return Err(vec!["快照为空或损坏（0 项恢复）".to_string()]);
    }
    let missing: Vec<String> = profile::REQUIRED_ITEMS
        .iter()
        .filter(|item| !ctx.data_dir.join(item).exists())
        .map(|item| (*item).to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// 查询单个槽位状态
pub fn slot_status(ctx: &Ctx, slot: &str) -> SlotStatus {
    let main = ctx.slot_dir(slot).ok();
    let bak = ctx.bak_dir(slot).ok();

    let exists = main.as_ref().map(|p| p.exists()).unwrap_or(false);
    let bak_exists = bak.as_ref().map(|p| p.exists()).unwrap_or(false);
    let probe = if exists {
        main.as_ref()
    } else if bak_exists {
        bak.as_ref()
    } else {
        None
    };

    SlotStatus {
        slot: slot.to_string(),
        exists,
        bak_exists,
        bytes: probe.map(|p| path_size(p)).unwrap_or(0),
        modified_at: probe
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64),
        // 凭据时间与设备标识均由调用方按需填（见字段注释：解析登录态属 uid 层、
        // 跨槽位比较属调用方视角）
        expired_at: None,
        refresh_expired_at: None,
        machine_id: None,
        shares_device_with: Vec::new(),
    }
}

/// 删除槽位（主槽 + 回退槽），返回释放的字节数
///
/// 为什么连 `.bak` 一起删：`.bak` 只服务于「覆盖前的最后退路」。槽位被用户主动删除后
/// 留着 `.bak`，会让恢复流程把它当作可用回退源——用户以为删干净了却还能切回去。
pub fn delete_slot(ctx: &Ctx, slot: &str) -> Result<u64, String> {
    let mut freed = 0u64;
    for p in [ctx.slot_dir(slot)?, ctx.bak_dir(slot)?] {
        if p.exists() {
            freed += path_size(&p);
            if p.is_dir() {
                std::fs::remove_dir_all(&p).map_err(|e| format!("删除快照失败: {e}"))?;
            } else {
                std::fs::remove_file(&p).map_err(|e| format!("删除快照失败: {e}"))?;
            }
        }
    }
    Ok(freed)
}

/// 读取当前账号标记（槽位名）；空文件/读取失败 → None
///
/// 为什么剥 BOM：历史文件可能由带 BOM 的写入方产生，而 `trim()` 不剥 U+FEFF，
/// 会读出 "\u{feff}uid" → 与真实槽位名恒不匹配（表现为每次都提示切换成功但界面不更新）。
pub fn read_current_slot(ctx: &Ctx) -> Option<String> {
    std::fs::read_to_string(ctx.current_account_file())
        .ok()
        .map(|s| s.trim().trim_start_matches('\u{feff}').trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 写入当前账号标记（仅在切换/保存真正成功后调用）
pub fn write_current_slot(ctx: &Ctx, slot: &str) -> Result<(), String> {
    std::fs::write(ctx.current_account_file(), slot)
        .map_err(|e| format!("写入当前账号标记失败: {e}"))
}

/// 清除当前账号标记（删除账号时用）
///
/// 为什么必须清：标记指向一个已被删除的槽位时，面板会把「当前账号」一直显示成一个不存在的
/// uid，而刷新/修复都无从纠正（它已经不是账号，只是残标记）
pub fn clear_current_slot(ctx: &Ctx) -> Result<(), String> {
    let file = ctx.current_account_file();
    if file.exists() {
        std::fs::remove_file(&file).map_err(|e| format!("清除当前账号标记失败: {e}"))?;
    }
    Ok(())
}

/// 枚举快照根目录下已有的槽位名（排除 `.bak`），用于发现「账号库里没有但快照还在」的孤儿槽
pub fn list_slots(ctx: &Ctx) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(&ctx.profiles_dir) else {
        return Vec::new();
    };
    let mut slots: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|name| !name.ends_with(".bak"))
        .collect();
    slots.sort();
    slots
}

/// 目录名是否为保留槽（`last`：切换前现场的滚动备份）
pub fn is_reserved_slot(name: &str) -> bool {
    name == super::LAST_SLOT
}

/// 枚举**账号槽**：`list_slots` 去掉保留槽
///
/// 为什么要与 `list_slots` 分开：保留槽的目录名按设计就不等于账号 id（内容是「切换前现场」），
/// 谁把它当账号用都会立刻出错——登记进账号库会造出一条「名字是别的账号、点切换必被拒绝」的
/// 假账号；按「名实是否相符」去归位则会每次修复都刷假告警（2026-09-20 实测）。
/// 保留槽仍要参与瘦身清理（它也是一份快照），所以不能在 `list_slots` 里直接剔除。
pub fn list_account_slots(ctx: &Ctx) -> Vec<String> {
    list_slots(ctx)
        .into_iter()
        .filter(|s| !is_reserved_slot(s))
        .collect()
}

/// 枚举快照根下的**全部**目录（含 `.bak` 回退代），返回 `(目录名, 是否回退代)`
///
/// 为什么修复流程需要连 `.bak` 一起看：被错误覆盖时，真正的上一代账号数据恰恰只存在于
/// `.bak` 里（2026-09-19 实测：账号 `3031811986829834` 的快照就压在 `168695880747001.bak`）。
pub fn list_all_dirs(ctx: &Ctx) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(&ctx.profiles_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, bool)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .map(|name| {
            let is_bak = name.ends_with(".bak");
            let base = name.strip_suffix(".bak").unwrap_or(&name).to_string();
            (base, is_bak)
        })
        .collect();
    dirs.sort();
    dirs
}

/// 递归收集目录下的全部文件
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取目录失败（{}）: {e}", dir.display()))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// 自底向上删除空目录（清理后留下的壳）
fn remove_empty_dirs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            remove_empty_dirs(&path);
            if std::fs::read_dir(&path).map(|mut d| d.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(&path);
            }
        }
    }
}

/// 清理槽位内**已不在白名单**的历史文件，返回释放的字节数
///
/// 为什么允许删除快照内容：判据是「恢复流程只会读白名单里的路径」（`restore` 就是按
/// `SNAPSHOT_ITEMS` 逐项拷贝的），因此删掉白名单外的文件**不可能改变任何未来的恢复结果**，
/// 只是把不再需要的重量丢掉。典型场景：白名单收窄前存下的快照里躺着 487MB 的
/// `Cache`/`Code Cache`（实测），不清理就永远占着，且 `.bak` 会让它翻倍。
pub fn prune_slot(ctx: &Ctx, slot: &str, sink: &dyn ProgressSink) -> Result<u64, String> {
    let dir = ctx.dir_by_name(slot)?;
    if !dir.is_dir() {
        return Ok(0);
    }
    let allowed: Vec<String> = profile::SNAPSHOT_ITEMS
        .iter()
        .map(|s| s.replace('/', "\\").to_lowercase())
        .collect();

    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(&dir, &mut files)?;

    let mut freed = 0u64;
    for f in files {
        let rel = f
            .strip_prefix(&dir)
            .map(|p| p.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let keep = allowed
            .iter()
            .any(|a| rel == *a || rel.starts_with(&format!("{a}\\")));
        if !keep {
            freed += std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            let _ = std::fs::remove_file(&f);
        }
    }
    remove_empty_dirs(&dir);

    if freed > 0 {
        sink.step(
            "repair",
            StepStatus::Info,
            &format!("{slot}：清理白名单外的历史文件，释放 {:.1} MB", freed as f64 / 1048576.0),
        );
    }
    Ok(freed)
}

/// 把槽位目录整体改名（同盘 rename，不复制内容，秒完成）
///
/// 目标已存在时**拒绝执行**而不是覆盖：改名场景下两个目录可能都是有效的登录态，
/// 覆盖等于毁掉一个账号的快照，必须由调用方决定后续（提示用户手工处理）。
pub fn rename_slot(ctx: &Ctx, from: &str, to: &str) -> Result<(), String> {
    // 用 `dir_by_name` 而非 `slot_dir`：修复流程要改名的正是 `<uid>.bak` 这种带后缀目录，
    // 而 `slot_dir` 会把点号按白名单字符集拦下（2026-09-20 实测踩过：修复槽位直接报
    // 「槽位名含非法字符 …168695880747001.bak」）。`dir_by_name` 校验的是基础名，强度不变。
    let src = ctx.dir_by_name(from)?;
    let dst = ctx.dir_by_name(to)?;
    if !src.exists() {
        return Err(format!("槽位 {from} 不存在"));
    }
    if dst.exists() {
        return Err(format!("目标槽位 {to} 已存在，未自动改名"));
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("槽位改名失败（{from} → {to}）: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traework::test_support::{fake_ctx, QuietSink};

    #[test]
    fn 拷贝条目先删后拷与目录替换语义() {
        let ctx = fake_ctx("copy-item");
        let base = ctx.profiles_dir.clone();
        let src_f = base.join("src").join("a.txt");
        std::fs::create_dir_all(src_f.parent().unwrap()).unwrap();
        std::fs::write(&src_f, "v1").unwrap();
        let dst_f = base.join("dst").join("a.txt");
        assert!(copy_item(&src_f, &dst_f));
        assert_eq!(std::fs::read_to_string(&dst_f).unwrap(), "v1");

        // 源缺失 → false 且不动目标（前置早退语义）
        std::fs::remove_file(&src_f).unwrap();
        assert!(!copy_item(&src_f, &dst_f));
        assert!(dst_f.exists(), "源缺失时不应误删目标");

        // 源存在则为替换而非追加
        std::fs::write(&src_f, "v2").unwrap();
        assert!(copy_item(&src_f, &dst_f));
        assert_eq!(std::fs::read_to_string(&dst_f).unwrap(), "v2");

        // 目录整体替换：目标里原有的多余文件被清除
        let src_d = base.join("sd");
        std::fs::create_dir_all(src_d.join("sub")).unwrap();
        std::fs::write(src_d.join("sub").join("keep.bin"), "data").unwrap();
        let dst_d = base.join("dd");
        std::fs::create_dir_all(&dst_d).unwrap();
        std::fs::write(dst_d.join("stale.bin"), "old").unwrap();
        assert!(copy_item(&src_d, &dst_d));
        assert!(dst_d.join("sub").join("keep.bin").exists());
        assert!(!dst_d.join("stale.bin").exists(), "替换语义必须清掉旧内容");

        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 体积统计递归求和() {
        let ctx = fake_ctx("size");
        std::fs::create_dir_all(ctx.profiles_dir.join("d").join("e")).unwrap();
        std::fs::write(ctx.profiles_dir.join("d").join("e").join("x"), "12345").unwrap();
        std::fs::write(ctx.profiles_dir.join("d").join("y"), "123").unwrap();
        assert_eq!(path_size(&ctx.profiles_dir.join("d")), 8);
        assert_eq!(path_size(&ctx.profiles_dir.join("nope")), 0);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 备份恢复往返_边车被清除且现场对称等于槽位() {
        let ctx = fake_ctx("roundtrip");
        let sink = QuietSink;
        let live = ctx.data_dir.clone();
        std::fs::create_dir_all(live.join("User").join("globalStorage")).unwrap();
        std::fs::create_dir_all(live.join("aha")).unwrap();
        std::fs::create_dir_all(live.join("Network")).unwrap();
        std::fs::write(live.join("User").join("globalStorage").join("storage.json"), "{\"a\":1}").unwrap();
        std::fs::write(live.join("User").join("globalStorage").join("state.vscdb"), "db-A").unwrap();
        std::fs::write(live.join("machineid"), "M-A").unwrap();
        std::fs::write(live.join("aha").join("t"), "aha-A").unwrap();
        std::fs::write(live.join("Network").join("Cookies"), "cookie-A").unwrap();
        // 白名单外的业务数据（对话库）不该被碰：跨账号替换会串数据
        std::fs::create_dir_all(live.join("ModularData")).unwrap();
        std::fs::write(live.join("ModularData").join("database.db"), "biz").unwrap();

        backup(&ctx, "uidA", &sink).unwrap();
        let slot = ctx.slot_dir("uidA").unwrap();
        assert!(slot.join("User").join("globalStorage").join("storage.json").exists());
        assert!(slot.join("machineid").exists());
        assert!(slot.join("aha").join("t").exists());
        assert!(slot.join("Network").join("Cookies").exists());
        assert!(!slot.join("ModularData").exists(), "白名单外的业务数据不入快照");

        // 破坏现场：换成 B 的痕迹 + 残留 WAL
        std::fs::write(live.join("User").join("globalStorage").join("storage.json"), "{\"b\":1}").unwrap();
        std::fs::write(live.join("User").join("globalStorage").join("state.vscdb"), "db-B").unwrap();
        std::fs::write(live.join("machineid"), "M-B").unwrap();
        std::fs::write(live.join("User").join("globalStorage").join("state.vscdb-wal"), "stale").unwrap();

        let restored = restore(&ctx, "uidA", &sink).unwrap();
        assert_eq!(restored, 5, "快照内共 5 项");
        assert_eq!(
            std::fs::read_to_string(live.join("User").join("globalStorage").join("storage.json")).unwrap(),
            "{\"a\":1}"
        );
        assert_eq!(std::fs::read_to_string(live.join("machineid")).unwrap(), "M-A");
        assert_eq!(std::fs::read_to_string(live.join("aha").join("t")).unwrap(), "aha-A");
        assert!(
            !live.join("User").join("globalStorage").join("state.vscdb-wal").exists(),
            "现场残留的 WAL 必须被清除，否则旧账号会回放复活"
        );
        assert_eq!(
            std::fs::read_to_string(live.join("ModularData").join("database.db")).unwrap(),
            "biz",
            "白名单外业务数据不受恢复影响"
        );
        assert!(verify_restore(&ctx, restored).is_ok());

        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 恢复时剔除槽位缺失项_现场残留被清掉() {
        let ctx = fake_ctx("symmetric");
        let sink = QuietSink;
        let live = ctx.data_dir.clone();
        std::fs::create_dir_all(live.join("User").join("globalStorage")).unwrap();
        std::fs::create_dir_all(live.join("Network")).unwrap();
        std::fs::write(live.join("User").join("globalStorage").join("state.vscdb.backup"), "stale").unwrap();
        std::fs::write(live.join("Network").join("Cookies"), "old").unwrap();

        // 槽位里只有两个必需项，且没有 state.vscdb.backup / Network
        let slot = ctx.slot_dir("uidX").unwrap();
        std::fs::create_dir_all(slot.join("User").join("globalStorage")).unwrap();
        std::fs::write(slot.join("User").join("globalStorage").join("storage.json"), "{}").unwrap();
        std::fs::write(slot.join("User").join("globalStorage").join("state.vscdb"), "db").unwrap();

        restore(&ctx, "uidX", &sink).unwrap();
        assert!(
            !live.join("User").join("globalStorage").join("state.vscdb.backup").exists(),
            "槽位没有的项必须从现场删除（对称恢复）"
        );
        assert!(!live.join("Network").exists(), "槽位没有的目录必须整目录删除");

        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 无快照时报错而非静默跳过() {
        let ctx = fake_ctx("noslot");
        let sink = QuietSink;
        let err = restore(&ctx, "uidMissing", &sink).unwrap_err();
        assert!(err.contains("无快照"), "{err}");
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 主槽缺失时回退bak且删除槽位连bak一起删() {
        let ctx = fake_ctx("bakfallback");
        let sink = QuietSink;
        let bak = ctx.bak_dir("uidB").unwrap();
        std::fs::create_dir_all(&bak).unwrap();
        std::fs::write(bak.join("marker"), "x").unwrap();
        assert_eq!(slot_status(&ctx, "uidB").bytes > 0, true);
        let (resolved, used_bak) = resolve_slot(&ctx, "uidB", &sink).unwrap();
        assert!(used_bak && resolved.ends_with("uidB.bak"));

        let freed = delete_slot(&ctx, "uidB").unwrap();
        assert!(freed > 0);
        assert!(!bak.exists(), "删除槽位必须连 .bak 一起删");
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 轮转bak保留上一代且只留一代() {
        let ctx = fake_ctx("rotate");
        let sink = QuietSink;
        let slot = ctx.slot_dir("u").unwrap();
        std::fs::create_dir_all(&slot).unwrap();
        std::fs::write(slot.join("f"), "gen1").unwrap();
        rotate_bak(&ctx, "u", &sink).unwrap();
        assert!(!slot.exists());
        assert_eq!(std::fs::read_to_string(ctx.bak_dir("u").unwrap().join("f")).unwrap(), "gen1");

        std::fs::create_dir_all(&slot).unwrap();
        std::fs::write(slot.join("f"), "gen2").unwrap();
        rotate_bak(&ctx, "u", &sink).unwrap();
        assert_eq!(std::fs::read_to_string(ctx.bak_dir("u").unwrap().join("f")).unwrap(), "gen2");
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 改名支持bak后缀目录() {
        // 回归：修复流程要改名的正是 `<uid>.bak`，早期用 slot_dir 校验会被字符集拦下
        let ctx = fake_ctx("rename-bak");
        let dir = ctx.profiles_dir.join("u.bak");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        rename_slot(&ctx, "u.bak", "v").unwrap();
        assert!(ctx.profiles_dir.join("v").join("f").exists());
        assert!(!dir.exists());

        // 目标已存在时拒绝，不覆盖
        std::fs::create_dir_all(ctx.profiles_dir.join("w")).unwrap();
        std::fs::create_dir_all(ctx.profiles_dir.join("u2.bak")).unwrap();
        assert!(rename_slot(&ctx, "u2.bak", "w").is_err());
        assert!(ctx.profiles_dir.join("w").exists());
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 当前账号标记读写与bom剥离() {
        let ctx = fake_ctx("current");
        assert_eq!(read_current_slot(&ctx), None);
        write_current_slot(&ctx, "uid1").unwrap();
        assert_eq!(read_current_slot(&ctx).as_deref(), Some("uid1"));
        // 带 BOM 的历史文件
        std::fs::write(ctx.current_account_file(), "\u{feff}uid2").unwrap();
        assert_eq!(read_current_slot(&ctx).as_deref(), Some("uid2"));
        // 空白文件 → None
        std::fs::write(ctx.current_account_file(), "  ").unwrap();
        assert_eq!(read_current_slot(&ctx), None);

        // 删除账号时清除标记：清完必须读不到，且重复清空不报错（幂等）
        write_current_slot(&ctx, "uid3").unwrap();
        clear_current_slot(&ctx).unwrap();
        assert_eq!(read_current_slot(&ctx), None);
        clear_current_slot(&ctx).unwrap();
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 恢复前校验拦下零项恢复与缺必需项() {
        let ctx = fake_ctx("verify");
        assert!(verify_restore(&ctx, 0).unwrap_err()[0].contains("0 项恢复"));
        // 有恢复项但缺 storage.json / state.vscdb
        let missing = verify_restore(&ctx, 3).unwrap_err();
        assert_eq!(missing.len(), 2);
        // 补齐必需项后通过
        std::fs::create_dir_all(ctx.data_dir.join("User").join("globalStorage")).unwrap();
        std::fs::write(ctx.data_dir.join("User").join("globalStorage").join("storage.json"), "{}").unwrap();
        std::fs::write(ctx.data_dir.join("User").join("globalStorage").join("state.vscdb"), "db").unwrap();
        assert!(verify_restore(&ctx, 3).is_ok());
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 枚举槽位排除bak目录() {
        let ctx = fake_ctx("listslots");
        std::fs::create_dir_all(ctx.profiles_dir.join("a")).unwrap();
        std::fs::create_dir_all(ctx.profiles_dir.join("b.bak")).unwrap();
        std::fs::create_dir_all(ctx.profiles_dir.join("c")).unwrap();
        std::fs::write(ctx.profiles_dir.join("current_account.txt"), "a").unwrap();
        assert_eq!(list_slots(&ctx), vec!["a".to_string(), "c".to_string()]);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 账号槽清单排除保留槽last() {
        let ctx = fake_ctx("account-slots");
        std::fs::create_dir_all(ctx.profiles_dir.join("a")).unwrap();
        std::fs::create_dir_all(ctx.profiles_dir.join(crate::traework::LAST_SLOT)).unwrap();
        std::fs::create_dir_all(ctx.profiles_dir.join("last.bak")).unwrap();
        // 保留槽必须在 `list_slots` 里（瘦身清理要覆盖它），但绝不能进账号槽清单
        assert!(list_slots(&ctx).contains(&crate::traework::LAST_SLOT.to_string()));
        assert_eq!(list_account_slots(&ctx), vec!["a".to_string()]);
        assert!(is_reserved_slot("last") && !is_reserved_slot("lastx"));
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }
}
