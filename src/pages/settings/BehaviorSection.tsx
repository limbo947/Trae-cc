import type { SettingsSectionProps } from "./shared";

/**
 * 开关类设置。
 *
 * 为什么一个文件放两个导出：这两块的归属分组不同——隐私模式跟着 Trae IDE 走，
 * 刷新与签到属通用设置。容器保留两个 `.settings-section` 外壳与标题，
 * 本文件只提供落在各自组内的条目。
 */

/** 刷新间隔的可选项（分钟）。后端只存数值，选项收敛在这里，改档位只动这一处。 */
const REFRESH_INTERVAL_OPTIONS = [5, 10, 30, 60];

/** 自动开启隐私模式（切换账号后由后端写入 state.vscdb） */
export function PrivacySetting({ settings, disabled, onPatch, onToast }: SettingsSectionProps) {
  const handlePrivacyHelp = () => {
    const message =
      "启用隐私模式后，TRAE不会存储或使用您的任何聊天交互内容（包括相关代码片段）用于分析、产品改进或模型训练。";
    if (onToast) {
      onToast("info", message, 4000);
    } else {
      alert(message);
    }
  };

  return (
    <div className="setting-item">
      <div className="setting-info">
        <div className="setting-label">
          自动开启隐私模式
          <button type="button" className="setting-help" onClick={handlePrivacyHelp} style={{ marginLeft: '6px' }}>
            ?
          </button>
        </div>
        <div className="setting-desc">切换账号后自动开启 Trae 隐私模式（需重启生效）</div>
      </div>
      <div className="setting-action">
        <button
          type="button"
          className={`pill-toggle ${settings.privacy_auto_enable ? "on" : ""}`}
          onClick={() =>
            onPatch(
              { privacy_auto_enable: !settings.privacy_auto_enable },
              "已更新隐私模式设置"
            )
          }
          disabled={disabled}
          role="switch"
          aria-checked={settings.privacy_auto_enable}
        >
          <span className="pill-track"></span>
          <span className="pill-thumb"></span>
        </button>
      </div>
    </div>
  );
}

/** 用量刷新、自动签到与开机静默（通用设置组） */
export function GeneralSettings({ settings, disabled, onPatch }: SettingsSectionProps) {
  return (
    <>
      <div className="setting-item">
        <div className="setting-info">
          <div className="setting-label">定时刷新用量</div>
          <div className="setting-desc">
            按所选间隔自动刷新账号使用量数据；窗口最小化时暂停，切换账号期间跳过本轮。
          </div>
        </div>
        <div className="setting-action" style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <select
            className="setting-select"
            value={settings.refresh_interval}
            onChange={(e) =>
              onPatch({ refresh_interval: Number(e.target.value) }, "已更新刷新间隔")
            }
            disabled={disabled || !settings.auto_refresh_enabled}
            title="刷新间隔"
          >
            {/* 手改过 settings.json 的间隔值也要能显示，否则下拉会渲染成空白 */}
            {!REFRESH_INTERVAL_OPTIONS.includes(settings.refresh_interval) && (
              <option value={settings.refresh_interval}>{settings.refresh_interval} 分钟</option>
            )}
            {REFRESH_INTERVAL_OPTIONS.map((minutes) => (
              <option key={minutes} value={minutes}>{minutes} 分钟</option>
            ))}
          </select>
          <button
            type="button"
            className={`pill-toggle ${settings.auto_refresh_enabled ? "on" : ""}`}
            onClick={() =>
              onPatch(
                { auto_refresh_enabled: !settings.auto_refresh_enabled },
                "已更新定时刷新设置"
              )
            }
            disabled={disabled}
            role="switch"
            aria-checked={settings.auto_refresh_enabled}
          >
            <span className="pill-track"></span>
            <span className="pill-thumb"></span>
          </button>
        </div>
      </div>

      <div className="setting-item">
        <div className="setting-info">
          <div className="setting-label">启动时自动签到</div>
          <div className="setting-desc">
            启动应用时为今日未签到的账号后台静默签到。关闭后仍可用右键菜单或「全部签到」手动执行。
          </div>
        </div>
        <div className="setting-action">
          <button
            type="button"
            className={`pill-toggle ${settings.auto_checkin_enabled ? "on" : ""}`}
            onClick={() =>
              onPatch(
                { auto_checkin_enabled: !settings.auto_checkin_enabled },
                "已更新自动签到设置"
              )
            }
            disabled={disabled}
            role="switch"
            aria-checked={settings.auto_checkin_enabled}
          >
            <span className="pill-track"></span>
            <span className="pill-thumb"></span>
          </button>
        </div>
      </div>

      <div className="setting-item">
        <div className="setting-info">
          <div className="setting-label">
            开机静默自动刷新 Token
            <span className="setting-badge success">强烈推荐</span>
          </div>
          <div className="setting-desc">
            开机时在后台静默启动，自动刷新所有账号 Token 并同步到 Trae IDE，确保打开 IDE 时 Token 始终有效且无需手动刷新。
          </div>
        </div>
        <div className="setting-action">
          <button
            type="button"
            className={`pill-toggle ${settings.auto_start_enabled ? "on" : ""}`}
            onClick={() =>
              onPatch(
                { auto_start_enabled: !settings.auto_start_enabled },
                "已更新开机静默刷新设置"
              )
            }
            disabled={disabled}
            role="switch"
            aria-checked={settings.auto_start_enabled}
          >
            <span className="pill-track"></span>
            <span className="pill-thumb"></span>
          </button>
        </div>
      </div>
    </>
  );
}
