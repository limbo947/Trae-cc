import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import * as api from "../../api";
import type { ToastFn } from "./shared";

interface LogSectionProps {
  onToast?: ToastFn;
}

/**
 * 应用日志：复制 / 导出 / 定位 / 清空。
 *
 * 为什么补齐这四个：后端早就注册了 `export_logs_cmd`、`get_log_file_path_cmd`、
 * `clear_logs_cmd`，但前端只有「复制」，三个命令一直是无人调用的死接口。
 * 反馈问题时「导出完整日志（含轮转历史）」比「复制最近 100 行」有用得多。
 */
export function LogSection({ onToast }: LogSectionProps) {
  const [logPath, setLogPath] = useState<string>("");
  // 二次确认：日志清空虽不致命，但会让后续排障失去现场，加一道点击成本
  const [confirmingClear, setConfirmingClear] = useState(false);
  const [clearing, setClearing] = useState(false);

  useEffect(() => {
    api.getLogFilePath()
      .then(setLogPath)
      .catch(() => setLogPath(""));
  }, []);

  useEffect(() => {
    if (!confirmingClear) return;
    // 确认态自动过期：避免用户点了第一次后走神，之后误触直接清空
    const timer = window.setTimeout(() => setConfirmingClear(false), 4000);
    return () => window.clearTimeout(timer);
  }, [confirmingClear]);

  // 复制日志
  const handleCopyLogs = async () => {
    try {
      const logs = await api.getLogs(100);
      if (logs.length === 0) {
        onToast?.("warning", "暂无日志内容");
        return;
      }
      const logContent = logs.join('\n');
      await navigator.clipboard.writeText(logContent);
      onToast?.("success", "日志已复制到剪贴板（最近100条）");
    } catch (err: any) {
      console.error("复制日志失败:", err);
      onToast?.("error", "复制日志失败: " + (err.message || "未知错误"));
    }
  };

  // 导出完整日志（后端会把 app.log.1..5 的历史一并追加）
  const handleExportLogs = async () => {
    try {
      const date = new Date().toISOString().split("T")[0];
      const path = await save({
        defaultPath: `trae-cc-logs-${date}.log`,
        filters: [{ name: "日志文件", extensions: ["log", "txt"] }],
      });
      if (!path) return;
      await api.exportLogs(path as string);
      onToast?.("success", "日志已导出（含历史轮转文件）");
    } catch (err: any) {
      onToast?.("error", err.message || "导出日志失败");
    }
  };

  // 在资源管理器中定位日志文件
  const handleRevealLogs = async () => {
    const path = logPath || (await api.getLogFilePath().catch(() => ""));
    if (!path) {
      onToast?.("warning", "日志文件尚未生成");
      return;
    }
    try {
      await revealItemInDir(path);
    } catch (err: any) {
      onToast?.("warning", `无法定位日志（文件可能尚未生成）：${err?.message || path}`);
    }
  };

  const handleClearLogs = async () => {
    if (!confirmingClear) {
      setConfirmingClear(true);
      return;
    }
    setConfirmingClear(false);
    setClearing(true);
    try {
      await api.clearLogs();
      onToast?.("success", "日志已清空");
    } catch (err: any) {
      onToast?.("error", err.message || "清空日志失败");
    } finally {
      setClearing(false);
    }
  };

  return (
    <div className="setting-item" style={{ alignItems: 'flex-start' }}>
      <div className="setting-info" style={{ flex: 1, overflow: 'hidden' }}>
        <div className="setting-label">应用日志</div>
        <div className="setting-desc">
          反馈问题时请导出完整日志（含历史轮转）；复制仅取最近 100 行。
        </div>
        {logPath && (
          <div style={{
            marginTop: '6px',
            fontFamily: 'Consolas, Monaco, "Courier New", monospace',
            fontSize: '12px',
            color: 'var(--text-muted)',
            wordBreak: 'break-all',
          }}>{logPath}</div>
        )}
      </div>
      <div className="setting-action" style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', paddingTop: '24px', marginLeft: '16px' }}>
        <button className="setting-btn" onClick={handleCopyLogs} title="复制最近100条日志">
          复制
        </button>
        <button className="setting-btn" onClick={handleExportLogs} title="导出完整日志文件">
          导出
        </button>
        <button className="setting-btn" onClick={handleRevealLogs} title="在资源管理器中定位日志">
          定位
        </button>
        <button
          className="setting-btn danger"
          onClick={handleClearLogs}
          disabled={clearing}
          title={confirmingClear ? "再点一次立即清空" : "清空全部日志"}
          style={{ whiteSpace: 'nowrap' }}
        >
          {clearing ? "清空中..." : confirmingClear ? "确认清空？" : "清空"}
        </button>
      </div>
    </div>
  );
}
