import { useEffect, useState } from "react";
import * as api from "../../api";
import { IconButton } from "./IconButton";
import type { ToastFn } from "./shared";

interface MachineCodeSectionProps {
  onToast?: ToastFn;
}

/**
 * 系统机器码（注册表 `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`）。
 *
 * 为什么单独成组、而不与 Trae 的 `machineid` 卡合并：这是两个来源不同、用途不同的值，
 * 界面上并排会让用户以为是一个东西的两处显示。这里只放系统值，Trae 的那个仍在
 * 上方「Machine ID」卡（它随该卡一起被清除登录状态重置）。
 * 本组的主要价值是「看清现状」——切账号时真正写进客户端的是账号绑定的机器码，
 * 重置系统值并不会改变账号身份，所以重置按钮刻意与复制拉开权重。
 */
export function MachineCodeSection({ onToast }: MachineCodeSectionProps) {
  const [machineId, setMachineId] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [resetting, setResetting] = useState(false);

  const load = async () => {
    setLoading(true);
    try {
      setMachineId(await api.getMachineId());
    } catch (err: any) {
      console.error("获取系统机器码失败:", err);
      setMachineId("未找到");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
  }, []);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(machineId);
      onToast?.("success", "系统机器码已复制到剪贴板");
    } catch {
      onToast?.("error", "复制失败");
    }
  };

  const handleReset = async () => {
    setResetting(true);
    try {
      const next = await api.resetMachineId();
      setMachineId(next);
      onToast?.("success", "系统机器码已重置");
    } catch (err: any) {
      // 写入 HKLM 需要管理员权限，失败信息由后端原样给出，直接展示而不是笼统报「重置失败」
      onToast?.("error", err.message || "重置系统机器码失败（需以管理员身份运行）");
    } finally {
      setResetting(false);
    }
  };

  const unavailable = !machineId || machineId === "未找到";

  return (
    <div className="setting-item" style={{ alignItems: 'flex-start' }}>
      <div className="setting-info" style={{ flex: 1, overflow: 'hidden' }}>
        <div className="setting-label">
          系统机器码
          <span className="setting-badge">注册表 MachineGuid</span>
        </div>
        <div className="setting-value-wrap">
          <div className="setting-value-box with-two-actions">
            {loading ? "加载中..." : machineId}
          </div>
          <div className="setting-value-actions">
            <IconButton onClick={load} disabled={loading} title="刷新">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/></svg>
            </IconButton>
            <IconButton onClick={handleCopy} disabled={unavailable || loading} title="复制">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
            </IconButton>
          </div>
        </div>
        <div className="setting-desc" style={{ marginTop: '8px' }}>
          与上方 Trae 的 machineid 是两个独立的值。重置需要管理员权限；
          切换账号时写入客户端的机器码来自账号绑定值，本卡的重置不会改变账号身份。
        </div>
      </div>
      <div className="setting-action" style={{ paddingTop: '32px', marginLeft: '16px' }}>
        <button
          className="setting-btn"
          onClick={handleReset}
          disabled={resetting || loading}
          style={{ whiteSpace: 'nowrap' }}
          title="生成一个新的随机机器码并写入注册表（需管理员权限）"
        >
          {resetting ? "重置中..." : "重置"}
        </button>
      </div>
    </div>
  );
}
