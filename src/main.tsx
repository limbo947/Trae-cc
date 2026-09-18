import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./App.css";
// Trae 设计语言层：必须晚于 App.css 与各组件 CSS 加载，同优先级靠后生效。
// tokens 重定义变量（暗=TraeCode / 亮=TraeWork），其余三个文件做结构覆盖。
import "./styles/trae-tokens.css";
import "./styles/trae-components.css";
import "./styles/trae-overlays.css";
import "./styles/trae-pages.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
