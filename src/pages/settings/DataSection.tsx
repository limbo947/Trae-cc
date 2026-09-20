import { useCallback, useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import * as api from "../../api";
import { exportAccountsToFile, importAccountsFromFile } from "../../utils/accountBackup";
import type { AppPaths, ToastFn } from "../../types";

interface DataSectionProps {
  onToast?: ToastFn;
  /**
   * 账号库被导入 / 清空后通知外层重拉列表。
   * 为什么必需：账号列表由 `App.tsx` 持有（只在挂载与特定操作后重载），
   * 设置页改了库却不通知，切回账号管理页会看到已经不存在（或还没出现）的账号。
   */
  onAccountsChanged?: () => void | Promise<void>;
}

/** 清空账号库的确认词。用文字而不是"再点一次"：不可逆操作需要一次主动输入。 */
const CLEAR_CONFIRM_WORD = "清空";

/**
 * 数据与备份：四条本地路径 + 账号库的导入 / 导出 / 清空。
 *
 * 为什么值得单开一组：本应用的数据散在四个位置（账号库、设置、日志、TraeWork 快照），
 * 换机迁移与排障时用户只能靠文档描述去找；这里把路径直接摆出来并给一键定位。
 */
export function DataSection({ onToast, onAccountsChanged }: DataSectionProps) {
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [accountCount, setAccountCount] = useState<number>(0);
  const [clearWord, setClearWord] = useState("");
  const [clearing, setClearing] = useState(false);

  const notify: ToastFn = useCallback(
    (type, message, duration) => onToast?.(type, message, duration),
    [onToast]
  );

  const refresh = useCallback(async () => {
    try {
      setPaths(await api.getAppPaths());
    } catch {
      // 路径取不到时保持 null，各条目渲染为「未找到」——不让整组消失
      setPaths(null);
    }
    try {
      // 账号数只用于「导出 N 个」的提示与清空影响说明，取不到就显示 0，不阻断其他功能
      setAccountCount((await api.getAccounts()).length);
    } catch {
      setAccountCount(0);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleImport = () => {
    importAccountsFromFile(notify, async () => {
      await refresh();
      await onAccountsChanged?.();
    });
  };

  const handleExport = () => {
    void exportAccountsToFile(accountCount, notify);
  };

  const handleClear = async () => {
    if (clearWord.trim() !== CLEAR_CONFIRM_WORD) return;
    setClearing(true);
    try {
      const removed = await api.clearAccounts();
      setClearWord("");
      await refresh();
      await onAccountsChanged?.();
      notify("success", `已清空账号库（移除 ${removed} 个账号）`);
    } catch (err: any) {
      notify("error", err.message || "清空账号库失败");
    } finally {
      setClearing(false);
    }
  };

  const clearEnabled = clearWord.trim() === CLEAR_CONFIRM_WORD && !clearing;

  return (
    <>
      <div className="setting-item" style={{ alignItems: 'flex-start' }}>
        <div className="setting-info" style={{ flex: 1, overflow: 'hidden' }}>
          <div className="setting-label">本地数据位置</div>
          <div className="setting-desc" style={{ marginTop: '4px' }}>
            路径由后端统一给出，点右侧图标可在资源管理器中定位。
          </div>
          <div style={{ marginTop: '12px', display: 'flex', flexDirection: 'column', gap: '10px' }}>
            <PathRow label="账号库" value={paths?.accounts ?? null} onToast={onToast} />
            <PathRow label="设置" value={paths?.settings ?? null} onToast={onToast} />
            <PathRow label="日志" value={paths?.logs ?? null} onToast={onToast} />
            <PathRow label="TraeWork 快照" value={paths?.traework_profiles ?? null} onToast={onToast} />
          </div>
        </div>
      </div>

      <div className="setting-item">
        <div className="setting-info">
          <div className="setting-label">账号库备份</div>
          <div className="setting-desc">
            导出为 JSON 备份文件，或从备份文件导入（导入为合并，不会覆盖现有账号）
          </div>
        </div>
        <div className="setting-action" style={{ display: 'flex', gap: '8px' }}>
          <button className="setting-btn" onClick={handleImport}>
            导入
          </button>
          <button className="setting-btn" onClick={handleExport}>
            导出
          </button>
        </div>
      </div>

      <div className="setting-item" style={{ alignItems: 'flex-start' }}>
        <div className="setting-info" style={{ flex: 1 }}>
          <div className="setting-label" style={{ color: 'var(--danger)' }}>
            清空账号库
          </div>
          <div className="setting-desc" style={{ color: 'var(--danger)' }}>
            不可恢复：当前 {accountCount} 个账号及其保存的 Cookies / 密码会一并删除。
            如需保留请先导出备份。输入「{CLEAR_CONFIRM_WORD}」以启用按钮。
          </div>
          <input
            type="text"
            className="setting-confirm-input"
            value={clearWord}
            onChange={(e) => setClearWord(e.target.value)}
            placeholder={`输入「${CLEAR_CONFIRM_WORD}」确认`}
          />
        </div>
        <div className="setting-action" style={{ paddingTop: '32px', marginLeft: '16px' }}>
          <button
            className="setting-btn danger"
            onClick={handleClear}
            disabled={!clearEnabled}
            style={{ whiteSpace: 'nowrap' }}
          >
            {clearing ? "清空中..." : "清空账号库"}
          </button>
        </div>
      </div>
    </>
  );
}

interface PathRowProps {
  label: string;
  value: string | null;
  onToast?: ToastFn;
}

/** 单条路径：展示 + 复制 + 在资源管理器中定位 */
function PathRow({ label, value, onToast }: PathRowProps) {
  const handleCopy = async () => {
    if (!value) return;
    try {
      await navigator.clipboard.writeText(value);
      onToast?.("success", `${label}路径已复制`);
    } catch {
      onToast?.("error", "复制失败");
    }
  };

  const handleReveal = async () => {
    if (!value) return;
    try {
      // 用 revealItemInDir 而不是 openPath：opener 的默认权限集只含前者，
      // 且「定位到文件」比「用默认程序打开」更符合这里的意图。
      await revealItemInDir(value);
    } catch (err: any) {
      // 文件可能尚未生成（例如还没写过设置、还没保存过快照），这不算故障
      onToast?.("warning", `无法定位（文件可能尚未生成）：${err?.message || value}`);
    }
  };

  return (
    <div className="setting-path-row">
      <span className="setting-path-label">{label}</span>
      <span className={`setting-path-value${value ? "" : " missing"}`}>
        {value || "未找到"}
      </span>
      <button
        className="setting-btn sm"
        onClick={handleCopy}
        disabled={!value}
        style={{ whiteSpace: 'nowrap' }}
      >
        复制
      </button>
      <button
        className="setting-btn sm"
        onClick={handleReveal}
        disabled={!value}
        title="在资源管理器中定位"
        style={{ whiteSpace: 'nowrap' }}
      >
        定位
      </button>
    </div>
  );
}
