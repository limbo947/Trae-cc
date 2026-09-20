import { useEffect } from "react";

// 主题精简为明/暗两档：暗色对应 TraeCode（品牌绿），亮色对应 TraeWork（品牌靛蓝）。
// 历史存储值 purple/green 统一按 dark 处理，避免老用户落到已删除的档位。
type Theme = "light" | "dark";

interface ThemeSwitcherProps {
  /**
   * 主题来自 settings.json（`AppSettings.theme`）。
   * null / 非法值都按 dark 处理——暗色是本应用的主主题，也是升级前的默认。
   */
  theme?: string | null;
  onThemeChange?: (theme: Theme) => void;
}

/**
 * 明暗主题切换。
 *
 * 为什么不再自己维护 state 与 localStorage：主题此前只存在浏览器存储里，
 * 与 settings.json 构成两套真源，既让「导出配置」不完整，也让设置页保存任意开关时
 * 可能把旧主题写回去。现在它只是 settings 的一个字段，组件退化为纯展示 + 回调，
 * 迁移逻辑集中在 `App.tsx`。
 */
export function ThemeSwitcher({ theme, onThemeChange }: ThemeSwitcherProps) {
  const current: Theme = theme === "light" ? "light" : "dark";

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", current);
  }, [current]);

  const isDark = current === "dark";

  return (
    <div className="theme-switcher">
      <button
        className="theme-btn"
        onClick={() => onThemeChange?.(isDark ? "light" : "dark")}
        title={isDark ? "切换到亮色模式" : "切换到暗色模式"}
        aria-label={isDark ? "切换到亮色模式" : "切换到暗色模式"}
      >
        {isDark ? (
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
            strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            {/* 暗色：月牙 */}
            <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
          </svg>
        ) : (
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
            strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            {/* 亮色：太阳 */}
            <circle cx="12" cy="12" r="4" />
            <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l1.41 1.41" />
          </svg>
        )}
      </button>
    </div>
  );
}
