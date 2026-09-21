//! 签到结果的展示层共享模块：冷却文案与批量汇总。
//!
//! 为什么独立成模块：面板侧（TraeworkPanel）渲染签到结果与 App.tsx 的工具栏汇总
//! 需要同一套文案与口径，留在 App.tsx 作私有函数面板够不着——历史上
//! `auth_plaintext`（Rust 侧）就因「两处各写一份」漂移过一次，展示层同理。
import type { CheckinResult } from "../types";

/**
 * 把冷却状态渲染成用户可读文案。
 *
 * 为什么按 reason 而不是按 cooldown_until 数值判定：auth_expired 的 until 是 i64::MAX，
 * 超出 JS 安全整数范围、JSON 反序列化后等值判断不可靠，因此后端契约规定前端只读字符串。
 */
export function describeCooldown(result: CheckinResult): string {
  const base = (() => {
    switch (result.cooldown_reason) {
      case "auth_expired":
        return "登录状态已失效，需重新登录";
      case "credential_stale":
        return "凭据已过期，请切换到该账号并重新保存登录态";
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

/** 批量签到的逐状态计数汇总 */
export interface CheckinSummary {
  text: string;
  level: "success" | "warning";
}

/**
 * 批量签到结果汇总（TraeCode 工具栏与 TraeWork 面板共用同一套口径）。
 *
 * skipped 单独计数、不计入失败：无凭据跳过不是失败，把它塞进 failed 会弹红字、
 * 让用户误以为出了错。coolNote / rateNote 仅在对应项存在时拼接，TraeCode-only
 * 批次的文案与旧版逐字一致（skipped 在那里恒为 0）。
 */
export function summarizeCheckin(results: CheckinResult[]): CheckinSummary {
  const count = (state: CheckinResult["state"]) =>
    results.filter((r) => r.state === state).length;
  const ok = count("ok");
  const already = count("already");
  const skipped = count("skipped");
  const cooldown = results.filter((r) => r.state === "cooldown");
  const rateLimited = results.filter((r) => r.state === "rate_limited");
  const failed = results.filter((r) => r.state === "failed");

  const skippedNote = skipped > 0 ? `，跳过 ${skipped}` : "";
  const cooldownNote = cooldown.length > 0 ? `，冷却 ${cooldown.length}` : "";
  if (failed.length > 0) {
    const rateNote = rateLimited.length > 0 ? `，限流 ${rateLimited.length}` : "";
    return {
      level: "warning",
      text: `签到完成：成功 ${ok}，已签到 ${already}${cooldownNote}${skippedNote}，失败 ${failed.length}${rateNote}（${failed[0].detail}）`,
    };
  }
  if (rateLimited.length > 0 || cooldown.length > 0) {
    const first = rateLimited[0] ?? cooldown[0];
    return {
      level: "warning",
      text: `签到完成：成功 ${ok}，已签到 ${already}${cooldownNote}${skippedNote}，限流 ${rateLimited.length}（${first.detail}）`,
    };
  }
  return {
    level: "success",
    text: `签到完成：成功 ${ok}，已签到 ${already}${skippedNote}`,
  };
}
