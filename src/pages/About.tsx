import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import * as api from "../api";
import type { UpdateInfo } from "../types";

/**
 * 关于页。
 *
 * 版本号改为运行时读取（`getVersion()` 读的是 tauri.conf.json 的版本，与
 * package.json / Cargo.toml 三处同步）：写死的版本号在每次发版后都会漂移，
 * 而这里是用户反馈问题时最可能引用的地方。
 * 更新链路（tauri-plugin-updater + `check_update` / `install_update`）此前已装配
 * 但没有任何 UI 入口，一并在此接上。
 */
export function About() {
  const [version, setVersion] = useState<string>("");
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [status, setStatus] = useState<string>("");

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion("未知"));
  }, []);

  const handleCheck = async () => {
    setChecking(true);
    setStatus("");
    try {
      const info = await api.checkUpdate();
      setUpdate(info);
      setStatus(info ? `发现新版本 ${info.version}` : "已是最新版本");
    } catch (err: any) {
      setUpdate(null);
      setStatus("检查更新失败：" + (err?.message || "未知错误"));
    } finally {
      setChecking(false);
    }
  };

  const handleInstall = async () => {
    setInstalling(true);
    try {
      setStatus("正在下载并安装，完成后应用将自动重启…");
      await api.installUpdate();
      // 安装成功后由 updater 接管并重启进程，这里不再复位 installing——
      // 复位会让按钮在下一次渲染里变回可点，用户可能重复触发下载
    } catch (err: any) {
      setStatus("安装更新失败：" + (err?.message || "未知错误"));
      setInstalling(false);
    }
  };

  return (
    <div className="about-page">
      <div className="about-card">
        {/* 头部横排 */}
        <div className="about-header">
          <img src="./logo.png" alt="Trae账号管理" className="about-logo" />
          <div className="about-header-text">
            <div className="title-row">
              <h1 className="about-title">Trae账号管理</h1>
              <span className="version">v{version || "…"}</span>
            </div>
          </div>
        </div>

        {/* 说明和信息横向排列 */}
        <div className="about-intro-section">
          <p className="about-desc">
            本地管理多个 Trae CN 账号的桌面工具：账号本地存储、一键切换 IDE 登录态与机器码、用量查询与统计图表。
            基于
            <a
              href="https://github.com/S-Trespassing/Trae账号管理"
              target="_blank"
              rel="noopener noreferrer"
              className="original-link"
            >
              原作者项目
            </a>
            进行二次开发。
          </p>

          <div className="about-info">
            <a
              href="https://github.com/HHH9201/Trae-CC.git"
              target="_blank"
              rel="noopener noreferrer"
              className="github-link"
            >
              <span className="label">GitHub</span>
              <span className="value">HHH9201/Trae-CC</span>
            </a>
          </div>

          <div className="about-info" style={{ alignItems: 'center', gap: '12px' }}>
            <span className="label">版本更新</span>
            <button
              className="setting-btn"
              onClick={handleCheck}
              disabled={checking || installing}
              style={{ whiteSpace: 'nowrap' }}
            >
              {checking ? "检查中..." : "检查更新"}
            </button>
            {update && (
              <button
                className="setting-btn danger"
                onClick={handleInstall}
                disabled={installing}
                style={{ whiteSpace: 'nowrap' }}
              >
                {installing ? "安装中..." : `安装 v${update.version}`}
              </button>
            )}
            {status && (
              <span style={{ fontSize: '13px', color: 'var(--text-secondary)' }}>{status}</span>
            )}
          </div>
        </div>

        {/* 页脚 */}
        <div className="about-footer">
          Made with ❤️ by HJH · MIT License
        </div>
      </div>
    </div>
  );
}
