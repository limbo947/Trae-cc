import type { ReactNode } from "react";

interface IconButtonProps {
  onClick: () => void;
  title: string;
  disabled?: boolean;
  children: ReactNode;
}

/**
 * 路径 / 机器码卡片右上角的图标按钮。
 *
 * 为什么抽成组件：原单文件里「刷新 / 复制 / 编辑」三个按钮的 15 行内联样式逐字重复，
 * 后续还要再加「打开所在目录」等同类按钮，不抽就会变成四份、五份副本。
 */
export function IconButton({ onClick, title, disabled, children }: IconButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      disabled={disabled}
      style={{
        padding: '6px',
        border: 'none',
        background: 'transparent',
        color: 'var(--text-secondary)',
        cursor: 'pointer',
        borderRadius: '6px',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        transition: 'all 0.2s'
      }}
      onMouseOver={(e) => e.currentTarget.style.backgroundColor = 'var(--bg-hover)'}
      onMouseOut={(e) => e.currentTarget.style.backgroundColor = 'transparent'}
    >
      {children}
    </button>
  );
}
