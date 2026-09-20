import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../../api";
import { IconButton } from "./IconButton";
import type { ToastFn } from "./shared";

interface ClientSectionProps {
  onToast?: ToastFn;
}

/**
 * 两个客户端的可执行文件路径（TraeCode 与 TraeWork）。
 *
 * 为什么并排放在同一组：两者是**完全独立的应用**（数据目录、进程名、切换机制都不同），
 * 但都属于「路径配置」这一件事，此前一个在设置页、一个只在 TraeWork 面板里，
 * 用户想确认「到底配了哪个」要翻两个页面。
 *
 * 路径不是可选项：TraeCode 的 `open_trae` 只认已保存的路径、不做兜底扫描，
 * 没配就会在切换账号时静默打不开 IDE。
 */
export function ClientSection({ onToast }: ClientSectionProps) {
  const [traePath, setTraePath] = useState<string>("");
  const [traePathLoading, setTraePathLoading] = useState(false);
  const [scanning, setScanning] = useState(false);

  const [traeworkPath, setTraeworkPath] = useState<string | null>(null);
  const [traeworkLoading, setTraeworkLoading] = useState(false);
  const [scanningTraework, setScanningTraework] = useState(false);

  // 加载 Trae IDE 路径
  const loadTraePath = async () => {
    setTraePathLoading(true);
    try {
      const path = await api.getTraePath();
      setTraePath(path);
    } catch (err: any) {
      console.error("获取 Trae IDE 路径失败:", err);
      setTraePath("");
    } finally {
      setTraePathLoading(false);
    }
  };

  // 加载 TraeWork 路径：没有独立的 get 命令，从概览里取
  const loadTraeworkPath = async () => {
    setTraeworkLoading(true);
    try {
      const overview = await api.traeworkOverview();
      setTraeworkPath(overview.exe_path);
    } catch (err: any) {
      console.error("获取 TraeWork 路径失败:", err);
      setTraeworkPath(null);
    } finally {
      setTraeworkLoading(false);
    }
  };

  useEffect(() => {
    loadTraePath();
    loadTraeworkPath();
  }, []);

  // 手动设置 Trae IDE 路径
  const handleSetTraePath = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{
          name: "Trae IDE",
          extensions: ["exe"]
        }],
        title: "选择 Trae CN.exe 文件"
      });

      if (selected) {
        const path = selected as string;
        await api.setTraePath(path);
        setTraePath(path);
        onToast?.("success", "Trae IDE 路径已保存");
      }
    } catch (err: any) {
      onToast?.("error", err.message || "选择文件失败");
    }
  };

  // 自动扫描 Trae IDE 路径
  const handleScanTraePath = async () => {
    setScanning(true);
    try {
      const path = await api.scanTraePath();
      setTraePath(path);
      // 为什么扫到后必须立刻落盘：`open_trae` 只认已保存的路径、不做兜底扫描，
      // 只更新界面状态的话扫描结果会在重启后消失，且切换账号时 IDE 打不开
      // ——该失败只写日志，用户在界面上看不到任何提示。
      try {
        await api.setTraePath(path);
        onToast?.("success", "已找到并保存 Trae IDE 路径: " + path);
      } catch (saveErr: any) {
        onToast?.("error", "已找到 Trae IDE，但保存路径失败：" + (saveErr?.message || "未知错误"));
      }
    } catch (err: any) {
      onToast?.("error", err.message || "未找到 Trae IDE，请手动设置");
      // 自动扫描失败，弹出手动选择对话框
      await handleSetTraePath();
    } finally {
      setScanning(false);
    }
  };

  // 手动设置 TraeWork 路径
  const handleSetTraeworkPath = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "TraeWork", extensions: ["exe"] }],
        title: "选择 TRAE SOLO CN.exe 文件"
      });
      if (!selected) return;
      const path = selected as string;
      await api.traeworkSetPath(path);
      setTraeworkPath(path);
      onToast?.("success", "TraeWork 路径已保存");
    } catch (err: any) {
      onToast?.("error", err?.message || "设置 TraeWork 路径失败");
    }
  };

  // 自动扫描 TraeWork 路径
  const handleScanTraeworkPath = async () => {
    setScanningTraework(true);
    try {
      const found = await api.traeworkScanPath();
      if (!found) {
        onToast?.("warning", "未自动找到 TraeWork，请手动指定 TRAE SOLO CN.exe");
        return;
      }
      setTraeworkPath(found);
      // 与 TraeCode 同理：`traework_scan_path` 只返回路径、不落盘
      try {
        await api.traeworkSetPath(found);
        onToast?.("success", "已找到并保存 TraeWork 路径：" + found);
      } catch (saveErr: any) {
        onToast?.("error", "已找到 TraeWork，但保存失败：" + (saveErr?.message || "未知错误"));
      }
    } catch (err: any) {
      onToast?.("error", err?.message || "扫描 TraeWork 失败");
    } finally {
      setScanningTraework(false);
    }
  };

  return (
    <>
      <div className="setting-item" style={{ alignItems: 'flex-start' }}>
        <div className="setting-info" style={{ flex: 1 }}>
          <div className="setting-label">Trae IDE 安装路径</div>
          <div className="setting-value-wrap">
            <div className="setting-value-box with-one-action">
              {traePathLoading ? "加载中..." : (traePath || "未设置")}
            </div>
            <div className="setting-value-actions">
              <IconButton onClick={handleSetTraePath} title="手动设置">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
              </IconButton>
            </div>
          </div>
          <div className="setting-desc" style={{ marginTop: '4px' }}>
            切换账号后会自动打开 Trae IDE
          </div>
        </div>
        <div className="setting-action" style={{ display: 'flex', alignItems: 'flex-start', paddingTop: '32px', marginLeft: '16px' }}>
          <button
            className="setting-btn"
            onClick={handleScanTraePath}
            disabled={scanning}
            style={{ whiteSpace: 'nowrap' }}
          >
            {scanning ? "扫描中..." : "自动扫描"}
          </button>
        </div>
      </div>

      <div className="setting-item" style={{ alignItems: 'flex-start' }}>
        <div className="setting-info" style={{ flex: 1 }}>
          <div className="setting-label">TraeWork 安装路径</div>
          <div className="setting-value-wrap">
            <div className="setting-value-box with-one-action">
              {traeworkLoading ? "加载中..." : (traeworkPath || "未设置")}
            </div>
            <div className="setting-value-actions">
              <IconButton onClick={handleSetTraeworkPath} title="手动设置">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
              </IconButton>
            </div>
          </div>
          <div className="setting-desc" style={{ marginTop: '4px' }}>
            与 Trae IDE 是不同应用，路径各自保存，互不影响
          </div>
        </div>
        <div className="setting-action" style={{ display: 'flex', alignItems: 'flex-start', paddingTop: '32px', marginLeft: '16px' }}>
          <button
            className="setting-btn"
            onClick={handleScanTraeworkPath}
            disabled={scanningTraework}
            style={{ whiteSpace: 'nowrap' }}
          >
            {scanningTraework ? "扫描中..." : "自动扫描"}
          </button>
        </div>
      </div>
    </>
  );
}
