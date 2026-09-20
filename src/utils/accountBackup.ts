import { save } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import type { ToastFn } from "../types";

/**
 * 账号库导出 / 导入的共享实现。
 *
 * 为什么抽出来：设置页的「数据与备份」与「添加账号」弹窗都需要这两个入口，
 * 各写一份的话筛选器、默认文件名、成功文案会各自漂移——同一个功能在两个位置
 * 表现不一致，用户会怀疑哪个才算数。
 */

/** 导出账号库到用户选定的文件。返回是否真的写了文件（用户取消为 false） */
export async function exportAccountsToFile(
  accountCount: number,
  notify: ToastFn
): Promise<boolean> {
  try {
    const date = new Date().toISOString().split("T")[0];
    const path = await save({
      defaultPath: `trae-accounts-${date}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return false;
    await api.exportAccountsToPath(path as string);
    notify("success", `已导出 ${accountCount} 个账号`);
    return true;
  } catch (err: any) {
    notify("error", err.message || "导出失败");
    return false;
  }
}

/**
 * 弹文件选择框导入账号。
 *
 * 用隐藏 `<input type=file>` 而不是 dialog 插件的 open()：导出走 save() 拿路径后
 * 交给后端读，而导入需要把文件**内容**交给后端（`import_accounts` 收的是字符串），
 * 隐藏 input 可以直接读文本，省一次「路径 → 读文件」的往返。
 *
 * @param onImported 导入完成后的回调（调用方通常要重新拉账号列表）
 */
export function importAccountsFromFile(
  notify: ToastFn,
  onImported?: (count: number) => void | Promise<void>
): void {
  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".json";
  input.onchange = async (e) => {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (!file) return;

    try {
      const text = await file.text();
      const count = await api.importAccounts(text);
      notify("success", `成功导入 ${count} 个账号`);
      await onImported?.(count);
    } catch (err: any) {
      notify("error", err.message || "导入失败");
    }
  };
  input.click();
}
