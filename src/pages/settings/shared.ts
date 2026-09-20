import type { AppSettings, ToastFn } from "../../types";

// ToastFn 已提升到共享类型（utils 下的导入/导出也要用），此处转发以保持
// 分区组件的既有导入路径不变。
export type { ToastFn };

/**
 * 分区组件的公共入参。
 *
 * 为什么 settings 只从容器下发、分区内不再存副本：原单文件实现里
 * `appSettings` state 与 `settings` prop 并存（两个真源），prop 变化时靠一个
 * useEffect 覆盖 state，任何一侧漏同步都会让开关显示与实际落盘值不一致。
 * 分区只读 props、只通过 `onPatch` 请求变更，双源在结构上就不可能出现。
 */
export interface SettingsSectionProps {
  settings: AppSettings;
  /** 设置尚未加载完成时禁用交互（对应原 `settingsDisabled`） */
  disabled: boolean;
  /** 局部更新：网络请求与落地统一由容器负责，分区不直接调 api */
  onPatch: (updates: Partial<AppSettings>, successMessage: string) => void;
  onToast?: ToastFn;
}
