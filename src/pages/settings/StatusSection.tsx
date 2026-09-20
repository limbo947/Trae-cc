import { useCallback, useEffect, useState } from "react";
import * as api from "../../api";
import type { AccountBrief, ToastFn } from "../../types";

interface StatusSectionProps {
  onToast?: ToastFn;
}

/**
 * 状态面板：进程占用与账号概况。
 *
 * 为什么设置页需要它：本页的操作大多有前置条件——清除登录状态要求 Trae 已关闭、
 * 保存/切换 TraeWork 快照要求目标客户端没有进程占用。这些前提原先只写在警告文案里，
 * 用户得自己判断；把「谁在运行」摆在按钮上方比写三行提示有效。
 *
 * 账号部分直接读 `getAccounts` 而不是走后端聚合：`AccountBrief` 里已经有
 * `is_current` 与 `checked_in_today`（且签到日期由后端按本地时区判定，前端不重算）。
 */
export function StatusSection({ onToast }: StatusSectionProps) {
  const [traeRunning, setTraeRunning] = useState<boolean | null>(null);
  const [traeworkRunning, setTraeworkRunning] = useState<boolean | null>(null);
  const [currentAccount, setCurrentAccount] = useState<string | null>(null);
  const [checkin, setCheckin] = useState<{ done: number; total: number } | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [status, accounts] = await Promise.all([
        api.getRuntimeStatus(),
        api.getAccounts().catch(() => [] as AccountBrief[]),
      ]);
      setTraeRunning(status.trae_running);
      setTraeworkRunning(status.traework_running);

      const traecode = accounts.filter((a) => a.app !== "traework");
      const current = traecode.find((a) => a.is_current);
      setCurrentAccount(current ? (current.email || current.name) : null);
      setCheckin({
        done: traecode.filter((a) => a.checked_in_today).length,
        total: traecode.length,
      });
    } catch (err: any) {
      // 进程状态取不到不影响本页其他功能，降级为「未知」而不是让整页报错
      setTraeRunning(null);
      setTraeworkRunning(null);
      onToast?.("warning", "获取运行状态失败：" + (err?.message || "未知错误"));
    } finally {
      setLoading(false);
    }
  }, [onToast]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="settings-section">
      <h3>状态</h3>
      <div className="setting-item">
        <div className="setting-info">
          <div className="setting-label">客户端与账号</div>
          <div className="setting-desc" style={{ marginTop: '4px' }}>
            清除登录状态前请先退出 Trae IDE；保存或切换 TraeWork 快照时会自动关闭对应客户端。
          </div>
          <div style={{ marginTop: '10px', display: 'flex', flexWrap: 'wrap', gap: '10px' }}>
            <StatusChip label="Trae IDE" value={traeRunning} />
            <StatusChip label="TraeWork" value={traeworkRunning} />
            <PlainChip
              label="当前账号"
              value={currentAccount ?? "未设置"}
              tone={currentAccount ? "normal" : "muted"}
            />
            <PlainChip
              label="今日签到"
              value={checkin ? `${checkin.done} / ${checkin.total}` : "—"}
              tone={checkin && checkin.total > 0 && checkin.done === checkin.total ? "ok" : "muted"}
            />

          </div>
        </div>
        <div className="setting-action" style={{ paddingTop: '24px', marginLeft: '16px' }}>
          <button
            className="setting-btn"
            onClick={refresh}
            disabled={loading}
            style={{ whiteSpace: 'nowrap' }}
          >
            {loading ? "刷新中..." : "刷新状态"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 运行状态：null 表示取不到（不是「未运行」） */
function StatusChip({ label, value }: { label: string; value: boolean | null }) {
  const text = value === null ? "未知" : value ? "运行中" : "未运行";
  const tone = value === null ? "muted" : value ? "ok" : "muted";
  return <span className={`setting-chip ${tone}`}>{label}：{text}</span>;
}

function PlainChip({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: "ok" | "normal" | "muted";
}) {
  return <span className={`setting-chip ${tone}`}>{label}：{value}</span>;
}
