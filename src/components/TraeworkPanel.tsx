import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import type {
  AccountBrief,
  TraeworkActionResult,
  TraeworkOverview,
  TraeworkStep,
  TraeworkUidEvidence,
} from "../types";
import "./TraeworkPanel.css";

interface TraeworkPanelProps {
  /** 全量账号（本组件自行筛出 traework，避免上层再维护一份派生状态） */
  accounts: AccountBrief[];
  onToast?: (type: "success" | "error" | "warning" | "info", message: string, duration?: number) => void;
  /** 保存登录态会新登记账号，需通知上层重新拉账号列表 */
  onAccountsChanged: () => void | Promise<void>;
}

/** 字节数展示（快照体积是用户唯一能感知「磁盘代价」的量） */
function formatBytes(bytes: number): string {
  if (!bytes) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatTime(sec: number | null): string {
  if (!sec) return "—";
  const d = new Date(sec * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/**
 * TraeWork（TRAE SOLO CN）账号面板。
 *
 * 为什么单独成面板而不并入账号列表：TraeWork 的切换是「关客户端 → 整组快照覆盖 → 再启动」，
 * 单次耗时数十秒且会打断用户正在用的窗口；与 TraeCode 的「秒切」混在同一张卡片上，用户
 * 会按同一预期去连点。独立的页面 + 明确的进行中遮罩是防误操作的组成部分，不是界面洁癖。
 */
export function TraeworkPanel({ accounts, onToast, onAccountsChanged }: TraeworkPanelProps) {
  const [overview, setOverview] = useState<TraeworkOverview | null>(null);
  const [discover, setDiscover] = useState<TraeworkUidEvidence | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [steps, setSteps] = useState<TraeworkStep[]>([]);
  const [reconciling, setReconciling] = useState(false);
  const busyStartedAt = useRef<number>(0);
  // 用 ref 做闸门而不是只看 state：同一个渲染帧内连点两次时 state 还没更新，
  // 仅靠 busyLabel 判空会两次都放行（这正是要防的重复杀进程场景）
  const inFlight = useRef(false);

  const traeworkAccounts = useMemo(
    () => accounts.filter((a) => a.app === "traework"),
    [accounts],
  );

  const refresh = useCallback(async () => {
    try {
      const [ov, dc] = await Promise.all([api.traeworkOverview(), api.traeworkDiscover()]);
      setOverview(ov);
      setDiscover(dc);
    } catch (err: any) {
      onToast?.("error", err?.message || "读取 TraeWork 状态失败");
    } finally {
      setLoading(false);
    }
  }, [onToast]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // 进行中计时：动作会阻塞数十秒，没有计时用户无法区分「在跑」和「卡死」
  useEffect(() => {
    if (!busyLabel) {
      setElapsed(0);
      return;
    }
    const timer = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - busyStartedAt.current) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [busyLabel]);

  /** 统一的动作包装：闸门 + 结果落地 + 错误上报 */
  const runAction = useCallback(
    async (label: string, action: () => Promise<TraeworkActionResult>) => {
      if (inFlight.current) {
        onToast?.("warning", "已有 TraeWork 操作进行中，请等待其结束");
        return;
      }
      inFlight.current = true;
      busyStartedAt.current = Date.now();
      setBusyLabel(label);
      setSteps([]);
      try {
        const result = await action();
        setSteps(result.steps);
        onToast?.("success", result.message, 3000);
      } catch (err: any) {
        onToast?.("error", err?.message || `${label}失败`, 6000);
      } finally {
        inFlight.current = false;
        setBusyLabel(null);
        await refresh();
        await onAccountsChanged();
      }
    },
    [onToast, refresh, onAccountsChanged],
  );

  const handleSave = useCallback(() => {
    if (!discover) return;
    if (!discover.confident) {
      onToast?.("error", discover.reason, 6000);
      return;
    }
    runAction("保存当前登录态", () => api.traeworkSaveCurrentLogin(discover.uid ?? undefined));
  }, [discover, onToast, runAction]);

  const handleSwitch = useCallback(
    (account: AccountBrief) => {
      runAction(`切换到 ${account.uid ?? account.name}`, () =>
        api.traeworkSwitchAccount(account.id),
      );
    },
    [runAction],
  );

  const handleDeleteSnapshot = useCallback(
    (account: AccountBrief) => {
      const slot = account.uid ?? account.name;
      if (!window.confirm(`确定删除账号 ${slot} 的快照吗？删除后需重新登录该账号再保存一次。`)) {
        return;
      }
      runAction(`删除 ${slot} 的快照`, async () => {
        const freed = await api.traeworkDeleteSnapshot(account.id);
        return {
          message: `已删除快照，释放 ${formatBytes(freed)}`,
          steps: [],
          account: null,
        };
      });
    },
    [runAction],
  );

  /** 修复槽位：不改代码不杀进程，纯目录归位，故不走 runAction 的遮罩流程 */
  const handleReconcile = useCallback(async () => {
    if (
      !window.confirm(
        "将扫描快照目录：① 把「内容与目录名不符」的快照按其中记录的真实账号改名归位；② 登记缺失的账号；③ 清理快照内不属于白名单的历史缓存（例如白名单收窄前存下的数百 MB Cache，删掉不会影响任何恢复结果）。是否继续？",
      )
    ) {
      return;
    }
    setReconciling(true);
    try {
      const result = await api.traeworkReconcile();
      setSteps(result.steps);
      onToast?.("success", result.message, 4000);
      // 未自动处理的项必须显式告知：通常意味着两个槽位同名冲突，需要用户手工决定
      if (result.skipped.length > 0) {
        onToast?.("warning", `有 ${result.skipped.length} 项未自动处理：${result.skipped.join("；")}`, 9000);
      }
    } catch (err: any) {
      onToast?.("error", err?.message || "修复槽位失败", 6000);
    } finally {
      setReconciling(false);
      await refresh();
      await onAccountsChanged();
    }
  }, [onToast, refresh, onAccountsChanged]);

  const handleScanPath = useCallback(async () => {
    try {
      const found = await api.traeworkScanPath();
      if (found) {
        onToast?.("success", `已找到 TraeWork：${found}`);
        await refresh();
      } else {
        onToast?.("warning", "未自动找到 TraeWork，请手动指定 TRAE SOLO CN.exe");
      }
    } catch (err: any) {
      onToast?.("error", err?.message || "扫描失败");
    }
  }, [onToast, refresh]);

  const handlePickPath = useCallback(async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "TraeWork", extensions: ["exe"] }],
      });
      if (!selected) return;
      const path = selected as string;
      await api.traeworkSetPath(path);
      onToast?.("success", "TraeWork 路径已保存");
      await refresh();
    } catch (err: any) {
      onToast?.("error", err?.message || "设置路径失败");
    }
  }, [onToast, refresh]);

  const snapshotOf = (account: AccountBrief) => {
    const slot = account.uid ?? account.name;
    return overview?.snapshots.find((s) => s.slot === slot);
  };

  const busy = busyLabel !== null;

  return (
    <main className="app-main traework-panel">
      <header className="traework-header">
        <div>
          <h2>TraeWork 账号</h2>
          <p className="traework-subtitle">
            TRAE SOLO CN 的登录态与 TraeCode 相互独立，切换采用「快照 / 恢复」，需要关闭客户端，
            单次约 10–30 秒。
          </p>
        </div>
        <button type="button" className="header-btn" onClick={refresh} disabled={busy || loading}>
          刷新
        </button>
      </header>

      <section className="traework-card">
        <div className="traework-row">
          <span className="traework-label">客户端</span>
          <span className={overview?.running ? "traework-tag ok" : "traework-tag"}>
            {overview?.running ? "运行中" : "未运行"}
          </span>
        </div>
        <div className="traework-row">
          <span className="traework-label">当前账号</span>
          <span className="traework-value">{overview?.current_slot ?? "未记录"}</span>
        </div>
        <div className="traework-row">
          <span className="traework-label">安装路径</span>
          <span className="traework-value mono" title={overview?.exe_path ?? ""}>
            {overview?.exe_path ?? "未设置（自定义安装需手动指定）"}
          </span>
          <button type="button" className="traework-mini-btn" onClick={handleScanPath} disabled={busy}>
            自动扫描
          </button>
          <button type="button" className="traework-mini-btn" onClick={handlePickPath} disabled={busy}>
            手动指定
          </button>
        </div>
        <div className="traework-row">
          <span className="traework-label">识别结果</span>
          <span className={discover?.confident ? "traework-value" : "traework-value warn"}>
            {discover?.reason ?? "读取中…"}
          </span>
        </div>
        <div className="traework-actions">
          <button
            type="button"
            className="traework-primary"
            onClick={handleSave}
            disabled={busy || loading || !discover?.confident}
          >
            {busy ? "执行中…" : "保存当前登录态"}
          </button>
          <button
            type="button"
            className="traework-mini-btn"
            onClick={handleReconcile}
            disabled={busy || reconciling}
            title="把内容与目录名不符的快照按其中记录的真实账号改名归位，并登记缺失的账号"
          >
            {reconciling ? "修复中…" : "修复槽位"}
          </button>
        </div>
        <p className="traework-hint">
          先在 TraeWork 里登录目标账号，再点「保存当前登录态」；换账号时用列表里的「切换」。
          若快照与槽名不符（切换被拒绝时提示），点「修复槽位」归位。
        </p>
      </section>

      <section className="traework-list">
        <h3>已保存的账号（{traeworkAccounts.length}）</h3>
        {traeworkAccounts.length === 0 && (
          <p className="traework-empty">还没有 TraeWork 账号，先用「保存当前登录态」登记一个。</p>
        )}
        {traeworkAccounts.map((account) => {
          const snap = snapshotOf(account);
          const slot = account.uid ?? account.name;
          const isCurrent = overview?.current_slot === slot;
          return (
            <div className="traework-item" key={account.id}>
              <div className="traework-item-main">
                <div className="traework-item-title">
                  <span className="mono">{slot}</span>
                  {isCurrent && <span className="traework-tag ok">当前</span>}
                  {snap?.exists ? (
                    <span className="traework-tag">{formatBytes(snap.bytes)}</span>
                  ) : (
                    <span className="traework-tag danger">无快照</span>
                  )}
                  {snap?.bak_exists && <span className="traework-tag">可回退代</span>}
                </div>
                <div className="traework-item-meta">
                  快照时间 {formatTime(snap?.modified_at ?? null)}
                </div>
              </div>
              <div className="traework-item-actions">
                <button
                  type="button"
                  className="traework-mini-btn"
                  onClick={() => handleSwitch(account)}
                  disabled={busy || isCurrent || !snap?.exists}
                  title={isCurrent ? "已经是当前账号" : undefined}
                >
                  切换
                </button>
                <button
                  type="button"
                  className="traework-mini-btn danger"
                  onClick={() => handleDeleteSnapshot(account)}
                  disabled={busy || !snap?.exists}
                >
                  删除快照
                </button>
              </div>
            </div>
          );
        })}
        {overview && overview.orphan_slots.length > 0 && (
          <p className="traework-hint">
            磁盘上还有 {overview.orphan_slots.length} 个未登记账号的快照：{overview.orphan_slots.join("、")}
          </p>
        )}
      </section>

      {steps.length > 0 && (
        <section className="traework-steps">
          <h3>上次执行过程</h3>
          {steps.map((s, i) => (
            <div className={`traework-step ${s.status}`} key={`${s.stage}-${i}`}>
              <span className="traework-step-stage">{s.stage}</span>
              <span>{s.message}</span>
            </div>
          ))}
        </section>
      )}

      {busy && (
        <div className="traework-overlay">
          <div className="traework-overlay-card">
            <div className="traework-spinner" />
            <div className="traework-overlay-title">{busyLabel}</div>
            <div className="traework-overlay-timer">已耗时 {elapsed} 秒</div>
            <p className="traework-hint">正在关闭并重启 TraeWork，请不要手动打开客户端，也不要重复点击。</p>
          </div>
        </div>
      )}
    </main>
  );
}
