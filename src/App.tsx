import { useCallback, useEffect, useRef, useState } from "react";
import { save, open } from "@tauri-apps/plugin-dialog";
import { Sidebar } from "./components/Sidebar";
import { AccountCard } from "./components/AccountCard";
import { AccountListItem } from "./components/AccountListItem";
import { AddAccountModal } from "./components/AddAccountModal";
import { ContextMenu } from "./components/ContextMenu";
import { DetailModal } from "./components/DetailModal";
import { AccountLoginModal } from "./components/AccountLoginModal";
import { Toast, ToastMessage } from "./components/Toast";
import { ConfirmModal } from "./components/ConfirmModal";
import { Stats } from "./pages/Stats";
import { Settings } from "./pages/settings/Settings";
import { About } from "./pages/About";
import { TraeworkPanel } from "./components/TraeworkPanel";

import * as api from "./api";
import type { Account, AccountBrief, AppSettings, UsageSummary } from "./types";
import { describeCooldown, summarizeCheckin } from "./utils/checkinDisplay";
import "./App.css";

interface AccountWithUsage extends AccountBrief {
  usage?: UsageSummary | null;
  password?: string | null;
  /** 列表 brief 不含用户 ID，打开详情时用完整账号补齐 */
  user_id?: string;
}

type ViewMode = "grid" | "list";
const USAGE_CACHE_KEY = "trae_usage_cache_v1";

/**
 * 是否为 TraeWork 账号。
 *
 * 为什么按「等于 traework」而不是「不等于 traecode」判定：旧版 accounts.json 没有 `app`
 * 字段，加载后由后端 `serde(default)` 补成 "traecode"，但前端在类型层面仍可能是 undefined。
 * 用「等于 traework」可以在字段缺失时安全地判为 traecode，不会把老账号误锁在 TraeWork 面板外。
 */
function isTraeworkAccount(account: { app?: string }): boolean {
  return account.app === "traework";
}

function App() {
  const [accounts, setAccounts] = useState<AccountWithUsage[]>([]);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [showAddModal, setShowAddModal] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [hasLoaded, setHasLoaded] = useState(false);
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [currentPage, setCurrentPage] = useState("accounts");
  const [viewMode, setViewMode] = useState<ViewMode>("grid");
  const [quotaFilter, setQuotaFilter] = useState<"all" | "with" | "without">("all");

  // Toast 通知状态
  const [toasts, setToasts] = useState<ToastMessage[]>([]);

  // 确认弹窗状态
  const [confirmModal, setConfirmModal] = useState<{
    isOpen: boolean;
    title: string;
    message: string;
    type: "danger" | "warning" | "info";
    confirmText?: string;
    cancelText?: string;
    onConfirm: () => void;
  } | null>(null);

  // 右键菜单状态
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    accountId: string;
  } | null>(null);

  // 详情弹窗状态
  const [detailAccount, setDetailAccount] = useState<AccountWithUsage | null>(null);

  // 刷新中的账号 ID
  const [refreshingIds, setRefreshingIds] = useState<Set<string>>(new Set());

  // 重新登录弹窗状态
  const [loginModal, setLoginModal] = useState<{
    accountId: string;
    accountName: string;
    initialEmail?: string;
  } | null>(null);

  const toastDedupRef = useRef<Map<string, number>>(new Map());
  const autoCheckinRanRef = useRef(false);

  // 账号切换进行中标志：定时刷新必须避开这段窗口——切换是全仓唯一「持锁做网络请求」
  // 的路径（AGENTS §5.2），定时器此时插进来只会让两边互相拖慢。
  const switchInProgressRef = useRef(false);
  // 定时器回调要用最新账号列表，但列表变化不该重建定时器（重建会重置计时），故用 ref 传递
  const traecodeAccountsRef = useRef<AccountWithUsage[]>([]);

  // 网络状态监听
  const offlineToastIdRef = useRef<string | null>(null);



  // 添加 Toast 通知
  const addToast = useCallback(
    (type: ToastMessage["type"], message: string, duration?: number, dedupeKey?: string) => {
      if (dedupeKey) {
        const now = Date.now();
        const last = toastDedupRef.current.get(dedupeKey);
        if (last && now - last < 800) {
          return;
        }
        toastDedupRef.current.set(dedupeKey, now);
      }
      const id =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
      setToasts((prev) => [...prev, { id, type, message, duration }]);
    },
    []
  );

  // 移除 Toast 通知
  const removeToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  useEffect(() => {
    const handleOffline = () => {
      const id = "network-offline";
      setToasts((prev) => {
        // 防止重复添加
        if (prev.some((t) => t.id === id)) return prev;
        return [...prev, { id, type: "error", message: "网络连接已断开，请检查网络设置", duration: 0 }];
      });
      offlineToastIdRef.current = id;
    };

    const handleOnline = () => {
      if (offlineToastIdRef.current) {
        removeToast(offlineToastIdRef.current);
        offlineToastIdRef.current = null;
      }
      addToast("success", "网络已重新连接", 3000);
    };

    // 初始化检查
    if (!navigator.onLine) {
      handleOffline();
    }

    window.addEventListener("offline", handleOffline);
    window.addEventListener("online", handleOnline);

    return () => {
      window.removeEventListener("offline", handleOffline);
      window.removeEventListener("online", handleOnline);
    };
  }, [addToast, removeToast]);

  const readUsageCache = useCallback((): Record<string, UsageSummary> => {
    try {
      const raw = localStorage.getItem(USAGE_CACHE_KEY);
      if (!raw) return {};
      const parsed = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object") return {};
      return parsed as Record<string, UsageSummary>;
    } catch {
      return {};
    }
  }, []);

  const updateUsageCache = useCallback(
    (updates: Record<string, UsageSummary>, accountIds?: string[]) => {
      const cache = readUsageCache();
      Object.entries(updates).forEach(([id, usage]) => {
        cache[id] = usage;
      });
      if (accountIds) {
        Object.keys(cache).forEach((id) => {
          if (!accountIds.includes(id)) {
            delete cache[id];
          }
        });
      }
      localStorage.setItem(USAGE_CACHE_KEY, JSON.stringify(cache));
    },
    [readUsageCache]
  );

  useEffect(() => {
    let active = true;
    api.getSettings()
      .then((settings) => {
        if (active) setAppSettings(settings);
      })
      .catch(() => {
        if (active) {
          setAppSettings({
            auto_refresh_enabled: true,
            refresh_interval: 30,
            privacy_auto_enable: true,
            auto_start_enabled: false,
            auto_checkin_enabled: true,
            theme: null,
            view_mode: null,
          });
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const refreshUsageForAccounts = useCallback(
    async (list: AccountBrief[]) => {
      if (list.length === 0) return;
      const results = await Promise.allSettled(
        list.map((account) => api.getAccountUsage(account.id))
      );
      const updates: Record<string, UsageSummary> = {};
      results.forEach((result, index) => {
        if (result.status === "fulfilled") {
          updates[list[index].id] = result.value;
        }
      });
      if (Object.keys(updates).length > 0) {
        setAccounts((prev) =>
          prev.map((account) =>
            updates[account.id] ? { ...account, usage: updates[account.id] } : account
          )
        );
        updateUsageCache(updates, list.map((a) => a.id));
      } else {
        updateUsageCache({}, list.map((a) => a.id));
      }
    },
    [updateUsageCache]
  );

  // 加载账号列表
  const loadAccounts = useCallback(async () => {
    setLoading(true);
    try {
      const list = await api.getAccounts();
      const cache = readUsageCache();
      const accountsWithUsage = list.map((account) => ({
        ...account,
        usage: cache[account.id] ?? null,
      }));
      setAccounts(accountsWithUsage);
      setError(null);
      setHasLoaded(true);
      // TraeWork 账号不走这条额度链路：它的凭据在快照的 storage.json 里、要按账号现解，
      // 与 traecode 读账号库 JWT/Cookies 是两条路（面板自己调 traework_credits）。
      // 这里仍把它们留在 accounts 里供 TraeWork 面板使用，只是不参与本页的批量刷新
      const usageTargets = list.filter((a) => !isTraeworkAccount(a));
      updateUsageCache({}, usageTargets.map((a) => a.id));
      setLoading(false);
      void refreshUsageForAccounts(usageTargets);
    } catch (err: any) {
      setError(err.message || "加载账号失败");
      setHasLoaded(true);
      setLoading(false);
    }
  }, [readUsageCache, refreshUsageForAccounts, updateUsageCache]);

  // 初始加载
  useEffect(() => {
    loadAccounts();
  }, [loadAccounts]);

  // 自动签到（方案B）：启动时检查「今日是否已签」，未签的账号后台静默执行（不弹提示）
  //
  // 开关只作用于 GUI 启动：`--silent` 无头模式没有界面，签到静默失败也不影响用户，
  // 那边保持「总是尝试」的既有行为（见 handle_silent_start）。
  // 设置未加载完成时先不执行——否则会在用户已关闭开关的情况下抢跑一次。
  useEffect(() => {
    if (!hasLoaded || autoCheckinRanRef.current || accounts.length === 0) return;
    if (!appSettings || !appSettings.auto_checkin_enabled) return;
    autoCheckinRanRef.current = true;
    void (async () => {
      try {
        const results = await api.autoCheckin();
        const handledIds = results
          .filter((r) => r.state === "ok" || r.state === "already")
          .map((r) => r.account_id);
        if (handledIds.length > 0) {
          // 自动签到同样要把徽标改成「已签到」，否则启动后仍显示未签到（与实际不符）
          setAccounts((prev) =>
            prev.map((a) => (handledIds.includes(a.id) ? { ...a, checked_in_today: true } : a))
          );
          const targets = accounts.filter((a) => handledIds.includes(a.id));
          if (targets.length > 0) {
            await refreshUsageForAccounts(targets);
          }
        }
      } catch (err) {
        // 静默执行：失败只记录日志，不打扰用户
        console.warn("[auto-checkin] 自动签到失败:", err);
      }
    })();
  }, [hasLoaded, accounts, refreshUsageForAccounts, appSettings]);

  // 定时刷新用量：间隔与开关都取自设置。
  // 两个跳过条件是必要的——窗口不可见时用户看不到结果，白白打接口；
  // 切换账号期间后端持锁做网络请求（全仓唯一特例），定时器插进去只会让两边互相拖慢。
  useEffect(() => {
    const enabled = appSettings?.auto_refresh_enabled ?? false;
    const minutes = appSettings?.refresh_interval ?? 0;
    if (!enabled || minutes <= 0) return;

    const timer = window.setInterval(() => {
      if (document.visibilityState !== "visible") return;
      if (switchInProgressRef.current) return;
      void refreshUsageForAccounts(traecodeAccountsRef.current);
    }, minutes * 60 * 1000);

    return () => window.clearInterval(timer);
  }, [appSettings?.auto_refresh_enabled, appSettings?.refresh_interval, refreshUsageForAccounts]);

  // 主题变更：写进 settings.json（唯一存储）。
  // 乐观更新是因为主题是即时可见的视觉状态，等接口往返会闪一下旧主题。
  const handleThemeChange = useCallback(
    async (theme: "light" | "dark") => {
      setAppSettings((prev) => (prev ? { ...prev, theme } : prev));
      try {
        const base = appSettings ?? (await api.getSettings());
        const saved = await api.updateSettings({ ...base, theme });
        setAppSettings(saved);
      } catch (err: any) {
        addToast("error", err.message || "保存主题失败");
      }
    },
    [appSettings, addToast]
  );

  // 主题迁移：老版本只把主题存在 localStorage，settings 里该字段为 null 时把它搬进来。
  // 只在「从未设置过」时迁移，避免把用户已选的主题覆盖掉。
  useEffect(() => {
    if (!appSettings || appSettings.theme) return;
    const legacy = localStorage.getItem("trae_theme_v1");
    void handleThemeChange(legacy === "light" ? "light" : "dark");
  }, [appSettings, handleThemeChange]);

  // 视图偏好：与主题同源写进 settings.json（不走 localStorage，理由见 lib.rs 的 `view_mode` 注释）。
  // 乐观更新——切换按钮要立刻响应，等接口往返再改会有一帧「点了没反应」。
  const handleViewModeChange = useCallback(
    async (mode: ViewMode) => {
      setViewMode(mode);
      try {
        const base = appSettings ?? (await api.getSettings());
        const saved = await api.updateSettings({ ...base, view_mode: mode });
        setAppSettings(saved);
      } catch {
        // 纯展示偏好：写盘失败不弹错打断用户，代价仅是下次启动回落默认视图
      }
    },
    [appSettings]
  );

  // 回填落盘的视图偏好：settings 异步加载，首帧拿不到。
  // 用 ref 只回填一次——否则用户手动切换后，写盘返回的新 appSettings 会把本地 state 拉回旧值。
  const viewModeHydratedRef = useRef(false);
  useEffect(() => {
    if (viewModeHydratedRef.current || !appSettings) return;
    viewModeHydratedRef.current = true;
    if (appSettings.view_mode === "list") setViewMode("list");
  }, [appSettings]);

  // 删除账号
  const handleDeleteAccount = async (accountId: string) => {
    setConfirmModal({
      isOpen: true,
      title: "删除账号",
      message: "确定要删除此账号吗？删除后无法恢复。",
      type: "danger",
      onConfirm: async () => {
        setConfirmModal(null);
        try {
          await api.removeAccount(accountId);
          setAccounts((prev) => prev.filter((account) => account.id !== accountId));
          setSelectedIds((prev) => {
            const next = new Set(prev);
            next.delete(accountId);
            return next;
          });
          addToast("success", "账号已删除");
        } catch (err: any) {
          addToast("error", err.message || "删除账号失败");
        }
      },
    });
  };

  // 刷新单个账号
  const handleRefreshAccount = async (
    accountId: string,
    options?: { silent?: boolean }
  ) => {
    // 防止重复刷新
    if (refreshingIds.has(accountId)) {
      return;
    }

    setRefreshingIds((prev) => new Set(prev).add(accountId));

    try {
      const usage = await api.getAccountUsage(accountId);
      setAccounts((prev) =>
        prev.map((a) => (a.id === accountId ? { ...a, usage } : a))
      );
      updateUsageCache({ [accountId]: usage });
      if (!options?.silent) {
        addToast("success", "数据刷新成功", 1500, "refresh-success");
      }
    } catch (err: any) {
      addToast("error", err.message || "刷新失败");
    } finally {
      setRefreshingIds((prev) => {
        const next = new Set(prev);
        next.delete(accountId);
        return next;
      });
    }
  };

  // 单账号签到（右键菜单手动触发）
  const handleCheckinAccount = async (accountId: string) => {
    try {
      const result = await api.checkinAccount(accountId);
      if (result.state === "ok" || result.state === "already") {
        // 签到成功/已签到：本地把该账号标为「今日已签到」，
        // 避免等整批 loadAccounts 才刷新（网格/列表的徽标能即时变绿）
        setAccounts((prev) =>
          prev.map((a) => (a.id === accountId ? { ...a, checked_in_today: true } : a))
        );
      }
      if (result.state === "ok") {
        addToast("success", result.detail);
      } else if (result.state === "already") {
        addToast("info", `${result.account_name} 今日已签到`);
      } else if (result.state === "skipped") {
        // skipped 是有理由的跳过（TraeWork 无可达凭据）：不是失败，按 info 展示原因
        addToast("info", `${result.account_name}：${result.detail}`, 6000);
      } else if (result.state === "cooldown") {
        // 冷却不是错误：本次未尝试，按 info 展示
        addToast("info", `${result.account_name}：${describeCooldown(result)}`, 4000);
      } else if (result.state === "rate_limited") {
        addToast("warning", `${result.account_name}：${result.detail}`);
      } else {
        addToast("warning", result.detail);
      }
      // 冷却/跳过均未发请求，刷新用量没有意义
      if (result.state !== "failed" && result.state !== "cooldown" && result.state !== "skipped") {
        await handleRefreshAccount(accountId, { silent: true });
      }
    } catch (err: any) {
      // 防重入被拒（「签到进行中」）属于状态提示而非错误
      const message: string = err?.message || "签到失败";
      addToast(message.includes("签到进行中") ? "info" : "error", message);
    }
  };

  // 重置签到设备号（9095「本设备今日已签到」的自救手段）
  //
  // 为什么要确认对话框：重置会清掉已签到日期，存在「同日二次领取」的理论通路，
  // 唯一防线是后端 claim 前必先查 status（今日已真签到成功的会被短路为 Already）。
  // 文案必须把这一点如实告知，避免用户误以为重置能重复拿积分。
  const handleResetDeviceId = async (accountId: string) => {
    const target = accounts.find((a) => a.id === accountId);
    setConfirmModal({
      isOpen: true,
      title: "重置设备标识",
      message:
        "将为此账号生成新的签到设备号，并清除签到冷却与今日签到记录，用于「本设备今日已签到」的自救。\n\n" +
        "今日已签到成功的账号重置后不会重复获得积分；限流（签到人数过多）是账号级限制，重置不会解除。\n\n" +
        "账号的 IDE 机器码不受影响。确定继续吗？",
      type: "warning",
      onConfirm: async () => {
        setConfirmModal(null);
        try {
          await api.resetAccountDeviceId(accountId);
          addToast("success", `${target?.name ?? "账号"} 设备标识已重置，下次自动签到将重试该账号`, 4000);
          await loadAccounts();
        } catch (err: any) {
          const message: string = err?.message || "重置设备标识失败";
          addToast(message.includes("签到进行中") ? "info" : "error", message);
        }
      },
    });
  };

  // 全部账号签到（工具栏手动触发）
  const handleCheckinAll = async () => {
    if (accounts.length === 0) return;
    addToast("info", "正在签到，请稍候...", 2000, "checkin-all-progress");
    try {
      const results = await api.checkinAllAccounts();
      // 成功的账号即时把徽标改为「已签到」（与后端写入 last_checkin_date 的口径一致）
      const checkedInIds = new Set(
        results.filter((r) => r.state === "ok" || r.state === "already").map((r) => r.account_id)
      );
      if (checkedInIds.size > 0) {
        setAccounts((prev) =>
          prev.map((a) => (checkedInIds.has(a.id) ? { ...a, checked_in_today: true } : a))
        );
      }
      // 汇总口径下沉到 utils/checkinDisplay（面板侧「全部签到」共用），skipped 单独计数不计失败
      const summary = summarizeCheckin(results);
      addToast(summary.level, summary.text, 5000);
      await refreshUsageForAccounts(accounts);
    } catch (err: any) {
      const message: string = err?.message || "批量签到失败";
      addToast(message.includes("签到进行中") ? "info" : "error", message);
    }
  };

  const handleAccountAdded = useCallback(
    (account: Account) => {
      console.log("[handleAccountAdded] 添加账号:", account.id, account.email);
      
      // 直接创建新账号对象
      const nextAccount: AccountWithUsage = {
        id: account.id,
        name: account.name,
        email: account.email,
        avatar_url: account.avatar_url,
        plan_type: account.plan_type,
        is_active: account.is_active ?? true,
        created_at: account.created_at,
        machine_id: account.machine_id,
        is_current: false,
        // 新添加的账号今日必然未签到（AccountBrief 的派生字段，Account 上没有）
        checked_in_today: false,
        // 添加账号的三条路径（浏览器登录 / 读取 Trae / 导入）都只产出 TraeCode 账号
        app: account.app ?? "traecode",
        uid: account.uid ?? null,
        usage: null,
        password: account.password ?? null,
      };
      
      setAccounts((prev) => {
        const existing = prev.find((item) => item.id === account.id);
        console.log("[handleAccountAdded] 已存在:", !!existing, "当前列表长度:", prev.length);
        if (existing) {
          console.log("[handleAccountAdded] 更新已有账号");
          return prev.map((item) => (item.id === account.id ? { ...nextAccount, is_current: item.is_current, usage: item.usage } : item));
        }
        console.log("[handleAccountAdded] 添加新账号到列表");
        return [...prev, nextAccount];
      });
      setError(null);
      setHasLoaded(true);
      
      // 延迟刷新账号信息
      setTimeout(() => {
        void handleRefreshAccount(account.id, { silent: true });
      }, 100);
    },
    [handleRefreshAccount]
  );

  // 选择账号
  const handleSelectAccount = (accountId: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(accountId)) {
        next.delete(accountId);
      } else {
        next.add(accountId);
      }
      return next;
    });
  };

  // 全选/取消全选（TraeCode 账号范围；TraeWork 账号不参与批量操作）
  const handleSelectAll = () => {
    const selectable = accounts.filter((account) => !isTraeworkAccount(account));
    if (selectedIds.size === selectable.length) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(selectable.map((account) => account.id)));
    }
  };

  // 右键菜单
  const handleContextMenu = (e: React.MouseEvent, accountId: string) => {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, accountId });
  };

  // 复制 Token
  const handleCopyToken = async (accountId: string) => {
    try {
      const account = await api.getAccount(accountId);
      if (account.jwt_token) {
        await navigator.clipboard.writeText(account.jwt_token);
        addToast("success", "Token 已复制到剪贴板");
      } else {
        addToast("warning", "该账号没有有效的 Token");
      }
    } catch (err: any) {
      addToast("error", err.message || "获取 Token 失败");
    }
  };

  // 检查并设置 Trae IDE 路径
  const checkAndSetTraePath = async (): Promise<boolean> => {
    try {
      // 先尝试获取已保存的路径
      await api.getTraePath();
      return true;
    } catch {
      // 路径未设置或无效，尝试自动扫描
      try {
        const path = await api.scanTraePath();
        addToast("success", "已自动找到 Trae IDE: " + path);
        return true;
      } catch {
        // 自动扫描失败，弹出手动选择对话框
        const selected = await open({
          multiple: false,
          filters: [{
            name: "Trae IDE",
            extensions: ["exe"]
          }],
          title: "请选择 Trae CN.exe 文件"
        });

        if (selected) {
          try {
            await api.setTraePath(selected as string);
            addToast("success", "Trae IDE 路径已设置");
            return true;
          } catch (err: any) {
            addToast("error", err.message || "设置路径失败");
            return false;
          }
        }
        return false;
      }
    }
  };

  // 切换账号 / 重新登录（同逻辑）
  const handleSwitchAccount = async (
    accountId: string,
    options?: { mode?: "switch" | "relogin"; force?: boolean }
  ) => {
    const account = accounts.find((a) => a.id === accountId);
    if (!account) return;

    // 先检查 Trae IDE 路径
    const pathValid = await checkAndSetTraePath();
    if (!pathValid) {
      addToast("error", "未设置 Trae IDE 路径，无法切换账号");
      return;
    }

    const mode = options?.mode ?? "switch";
    const force = options?.force ?? mode === "relogin";
    const title = mode === "relogin" ? "重新登录" : "切换账号";
    const message =
      mode === "relogin"
        ? `确定要重新登录账号 "${account.email || account.name}" 吗？\n\n系统将自动关闭 Trae IDE 并重新写入登录信息。`
        : `确定要切换到账号 "${account.email || account.name}" 吗？\n\n系统将自动关闭 Trae IDE 并切换登录信息。`;
    const infoToast = mode === "relogin" ? "正在重新登录，请稍候..." : "正在切换账号，请稍候...";
    const successToast = mode === "relogin" ? "账号重新登录完成" : "账号切换成功";
    const errorToast = mode === "relogin" ? "重新登录失败" : "切换账号失败";

    setConfirmModal({
      isOpen: true,
      title,
      message,
      type: "warning",
      onConfirm: async () => {
        setConfirmModal(null);
        addToast("info", infoToast);
        // 置位期间定时刷新跳过本轮：切换命令内部要持锁做 Token 刷新等网络请求
        switchInProgressRef.current = true;
        try {
          await api.switchAccount(accountId, { force });
          await loadAccounts();
          addToast("success", successToast);
        } catch (err: any) {
          addToast("error", err.message || errorToast);
        } finally {
          switchInProgressRef.current = false;
        }
      },
    });
  };

  // 查看详情
  const handleViewDetail = async (accountId: string) => {
    const account = accounts.find((a) => a.id === accountId);
    if (!account) return;
    try {
      const full = await api.getAccount(accountId);
      setDetailAccount({
        ...account,
        email: full.email,
        password: full.password ?? null,
        user_id: full.user_id,
      });
    } catch (err: any) {
      addToast("error", err.message || "获取账号详情失败");
      setDetailAccount(account);
    }
  };

  const handleUpdateCredentials = async (
    accountId: string,
    updates: { email?: string; password?: string }
  ) => {
    try {
      const updated = await api.updateAccountProfile(accountId, {
        email: updates.email ?? null,
        password: updates.password ?? null,
      });
      setAccounts((prev) =>
        prev.map((account) =>
          account.id === accountId
            ? { ...account, email: updated.email, password: updated.password ?? null }
            : account
        )
      );
      setDetailAccount((prev) =>
        prev && prev.id === accountId
          ? { ...prev, email: updated.email, password: updated.password ?? null }
          : prev
      );
      addToast("success", "账号信息已更新", 1000);
    } catch (err: any) {
      addToast("error", err.message || "更新账号信息失败");
      throw err;
    }
  };

  const handleRelogin = async (
    accountId: string,
    options?: { forceManual?: boolean; source?: "update-token" | "relogin"; suppressToast?: boolean }
  ) => {
    try {
      if (options?.source === "update-token" && !options?.suppressToast) {
        addToast("info", "正在更新 Token...", 2000, "update-token-progress");
      }
      const account = await api.getAccount(accountId);
      const email = account.email || account.name;
      const maskedEmail = account.email?.includes("*") ?? false;

      if (!options?.forceManual) {
        if (account.cookies) {
          try {
            await api.refreshToken(accountId);
            await handleRefreshAccount(accountId, { silent: true });
            addToast("success", "已使用 Cookie 刷新 Token");
            return;
          } catch {}
        }

        if (account.password && account.email && !maskedEmail) {
          try {
            await api.refreshTokenWithPassword(accountId, account.password);
            await handleRefreshAccount(accountId, { silent: true });
            addToast("success", "已使用保存的密码刷新 Token");
            return;
          } catch {}
        }
      }

      if (options?.source === "update-token") {
        const accountLabel = account.email || account.name || "未知账号";
        const passwordLabel = account.password || "未保存";
        setConfirmModal({
          isOpen: true,
          title: "凭据似乎失效了",
          message:
            `账号: ${accountLabel}\n` +
            `密码: ${passwordLabel}\n\n` +
            "账号凭据似乎失效了或网络波动导致的连接异常。请检查网络，或尝试重新登录。",
          type: "warning",
          confirmText: "知道了",
          cancelText: "关闭",
          onConfirm: () => {
            setConfirmModal(null);
          },
        });
        return;
      }

      setLoginModal({
        accountId,
        accountName: email,
        initialEmail: maskedEmail ? "" : account.email,
      });
    } catch (err: any) {
      addToast("error", err.message || "重新登录失败");
    }
  };

  const handleUpdateToken = async (accountId: string) => {
    if (refreshingIds.has(accountId)) {
      return;
    }

    setRefreshingIds((prev) => new Set(prev).add(accountId));
    addToast("info", "正在更新 Token...", 2000, "update-token-progress");
    try {
      const usage = await api.getAccountUsage(accountId);
      setAccounts((prev) =>
        prev.map((a) => (a.id === accountId ? { ...a, usage } : a))
      );
      updateUsageCache({ [accountId]: usage });
      addToast("success", "Token 已更新", 1500, "update-token-success");
    } catch (err: any) {
      void handleRelogin(accountId, {
        source: "update-token",
        forceManual: true,
        suppressToast: true,
      });
    } finally {
      setRefreshingIds((prev) => {
        const next = new Set(prev);
        next.delete(accountId);
        return next;
      });
    }
  };

  const handleLoginSubmit = async (accountId: string, email: string, password: string) => {
    const usage = await api.loginAccountWithEmail(accountId, email, password);
    setAccounts((prev) =>
      prev.map((account) =>
        account.id === accountId
          ? { ...account, email, password, usage }
          : account
      )
    );
    updateUsageCache({ [accountId]: usage });
    setDetailAccount((prev) =>
      prev && prev.id === accountId
        ? { ...prev, email, password }
        : prev
    );
    addToast("success", "重新登录成功");
  };

  const handleBuyPro = async (accountId: string) => {
    try {
      await api.openPricing(accountId);
      addToast("info", "已打开购买页面");
    } catch (err: any) {
      addToast("error", err.message || "打开购买页面失败");
    }
  };

  // 导出账号
  const handleExportAccounts = async () => {
    try {
      const date = new Date().toISOString().split("T")[0];
      const path = await save({
        defaultPath: `trae-accounts-${date}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return;
      await api.exportAccountsToPath(path as string);
      addToast("success", `已导出 ${accounts.length} 个账号`);
    } catch (err: any) {
      addToast("error", err.message || "导出失败");
    }
  };

  // 导入账号
  const handleImportAccounts = () => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json";
    input.onchange = async (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (!file) return;

      try {
        const text = await file.text();
        const count = await api.importAccounts(text);
        addToast("success", `成功导入 ${count} 个账号`);
        await loadAccounts();
      } catch (err: any) {
        addToast("error", err.message || "导入失败");
      }
    };
    input.click();
  };

  // 批量刷新选中账号
  const handleBatchRefresh = async () => {
    if (selectedIds.size === 0) {
      addToast("warning", "请先选择要刷新的账号");
      return;
    }

    addToast("info", `正在刷新 ${selectedIds.size} 个账号...`);

    for (const id of selectedIds) {
      await handleRefreshAccount(id, { silent: true });
    }
  };

  // 批量删除选中账号
  const handleBatchDelete = () => {
    if (selectedIds.size === 0) {
      addToast("warning", "请先选择要删除的账号");
      return;
    }

    setConfirmModal({
      isOpen: true,
      title: "批量删除",
      message: `确定要删除选中的 ${selectedIds.size} 个账号吗？此操作无法撤销。`,
      type: "danger",
      onConfirm: async () => {
        try {
          for (const id of selectedIds) {
            await api.removeAccount(id);
          }
          setSelectedIds(new Set());
          addToast("success", `已删除 ${selectedIds.size} 个账号`);
          await loadAccounts();
        } catch (err: any) {
          addToast("error", err.message || "删除失败");
        }
        setConfirmModal(null);
      },
    });
  };

  // TraeCode 账号子集：账号管理页/统计页/批量操作都只处理它，TraeWork 账号走独立面板
  //（两套切换机制相反，混在一个列表里会让用户按同一预期操作）
  const traecodeAccounts = accounts.filter((account) => !isTraeworkAccount(account));
  // 定时器通过 ref 读最新列表：把列表写进定时器的依赖会让每次刷新都重建定时器、重置计时
  traecodeAccountsRef.current = traecodeAccounts;
  const visibleAccounts = Array.isArray(accounts)
    ? [...accounts]
        .filter((account) => {
          // TraeWork 账号不在本页展示
          if (isTraeworkAccount(account)) {
            return false;
          }
          // 额度筛选
          if (quotaFilter === "with") {
            // 有剩余额度：usage 存在且 fast_dollar_left > 0
            return account.usage && account.usage.fast_dollar_left > 0;
          } else if (quotaFilter === "without") {
            // 无剩余额度：usage 不存在或 fast_dollar_left <= 0
            return !account.usage || account.usage.fast_dollar_left <= 0;
          }
          return true;
        })
        .sort((a, b) => {
          // 当前使用的账号排在最前面
          if (a.is_current && !b.is_current) return -1;
          if (!a.is_current && b.is_current) return 1;
          return 0;
        })
    : [];

  return (
    <div className="app">
      <Sidebar
        currentPage={currentPage}
        onNavigate={setCurrentPage}
        theme={appSettings?.theme ?? null}
        onThemeChange={handleThemeChange}
      />

      <div className="app-content">
        {error && (
          <div className="error-banner">
            {error}
            <button onClick={() => setError(null)}>×</button>
          </div>
        )}

        {currentPage === "stats" && (
          <Stats accounts={traecodeAccounts} hasLoaded={hasLoaded} />
        )}

        {currentPage === "traework" && (
          <TraeworkPanel
            accounts={accounts}
            onToast={addToast}
            onAccountsChanged={loadAccounts}
          />
        )}

        {currentPage === "accounts" && (
          <>
            <main className="app-main">
              {traecodeAccounts.length > 0 && (
                <div className="toolbar">
                  <div className="toolbar-left">
                    <button
                      type="button"
                      className="header-btn"
                      onClick={handleSelectAll}
                      style={{ padding: "8px 14px" }}
                    >
                      {selectedIds.size === traecodeAccounts.length && traecodeAccounts.length > 0 ? "取消全选" : "全选"}
                    </button>
                    <div className="quota-filter">
                      <select
                        className="quota-filter-select"
                        value={quotaFilter}
                        onChange={(e) => setQuotaFilter(e.target.value as "all" | "with" | "without")}
                        title="额度筛选"
                      >
                        <option value="all">全部额度</option>
                        <option value="with">有剩余额度</option>
                        <option value="without">无剩余额度</option>
                      </select>
                      <svg className="quota-filter-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" width="16" height="16">
                        <polyline points="6 9 12 15 18 9"/>
                      </svg>
                    </div>
                    <div className="view-toggle">
                      <button
                        className={`view-btn ${viewMode === "grid" ? "active" : ""}`}
                        onClick={() => handleViewModeChange("grid")}
                        title="卡片视图"
                      >
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" width="16" height="16">
                          <rect x="3" y="3" width="7" height="7"/>
                          <rect x="14" y="3" width="7" height="7"/>
                          <rect x="3" y="14" width="7" height="7"/>
                          <rect x="14" y="14" width="7" height="7"/>
                        </svg>
                      </button>
                      <button
                        className={`view-btn ${viewMode === "list" ? "active" : ""}`}
                        onClick={() => handleViewModeChange("list")}
                        title="列表视图"
                      >
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" width="16" height="16">
                          <line x1="8" y1="6" x2="21" y2="6"/>
                          <line x1="8" y1="12" x2="21" y2="12"/>
                          <line x1="8" y1="18" x2="21" y2="18"/>
                          <line x1="3" y1="6" x2="3.01" y2="6"/>
                          <line x1="3" y1="12" x2="3.01" y2="12"/>
                          <line x1="3" y1="18" x2="3.01" y2="18"/>
                        </svg>
                      </button>
                    </div>
                    {selectedIds.size > 0 && (
                      <div className="batch-actions">
                        <button className="batch-btn" onClick={handleBatchRefresh}>
                          <svg
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            width="14"
                            height="14"
                          >
                            <path d="M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>
                          </svg>
                          刷新
                        </button>
                        <button className="batch-btn danger" onClick={handleBatchDelete}>
                          <svg
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            width="14"
                            height="14"
                          >
                            <path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>
                          </svg>
                          删除
                        </button>
                      </div>
                    )}
                  </div>
                  <div className="toolbar-right">
                    <button className="header-btn" onClick={handleCheckinAll} style={{ padding: "8px 14px", fontSize: "13px" }}>
                      ✅ 全部签到
                    </button>
                    <button className="add-btn" onClick={() => setShowAddModal(true)} style={{padding: '8px 16px', fontSize: '13px'}}>
                      <span>+</span> 添加账号
                    </button>
                  </div>
                </div>
              )}

              {loading ? (
                <div className="loading">
                  <div className="spinner"></div>
                  <p>加载中...</p>
                </div>
              ) : accounts.length === 0 ? (
                <div className="empty-state">
                  <div className="empty-icon">📋</div>
                  <h3>暂无账号</h3>
                  <p>点击上方按钮添加账号，或导入已有账号</p>
                  <div className="empty-actions">
                    <button className="empty-btn primary" onClick={() => setShowAddModal(true)}>
                      添加账号
                    </button>
                    <button className="empty-btn" onClick={handleImportAccounts}>
                      导入账号
                    </button>
                  </div>
                </div>
              ) : viewMode === "grid" ? (
                <div className="account-grid">
                  {visibleAccounts.map((account) => (
                    <AccountCard
                      key={account.id}
                      account={account}
                      usage={account.usage || null}
                      selected={selectedIds.has(account.id)}
                      checkedInToday={account.checked_in_today}
                      onSelect={handleSelectAccount}
                      onContextMenu={handleContextMenu}
                      onToast={addToast}
                    />
                  ))}
                </div>
              ) : (
                <div className="account-list">
                  <div className="list-header">
                    <div className="list-col checkbox"></div>
                    <div className="list-col avatar"></div>
                    <div className="list-col info">账号信息</div>
                    <div className="list-col plan">套餐</div>
                    <div className="list-col usage">使用量</div>
                    <div className="list-col reset">重置时间</div>
                    <div className="list-col status">状态</div>
                    <div className="list-col actions"></div>
                  </div>
                  {visibleAccounts.map((account) => (
                    <AccountListItem
                      key={account.id}
                      account={account}
                      usage={account.usage || null}
                      selected={selectedIds.has(account.id)}
                      checkedInToday={account.checked_in_today}
                      onSelect={handleSelectAccount}
                      onContextMenu={handleContextMenu}
                    />
                  ))}
                </div>
              )}
            </main>
          </>
        )}

        {currentPage === "settings" && (
          <Settings
            onToast={addToast}
            settings={appSettings}
            onSettingsChange={setAppSettings}
            onAccountsChanged={loadAccounts}
          />
        )}

        {currentPage === "about" && <About />}
      </div>

      {/* Toast 通知 */}
      <Toast messages={toasts} onRemove={removeToast} />

      {/* 确认弹窗 */}
      {confirmModal && (
        <ConfirmModal
          isOpen={confirmModal.isOpen}
          title={confirmModal.title}
          message={confirmModal.message}
          type={confirmModal.type}
          confirmText={confirmModal.confirmText}
          cancelText={confirmModal.cancelText}
          onConfirm={confirmModal.onConfirm}
          onCancel={() => setConfirmModal(null)}
        />
      )}

      {/* 右键菜单 */}

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          onRelogin={() => {
            handleSwitchAccount(contextMenu.accountId, { mode: "relogin" });
            setContextMenu(null);
          }}
          onViewDetail={() => {
            void handleViewDetail(contextMenu.accountId);
            setContextMenu(null);
          }}
          onRefresh={() => {
            handleRefreshAccount(contextMenu.accountId);
            setContextMenu(null);
          }}
          onCheckin={() => {
            void handleCheckinAccount(contextMenu.accountId);
            setContextMenu(null);
          }}
          onResetDeviceId={() => {
            void handleResetDeviceId(contextMenu.accountId);
            setContextMenu(null);
          }}
          onUpdateToken={() => {
            void handleUpdateToken(contextMenu.accountId);
            setContextMenu(null);
          }}
          onCopyToken={() => {
            handleCopyToken(contextMenu.accountId);
            setContextMenu(null);
          }}
          onSwitchAccount={() => {
            handleSwitchAccount(contextMenu.accountId);
            setContextMenu(null);
          }}
          onBuyPro={() => {
            void handleBuyPro(contextMenu.accountId);
            setContextMenu(null);
          }}
          onDelete={() => {
            handleDeleteAccount(contextMenu.accountId);
            setContextMenu(null);
          }}
          isCurrent={accounts.find(a => a.id === contextMenu.accountId)?.is_current || false}
        />
      )}

      {/* 添加账号弹窗 */}
      <AddAccountModal
        isOpen={showAddModal}
        onClose={() => setShowAddModal(false)}
        onToast={addToast}
        onAccountAdded={handleAccountAdded}
        onImportAccounts={handleImportAccounts}
        onExportAccounts={handleExportAccounts}
        canExport={accounts.length > 0}
      />

      {/* 详情弹窗 */}
      <DetailModal
        isOpen={!!detailAccount}
        onClose={() => setDetailAccount(null)}
        account={detailAccount}
        usage={detailAccount?.usage || null}
        onUpdateCredentials={handleUpdateCredentials}
        onToast={addToast}
      />

      <AccountLoginModal
        isOpen={!!loginModal}
        accountId={loginModal?.accountId || ""}
        accountName={loginModal?.accountName || ""}
        initialEmail={loginModal?.initialEmail}
        onClose={() => setLoginModal(null)}
        onSubmit={handleLoginSubmit}
      />
    </div>
  );
}

export default App;
