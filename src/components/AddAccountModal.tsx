import { useRef, useState, useEffect } from "react";
import * as api from "../api";
import type { Account } from "../types";

interface AddAccountModalProps {
  isOpen: boolean;
  onClose: () => void;
  onToast?: (type: "success" | "error" | "warning" | "info", message: string) => void;
  onAccountAdded?: (account: Account) => void;
  onImportAccounts?: () => void;
  onExportAccounts?: () => void;
  canExport?: boolean;
}

// 主模式只保留两条真实可用的路径：浏览器登录抓取凭据、更多菜单里的本地读取与导入导出。
// 曾经的「快速注册」「扫码领号」依赖已删除的代理后端，入口一并移除，避免留下点了就报错的按钮。
type AddMode = "browser" | "more";
type MoreSubMode = "trae-ide" | null;

export function AddAccountModal({
  isOpen,
  onClose,
  onToast,
  onAccountAdded,
  onImportAccounts,
  onExportAccounts,
  canExport = false,
}: AddAccountModalProps) {
  const [mode, setMode] = useState<AddMode>("browser");
  const [moreSubMode, setMoreSubMode] = useState<MoreSubMode>(null);
  const [showMoreDropdown, setShowMoreDropdown] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const dropdownRef = useRef<HTMLDivElement>(null);

  // 浏览器登录表单状态
  const [loginProgress, setLoginProgress] = useState(0);
  const [loginStatus, setLoginStatus] = useState("");

  // 点击外部关闭下拉菜单
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setShowMoreDropdown(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  if (!isOpen) return null;

  const handleReadTraeAccount = async () => {
    setLoading(true);
    setError("");

    try {
      const result = await api.readTraeAccount();
      // 后端已区分「新增 / 补全 / 已存在 / 本机无登录态」并给出文案。
      // 只有真正改动了账号库才关闭弹窗；其余情形留在原地展示说明——尤其「已存在」不是错误，
      // 但用户必须知道为什么不新增，否则只会反复点击（文案不再含糊地二选一）
      if (result.status === "added" || result.status === "updated") {
        if (result.account) onAccountAdded?.(result.account);
        onToast?.("success", result.message);
        handleClose();
      } else {
        setError(result.message);
      }
    } catch (err: any) {
      setError(err.message || "读取 Trae IDE 账号失败");
    } finally {
      setLoading(false);
    }
  };

  // 浏览器自动登录
  const handleBrowserAutoLogin = async () => {
    setLoading(true);
    setError("");
    setLoginProgress(10);
    setLoginStatus("正在打开浏览器...");

    try {
      // 第一步：打开浏览器窗口
      await api.startBrowserLogin();
      setLoginProgress(30);
      setLoginStatus("请在浏览器中完成登录...");

      // 第二步：等待登录完成并获取账号
      const account = await api.finishBrowserLogin();

      setLoginProgress(100);
      setLoginStatus("登录成功!");

      // 通知父组件添加账号
      onAccountAdded?.(account);

      // 延迟关闭弹窗
      setTimeout(() => {
        setLoading(false);
        setLoginProgress(0);
        setLoginStatus("");
        onClose();
        onToast?.("success", `登录成功，已导入账号: ${account.email}`);
      }, 800);
    } catch (err: any) {
      setError(err.message || "自动登录失败");
      setLoading(false);
      setLoginProgress(0);
      setLoginStatus("");
    }
  };

  const handleClose = () => {
    setError("");
    setMode("browser");
    setMoreSubMode(null);
    setShowMoreDropdown(false);
    setLoginProgress(0);
    setLoginStatus("");
    void api.cancelBrowserLogin();
    onClose();
  };

  const handleImport = () => {
    onImportAccounts?.();
    handleClose();
  };

  const handleExport = () => {
    onExportAccounts?.();
    handleClose();
  };

  const handleMoreSelect = (subMode: MoreSubMode) => {
    setMoreSubMode(subMode);
    setMode("more");
    setShowMoreDropdown(false);
  };

  return (
    <div className="modal-overlay" onClick={handleClose}>
      <div className="modal-content add-account-modal" onClick={(e) => e.stopPropagation()}>
        <div className="add-account-header">
          <h2>添加账号</h2>
          {/* 更多下拉菜单 */}
          <div className="more-dropdown-container header-dropdown" ref={dropdownRef}>
            <button
              className="more-dropdown-btn"
              onClick={() => setShowMoreDropdown(!showMoreDropdown)}
              disabled={loading}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <circle cx="12" cy="12" r="1" />
                <circle cx="19" cy="12" r="1" />
                <circle cx="5" cy="12" r="1" />
              </svg>
              更多
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{marginLeft: '4px'}}>
                <polyline points="6 9 12 15 18 9" />
              </svg>
            </button>

            {showMoreDropdown && (
              <div className="more-dropdown-menu dropdown-right">
                <button
                  className={`dropdown-item ${moreSubMode === "trae-ide" ? "active" : ""}`}
                  onClick={() => handleMoreSelect("trae-ide")}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
                    <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
                    <line x1="12" y1="22.08" x2="12" y2="12" />
                  </svg>
                  从 Trae 读取
                </button>
                <button
                  className="dropdown-item"
                  onClick={() => {
                    setShowMoreDropdown(false);
                    handleImport();
                  }}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                    <polyline points="7 10 12 15 17 10" />
                    <line x1="12" y1="15" x2="12" y2="3" />
                  </svg>
                  导入账号
                </button>
                <button
                  className="dropdown-item"
                  onClick={() => {
                    setShowMoreDropdown(false);
                    handleExport();
                  }}
                  disabled={!canExport}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                    <polyline points="17 8 12 3 7 8" />
                    <line x1="12" y1="3" x2="12" y2="15" />
                  </svg>
                  导出账号
                </button>
              </div>
            )}
          </div>
        </div>

        <div className="add-mode-tabs">
          <button
            className={`mode-tab ${mode === "browser" ? "active" : ""}`}
            onClick={() => { setMode("browser"); setError(""); }}
            disabled={loading}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="12" cy="12" r="10" />
              <line x1="2" y1="12" x2="22" y2="12" />
              <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
            </svg>
            浏览器登录
          </button>
        </div>

        {mode === "browser" ? (
          <div className="trae-ide-mode">
            <div className="mode-description" style={{ minHeight: 'auto', padding: '24px' }}>
              <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
                <circle cx="12" cy="12" r="10" />
                <line x1="2" y1="12" x2="22" y2="12" />
                <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
              </svg>
              <h3>浏览器登录</h3>
              <p>打开 Trae 官网登录页面，登录成功后自动导入</p>
            </div>

            {/* 进度显示 */}
            {loading && loginProgress > 0 && (
              <div className="register-progress-container" style={{ margin: '0 24px 20px' }}>
                <div className="register-progress-status">
                  {loginProgress >= 100 ? '✓ ' : ''}{loginStatus}
                </div>
                <div className="register-progress-bar">
                  <div
                    className="register-progress-fill"
                    style={{ width: `${loginProgress}%` }}
                  />
                </div>
                <div className="register-progress-percent">{loginProgress}%</div>
              </div>
            )}

            {error && <div className="error-message" style={{ margin: '0 24px 16px' }}>{error}</div>}

            <div className="modal-actions">
              <button type="button" onClick={handleClose} disabled={loading}>
                取消
              </button>
              <button
                type="button"
                className="primary"
                onClick={handleBrowserAutoLogin}
                disabled={loading}
              >
                {loading ? "等待登录..." : "打开登录页面"}
              </button>
            </div>
          </div>
        ) : mode === "more" && moreSubMode === "trae-ide" ? (
          <div className="trae-ide-mode">
            <div className="mode-description">
              <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
                <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
                <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
                <line x1="12" y1="22.08" x2="12" y2="12" />
              </svg>
              <h3>自动检测本地 Trae IDE 账号</h3>
              <p>系统将自动读取本地 Trae IDE 客户端当前登录的账号信息</p>
            </div>

            {error && <div className="error-message">{error}</div>}

            <div className="modal-actions">
              <button type="button" onClick={handleClose} disabled={loading}>
                取消
              </button>
              <button
                type="button"
                className="primary"
                onClick={handleReadTraeAccount}
                disabled={loading}
              >
                {loading ? "读取中..." : "读取本地账号"}
              </button>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}