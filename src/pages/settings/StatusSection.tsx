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
 * 展示按应用分两行：TraeCode 的「当前账号」取 `is_current`，TraeWork 的取
 * `traeworkOverview().current_slot`（current_account.txt 才是它的真源，见 §5.9）。
 */
export function StatusSection({ onToast }: StatusSectionProps) {
  const [traeRunning, setTraeRunning] = useState<boolean | null>(null);
  const [traeworkRunning, setTraeworkRunning] = useState<boolean | null>(null);
  const [tcAccount, setTcAccount] = useState<string | null>(null);
  const [tcCheckin, setTcCheckin] = useState<{ done: number; total: number } | null>(null);
  const [twAccount, setTwAccount] = useState<string | null>(null);
  const [twCheckin, setTwCheckin] = useState<{ done: number; total: number } | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [status, accounts, overview] = await Promise.all([
        api.getRuntimeStatus(),
        api.getAccounts().catch(() => [] as AccountBrief[]),
        // TraeWork 概览失败（如未配置安装路径）只影响「当前账号」一格，不应拖垮整面板
        api.traeworkOverview().catch(() => null),
      ]);
      setTraeRunning(status.trae_running);
      setTraeworkRunning(status.traework_running);

      const traecode = accounts.filter((a) => a.app !== "traework");
      const current = traecode.find((a) => a.is_current);
      setTcAccount(current ? (current.email || current.name) : null);
      setTcCheckin({
        done: traecode.filter((a) => a.checked_in_today).length,
        total: traecode.length,
      });

      const traework = accounts.filter((a) => a.app === "traework");
      // TraeWork 的「当前账号」真源是 current_account.txt（槽位 = uid），不是 is_current；
      // 映射回账号名显示，匹配不到时退回原始 uid，与 TraeWork 面板口径一致
      const slot = overview?.current_slot ?? null;
      if (!slot) {
        setTwAccount(null);
      } else {
        const matched = traework.find((a) => (a.uid ?? a.name) === slot);
        setTwAccount(matched ? (matched.name || matched.email || slot) : slot);
      }
      setTwCheckin({
        done: traework.filter((a) => a.checked_in_today).length,
        total: traework.length,
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
          <div style={{ marginTop: '10px', display: 'flex', flexDirection: 'column', gap: '8px' }}>
            <div className="status-group-row">
              <span className="status-group-label">TraeWork</span>
              <StatusChip label="状态" value={traeworkRunning} />
              <PlainChip
                label="当前账号"
                value={twAccount ?? "未设置"}
                tone={twAccount ? "normal" : "muted"}
              />
              <PlainChip
                label="今日签到"
                value={twCheckin ? `${twCheckin.done} / ${twCheckin.total}` : "—"}
                tone={twCheckin && twCheckin.total > 0 && twCheckin.done === twCheckin.total ? "ok" : "muted"}
              />
            </div>
            <div className="status-group-row">
              <span className="status-group-label">TraeCode</span>
              <StatusChip label="状态" value={traeRunning} />
              <PlainChip
                label="当前账号"
                value={tcAccount ?? "未设置"}
                tone={tcAccount ? "normal" : "muted"}
              />
              <PlainChip
                label="今日签到"
                value={tcCheckin ? `${tcCheckin.done} / ${tcCheckin.total}` : "—"}
                tone={tcCheckin && tcCheckin.total > 0 && tcCheckin.done === tcCheckin.total ? "ok" : "muted"}
              />
            </div>
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
