import { useEffect, useState } from "react";

// 主题精简为明/暗两档：暗色对应 TraeCode（品牌绿），亮色对应 TraeWork（品牌靛蓝）。
// 历史存储值 purple/green 统一迁移为 dark，避免老用户落到已删除的档位。
type Theme = "light" | "dark";

const THEME_KEY = "trae_theme_v1";

function readInitialTheme(): Theme {
  if (typeof window === "undefined") return "dark";
  const saved = localStorage.getItem(THEME_KEY);
  if (saved === "light") return "light";
  if (saved === "dark" || saved === "purple" || saved === "green") return "dark";
  // 暗色为本应用的主主题，未设置过时默认暗色
  return "dark";
}

export function ThemeSwitcher() {
  const [theme, setTheme] = useState<Theme>(readInitialTheme);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);

  const isDark = theme === "dark";

  return (
    <div className="theme-switcher">
      <button
        className="theme-btn"
        onClick={() => setTheme(isDark ? "light" : "dark")}
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
            <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
          </svg>
        )}
      </button>
    </div>
  );
}
