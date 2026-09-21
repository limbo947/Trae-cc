//! TraeWork 设备标识层：槽位指纹读取、保存时对齐、切换时写入系统注册表。
//!
//! ## 为什么单开一个模块
//!
//! `snapshot` 层是**纯文件系统原语**（可随意在临时目录上调用），而本模块要做的是写
//! `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` 这种**系统级副作用**。混进去会让
//! 快照层不再是纯原语，也超出它「只认文件系统」的既有职责（分层见 `mod.rs` 头部）。
//!
//! ## 设备标识归属于 `user_id`，不归属于记录/app（2026-09-21 修正，勿退回）
//!
//! 两个约束必须同时满足，而它们指向同一个答案：
//!
//! - **不同真实账号 → 不同设备**：否则「一台设备绑太多账号」触发设备级限制
//!   （「该设备绑定的账户数量已达上限」）。
//! - **同一真实账号 → 同一设备**：官方口径「同一台电脑同时登录 TraeCode 和 TraeWork 只算
//!   1 台设备」。而账号库里同一账号本就有**两条记录**（`app` 不同、`user_id` 相同，实测 3 个
//!   `user_id` 跨应用共存），若各持一套标识（traecode `caf505a4…` / traework `65e414a1…`），
//!   服务端就把它当两台设备——这是**账号级风控**（`Login Abnormality`）的成因方向。
//!
//! 所以本模块的对齐目标是**账号库按 `user_id` 给出的共享值**
//! （`AccountManager::shared_machine_id`），而不是「撞号就随便生成一个新值」——后者只解决了
//! 第一条约束，反而把第二条弄坏。
//!
//! ## 为什么原方案不够：一个被实测推翻的假设（2026-09-21）
//!
//! 既有文档假定「机器码与 aha 随快照走 → 天然做到一账号一设备」。**实测否定了这个假设**：
//! 7 个槽位目录里，4 个账号槽的 `machineid` 与 `telemetry.machineId` 完全相同，而
//! `telemetry.devDeviceId` 更是 7 个**全部相同**（只有一个账号槽是独立值）。
//!
//! 根因是保存流程只把**现场原样拷进槽位**，而客户端不会因为换了登录账号就重写 `machineid`
//! 文件——于是每个账号存下来的都是同一个身份。「随快照走」只在**现场真的变过**时才等于
//! 「互不相同」，这两件事被混为一谈了。
//!
//! 注册表层则完全没参与：实测 `MachineGuid` 等于某条 **traecode** 账号的绑定值，
//! 与任何一个 TraeWork 槽位的 `machineid` 都不同。
//!
//! ## 边界（不要越界）
//!
//! - **不做批量重置**：对齐只在用户主动点「保存当前登录态」时对该槽位生效，存量重复值
//!   由用户逐个重新保存来修正（与 `TraeWork账号切换计划.md:291` 的边界一致）。
//! - **不伪造 aha**：`aha\TinyStorage` 内的设备号是加密 blob（密钥不在手），只能删不能写；
//!   复用 `device_reset::reset_aha_device_id` 的外科式删除，删后由客户端按自己的规则重生成。
//! - **风控冷却期内不要执行**：对齐会改变服务端看到的设备身份，账号已被标记时可能加重处罚
//!   （2026-09-21 的 `Login Abnormality` 事件），应先等冷却结束。

use std::path::{Path, PathBuf};

use super::profile::Ctx;
use super::{ProgressSink, StepStatus};

/// 快照内承载机器标识的文件（白名单内，值是一个 UUID）
const MACHINE_ID_FILE: &str = "machineid";

/// telemetry 三件套在 `storage.json` 里的键名（与 traecode 侧同名同义）
const TELEMETRY_MACHINE_ID: &str = "telemetry.machineId";
const TELEMETRY_DEV_DEVICE_ID: &str = "telemetry.devDeviceId";
const TELEMETRY_SQM_ID: &str = "telemetry.sqmId";

/// 设备标识指纹：只保留判定「两个槽位是否指向同一台设备」所需的最小字段集
///
/// 为什么三项都要看而不是只看 `machineid`：实测存在三者不一致的情形——同一个
/// `telemetry.devDeviceId` 可以配不同的 `machineid`。只看一项会漏掉这种「部分隔离」，
/// 而部分隔离在服务端看来仍然是同一台设备。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fingerprint {
    /// `machineid` 文件内容
    pub machine_id: Option<String>,
    /// `storage.json` 的 `telemetry.machineId`
    pub telemetry_machine_id: Option<String>,
    /// `storage.json` 的 `telemetry.devDeviceId`
    pub dev_device_id: Option<String>,
}

impl Fingerprint {
    /// 三项全空 = 读不到任何标识（空目录/上游改了布局），此时**不参与**撞号判定
    pub fn is_empty(&self) -> bool {
        self.machine_id.is_none()
            && self.telemetry_machine_id.is_none()
            && self.dev_device_id.is_none()
    }

    /// 是否与另一个指纹指向同一台设备：**任一非空字段相同**即算撞号
    ///
    /// 为什么用「任一相同」这种保守判据：漏判的代价是账号继续共用设备身份（正是要修的
    /// 问题），误判的代价只是多归一一次（多生成一套标识）。两侧代价不对称时偏安全方向。
    pub fn clashes_with(&self, other: &Self) -> bool {
        let same = |a: &Option<String>, b: &Option<String>| match (a, b) {
            (Some(x), Some(y)) => !x.is_empty() && x == y,
            _ => false,
        };
        same(&self.machine_id, &other.machine_id)
            || same(&self.telemetry_machine_id, &other.telemetry_machine_id)
            || same(&self.dev_device_id, &other.dev_device_id)
    }
}

/// 读某个目录（槽位快照或现场）的设备标识指纹
fn fingerprint_of(dir: &Path) -> Fingerprint {
    let machine_id = std::fs::read_to_string(dir.join(MACHINE_ID_FILE))
        .ok()
        .map(|s| s.trim().trim_start_matches('\u{feff}').to_string())
        .filter(|s| !s.is_empty());

    // storage.json 可能带 BOM（历史写入方留下），不剥会让 serde 直接解析失败，
    // 表现为「telemetry 读不到」——而文件其实是好的
    let json = std::fs::read_to_string(dir.join("User").join("globalStorage").join("storage.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}')).ok());
    let pick = |key: &str| {
        json.as_ref()
            .and_then(|j| j.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };

    Fingerprint {
        machine_id,
        telemetry_machine_id: pick(TELEMETRY_MACHINE_ID),
        dev_device_id: pick(TELEMETRY_DEV_DEVICE_ID),
    }
}

/// 槽位快照目录：主槽优先，缺失时回退 `.bak`（与 `snapshot::resolve_slot` 同语义）
fn slot_source_dir(ctx: &Ctx, slot: &str) -> Option<PathBuf> {
    let main = ctx.slot_dir(slot).ok()?;
    if main.is_dir() {
        return Some(main);
    }
    let bak = ctx.bak_dir(slot).ok()?;
    if bak.is_dir() {
        return Some(bak);
    }
    None
}

/// 读槽位的设备标识指纹（主槽缺失时读回退代）
pub fn read_fingerprint(ctx: &Ctx, slot: &str) -> Fingerprint {
    slot_source_dir(ctx, slot)
        .map(|d| fingerprint_of(&d))
        .unwrap_or_default()
}

/// 读**现场**（客户端正在使用的那一份）的设备标识指纹
pub fn read_live_fingerprint(ctx: &Ctx) -> Fingerprint {
    fingerprint_of(&ctx.data_dir)
}

/// 与本槽位设备标识相同、但槽位名不同的**账号槽**
///
/// 为什么排除保留槽（`last` / `last.bak`）：它的内容按设计就是「某个账号的快照」，算进来会
/// 让每个账号都恒被判为撞号——每次保存都重新生成一套标识，账号的设备身份永不固定，
/// 反而制造出「设备频繁轮换」这个风控强特征。
pub fn clashes(ctx: &Ctx, slot: &str) -> Vec<String> {
    let mine = read_fingerprint(ctx, slot);
    if mine.is_empty() {
        return Vec::new();
    }
    super::snapshot::list_account_slots(ctx)
        .into_iter()
        .filter(|s| s != slot)
        .filter(|s| mine.clashes_with(&read_fingerprint(ctx, s)))
        .collect()
}

/// 设备标识对齐结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeOutcome {
    /// 已与目标值一致，未做任何改动
    Aligned,
    /// 已把槽位与现场改成目标值
    Realigned {
        /// 改动前的 machineid（用于日志与前端提示）
        previous_machine_id: Option<String>,
        /// 为何要对齐（日志与提示共用同一句话，避免两处措辞漂移）
        reason: String,
    },
    /// 读不到标识或写入失败（**不阻断保存**，由调用方降级为告警）
    Skipped(String),
}

/// 保存登录态时对齐设备标识
///
/// 目标值有两个来源，**顺序不可颠倒**：
/// 1. `shared`（由调用方从账号库按 `user_id` 取出，见 `AccountManager::shared_machine_id`）
///    —— 这是主路径：同一真实账号在 TraeCode 与 TraeWork 侧**必须是同一台设备**（官方口径
///    「同一台电脑同时登录 TraeCode 和 TraeWork 只算 1 台设备」）。
/// 2. `shared` 缺失（账号还没进账号库）时的兜底：与其它账号槽位撞号才生成一个新值，
///    否则不动。**没有兜底会让首次保存的账号沿用现场那个被别人用过的标识**。
///
/// 为什么挂在「保存」而不是「切换」：保存是用户**主动声明**「这个槽位就是当前这个账号」的时刻，
/// 此时才有「该给这个账号分配什么设备身份」的语义；切换只是把既有身份搬到现场，在那里改值
/// 等于偷偷重写历史快照。
pub fn normalize_on_save(
    ctx: &Ctx,
    slot: &str,
    sink: &dyn ProgressSink,
    shared: Option<&str>,
) -> NormalizeOutcome {
    let current = read_fingerprint(ctx, slot).machine_id;
    let shared_with = clashes(ctx, slot);

    let shared = shared.map(str::trim).filter(|s| !s.is_empty());
    let (target, reason) = match shared {
        Some(target) => {
            let mut reason =
                "与 TraeCode 侧同一账号统一为同一设备标识（同机两应用只算 1 台设备）".to_string();
            if !shared_with.is_empty() {
                reason = format!("{reason}；同时解除了与 {} 的撞号", shared_with.join("、"));
            }
            (target.to_string(), reason)
        }
        None if !shared_with.is_empty() => (
            crate::machine::generate_machine_guid(),
            format!(
                "账号库中没有该账号的设备标识，且与 {} 撞号，已生成独立标识",
                shared_with.join("、")
            ),
        ),
        None => {
            sink.step("device", StepStatus::Ok, "设备标识无需改动");
            return NormalizeOutcome::Aligned;
        }
    };

    if current.as_deref() == Some(target.as_str()) {
        sink.step("device", StepStatus::Ok, "设备标识已与目标值一致");
        return NormalizeOutcome::Aligned;
    }

    match apply_identity(ctx, slot, &target) {
        Ok(()) => {
            sink.step("device", StepStatus::Ok, &reason);
            NormalizeOutcome::Realigned {
                previous_machine_id: current,
                reason,
            }
        }
        Err(e) => {
            // 对齐失败不该让整次保存失败：登录态本身是好的，用户重新点一次就能重试
            let msg = format!("设备标识对齐失败（登录态已保存，可重试）: {e}");
            sink.step("device", StepStatus::Warn, &msg);
            NormalizeOutcome::Skipped(e)
        }
    }
}

/// 把槽位快照与**现场**同时改成指定的设备标识
///
/// 为什么两处**必须成对写**：槽位是「下次切换的恢复源」，现场是「客户端下次启动读的」。
/// 只写一处，另一处会在下一次保存或切换时把旧值铺回来——用户看到的现象是「每次点保存都
/// 提示已对齐，但账号之间始终不一致」，而代码看起来每步都对。
fn apply_identity(ctx: &Ctx, slot: &str, machine_id: &str) -> Result<(), String> {
    let new_machine_id = machine_id.to_string();

    let mut targets: Vec<PathBuf> = Vec::new();
    if let Some(dir) = slot_source_dir(ctx, slot) {
        targets.push(dir);
    } else {
        return Err(format!("槽位 {slot} 没有可用的快照目录"));
    }
    targets.push(ctx.data_dir.clone());

    for dir in targets {
        let id_path = dir.join(MACHINE_ID_FILE);
        std::fs::write(&id_path, &new_machine_id)
            .map_err(|e| format!("写入 {} 失败: {e}", id_path.display()))?;

        // telemetry 是附加标识：写不进去（文件缺失/JSON 异常）不阻断，主判据是 machineid
        if let Err(e) = patch_telemetry(&dir, &new_machine_id) {
            log::warn!("[{dir}] telemetry 改写失败（不阻断归一）: {e}", dir = dir.display());
        }

        // aha 层：加密 blob 只能删不能写，删后由客户端生成新值。
        // best-effort——保留着旧 aha 只会让隔离不彻底，不该让归一整体失败
        if let Err(e) = crate::device_reset::reset_aha_device_id(&dir) {
            log::warn!("[{dir}] aha 设备标识重置失败（不阻断归一）: {e}", dir = dir.display());
        }
    }

    Ok(())
}

/// 改写 `storage.json` 的 telemetry 三件套（生成规则复用 `machine::telemetry_ids`）
///
/// 【实测更正 2026-09-21】「`telemetry.machineId` = sha256(machineid)」这个假设**已被证伪**：
/// 本机两组对照全部对不上（`sha256(65e414a1…)` = `84b55d30…`，而槽位里实际是 `0513f181…`；
/// `sha256(67b08330…)` = `32e8bbde…`，实际是 `5f971682…`），该字段实际**由客户端自行维护**。
///
/// 所以这里写入的是**尽力而为的兜底值**，下次启动可能被客户端改成它自己的算法值。
/// 这不影响隔离结论——无论客户端是否覆盖，只要 `machineid` 各不相同，它重算出的值也必然不同
/// （同一目录下 `machineid` 与 `telemetry.machineId` 实测是一一对应的）。**真正完全可控的层
/// 是 `machineid` 文件与系统注册表**，判断隔离是否生效要看那两层。
fn patch_telemetry(dir: &Path, machine_id: &str) -> Result<(), String> {
    let path = dir.join("User").join("globalStorage").join("storage.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(()); // 文件不存在：不是错误（未登录过的新槽位）
    };
    let mut json: serde_json::Value = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("storage.json 不是合法 JSON: {e}"))?;
    let Some(obj) = json.as_object_mut() else {
        return Err("storage.json 顶层不是对象".to_string());
    };

    let (telemetry_machine_id, dev_device_id, sqm_id) = crate::machine::telemetry_ids(machine_id);
    obj.insert(TELEMETRY_MACHINE_ID.to_string(), serde_json::json!(telemetry_machine_id));
    obj.insert(TELEMETRY_DEV_DEVICE_ID.to_string(), serde_json::json!(dev_device_id));
    obj.insert(TELEMETRY_SQM_ID.to_string(), serde_json::json!(sqm_id));

    let text = serde_json::to_string_pretty(&json).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 注册表读写出口
///
/// 为什么抽成 trait：写 `HKLM` 是系统级副作用，测试**绝不能**碰真实注册表。出口注入化之后，
/// 「槽位没有标识时跳过」「写失败只告警不阻断」「写后读回不一致」这三条判定都能在单元测试里
/// 验证——这正是本次「可验证实验」要的能力（否则只能靠人手动切一次账号去肉眼看）。
pub trait RegistryAccess {
    fn get(&self) -> Result<String, String>;
    fn set(&self, value: &str) -> Result<(), String>;
}

/// 生产实现：真实系统注册表
pub struct SystemRegistry;

impl RegistryAccess for SystemRegistry {
    fn get(&self) -> Result<String, String> {
        crate::machine::get_machine_guid().map_err(|e| e.to_string())
    }

    fn set(&self, value: &str) -> Result<(), String> {
        crate::machine::set_machine_guid(value).map_err(|e| e.to_string())
    }
}

/// 注册表同步结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryOutcome {
    /// 已写入且读回一致
    Applied {
        value: String,
        /// 写入前注册表值与目标是否不同（false = 本来就一致，便于对账「这次到底动没动」）
        changed: bool,
    },
    /// 槽位没有可用标识，未改动注册表
    Skipped(String),
    /// 写入失败或读回不一致
    Failed(String),
}

/// 把槽位的 `machineid` 写入系统注册表并**读回验证**
///
/// 为什么失败不阻断切换：`HKLM` 写入需要管理员权限，而注册表只是设备标识的**一层**——
/// 登录态已经恢复，缺这一层切换本身仍然成立（与 traecode 侧「写注册表失败忽略」同源）。
/// 但必须留一条 `warn` 步骤：静默失败会让用户以为隔离已生效，从而失去排查依据。
pub fn apply_registry(
    ctx: &Ctx,
    slot: &str,
    sink: &dyn ProgressSink,
    reg: &dyn RegistryAccess,
) -> RegistryOutcome {
    let Some(target) = read_fingerprint(ctx, slot).machine_id else {
        let msg = format!("槽位 {slot} 快照内没有 machineid，跳过注册表设备标识同步");
        sink.step("device", StepStatus::Warn, &msg);
        return RegistryOutcome::Skipped(msg);
    };

    let before = reg.get().ok();
    let changed = before.as_deref() != Some(target.as_str());

    if let Err(e) = reg.set(&target) {
        let msg = format!("写入注册表 MachineGuid 失败（需要管理员权限）: {e}");
        sink.step("device", StepStatus::Warn, &msg);
        return RegistryOutcome::Failed(msg);
    }

    // 读回验证：`set` 返回成功不等于真的生效（权限拦截/UAC 虚拟化/组策略都可能让它静默落空），
    // 而「以为写成了其实没写」会让后续所有对账失去依据。这一步就是本次实验的观测点。
    match reg.get() {
        Ok(now) if now == target => {
            let msg = if changed {
                format!("注册表设备标识已同步为该账号的 machineid（{}）", short(&target))
            } else {
                format!("注册表设备标识本就与该账号一致（{}）", short(&target))
            };
            sink.step("device", StepStatus::Ok, &msg);
            RegistryOutcome::Applied {
                value: target,
                changed,
            }
        }
        Ok(now) => {
            let msg = format!(
                "注册表 MachineGuid 读回不一致：期望 {}，实际 {}",
                short(&target),
                short(&now)
            );
            sink.step("device", StepStatus::Warn, &msg);
            RegistryOutcome::Failed(msg)
        }
        Err(e) => {
            let msg = format!("写入后读回注册表失败，无法确认是否生效: {e}");
            sink.step("device", StepStatus::Warn, &msg);
            RegistryOutcome::Failed(msg)
        }
    }
}

/// 设备标识的展示用短形（日志与前端提示里只给前 8 位，便于人工对账又不至于刷屏）
///
/// 注意：设备标识**不是凭据**（traecode 侧一直整值打日志），这里取短形纯粹是阅读性考虑。
pub fn short(value: &str) -> String {
    if value.chars().count() <= 8 {
        return value.to_string();
    }
    format!("{}…", value.chars().take(8).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traework::test_support::{fake_ctx, QuietSink};
    use std::path::Path;

    /// 在指定目录写一份最小现场：`machineid` + 带 telemetry 的 `storage.json`
    fn write_identity(dir: &Path, machine_id: &str, telemetry_machine_id: &str, dev_device_id: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(MACHINE_ID_FILE), machine_id).unwrap();
        let gs = dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(
            gs.join("storage.json"),
            format!(
                r#"{{"iCubeAuthInfo://icube.cloudide":"cipher","telemetry.machineId":"{telemetry_machine_id}","telemetry.devDeviceId":"{dev_device_id}"}}"#
            ),
        )
        .unwrap();
    }

    /// 造一个含两个账号槽的上下文：`a` 与 `b` 共用同一套设备标识
    fn ctx_with_shared_pair(tag: &str) -> Ctx {
        let ctx = fake_ctx(tag);
        for slot in ["a", "b"] {
            write_identity(&ctx.profiles_dir.join(slot), "mid-shared", "tel-shared", "dev-shared");
        }
        // 现场与 a 一致（保存后必然如此）
        write_identity(&ctx.data_dir, "mid-shared", "tel-shared", "dev-shared");
        ctx
    }

    #[test]
    fn 指纹任一字段相同即判为撞号() {
        let base = Fingerprint {
            machine_id: Some("m1".into()),
            telemetry_machine_id: Some("t1".into()),
            dev_device_id: Some("d1".into()),
        };
        // 只有 devDeviceId 相同 → 仍是同一台设备（实测存在这种「部分隔离」）
        let partial = Fingerprint {
            machine_id: Some("m2".into()),
            telemetry_machine_id: Some("t2".into()),
            dev_device_id: Some("d1".into()),
        };
        assert!(base.clashes_with(&partial));
        // 三项都不同 → 已隔离
        let other = Fingerprint {
            machine_id: Some("m3".into()),
            telemetry_machine_id: Some("t3".into()),
            dev_device_id: Some("d3".into()),
        };
        assert!(!base.clashes_with(&other));
        // 空指纹不参与判定：否则「读不到」会被当成「都一样」
        assert!(!base.clashes_with(&Fingerprint::default()));
        assert!(!Fingerprint::default().clashes_with(&Fingerprint::default()));
    }

    #[test]
    fn 能识别出撞号的槽位且排除保留槽与自身() {
        let ctx = ctx_with_shared_pair("device-clash");
        // 保留槽的内容按设计就是某个账号的快照：必须排除，否则每个账号恒被判为撞号
        write_identity(&ctx.profiles_dir.join("last"), "mid-shared", "tel-shared", "dev-shared");

        assert_eq!(clashes(&ctx, "a"), vec!["b".to_string()]);
        assert_eq!(clashes(&ctx, "b"), vec!["a".to_string()]);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    /// 本轮的核心契约：同一真实账号在 TraeCode 与 TraeWork 侧必须是**同一台设备**。
    /// 因此传入共享值时应**无条件对齐**（哪怕没有任何撞号），且槽位与现场都要改成它。
    #[test]
    fn 有共享值时对齐到该值而不是另生成一套() {
        let ctx = fake_ctx("device-shared");
        write_identity(&ctx.profiles_dir.join("a"), "mid-tc", "tel-tc", "dev-tc");
        write_identity(&ctx.data_dir, "mid-tc", "tel-tc", "dev-tc");
        // 另一个账号用完全不同的身份：本用例下**不存在撞号**，用来证明对齐不依赖撞号触发
        write_identity(&ctx.profiles_dir.join("b"), "mid-b", "tel-b", "dev-b");
        let sink = QuietSink;

        let outcome = normalize_on_save(&ctx, "a", &sink, Some("mid-from-traecode"));
        assert!(
            matches!(outcome, NormalizeOutcome::Realigned { .. }),
            "应无条件对齐，实际 {outcome:?}"
        );

        let a = read_fingerprint(&ctx, "a");
        assert_eq!(a.machine_id.as_deref(), Some("mid-from-traecode"));
        assert_eq!(
            read_live_fingerprint(&ctx).machine_id.as_deref(),
            Some("mid-from-traecode"),
            "现场未同步则会被下次保存回滚"
        );
        // telemetry 三件套要跟着换（实测 devDeviceId 是 7 个槽位全同的那一项）
        assert_ne!(a.dev_device_id.as_deref(), Some("dev-tc"));
        assert_ne!(a.telemetry_machine_id.as_deref(), Some("tel-tc"));

        // 幂等：已一致时不再改动
        assert_eq!(
            normalize_on_save(&ctx, "a", &sink, Some("mid-from-traecode")),
            NormalizeOutcome::Aligned
        );
        // 不传共享值、且无撞号时也不该动它（避免把已对齐好的值又改掉）
        assert_eq!(
            normalize_on_save(&ctx, "a", &sink, None),
            NormalizeOutcome::Aligned
        );
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    /// 兜底路径：账号还没进账号库（拿不到共享值）时，撞号才生成新标识
    #[test]
    fn 无共享值时撞号才生成新标识且槽位与现场同步() {
        let ctx = ctx_with_shared_pair("device-fallback");
        let sink = QuietSink;

        let outcome = normalize_on_save(&ctx, "a", &sink, None);
        assert!(
            matches!(outcome, NormalizeOutcome::Realigned { .. }),
            "应识别为撞号并生成新标识，实际 {outcome:?}"
        );

        let a = read_fingerprint(&ctx, "a");
        let b = read_fingerprint(&ctx, "b");
        let live = read_live_fingerprint(&ctx);

        // 关键断言一：a 换到了新身份，且与 b 不再撞号
        assert_ne!(a.machine_id, Some("mid-shared".to_string()));
        assert!(!a.clashes_with(&b), "归一后 a 与 b 仍是同一设备: {a:?} / {b:?}");
        // 关键断言二：**现场也换了**——否则下一次保存会把重复值拷回来（对齐被无声回滚）
        assert_eq!(live.machine_id, a.machine_id, "现场未同步，对齐会被下次保存回滚");
        assert!(!live.clashes_with(&b), "现场仍是共用身份");
        assert_ne!(a.dev_device_id, Some("dev-shared".to_string()));
        assert_ne!(a.telemetry_machine_id, Some("tel-shared".to_string()));
        // b 保持原样：对齐不越界去改别的账号
        assert_eq!(b.machine_id, Some("mid-shared".to_string()));

        assert_eq!(
            normalize_on_save(&ctx, "a", &sink, None),
            NormalizeOutcome::Aligned
        );
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 没有标识的槽位不参与撞号也不阻断保存() {
        let ctx = fake_ctx("device-empty");
        std::fs::create_dir_all(ctx.profiles_dir.join("a")).unwrap();
        write_identity(&ctx.profiles_dir.join("b"), "mid-b", "tel-b", "dev-b");

        assert!(clashes(&ctx, "a").is_empty(), "读不到标识时应判定为「无撞号」");
        assert_eq!(
            normalize_on_save(&ctx, "a", &QuietSink, None),
            NormalizeOutcome::Aligned
        );
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    /// 可注入的注册表出口：内存实现，测试绝不触碰真实 HKLM
    struct FakeRegistry {
        value: std::cell::RefCell<String>,
        /// 置位后 `set` 返回错误，用于验证「写失败不阻断」的分支
        fail_set: bool,
        /// 置位后「写入成功但读回仍是旧值」，用于验证读回校验能抓到静默失败
        ignore_set: bool,
    }

    impl FakeRegistry {
        fn new(initial: &str) -> Self {
            Self {
                value: std::cell::RefCell::new(initial.to_string()),
                fail_set: false,
                ignore_set: false,
            }
        }
    }

    impl RegistryAccess for FakeRegistry {
        fn get(&self) -> Result<String, String> {
            Ok(self.value.borrow().clone())
        }

        fn set(&self, value: &str) -> Result<(), String> {
            if self.fail_set {
                return Err("拒绝访问（模拟非管理员）".to_string());
            }
            if !self.ignore_set {
                *self.value.borrow_mut() = value.to_string();
            }
            Ok(())
        }
    }

    #[test]
    fn 切换时把槽位的机器标识写进注册表并读回一致() {
        let ctx = fake_ctx("device-registry");
        write_identity(&ctx.profiles_dir.join("a"), "mid-a", "tel-a", "dev-a");
        let reg = FakeRegistry::new("mid-other");

        let outcome = apply_registry(&ctx, "a", &QuietSink, &reg);

        assert_eq!(
            outcome,
            RegistryOutcome::Applied {
                value: "mid-a".to_string(),
                changed: true
            }
        );
        assert_eq!(reg.get().unwrap(), "mid-a");
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 注册表本就一致时报告未发生变更() {
        let ctx = fake_ctx("device-registry-same");
        write_identity(&ctx.profiles_dir.join("a"), "mid-a", "tel-a", "dev-a");
        let reg = FakeRegistry::new("mid-a");

        assert_eq!(
            apply_registry(&ctx, "a", &QuietSink, &reg),
            RegistryOutcome::Applied {
                value: "mid-a".to_string(),
                changed: false
            }
        );
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 注册表写入失败或静默不生效都归为失败() {
        let ctx = fake_ctx("device-registry-fail");
        write_identity(&ctx.profiles_dir.join("a"), "mid-a", "tel-a", "dev-a");

        // 非管理员：set 直接报错
        let denied = FakeRegistry {
            fail_set: true,
            ..FakeRegistry::new("mid-other")
        };
        assert!(matches!(
            apply_registry(&ctx, "a", &QuietSink, &denied),
            RegistryOutcome::Failed(_)
        ));

        // set 假装成功但值没变（UAC 虚拟化/组策略拦截的形状）：必须靠读回校验抓到
        let silent = FakeRegistry {
            ignore_set: true,
            ..FakeRegistry::new("mid-other")
        };
        assert!(matches!(
            apply_registry(&ctx, "a", &QuietSink, &silent),
            RegistryOutcome::Failed(_)
        ));

        // 槽位没有 machineid：跳过而不是失败（旧快照可能没有该文件）
        std::fs::create_dir_all(ctx.profiles_dir.join("empty")).unwrap();
        let reg = FakeRegistry::new("mid-other");
        assert!(matches!(
            apply_registry(&ctx, "empty", &QuietSink, &reg),
            RegistryOutcome::Skipped(_)
        ));
        assert_eq!(reg.get().unwrap(), "mid-other", "跳过时不得改动注册表");
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 短形只保留前八位() {
        assert_eq!(short("caf505a4-472f-4723"), "caf505a4…");
        assert_eq!(short("short"), "short");
    }
}
