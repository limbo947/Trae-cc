import { useEffect, useState } from "react";
import * as api from "../../api";
import { IconButton } from "./IconButton";
import type { ToastFn } from "./shared";

interface MachineIdSectionProps {
  onToast?: ToastFn;
}

/**
 * Trae IDE 的 machineid 展示卡 + 清除登录状态入口。
 *
 * 为什么两块放一起：清除登录状态会重置同一个 `machineid` 文件，用户需要
 * 在按下按钮前看到当前值，拆开会让「会变成什么」失去参照。
 * 确认弹窗随组件走（`position: fixed`，DOM 位置不影响呈现）。
 */
export function MachineIdSection({ onToast }: MachineIdSectionProps) {
  const [traeMachineId, setTraeMachineId] = useState<string>("");
  const [traeRefreshing, setTraeRefreshing] = useState(false);
  const [clearingTrae, setClearingTrae] = useState(false);

  // 清除登录状态确认对话框
  const [showClearConfirm, setShowClearConfirm] = useState(false);

  // 加载 Trae IDE 机器码
  const loadTraeMachineId = async () => {
    setTraeRefreshing(true);
    try {
      const id = await api.getTraeMachineId();
      setTraeMachineId(id);
    } catch (err: any) {
      console.error("获取 Trae IDE 机器码失败:", err);
      setTraeMachineId("未找到");
    } finally {
      setTraeRefreshing(false);
    }
  };

  useEffect(() => {
    loadTraeMachineId();
  }, []);

  // 复制 Trae IDE 机器码
  const handleCopyTraeMachineId = async () => {
    try {
      await navigator.clipboard.writeText(traeMachineId);
      onToast?.("success", "Trae IDE 机器码已复制到剪贴板");
    } catch {
      onToast?.("error", "复制失败");
    }
  };

  // 清除 Trae IDE 登录状态
  const handleClearTraeLoginState = () => {
    setShowClearConfirm(true);
  };

  // 确认清除
  const confirmClearTraeLoginState = async () => {
    setShowClearConfirm(false);
    setClearingTrae(true);
    try {
      await api.clearTraeLoginState();
      await loadTraeMachineId(); // 重新加载新的机器码
      onToast?.("success", "Trae IDE 登录状态已清除，请手动删除 %APPDATA%\\Trae CN 文件夹后重启电脑");
    } catch (err: any) {
      onToast?.("error", err.message || "清除失败");
    } finally {
      setClearingTrae(false);
    }
  };

  return (
    <>
      {/* Machine ID */}
      <div className="setting-item" style={{ alignItems: 'flex-start' }}>
        <div className="setting-info" style={{ flex: 1, overflow: 'hidden' }}>
          <div className="setting-label">
            Machine ID
            <span className="setting-badge">客户端唯一标识</span>
          </div>
          <div className="setting-value-wrap">
            <div className="setting-value-box with-two-actions">
              {traeRefreshing ? "加载中..." : traeMachineId}
            </div>
            <div className="setting-value-actions">
              <IconButton onClick={loadTraeMachineId} disabled={traeRefreshing} title="刷新">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/></svg>
              </IconButton>
              <IconButton
                onClick={handleCopyTraeMachineId}
                disabled={!traeMachineId || traeRefreshing || traeMachineId === "未找到"}
                title="复制"
              >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
              </IconButton>
            </div>
          </div>
          <div className="setting-desc" style={{ marginTop: '8px', color: 'var(--warning)', display: 'flex', flexDirection: 'column', gap: '4px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '4px' }}>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12" y2="16"/></svg>
              <span>清除登录状态会重置机器码，需重新登录 Trae IDE</span>
            </div>
            <div style={{ fontSize: '12px', color: 'var(--text-secondary)', marginLeft: '16px', marginTop: '4px' }}>
              Windows 用户还需手动删除：%APPDATA%\Trae CN 文件夹
            </div>
            <div style={{ fontSize: '12px', color: 'var(--warning)', marginLeft: '16px', marginTop: '8px', display: 'flex', alignItems: 'center', gap: '4px' }}>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
              <span>此卡显示的是 Trae 客户端的 machineid 文件（%APPDATA%\Trae CN\machineid）；系统注册表的 MachineGuid 是另一个独立的值，切换账号时需以管理员身份运行才能同步写入</span>
            </div>
          </div>
        </div>
        <div className="setting-action" style={{ display: 'flex', alignItems: 'flex-start', paddingTop: '32px', marginLeft: '16px' }}>
          <button
            className="setting-btn danger"
            onClick={handleClearTraeLoginState}
            disabled={clearingTrae || traeRefreshing}
            style={{ whiteSpace: 'nowrap' }}
          >
            {clearingTrae ? "清除中..." : "清除登录状态"}
          </button>
        </div>
      </div>

      {/* 清除登录状态确认对话框 */}
      {showClearConfirm && (
        <div style={{
          position: 'fixed',
          top: 0,
          left: 0,
          right: 0,
          bottom: 0,
          backgroundColor: 'var(--scrim)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          zIndex: 9999,
        }} onClick={() => setShowClearConfirm(false)}>
          <div style={{
            backgroundColor: 'var(--bg-base-secondary)',
            borderRadius: 'var(--radius-8)',
            padding: '24px',
            maxWidth: '480px',
            width: '90%',
            maxHeight: '80vh',
            overflow: 'auto',
            boxShadow: 'var(--shadow-lg)',
            border: '1px solid var(--border-neutral-l2)',
          }} onClick={(e) => e.stopPropagation()}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px', marginBottom: '16px' }}>
              <div style={{
                width: '40px',
                height: '40px',
                borderRadius: '50%',
                backgroundColor: 'var(--warning-bg)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '20px',
              }}>⚠️</div>
              <h3 style={{ margin: 0, fontSize: '18px', fontWeight: 600, color: 'var(--text-primary)' }}>
                确定要清除 Trae IDE 登录状态吗？
              </h3>
            </div>

            <div style={{ color: 'var(--text-secondary)', lineHeight: 1.6, fontSize: '14px' }}>
              <div style={{ marginBottom: '16px' }}>
                <div style={{ fontWeight: 600, color: 'var(--text-primary)', marginBottom: '8px' }}>
                  【本软件将执行的操作】
                </div>
                <ul style={{ margin: 0, paddingLeft: '20px' }}>
                  <li>重置 Trae IDE 机器码（machineid 文件）</li>
                  <li>清除 Trae 登录态文件（storage.json、state.vscdb、Cookies 及 aha 设备标识）</li>
                </ul>
              </div>

              <div style={{ marginBottom: '16px', padding: '12px', backgroundColor: 'var(--danger-bg)', borderRadius: '8px', border: '1px solid var(--danger-border)' }}>
                <div style={{ fontWeight: 600, color: 'var(--danger)', marginBottom: '8px' }}>
                  【Windows 用户需手动操作（重要）】
                </div>
                <div style={{ marginBottom: '8px' }}>
                  本软件不会删除数据目录本身，如需彻底重置，请手动删除：
                </div>
                <ol style={{ margin: 0, paddingLeft: '20px', fontFamily: 'monospace', fontSize: '13px' }}>
                  <li>%APPDATA%\Trae CN</li>
                </ol>
              </div>

              <div style={{ marginBottom: '16px' }}>
                <div style={{ fontWeight: 600, color: 'var(--text-primary)', marginBottom: '8px' }}>
                  【完整操作流程】
                </div>
                <ol style={{ margin: 0, paddingLeft: '20px' }}>
                  <li>退出 Trae IDE（确保进程已关闭）</li>
                  <li>点击"确定"执行本软件清除操作</li>
                  <li>手动删除上述文件夹</li>
                  <li>重启电脑</li>
                  <li>重新打开 Trae IDE 注册/登录新账号</li>
                </ol>
              </div>

              <div style={{ color: 'var(--warning)', fontSize: '13px' }}>
                请确保 Trae IDE 已完全关闭后再继续！
              </div>
            </div>

            <div style={{ display: 'flex', gap: '12px', marginTop: '24px', justifyContent: 'flex-end' }}>
              <button
                onClick={() => setShowClearConfirm(false)}
                style={{
                  padding: '10px 20px',
                  border: '1px solid var(--border)',
                  borderRadius: '8px',
                  backgroundColor: 'transparent',
                  color: 'var(--text-secondary)',
                  cursor: 'pointer',
                  fontSize: '14px',
                  transition: 'all 0.2s',
                }}
                onMouseOver={(e) => {
                  e.currentTarget.style.backgroundColor = 'var(--bg-hover)';
                }}
                onMouseOut={(e) => {
                  e.currentTarget.style.backgroundColor = 'transparent';
                }}
              >
                取消
              </button>
              <button
                onClick={confirmClearTraeLoginState}
                disabled={clearingTrae}
                style={{
                  padding: '10px 20px',
                  border: 'none',
                  borderRadius: '8px',
                  backgroundColor: 'var(--danger)',
                  color: 'white',
                  cursor: clearingTrae ? 'not-allowed' : 'pointer',
                  fontSize: '14px',
                  fontWeight: 500,
                  opacity: clearingTrae ? 0.7 : 1,
                  transition: 'all 0.2s',
                }}
                onMouseOver={(e) => {
                  if (!clearingTrae) {
                    e.currentTarget.style.backgroundColor = 'var(--danger-hover)';
                  }
                }}
                onMouseOut={(e) => {
                  e.currentTarget.style.backgroundColor = 'var(--danger)';
                }}
              >
                {clearingTrae ? '清除中...' : '确定'}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
