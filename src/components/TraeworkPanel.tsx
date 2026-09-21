import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import type {
  AccountBrief,
  CheckinResult,
  TraeworkActionResult,
  TraeworkOverview,
  TraeworkSlotStatus,
  TraeworkStep,
  TraeworkUidEvidence,
  UsageSummary,
} from "../types";
import { describeCooldown, summarizeCheckin } from "../utils/checkinDisplay";
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

/** 阈值：凭据剩余不足这个天数就提前警示（用户仍来得及切一次并重新保存） */
const CREDENTIAL_WARN_DAYS = 30;

/**
 * 设备标识的短形（与后端 `device::short` 同口径：前 8 位）。
 *
 * 为什么只给短形：完整 UUID 有 36 字符，塞进列表会把「槽位 / 快照时间 / 凭据期限」这些
 * 更常看的信息挤走；而判断「是否撞号」只需前 8 位就足够——撞号时值完全相同，前 8 位必然相同。
 */
function shortId(value: string | null | undefined): string {
  if (!value) return "未知";
  return value.length > 8 ? `${value.slice(0, 8)}…` : value;
}

/**
 * 快照凭据状态：这张卡还能免登录多久。
 *
 * 为什么按 refresh 而不是 access 判定「失效」：槽位里两份凭据的寿命差一个量级——access 只活
 * 14 天，但它过期只是让客户端在启动时静默续期一次；真正决定「这个槽位必须重新登录」的是
 * refresh（客户端记录的期限是签发后 180 天）。只盯 access 会把仍可用的槽位误标成失效。
 */
function credentialInfo(snap: TraeworkSlotStatus | undefined): {
  level: "ok" | "warn" | "danger";
  /** 标题行上的警示标签（正常状态为 null，避免每张卡都挂满徽标） */
  tag: string | null;
  /** meta 行上的说明文字 */
  text: string;
  /** 悬停提示：access 的到期时间只影响「切过去要不要静默续期」，放在这里不占版面 */
  title: string;
} | null {
  const refresh = snap?.refresh_expired_at;
  if (!refresh) return null;
  const ms = Date.parse(refresh);
  if (Number.isNaN(ms)) return null;

  const day = refresh.slice(0, 10);
  const access = snap?.expired_at;
  const title = access ? `access token 到期 ${access}` : "";
  const days = Math.ceil((ms - Date.now()) / 86400000);
  if (days <= 0) {
    return { level: "danger", tag: "凭据已失效", text: `凭据 ${day} 已过期，需重新登录该账号再保存一次`, title };
  }
  if (days <= CREDENTIAL_WARN_DAYS) {
    return { level: "warn", tag: `凭据 ${days} 天后失效`, text: `凭据续期至 ${day}`, title };
  }
  return { level: "ok", tag: null, text: `凭据续期至 ${day}`, title };
}

/** 单个账号的积分查询结果：成功给 summary，失败给 error（渲染成「积分 未知」+ 悬停原因） */
interface CreditLookup {
  summary?: UsageSummary;
  error?: string;
}

/**
 * 积分标签的悬停说明。
 *
 * 为什么分类明细只放 title 不铺在列表里：CN 版积分分「通用」与「Work 专属」两类，四个数字
 * 并排会把账号行变成一串读不懂的数；而扫列表时只需要「还剩多少 / 一共多少」。
 */
function creditsTitle(s: UsageSummary): string {
  const lines = [`积分余额：剩余 ${Math.round(s.credits_left)} / 共 ${Math.round(s.credits_total)}`];
  if (s.credits_general_total > 0) {
    lines.push(
      `通用积分：剩余 ${Math.round(s.credits_general_left)} / 共 ${Math.round(s.credits_general_total)}`,
    );
  }
  if (s.credits_work_total > 0) {
    lines.push(
      `Work 专属：剩余 ${Math.round(s.credits_work_left)} / 共 ${Math.round(s.credits_work_total)}`,
    );
  }
  return lines.join("\n");
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
  /** 各账号积分余额（按 account.id 索引） */
  const [credits, setCredits] = useState<Record<string, CreditLookup>>({});
  /** 正在签到的目标（账号 id 或 "all"）：签到是秒级网络请求，不进 runAction 的长遮罩，
   *  只需按钮级防连点 + 与其它动作互斥（共用 inFlight 闸门） */
  const [checkinBusy, setCheckinBusy] = useState<string | null>(null);
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

  /**
   * 拉取各账号的积分余额。
   *
   * 为什么每个账号各自 catch：一个账号凭据过期或快照缺失，不该让整列积分一起变成空白；
   * 失败项渲染成「积分 未知」，原因放悬停。
   * 为什么跟账号列表走而不是缓存住：积分是「现读磁盘 + 联网」得来的，缓存会表现为
   * 「刚保存完登录态，积分还是上一轮的」。
   */
  const loadCredits = useCallback(async () => {
    const ids = traeworkAccounts.map((a) => a.id);
    if (ids.length === 0) {
      setCredits({});
      return;
    }
    const next: Record<string, CreditLookup> = {};
    await Promise.all(
      ids.map(async (id) => {
        try {
          next[id] = { summary: await api.traeworkCredits(id) };
        } catch (err: any) {
          next[id] = { error: err?.message || "积分查询失败" };
        }
      }),
    );
    setCredits(next);
  }, [traeworkAccounts]);

  useEffect(() => {
    void loadCredits();
  }, [loadCredits]);

  /** 顶栏「刷新」：概览与积分两个数据源互不依赖，并行走 */
  const handleRefresh = useCallback(() => {
    void refresh();
    void loadCredits();
  }, [refresh, loadCredits]);

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
    async (
      label: string,
      action: () => Promise<TraeworkActionResult>,
      // 切换的返回文案带「建议重新保存登录态」的提醒，比其余动作长，需要更长的展示时间
      successDuration = 3000,
    ) => {
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
        onToast?.("success", result.message, successDuration);
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
      runAction(
        `切换到 ${account.uid ?? account.name}`,
        () => api.traeworkSwitchAccount(account.id),
        9000,
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

  /**
   * 移除账号（记录 + 快照一起删）。
   *
   * 为什么需要它：TraeWork 账号在账号管理页被过滤掉，本面板是它唯一的出口；而「删除快照」
   * 只删磁盘文件，账号记录会留在列表里变成「无快照」条目——用户找不到任何入口清掉它。
   */
  const handleRemoveAccount = useCallback(
    (account: AccountBrief) => {
      const slot = account.uid ?? account.name;
      if (
        !window.confirm(
          `确定移除账号 ${slot} 吗？账号记录与磁盘上的快照（含回退代）都会被删除，此操作无法撤销。\n\n` +
            `注：TraeWork 侧的登录状态不受影响，账号本身也不会被注销；如需再次使用，重新登录一次并「保存当前登录态」即可。`,
        )
      ) {
        return;
      }
      runAction(`移除 ${slot}`, () => api.traeworkRemoveAccount(account.id), 6000);
    },
    [runAction],
  );

  /** 单账号签到结果 → toast（ok/already 成功、skipped/cooldown info、failed/rate_limited warning） */
  const reportCheckinResult = useCallback(
    (result: CheckinResult) => {
      switch (result.state) {
        case "ok":
          onToast?.("success", result.detail);
          break;
        case "already":
          onToast?.("info", `${result.account_name} 今日已签到`);
          break;
        case "skipped":
          // 无可达凭据是有理由的跳过：info 呈现后端给的自救指引（切换 + 重新保存）
          onToast?.("info", `${result.account_name}：${result.detail}`, 6000);
          break;
        case "cooldown":
          onToast?.("info", `${result.account_name}：${describeCooldown(result)}`, 4000);
          break;
        default:
          onToast?.("warning", `${result.account_name}：${result.detail}`);
      }
    },
    [onToast],
  );

  /**
   * 单账号签到（面板卡片按钮）。
   *
   * 为什么不复用 runAction：它假定返回 TraeworkActionResult 并把「进行中」长遮罩挂上，
   * 而签到返回 CheckinResult 且是秒级请求，挂遮罩反而像卡死。共用 inFlight 闸门保证
   * 签到与切换/保存互斥——签到要读现场 storage.json，切换会整组覆盖它。
   */
  const handleCheckinAccount = useCallback(
    async (account: AccountBrief) => {
      if (inFlight.current) {
        onToast?.("warning", "已有 TraeWork 操作进行中，请等待其结束");
        return;
      }
      inFlight.current = true;
      setCheckinBusy(account.id);
      try {
        reportCheckinResult(await api.checkinAccount(account.id));
      } catch (err: any) {
        const message: string = err?.message || "签到失败";
        onToast?.(message.includes("签到进行中") ? "info" : "error", message, 6000);
      } finally {
        inFlight.current = false;
        setCheckinBusy(null);
        // 刷新账号列表，「今日已签」徽标即时到位
        await onAccountsChanged();
      }
    },
    [onToast, reportCheckinResult, onAccountsChanged],
  );

  /** 全部 TraeWork 账号签到（面板按钮）：逐状态计数汇总，skipped 单独计数不计入失败 */
  const handleCheckinAll = useCallback(async () => {
    if (inFlight.current) {
      onToast?.("warning", "已有 TraeWork 操作进行中，请等待其结束");
      return;
    }
    if (traeworkAccounts.length === 0) return;
    inFlight.current = true;
    setCheckinBusy("all");
    try {
      const results = await api.traeworkCheckinAll();
      const summary = summarizeCheckin(results);
      onToast?.(summary.level, summary.text, 5000);
    } catch (err: any) {
      const message: string = err?.message || "批量签到失败";
      onToast?.(message.includes("签到进行中") ? "info" : "error", message, 6000);
    } finally {
      inFlight.current = false;
      setCheckinBusy(null);
      await onAccountsChanged();
    }
  }, [onToast, traeworkAccounts, onAccountsChanged]);

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
        // `traework_scan_path` 只返回路径、不落盘（与 TraeCode 的 scan 同款），
        // 不显式保存的话这次扫描在重启后消失，而设置页那边会一直显示「未设置」
        await api.traeworkSetPath(found);
        onToast?.("success", `已找到并保存 TraeWork：${found}`);
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

  // 当前账号把 uid 映射成可读名，避免这一行只给一串数字
  const currentSlot = overview?.current_slot ?? null;
  const currentAccount = currentSlot
    ? traeworkAccounts.find((a) => (a.uid ?? a.name) === currentSlot)
    : undefined;
  const currentLabel = !currentSlot
    ? "未记录"
    : currentAccount && currentAccount.name && currentAccount.name !== currentSlot
      ? `${currentAccount.name}（${currentSlot}）`
      : currentSlot;

  // 注册表值与现场值不同。两侧任一为空时**不判为不同**：读注册表失败（非管理员/组策略）
  // 时值为 null，此时报「不同」是假告警，会把真问题淹掉
  const registryOutOfSync =
    !!overview?.registry_machine_id &&
    !!overview?.live_machine_id &&
    overview.registry_machine_id !== overview.live_machine_id;

  const busy = busyLabel !== null;

  return (
    <main className="app-main traework-panel">
      <header className="traework-header">
        <div>
          <h2 className="page-title">TraeWork 账号</h2>
          <p className="traework-subtitle">
            TRAE SOLO CN 的登录态与 TraeCode 相互独立，切换采用「快照 / 恢复」，需要关闭客户端，
            单次约 10–30 秒。
          </p>
        </div>
        <button type="button" className="header-btn" onClick={handleRefresh} disabled={busy || loading}>
          刷新
        </button>
      </header>

      <section className="traework-card">
        <div className="traework-row">
          <span className="traework-label">客户端</span>
          <span className={overview?.running ? "card-status normal" : "tag plan"}>
            {overview?.running ? "运行中" : "未运行"}
          </span>
        </div>
        <div className="traework-row">
          <span className="traework-label">当前账号</span>
          <span className="traework-value">{currentLabel}</span>
        </div>
        <div className="traework-row">
          <span className="traework-label">安装路径</span>
          <span className="traework-value mono" title={overview?.exe_path ?? ""}>
            {overview?.exe_path ?? "未设置（自定义安装需手动指定）"}
          </span>
          <button type="button" className="header-btn" onClick={handleScanPath} disabled={busy}>
            自动扫描
          </button>
          <button type="button" className="header-btn" onClick={handlePickPath} disabled={busy}>
            手动指定
          </button>
        </div>
        <div className="traework-row">
          <span className="traework-label">识别结果</span>
          <span className={discover?.confident ? "traework-value" : "traework-value warn"}>
            {discover?.reason ?? "读取中…"}
          </span>
        </div>
        {/* 设备标识对账：注册表 / 现场 / 各槽位是三个不同的层，「隔离有没有生效」只能三处对照着看 */}
        <div className="traework-row">
          <span className="traework-label">注册表设备标识</span>
          <span
            className="traework-value mono"
            title="HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid。切换 TraeWork 账号时会同步为目标账号快照里的 machineid；写入 HKLM 需要管理员权限"
          >
            {shortId(overview?.registry_machine_id)}
          </span>
          {registryOutOfSync && (
            <span className="card-status warn" title="注册表值与现场值不同：注册表由「切换账号」写入，两者不同说明切换到当前账号后现场值又变过（或从未切换过）">
              与现场不同
            </span>
          )}
        </div>
        <div className="traework-row">
          <span className="traework-label">现场设备标识</span>
          <span
            className="traework-value mono"
            title="TraeWork 数据目录下 machineid 文件的当前内容，即客户端这次启动实际在用的设备标识"
          >
            {shortId(overview?.live_machine_id)}
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
            className="header-btn"
            onClick={handleCheckinAll}
            disabled={busy || loading || checkinBusy !== null || traeworkAccounts.length === 0}
            title="为全部 TraeWork 账号执行每日签到；无可达凭据的账号会跳过并提示"
          >
            {checkinBusy === "all" ? "签到中…" : "全部签到"}
          </button>
          <button
            type="button"
            className="header-btn"
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
        <p className="traework-hint">
          列表里的「删除快照」只清磁盘、账号保留（之后可重新登录再存一次）；想让账号从这个列表
          里彻底消失，用「移除账号」。卡片上的「凭据续期至」是该槽位还能免登录的期限，快到期时
          会有警示——切过去用一次并重新「保存当前登录态」即可续上。
        </p>
        <p className="traework-hint">
          每个 TraeWork 账号在 Trae 侧应表现为一台独立设备。卡片上出现「设备标识共用」时，说明两个
          账号当前的设备标识相同（容易被服务端归并为同一台设备）；切到该账号并重新点「保存当前
          登录态」会自动生成独立标识，同时同步到上方的「注册表设备标识」那一层。
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
          // 名字与 uid 不同才说明已回填过真实用户名；相同则直接展示 uid（等价于旧行为）
          const displayName = account.name && account.name !== slot ? account.name : slot;
          const isCurrent = overview?.current_slot === slot;
          const cred = credentialInfo(snap);
          // 积分（与上面的 cred「凭据状态」是两回事，勿混）
          const balance = credits[account.id];
          return (
            <div className="traework-item" key={account.id}>
              <div className="traework-item-main">
                <div className="traework-item-title">
                  <span className="traework-name">{displayName}</span>
                  {/* 徽标用设计层 .card-status 规格（阶段 5 统一后的目标形态），
                      数据源是后端按本地日期判定的 checked_in_today，前端不做时区换算 */}
                  {account.checked_in_today && <span className="card-status normal">今日已签</span>}
                  {balance?.summary && balance.summary.credits_total > 0 && (
                    <span className="tag plan credits" title={creditsTitle(balance.summary)}>
                      积分 {Math.round(balance.summary.credits_left)} /{" "}
                      {Math.round(balance.summary.credits_total)}
                    </span>
                  )}
                  {balance?.error && (
                    <span className="card-status warn" title={balance.error}>
                      积分 未知
                    </span>
                  )}
                  {isCurrent && <span className="card-status normal">当前</span>}
                  {snap?.exists ? (
                    <span className="tag plan">{formatBytes(snap.bytes)}</span>
                  ) : (
                    <span className="card-status expired">无快照</span>
                  )}
                  {snap?.bak_exists && <span className="tag plan">可回退代</span>}
                  {/* 凭据警示：warn=临近失效（状态面），danger=已失效（错误面，同 .expired） */}
                  {cred?.tag && (
                    <span className={cred.level === "warn" ? "card-status warn" : "card-status expired"}>
                      {cred.tag}
                    </span>
                  )}
                  {/* 设备标识撞号：这些账号在 Trae 看来是同一台设备，正是设备维度限制的成因 */}
                  {snap && snap.shares_device_with.length > 0 && (
                    <span
                      className="card-status warn"
                      title={`与 ${snap.shares_device_with.join("、")} 共用同一设备标识。切到该账号后重新「保存当前登录态」即可自动生成独立标识。`}
                    >
                      设备标识共用
                    </span>
                  )}
                </div>
                <div className="traework-item-meta" title={cred?.title || undefined}>
                  <span className="mono">{slot}</span> · 快照时间 {formatTime(snap?.modified_at ?? null)}
                  {snap?.machine_id && <> · 设备 {shortId(snap.machine_id)}</>}
                  {cred && <> · {cred.text}</>}
                </div>
              </div>
              <div className="traework-item-actions">
                <button
                  type="button"
                  className="header-btn"
                  onClick={() => handleCheckinAccount(account)}
                  disabled={busy || checkinBusy !== null}
                  title="每日签到领取积分；凭据不可达时会跳过并提示"
                >
                  {checkinBusy === account.id ? "签到中…" : "签到"}
                </button>
                <button
                  type="button"
                  className="header-btn"
                  onClick={() => handleSwitch(account)}
                  disabled={busy || checkinBusy !== null || isCurrent || !snap?.exists}
                  title={isCurrent ? "已经是当前账号" : undefined}
                >
                  切换
                </button>
                <button
                  type="button"
                  className="header-btn danger"
                  onClick={() => handleDeleteSnapshot(account)}
                  disabled={busy || !snap?.exists}
                  title="只删除磁盘上的快照，账号仍留在列表里（之后可重新登录再保存）"
                >
                  删除快照
                </button>
                <button
                  type="button"
                  className="header-btn danger"
                  onClick={() => handleRemoveAccount(account)}
                  disabled={busy}
                  title="删除账号记录及其快照，账号不再出现在列表里"
                >
                  移除账号
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
