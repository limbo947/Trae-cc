//! Trae IDE 侧设备标识重置（aha 层）
//!
//! 为什么需要：切换/清登流程已覆盖 `machineid` 文件、`storage.json` telemetry 三件套、
//! 注册表 `MachineGuid`，但漏了 `%APPDATA%\Trae CN\aha\TinyStorage` 内的
//! `aha.device.device_id`，导致 IDE 侧设备标识不完整。
//!
//! 风险很低：登录态在 `storage.json` 的 `iCube*` 键与 `state.vscdb`，与 aha 无关，
//! **不需要重新登录**。唯一真实风险是「Trae 运行中被回写覆盖」，靠调用时序（先 kill）规避。

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Result};

/// TinyStorage 内的设备标识键
const DEVICE_ID_KEY: &str = "aha.device.device_id";

/// TinyStorage 顶层容器键
///
/// **键是嵌套结构（本机实测，勿按顶层实现）**：
/// `{"tiny_storage_data":{"aha.device.device_id":…, "aha_access_policy":…, …}}`。
/// 在顶层 `remove` 会静默落空，而 best-effort 风格会把失败吞掉，极难发现。
const STORAGE_CONTAINER: &str = "tiny_storage_data";

/// 删除 aha 层设备标识（外科式删单键，不删文件）
///
/// 全程幂等且 best-effort：文件/子对象/键不存在都视为已重置。
/// JSON 解析失败时**保持原样**——把「重置」变成「数据丢失」不可接受。
pub fn reset_aha_device_id(trae_path: &Path) -> Result<()> {
    let storage_path = trae_path.join("aha").join("TinyStorage");
    if !storage_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&storage_path)
        .map_err(|e| anyhow!("读取 TinyStorage 失败: {}", e))?;

    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) else {
        log::warn!("TinyStorage 不是合法 JSON，跳过 aha 设备标识重置");
        return Ok(());
    };

    let Some(container) = json.get_mut(STORAGE_CONTAINER).and_then(|v| v.as_object_mut()) else {
        return Ok(());
    };

    // 只删设备标识。保留同层 aha_access_policy（服务端下发的策略 blob，删了只会多一次
    // 往返且无隔离收益）/ aha_doctor_domain / aha_last_renderer_oom，
    // 也避免将来新增的键被我们的整体重写抹掉。
    // 为什么不能写「合法值」：该值是加密 blob，密钥不在我们掌握中，写入非法值会让客户端
    // 解密路径异常——删除永远比伪造安全。
    if container.remove(DEVICE_ID_KEY).is_none() {
        return Ok(());
    }

    let serialized =
        serde_json::to_string(&json).map_err(|e| anyhow!("序列化 TinyStorage 失败: {}", e))?;
    write_atomic(&storage_path, &serialized)
}

/// 原子写回：写同目录临时文件 + rename
///
/// 为什么必须原子：进程崩溃时直写会把「外科式删除」变成「文件截断」。
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content).map_err(|e| anyhow!("写入 TinyStorage 临时文件失败: {}", e))?;
    fs::rename(&tmp_path, path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        anyhow!("替换 TinyStorage 失败: {}", e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机实测的真实结构：设备号在 `tiny_storage_data` **子对象**里。
    /// 顶层 remove 会静默落空，因此断言同层其余 3 键必须原样保留。
    const SAMPLE: &str = r#"{"tiny_storage_data":{"aha.device.device_id":"encrypted-blob","aha_access_policy":"policy-blob","aha_doctor_domain":[],"aha_last_renderer_oom":false}}"#;

    fn temp_trae_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("trae-cc-device-reset-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("aha")).unwrap();
        dir
    }

    #[test]
    fn removes_only_device_id_key() {
        let trae_path = temp_trae_path("remove");
        let storage = trae_path.join("aha").join("TinyStorage");
        fs::write(&storage, SAMPLE).unwrap();

        reset_aha_device_id(&trae_path).unwrap();

        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&storage).unwrap()).unwrap();
        let container = json.get(STORAGE_CONTAINER).unwrap().as_object().unwrap();
        assert!(!container.contains_key(DEVICE_ID_KEY));
        assert!(container.contains_key("aha_access_policy"));
        assert!(container.contains_key("aha_doctor_domain"));
        assert!(container.contains_key("aha_last_renderer_oom"));
        assert_eq!(container.len(), 3);

        // 幂等：再次调用仍是成功且不再改动
        reset_aha_device_id(&trae_path).unwrap();
        assert!(!storage.with_extension("tmp").exists());
    }

    /// 文件缺失 / 键缺失 / JSON 损坏都不得变成错误或数据丢失
    #[test]
    fn stays_idempotent_and_never_destroys_data() {
        let trae_path = temp_trae_path("safe");
        let storage = trae_path.join("aha").join("TinyStorage");

        // 文件不存在
        reset_aha_device_id(&trae_path).unwrap();

        // 损坏的 JSON：必须保持原样
        fs::write(&storage, "{not json").unwrap();
        reset_aha_device_id(&trae_path).unwrap();
        assert_eq!(fs::read_to_string(&storage).unwrap(), "{not json");

        // 顶层没有 tiny_storage_data 容器
        fs::write(&storage, r#"{"other":1}"#).unwrap();
        reset_aha_device_id(&trae_path).unwrap();
        assert_eq!(fs::read_to_string(&storage).unwrap(), r#"{"other":1}"#);
    }
}
