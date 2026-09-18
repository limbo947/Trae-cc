import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
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
import { Settings } from "./pages/Settings";
import { About } from "./pages/About";

import * as api from "./api";
import type { Account, AccountBrief, AppSettings, CheckinResult, UsageSummary } from "./types";
import "./App.css";

interface AccountWithUsage extends AccountBrief {
  usage?: UsageSummary | null;
  password?: string | null;
}

type ViewMode = "grid" | "list";
const USAGE_CACHE_KEY = "trae_usage_cache_v1";

/**
 * 把冷却状态渲染成用户可读文案。
 *
 * 为什么按 reason 而不是按 cooldown_until 数值判定：auth_expired 的 until 是 i64::MAX，
 * 超出 JS 安全整数范围、JSON 反序列化后等值判断不可靠，因此后端契约规定前端只读字符串。
 */
function describeCooldown(result: CheckinResult): string {
  const base = (() => {
    switch (result.cooldown_reason) {
      case "auth_expired":
        return "登录状态已失效，需重新登录";
      case "rate_limited":
        return "签到人数过多，稍后再试";
      case "risk_control":
        return "账号权益不足，暂不可签到";
      case "server_error":
        return "服务端暂时不可用，稍后再试";
      default:
        return result.detail;
    }
  })();

  // 剩余时间只对「有时限」的冷却展示；auth_expired 的 until 是 i64::MAX 哨兵，不参与数值运算
  if (result.cooldown_reason && result.cooldown_reason !== "auth_expired") {
    const remainMs = (result.cooldown_until ?? 0) * 1000 - Date.now();
    if (remainMs > 0) {
      return `${base}（剩余约 ${Math.max(1, Math.ceil(remainMs / 60000))} 分钟）`;
    }
  }
  return base;
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
  const [emailFilter, setEmailFilter] = useState("");
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

  const quickRegisterNoticeRef = useRef<Map<string, number>>(new Map());
  const toastDedupRef = useRef<Map<string, number>>(new Map());
  const autoCheckinRanRef = useRef(false);
  const quickRegisterShowWindow = appSettings?.quick_register_show_window ?? false;

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
            quick_register_show_window: false,
            auto_refresh_enabled: true,
            privacy_auto_enable: true,
            auto_start_enabled: false,
            api_key: "9201",
            custom_tempmail_config: {
              api_url: "",
              secret_key: "",
              email_domain: "",
            },
          });
        }
      });
    return () => {
      active = false;
    };
  }, []);



  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<{ id?: string; message: string }>("quick_register_notice", (event) => {
      if (quickRegisterShowWindow) {
        return;
      }
      const { id, message } = event.payload || {};
      if (!message) return;
      const key = id || message;
      const now = Date.now();
      const last = quickRegisterNoticeRef.current.get(key);
      if (last && now - last < 800) {
        return;
      }
      quickRegisterNoticeRef.current.set(key, now);
      addToast("success", message, 2500);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, [addToast, quickRegisterShowWindow]);

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
      updateUsageCache({}, list.map((a) => a.id));
      setLoading(false);
      void refreshUsageForAccounts(list);
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
  useEffect(() => {
    if (!hasLoaded || autoCheckinRanRef.current || accounts.length === 0) return;
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
  }, [hasLoaded, accounts, refreshUsageForAccounts]);

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
      } else if (result.state === "cooldown") {
        // 冷却不是错误：本次未尝试，按 info 展示
        addToast("info", `${result.account_name}：${describeCooldown(result)}`, 4000);
      } else if (result.state === "rate_limited") {
        addToast("warning", `${result.account_name}：${result.detail}`);
      } else {
        addToast("warning", result.detail);
      }
      // 冷却中未发请求，刷新用量没有意义
      if (result.state !== "failed" && result.state !== "cooldown") {
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
      const ok = results.filter((r) => r.state === "ok").length;
      const already = results.filter((r) => r.state === "already").length;
      const cooldown = results.filter((r) => r.state === "cooldown");
      const rateLimited = results.filter((r) => r.state === "rate_limited");
      const failed = results.filter((r) => r.state === "failed");
      const cooldownNote = cooldown.length > 0 ? `，冷却 ${cooldown.length}` : "";
      if (failed.length > 0) {
        const rateNote = rateLimited.length > 0 ? `，限流 ${rateLimited.length}` : "";
        addToast("warning", `签到完成：成功 ${ok}，已签到 ${already}${cooldownNote}，失败 ${failed.length}${rateNote}（${failed[0].detail}）`, 5000);
      } else if (rateLimited.length > 0 || cooldown.length > 0) {
        const first = rateLimited[0] ?? cooldown[0];
        addToast("warning", `签到完成：成功 ${ok}，已签到 ${already}${cooldownNote}，限流 ${rateLimited.length}（${first.detail}）`, 5000);
      } else {
        addToast("success", `签到完成：成功 ${ok}，已签到 ${already}`, 3000);
      }
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

  // 全选/取消全选
  const handleSelectAll = () => {
    if (selectedIds.size === accounts.length) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(accounts.map((account) => account.id)));
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
        try {
          await api.switchAccount(accountId, { force });
          await loadAccounts();
          addToast("success", successToast);
        } catch (err: any) {
          addToast("error", err.message || errorToast);
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

  const normalizedFilter = (emailFilter || "").trim().toLowerCase();
  const visibleAccounts = Array.isArray(accounts)
    ? [...accounts]
        .filter((account) => {
          // 邮箱搜索过滤
          if (normalizedFilter && !(account.email || account.name || "").toLowerCase().includes(normalizedFilter)) {
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
      <Sidebar currentPage={currentPage} onNavigate={setCurrentPage} />

      <div className="app-content">
        {error && (
          <div className="error-banner">
            {error}
            <button onClick={() => setError(null)}>×</button>
          </div>
        )}

        {currentPage === "stats" && (
          <Stats accounts={accounts} hasLoaded={hasLoaded} />
        )}

        {currentPage === "accounts" && (
          <>
            <main className="app-main">
              {accounts.length > 0 && (
                <div className="toolbar">
                  <div className="toolbar-left">
                    <button
                      type="button"
                      className="header-btn"
                      onClick={handleSelectAll}
                      style={{ padding: "8px 14px" }}
                    >
                      {selectedIds.size === accounts.length && accounts.length > 0 ? "取消全选" : "全选"}
                    </button>
                    <div className="toolbar-search">
                      <svg
                        className="search-icon"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                      >
                        <circle cx="11" cy="11" r="8"></circle>
                        <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
                      </svg>
                      <input
                        type="text"
                        className="toolbar-search-input"
                        placeholder="搜索邮箱..."
                        value={emailFilter}
                        onChange={(event) => setEmailFilter(event.target.value)}
                      />
                    </div>
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
                        onClick={() => setViewMode("grid")}
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
                        onClick={() => setViewMode("list")}
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
        quickRegisterShowWindow={quickRegisterShowWindow}
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
