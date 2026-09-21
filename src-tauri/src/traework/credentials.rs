//! 按槽位解析 TraeWork 接口凭据的公共入口（积分查询与签到共用）。
//!
//! 为什么单独成模块而不是塞进 `uid.rs`：本函数依赖 `snapshot::read_current_slot`
//! （编排层原语），而 `uid.rs` 目前只依赖 `profile::Ctx`；塞进去会让解析层反向
//! 依赖快照层，破坏 `mod.rs` 头部声明的三层职责边界。
//!
//! 为什么必须抽出来：这段「当前槽读实时现场 → 主槽快照 → `.bak`」的优先级逻辑原先
//! 写在 `traework_credits` 命令的 `spawn_blocking` 闭包里，签到侧够不着——重写一遍
//! 必然与积分侧漂移（历史上 `auth_plaintext` 就漂移过一次）。积分与签到读同一份
//! 凭据，两处各写一份优先级迟早出现「积分能用、签到却报凭据失效」的岔路。

use super::profile::Ctx;
use super::{snapshot, uid};

/// 解出指定槽位可用的接口 token
///
/// 优先级（与原 `traework_credits` 闭包逐句等价，语义照搬）：
/// 1. 槽位是**当前账号** → 读实时现场的 `storage.json`。客户端启动时会用快照里的
///    `refreshToken` 换发新凭据并写回**现场**，快照文件不会跟着更新——读快照只会
///    拿到一份必然过期的旧 token，表现为「刚切过去的账号签到/查积分却报凭据失效」；
/// 2. 非当前槽且主槽存在 → 读主槽快照；
/// 3. 主槽缺失 → 读 `.bak` 回退代：快照被覆盖过的情况下，能用的凭据只可能在 `.bak` 里。
pub fn resolve_token(ctx: &Ctx, slot: &str) -> Result<String, String> {
    let credentials = if snapshot::read_current_slot(ctx).as_deref() == Some(slot) {
        uid::credentials_from_storage(&uid::live_storage(ctx))
    } else if ctx.slot_dir(slot)?.exists() {
        uid::slot_credentials(ctx, slot)
    } else {
        uid::slot_credentials(ctx, &format!("{slot}.bak"))
    };
    credentials
        .map(|c| c.token)
        .ok_or_else(|| format!(
            "快照里没有可用凭据（storage.json 缺失，或其中没有 token）。请切换到 {slot} 并重新「保存当前登录态」"
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traework::test_support::fake_ctx;

    /// 在指定目录（实时现场或槽目录）写一份带 tc 密文 auth 的 storage.json，
    /// auth 明文里带非空 token（格式与真实客户端一致）
    fn write_storage(dir: &std::path::Path, token: &str) {
        let plain = format!(r#"{{"userId":"slot","token":"{token}","refreshToken":"R"}}"#);
        let enc = crate::tc_crypto::encrypt_storage_value(&plain).unwrap();
        let gs = dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&gs).unwrap();
        std::fs::write(
            gs.join("storage.json"),
            format!(r#"{{"iCubeAuthInfo://icube.cloudide":"{enc}"}}"#),
        )
        .unwrap();
    }

    fn live_dir(ctx: &Ctx) -> std::path::PathBuf {
        ctx.data_dir.clone()
    }

    fn slot_dir(ctx: &Ctx, slot: &str) -> std::path::PathBuf {
        ctx.profiles_dir.join(slot)
    }

    #[test]
    fn 当前槽读实时现场而非快照() {
        let ctx = fake_ctx("cred-current");
        // 快照里放一份旧 token、现场放新 token：当前槽必须读到现场那份，
        // 否则客户端续期后的新凭据会被快照旧值顶掉
        write_storage(&slot_dir(&ctx, "168695880747001"), "T-snapshot");
        write_storage(&live_dir(&ctx), "T-live");
        snapshot::write_current_slot(&ctx, "168695880747001").unwrap();

        let token = resolve_token(&ctx, "168695880747001").unwrap();
        assert_eq!(token, "T-live");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 非当前槽读主槽快照() {
        let ctx = fake_ctx("cred-main");
        // 当前账号是别人：本槽只能走快照路径
        write_storage(&live_dir(&ctx), "T-live");
        snapshot::write_current_slot(&ctx, "3031811986829834").unwrap();
        write_storage(&slot_dir(&ctx, "168695880747001"), "T-main");

        let token = resolve_token(&ctx, "168695880747001").unwrap();
        assert_eq!(token, "T-main");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 主槽缺失时回退bak() {
        let ctx = fake_ctx("cred-bak");
        snapshot::write_current_slot(&ctx, "3031811986829834").unwrap();
        write_storage(&slot_dir(&ctx, "168695880747001.bak"), "T-bak");

        // `.bak` 的点号不在槽位白名单字符集内，必须经由 dir_by_name 的显式剥离路径读取
        let token = resolve_token(&ctx, "168695880747001").unwrap();
        assert_eq!(token, "T-bak");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }

    #[test]
    fn 三处都无凭据时报错且文案可操作() {
        let ctx = fake_ctx("cred-empty");
        snapshot::write_current_slot(&ctx, "3031811986829834").unwrap();

        let err = resolve_token(&ctx, "168695880747001").unwrap_err();
        // 文案与原 traework_credits 实现逐字一致（「重新「保存当前登录态」」是用户唯一
        // 可操作的自救提示）；断言其核心短语而非「重新保存」连续子串
        assert!(err.contains("保存当前登录态"), "文案必须指向用户可操作的自救动作: {err}");
        assert!(err.contains("168695880747001"), "文案要带上槽位名: {err}");
        let _ = std::fs::remove_dir_all(&ctx.data_dir);
        let _ = std::fs::remove_dir_all(&ctx.profiles_dir);
    }
}
