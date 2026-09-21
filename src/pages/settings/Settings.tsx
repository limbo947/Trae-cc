import { useEffect, useMemo, useState } from "react";
import * as api from "../../api";
import type { AppSettings } from "../../types";
import { StatusSection } from "./StatusSection";
import { MachineIdSection } from "./MachineIdSection";
import { ClientSection } from "./ClientSection";
import { MachineCodeSection } from "./MachineCodeSection";
import { GeneralSettings, PrivacySetting } from "./BehaviorSection";
import { LogSection } from "./LogSection";
import { DataSection } from "./DataSection";
import type { SettingsSectionProps, ToastFn } from "./shared";
import "./Settings.css";

interface SettingsProps {
  onToast?: ToastFn;
  settings?: AppSettings | null;
  onSettingsChange?: (settings: AppSettings) => void;
  /**
   * 账号库被导入 / 清空后通知外层重拉列表。
   * 为什么必须透传出去：账号列表由 `App.tsx` 持有且只在挂载与特定操作后重载，
   * 设置页改了库而不通知，切回账号管理页看到的仍是旧数据。
   */
  onAccountsChanged?: () => void | Promise<void>;
}

/**
 * 设置页容器：只负责「设置的读取、局部更新、分发」。
 *
 * 为什么设置状态只留在这里：原实现同时存在 `appSettings` state 与 `settings` prop
 * 两个真源，靠 useEffect 从 prop 覆盖 state，任一侧漏同步就会出现「开关显示与实际
 * 落盘值不一致」。分区组件只读 props、只通过 onPatch 请求变更，双源在结构上消失。
 *
 * 分组顺序按「越靠下越危险」：常规配置 → 机器码 → 通用 → 数据与备份（含不可逆的清空）。
 */
export function Settings({ onToast, settings, onSettingsChange, onAccountsChanged }: SettingsProps) {
  const defaultSettings = useMemo<AppSettings>(
    () => ({
      auto_refresh_enabled: true,
      refresh_interval: 30,
      privacy_auto_enable: true,
      auto_start_enabled: false,
      auto_checkin_enabled: true,
      theme: null,
      view_mode: null,
    }),
    []
  );

  const [appSettings, setAppSettings] = useState<AppSettings | null>(settings ?? null);

  useEffect(() => {
    if (settings) {
      setAppSettings(settings);
    }
  }, [settings]);

  useEffect(() => {
    if (appSettings) return;
    api.getSettings()
      .then((value) => setAppSettings(value))
      .catch(() => setAppSettings(defaultSettings));
  }, [appSettings, defaultSettings]);

  const currentSettings = appSettings ?? defaultSettings;
  const settingsDisabled = !appSettings;

  const patchSettings = async (updates: Partial<AppSettings>, successMessage: string) => {
    const next = { ...currentSettings, ...updates };
    try {
      const saved = await api.updateSettings(next);
      setAppSettings(saved);
      onSettingsChange?.(saved);
      onToast?.("success", successMessage, 1000);
    } catch (err: any) {
      onToast?.("error", err.message || "更新设置失败");
    }
  };

  const sectionProps: SettingsSectionProps = {
    settings: currentSettings,
    disabled: settingsDisabled,
    onPatch: patchSettings,
    onToast,
  };

  return (
    <div className="settings-page">
      <StatusSection onToast={onToast} />

      {/* Trae IDE 设置 */}
      <div className="settings-section">
        <h3>Trae IDE 配置</h3>
        <MachineIdSection onToast={onToast} />
        <ClientSection onToast={onToast} />
        <PrivacySetting {...sectionProps} />
      </div>

      <div className="settings-section">
        <h3>机器码</h3>
        <MachineCodeSection onToast={onToast} />
      </div>

      <div className="settings-section">
        <h3>通用设置</h3>
        <GeneralSettings {...sectionProps} />
        <LogSection onToast={onToast} />
      </div>

      <div className="settings-section">
        <h3>数据与备份</h3>
        <DataSection onToast={onToast} onAccountsChanged={onAccountsChanged} />
      </div>
    </div>
  );
}
