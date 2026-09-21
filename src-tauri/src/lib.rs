// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod account;
mod autostart;
mod machine;
mod privacy;
mod device_reset;
mod browser_auto_login;
mod logger;
mod tc_crypto;
mod traework;
mod window_state;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use tokio::io::AsyncWriteExt;
use tokio::sync::{oneshot, Mutex};
use tauri::{AppHandle, Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri::webview::PageLoadEvent;
use tauri_plugin_updater::UpdaterExt;
use uuid::Uuid;
use warp::Filter;

use account::{AccountBrief, AccountManager, Account, TraeIdeReadOutcome};
use api::{TraeApiClient, UsageSummary, UsageQueryResponse, UserStatisticResult};

#[cfg(target_os = "windows")]
fn hide_console_window() {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub auto_refresh_enabled: bool,
    /// 用量自动刷新的间隔（分钟）。前端按此值起定时器；0 视为不刷新。
    pub refresh_interval: u32,
    pub privacy_auto_enable: bool,
    pub auto_start_enabled: bool,
    /// 启动时静默自动签到。
    ///
    /// 只作用于 GUI 启动路径：`--silent` 无头模式没有界面，签到失败也不会打扰用户，
    /// 那边保持「总是尝试」的既有行为（见 `handle_silent_start`）。
    pub auto_checkin_enabled: bool,
    /// 界面主题：`light` / `dark`。
    ///
    /// 用 `Option` 而不是带默认值的 `String`：老版本的主题只存在 `localStorage` 里，
    /// 若这里给默认值，前端就分不清「用户从未在本字段设置过」与「用户显式选了 dark」，
    /// 迁移时会把老用户的 light 偏好悄悄覆盖成 dark。
    pub theme: Option<String>,
    /// TraeCode 账号页的视图偏好：`grid`（卡片）/ `list`（列表）。
    ///
    /// 为什么放进 settings.json 而不是继续用 localStorage：主题的历史教训是
    /// 「浏览器存储 + settings.json 两套真源」会让导出配置不完整、并在无关保存时互相覆盖
    /// （见 `ThemeSwitcher`）。视图偏好同属界面状态，跟随同一真源才不会重蹈覆辙。
    ///
    /// `None` 表示从未设置过（老配置），由前端回落为 `grid`。
    pub view_mode: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_refresh_enabled: true,
            // 30 分钟：够及时又不给接口压力（下拉提供 5/10/30/60）
            refresh_interval: 30,
            privacy_auto_enable: true,
            auto_start_enabled: false,
            auto_checkin_enabled: true,
            theme: None,
            view_mode: None,
        }
    }
}

fn get_settings_path() -> anyhow::Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("com", "hhj", "trae-cc")
        .ok_or_else(|| anyhow::anyhow!("无法获取应用配置目录"))?;
    let config_dir = proj_dirs.config_dir();
    fs::create_dir_all(config_dir)?;
    Ok(config_dir.join("settings.json"))
}

fn load_settings_from_disk() -> anyhow::Result<AppSettings> {
    let path = get_settings_path()?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let content = fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Ok(AppSettings::default());
    }
    let settings = serde_json::from_str(&content)
        .unwrap_or_else(|_| AppSettings::default());
    Ok(settings)
}

fn save_settings_to_disk(settings: &AppSettings) -> anyhow::Result<()> {
    let path = get_settings_path()?;
    let content = serde_json::to_string_pretty(settings)?;
    fs::write(path, content)?;
    Ok(())
}

/// 应用状态
pub struct AppState {
    pub account_manager: Mutex<AccountManager>,
    browser_login: Mutex<Option<BrowserLoginSession>>,
    browser_login_cancel: Mutex<Option<oneshot::Sender<()>>>,
    settings: Mutex<AppSettings>,
}

struct BrowserLoginSession {
    receiver: oneshot::Receiver<(String, String)>,
    shutdown: Arc<StdMutex<Option<oneshot::Sender<()>>>>,
    cancel: oneshot::Receiver<()>,
    window_close: oneshot::Receiver<()>,
    webview: WebviewWindow,
    credentials: Arc<StdMutex<BrowserLoginCredentials>>,
}

#[derive(Debug, Default, Clone)]
struct BrowserLoginCredentials {
    email: Option<String>,
    password: Option<String>,
}

/// 错误类型
#[derive(Debug, serde::Serialize)]
pub struct ApiError {
    pub message: String,
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

type Result<T> = std::result::Result<T, ApiError>;

// ============ Tauri 命令 ============

/// 添加账号（通过 Token，可选 Cookies）
#[tauri::command]
async fn add_account_by_token(token: String, cookies: Option<String>, state: State<'_, AppState>) -> Result<Account> {
    log::info!("Adding account by token");
    let mut manager = state.account_manager.lock().await;
    let result = manager.add_account_by_token(token, cookies, None).await.map_err(ApiError::from);
    match &result {
        Ok(_) => log::info!("Account added successfully by token"),
        Err(e) => log::error!("Failed to add account by token: {:?}", e),
    }
    result
}

/// 添加账号（通过邮箱密码登录）
#[tauri::command]
async fn add_account_by_email(email: String, password: String, state: State<'_, AppState>) -> Result<Account> {
    log::info!("Adding account by email: {}", email);
    let mut manager = state.account_manager.lock().await;
    let result = manager.add_account_by_email(email, password).await.map_err(ApiError::from);
    match &result {
        Ok(_) => log::info!("Account added successfully by email"),
        Err(e) => log::error!("Failed to add account by email: {:?}", e),
    }
    result
}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings> {
    let settings = state.settings.lock().await;
    Ok(settings.clone())
}

#[tauri::command]
async fn update_settings(settings: AppSettings, state: State<'_, AppState>) -> Result<AppSettings> {
    // 只在开关真正翻转时才写注册表：否则任何一次无关设置的保存都会被注册表问题挡住。
    let auto_start_changed = {
        let current = state.settings.lock().await;
        current.auto_start_enabled != settings.auto_start_enabled
    };

    {
        let mut current = state.settings.lock().await;
        *current = settings.clone();
    }
    save_settings_to_disk(&settings).map_err(ApiError::from)?;

    if auto_start_changed {
        if let Err(err) = autostart::set_auto_start(settings.auto_start_enabled) {
            // 注册表写入可能被策略或安全软件拦截，属环境问题，不该让「隐私模式」等
            // 无关设置一起保存失败；启动时会用落盘值重写一次注册表，下次启动即自愈。
            log::warn!("写入开机自启动注册表失败（设置已保存）: {}", err);
        }
    }

    Ok(settings)
}

/// 下载并运行更新安装包（Windows: .msi）
#[tauri::command]
async fn download_and_run_installer(url: String) -> Result<String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(anyhow::anyhow!("安装包链接为空").into());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(anyhow::anyhow!("安装包链接无效").into());
    }

    // Prefer keeping the original filename, but avoid collisions.
    let raw_filename = url
        .split('/')
        .last()
        .unwrap_or("Trae账号管理Update.msi")
        .split('?')
        .next()
        .unwrap_or("Trae账号管理Update.msi")
        .trim();
    let filename = if raw_filename.is_empty() {
        "Trae账号管理Update.msi"
    } else {
        raw_filename
    };

    let mut dest_path = std::env::temp_dir();
    dest_path.push(format!(
        "Trae账号管理-update-{}-{}",
        Uuid::new_v4(),
        filename
    ));

    let client = Client::builder()
        .user_agent("Trae账号管理 @ Updater")
        .timeout(Duration::from_secs(60 * 30))
        .build()
        .map_err(|e| ApiError::from(anyhow::Error::new(e)))?;

    let mut response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ApiError::from(anyhow::Error::new(e)))?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("下载失败: {}", response.status()).into());
    }

    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| ApiError::from(anyhow::Error::new(e)))?;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ApiError::from(anyhow::Error::new(e)))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| ApiError::from(anyhow::Error::new(e)))?;
    }
    file.flush()
        .await
        .map_err(|e| ApiError::from(anyhow::Error::new(e)))?;
    drop(file);

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("msiexec")
            .arg("/i")
            .arg(dest_path.to_string_lossy().to_string())
            .spawn()
            .map_err(|e| anyhow::anyhow!("无法启动安装程序: {}", e))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        open::that(&dest_path).map_err(|e| anyhow::anyhow!("无法打开安装程序: {}", e))?;
    }

    Ok(dest_path.to_string_lossy().to_string())
}



fn build_browser_login_script(port: u16) -> String {
    let script = r#"(function() {
  if (window.__traeAutoInjected) return;
  window.__traeAutoInjected = true;

  const callback = "http://127.0.0.1:__PORT__/callback";
  let loginTriggered = false;
  const normalize = (text) => (text || "").toLowerCase();
  const STORAGE_EMAIL_KEY = "__trae_login_email";
  const STORAGE_PASSWORD_KEY = "__trae_login_password";
  let capturedEmail = "";
  let capturedPassword = "";
  let lastSentEmail = "";
  let lastSentPassword = "";
  const boundInputs = new WeakSet();
  try {
    capturedEmail = sessionStorage.getItem(STORAGE_EMAIL_KEY) || "";
    capturedPassword = sessionStorage.getItem(STORAGE_PASSWORD_KEY) || "";
  } catch {}
  const captureEmail = (value) => {
    const next = (value || "").trim();
    if (next) {
      capturedEmail = next;
      try {
        sessionStorage.setItem(STORAGE_EMAIL_KEY, capturedEmail);
      } catch {}
    }
  };
  const capturePassword = (value) => {
    const next = (value || "").toString();
    if (next) {
      capturedPassword = next;
      try {
        sessionStorage.setItem(STORAGE_PASSWORD_KEY, capturedPassword);
      } catch {}
    }
  };
  const maybeCapture = (el) => {
    if (!el || !el.getAttribute) return;
    const type = normalize(el.getAttribute("type") || "");
    const name = normalize(el.getAttribute("name") || "");
    const autocomplete = normalize(el.getAttribute("autocomplete") || "");
    const placeholder = normalize(el.getAttribute("placeholder") || "");
    const value = typeof el.value === "string" ? el.value : "";
    const trimmedValue = value.trim();
    if (type === "password" || name.includes("password") || autocomplete.includes("password") || placeholder.includes("password")) {
      capturePassword(value);
    }
    if (
      type === "email" ||
      name.includes("email") ||
      name.includes("account") ||
      autocomplete.includes("email") ||
      placeholder.includes("email") ||
      placeholder.includes("邮箱")
    ) {
      captureEmail(value);
    } else if (!capturedEmail && trimmedValue.includes("@")) {
      captureEmail(trimmedValue);
    }
  };
  const bindInput = (input) => {
    if (!input || boundInputs.has(input) || !input.addEventListener) return;
    boundInputs.add(input);
    const handler = () => {
      maybeCapture(input);
      syncCredentials();
    };
    input.addEventListener("input", handler);
    input.addEventListener("change", handler);
    input.addEventListener("blur", handler);
  };
  const applyCredentialField = (key, value) => {
    if (typeof value !== "string") return;
    const lower = normalize(key);
    if (lower.includes("email")) {
      captureEmail(value);
    }
    if (
      lower.includes("password") ||
      lower.includes("passwd") ||
      lower === "pwd" ||
      lower.endsWith("password")
    ) {
      capturePassword(value);
    }
  };
  const extractCredentialsFromBody = (body) => {
    if (!body) return;
    try {
      if (typeof body === "string") {
        const trimmed = body.trim();
        if (!trimmed) return;
        if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
          const data = JSON.parse(trimmed);
          if (data && typeof data === "object") {
            Object.keys(data).forEach((key) => applyCredentialField(key, data[key]));
          }
        } else {
          const params = new URLSearchParams(trimmed);
          params.forEach((value, key) => applyCredentialField(key, value));
        }
        syncCredentials();
        return;
      }
      if (body instanceof URLSearchParams) {
        body.forEach((value, key) => applyCredentialField(key, value));
        syncCredentials();
        return;
      }
      if (typeof FormData !== "undefined" && body instanceof FormData) {
        body.forEach((value, key) => {
          if (typeof value === "string") {
            applyCredentialField(key, value);
          }
        });
        syncCredentials();
        return;
      }
    } catch {}
  };
  const hookValueSetter = () => {
    try {
      if (window.__traeValueHooked) return;
      if (!window.HTMLInputElement) return;
      const proto = HTMLInputElement.prototype;
      const desc = Object.getOwnPropertyDescriptor(proto, "value");
      if (!desc || !desc.set || !desc.get) return;
      Object.defineProperty(proto, "value", {
        get: function() {
          return desc.get.call(this);
        },
        set: function(val) {
          desc.set.call(this, val);
          try {
            maybeCapture(this);
            syncCredentials();
          } catch {}
        }
      });
      window.__traeValueHooked = true;
    } catch {}
  };
  const getInputFromEvent = (event) => {
    const path = typeof event.composedPath === "function" ? event.composedPath() : (event.path || []);
    if (path && path.length) {
      for (const node of path) {
        if (node && node.tagName && node.tagName.toLowerCase() === "input") {
          return node;
        }
      }
    }
    return event.target;
  };
  const scanRoot = (root) => {
    if (!root) return;
    try {
      const inputs = root.querySelectorAll ? root.querySelectorAll("input") : [];
      if (inputs && inputs.length) {
        inputs.forEach((input) => {
          maybeCapture(input);
          bindInput(input);
        });
      }
      const elements = root.querySelectorAll ? root.querySelectorAll("*") : [];
      if (elements && elements.length) {
        elements.forEach((el) => {
          if (el && el.shadowRoot) {
            scanRoot(el.shadowRoot);
          }
          if (el && el.tagName && el.tagName.toLowerCase() === "iframe") {
            try {
              scanRoot(el.contentDocument || (el.contentWindow && el.contentWindow.document));
            } catch {}
          }
        });
      }
    } catch {}
  };
  const scanInputs = () => {
    scanRoot(document);
    syncCredentials();
  };
  const tryAcceptCookies = () => {
    const cookieSelectors = [
      'button.cm__btn',
      '.cm__btn[role=\"button\"]',
      '.cm__btn'
    ];
    for (const selector of cookieSelectors) {
      const btn = document.querySelector(selector);
      if (btn) {
        btn.click();
        return true;
      }
    }
    const candidates = Array.from(
      document.querySelectorAll("button, [role='button'], input[type='button'], input[type='submit'], a")
    );
    const matchText = (text) => {
      const val = (text || "").toLowerCase();
      return (
        val.includes("got it") ||
        val.includes("accept") ||
        val.includes("agree") ||
        val.includes("允许") ||
        val.includes("同意")
      );
    };
    for (const el of candidates) {
      const text = el.innerText || el.textContent || "";
      if (matchText(text)) {
        el.click();
        return true;
      }
    }
    const wrapper = document.querySelector(".cm-wrapper, .cc__wrapper, .cookie-banner, .cookie-consent");
    if (wrapper) {
      wrapper.remove();
      return true;
    }
    return false;
  };
  const sendPayload = (payload) => {
    const params = new URLSearchParams();
    Object.keys(payload || {}).forEach((key) => {
      const value = payload[key];
      if (value === undefined || value === null || value === "") return;
      params.append(key, value);
    });
    if (capturedEmail) params.append("email", capturedEmail);
    if (capturedPassword) params.append("password", capturedPassword);
    const url = callback + "?" + params.toString();
    if (navigator.sendBeacon) {
      navigator.sendBeacon(url);
    } else {
      fetch(url, { mode: "no-cors" });
    }
  };
  const syncCredentials = () => {
    if (!capturedEmail && !capturedPassword) return;
    if (capturedEmail === lastSentEmail && capturedPassword === lastSentPassword) return;
    lastSentEmail = capturedEmail;
    lastSentPassword = capturedPassword;
    sendPayload({ state: "credentials" });
  };
  const normalizeUrl = (raw) => {
    if (!raw) return "";
    try {
      return new URL(raw, location.href).toString();
    } catch {
      return String(raw);
    }
  };

  const sendToken = (token, url) => {
    if (!token) return;
    loginTriggered = true;
    sendPayload({ token, url: normalizeUrl(url) });
  };
  const sendState = (state, href) => {
    if (!state) return;
    loginTriggered = true;
    sendPayload({ state, href: href || "" });
  };
  const isLoginCompleteUrl = (href) => {
    if (!href) return false;
    const lower = href.toLowerCase();
    if (lower.includes("/login")) return false;
    if (lower.includes("passport")) return false;
    if (lower.includes("sign-up") || lower.includes("signup") || lower.includes("register")) return false;
    if (lower.includes("terms") || lower.includes("privacy")) return false;
    return true;
  };
  const parseToken = (data) => {
    if (!data) return null;
    return (
      data.result?.token ||
      data.result?.Token ||
      data.Result?.token ||
      data.Result?.Token ||
      null
    );
  };

  const markLoginTriggered = () => {
    loginTriggered = true;
  };
  const tryFetch = async () => {
    // 仅适配国内版：CN 版统一 API 域名
    const endpoints = [
      "https://api.trae.com.cn/cloudide/api/v3/common/GetUserToken"
    ];
    const headers = {
      "content-type": "application/json",
      "accept": "application/json, text/plain, */*",
      "origin": "https://www.trae.com.cn",
      "referer": "https://www.trae.com.cn/"
    };
    for (const endpoint of endpoints) {
      try {
        const res = await fetch(endpoint, {
          method: "POST",
          credentials: "include",
          headers,
          body: "{}"
        });
        const data = await res.json();
        const token = parseToken(data);
        if (token) {
          sendToken(token, res.url);
          return;
        }
      } catch {}
    }
  };

  const hookFetch = () => {
    const orig = window.fetch;
    window.fetch = async (...args) => {
      try {
        const input = args[0];
        const init = args[1];
        if (init && init.body) {
          extractCredentialsFromBody(init.body);
        } else if (input && typeof input === "object" && typeof input.clone === "function") {
          input.clone().text().then((text) => extractCredentialsFromBody(text)).catch(() => {});
        }
      } catch {}
      const res = await orig(...args);
      try {
        if (typeof res.url === "string" && res.url.includes("GetUserToken")) {
          const data = await res.clone().json();
          const token = parseToken(data);
          if (token) sendToken(token, res.url);
        }
      } catch {}
      return res;
    };
  };

  const hookXHR = () => {
    const origOpen = XMLHttpRequest.prototype.open;
    const origSend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function(method, url, ...rest) {
      this.__trae_url = url;
      return origOpen.apply(this, [method, url, ...rest]);
    };
    XMLHttpRequest.prototype.send = function(body) {
      try {
        extractCredentialsFromBody(body);
      } catch {}
      this.addEventListener("load", function() {
        try {
          if ((this.__trae_url || "").includes("GetUserToken")) {
            const data = JSON.parse(this.responseText);
            const token = parseToken(data);
            if (token) sendToken(token, this.__trae_url);
          }
        } catch {}
      });
      return origSend.apply(this, arguments);
    };
  };

  hookFetch();
  hookXHR();
  hookValueSetter();
  tryFetch();
  tryAcceptCookies();
  scanInputs();
  setInterval(tryFetch, 3000);
  setInterval(tryAcceptCookies, 1500);
  setInterval(scanInputs, 2000);
  try {
    const observer = new MutationObserver(() => scanInputs());
    const target = document.documentElement || document;
    observer.observe(target, { childList: true, subtree: true });
  } catch {}
  document.addEventListener("submit", () => {
    scanInputs();
    markLoginTriggered();
  }, true);
  syncCredentials();
  document.addEventListener("click", (event) => {
    const target = event.target;
    if (!target || !target.closest) return;
    scanInputs();
    const button = target.closest("button, [role='button'], a, input[type='button'], input[type='submit']");
    if (!button) return;
    const text = normalize(button.innerText || button.textContent || button.getAttribute("aria-label"));
    if (
      text.includes("log in") ||
      text.includes("login") ||
      text.includes("sign in") ||
      text.includes("sign-in") ||
      text.includes("github") ||
      text.includes("google") ||
      text.includes("continue") ||
      text.includes("登录") ||
      text.includes("继续") ||
      text.includes("授权")
    ) {
      markLoginTriggered();
    }
  }, true);
  document.addEventListener("input", (event) => {
    const target = getInputFromEvent(event);
    if (!target) return;
    maybeCapture(target);
    syncCredentials();
    const targetType = target.getAttribute ? normalize(target.getAttribute("type") || "") : "";
    if (targetType === "password") markLoginTriggered();
  }, true);
  let lastHref = location.href;
  let stateSent = false;
  const checkHref = () => {
    const href = location.href;
    if (href !== lastHref) {
      lastHref = href;
      if (!stateSent && isLoginCompleteUrl(href)) {
        stateSent = true;
        sendState("logged_in", href);
        tryFetch();
      }
    }
  };
  setInterval(checkHref, 1000);
  if (isLoginCompleteUrl(location.href)) {
    stateSent = true;
    sendState("logged_in", location.href);
    tryFetch();
  }
})();"#;
    script.replace("__PORT__", &port.to_string())
}

#[allow(dead_code)]
fn collect_trae_cookies(webview: &WebviewWindow, extra_url: Option<&str>) -> String {
    let mut cookie_map: HashMap<String, String> = HashMap::new();
    let mut urls = vec![
        "https://www.trae.com.cn/".to_string(),
        "https://api.trae.com.cn/".to_string(),
    ];
    
    if let Some(url) = extra_url {
        if !url.is_empty() {
             // 尝试提取 base url (e.g. https://api.trae.com.cn)
             if let Ok(parsed) = Url::parse(url) {
                 let base = format!("{}://{}/", parsed.scheme(), parsed.host_str().unwrap_or_default());
                 urls.push(base);
             }
             urls.push(url.to_string());
        }
    }

    for raw_url in urls {
        if let Ok(url) = Url::parse(&raw_url) {
            if let Ok(cookies) = webview.cookies_for_url(url) {
                for cookie in cookies {
                    cookie_map
                        .entry(cookie.name().to_string())
                        .or_insert(cookie.value().to_string());
                }
            }
        }
    }

    let mut cookies = cookie_map
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    if !cookies.is_empty()
        && !cookies.contains("store-idc=")
        && !cookies.contains("trae-target-idc=")
    {
        // 国内版 IDC 标识（推断值，仅在 cookie 缺失时补全）
        cookies.push_str("; store-idc=alicn");
    }
    cookies
}

/// 打开登录窗口前清掉上一轮登录留下的 web 会话（登录窗口的轻量隔离）
///
/// 为什么必须**先清后导航**：登录窗口与购买窗口共用 WebView2 的默认数据目录
/// （`%LOCALAPPDATA%\com.hhj.trae-cc\EBWebView`），上一次登录在里面留下的 `trae.com.cn`
/// Cookie 会被登录页的**首次请求**原样带上去，表现为「打开登录窗口时还带着上次那个账号」。
/// 原实现在窗口已经开始加载之后才调用清理，那一次请求早已把旧 Cookie 发出去了——清理一直
/// 都在，只是晚了一步。
///
/// 为什么用 `about:blank` 起手：`clear_all_browsing_data` 只能作用于已建好的 webview，
/// 建窗时直接给登录 URL 的话，清理必然晚于首次请求。
///
/// 为什么在整库清理之外再按域删一遍 Cookie：`ClearBrowsingData` 是异步落盘的，显式删除
/// 覆盖它尚未生效的窗口期。只删 trae 域而非全量——购买窗口 `open_pricing` 有意走
/// 「清 Cookie → 写入目标账号 Cookie → 跳转」，两个窗口又共用数据目录，全量清理会顺带
/// 抹掉用户在购买窗口里的其它站点状态。
fn clear_login_webview_session(webview: &WebviewWindow) {
    if let Err(e) = webview.clear_all_browsing_data() {
        log::warn!("清理登录窗口浏览数据失败: {e}");
    }

    let cookies = match webview.cookies() {
        Ok(cookies) => cookies,
        Err(e) => {
            log::warn!("读取登录窗口 Cookie 失败，跳过按域清理: {e}");
            return;
        }
    };

    // 域名判据同时覆盖 `trae.com.cn` 与 `trae.cn`（签到/API 在后者），容忍前导点与子域
    let is_trae = |domain: Option<&str>| {
        domain
            .map(|d| d.trim_start_matches('.').to_lowercase())
            .is_some_and(|d| {
                d == "trae.com.cn"
                    || d.ends_with(".trae.com.cn")
                    || d == "trae.cn"
                    || d.ends_with(".trae.cn")
            })
    };

    let mut removed = 0usize;
    for cookie in cookies {
        if !is_trae(cookie.domain()) {
            continue;
        }
        match webview.delete_cookie(cookie) {
            Ok(()) => removed += 1,
            // 只记数量不记内容：Cookie 是凭据，禁止落日志
            Err(e) => log::warn!("删除登录窗口残留 Cookie 失败: {e}"),
        }
    }
    log::info!("登录窗口已清理浏览数据，并显式删除 {removed} 条 trae 域 Cookie");
}

#[tauri::command]
async fn start_browser_login(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let mut browser_login = state.browser_login.lock().await;
    if browser_login.is_some() {
        return Err(anyhow::anyhow!("浏览器登录已在进行中").into());
    }
    let (token_tx, token_rx) = oneshot::channel::<(String, String)>();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let (window_close_tx, window_close_rx) = oneshot::channel::<()>();
    let token_sender = Arc::new(StdMutex::new(Some(token_tx)));
    let shutdown_sender = Arc::new(StdMutex::new(Some(shutdown_tx)));
    let window_close_sender = Arc::new(StdMutex::new(Some(window_close_tx)));
    let credentials = Arc::new(StdMutex::new(BrowserLoginCredentials::default()));

    let token_sender_route = token_sender.clone();
    let shutdown_sender_route = shutdown_sender.clone();
    let credentials_route = credentials.clone();
    let route = warp::path("callback")
        .and(warp::query::<HashMap<String, String>>())
        .map(move |query: HashMap<String, String>| {
            let mut log_query = query.clone();
            if log_query.contains_key("password") {
                log_query.insert("password".to_string(), "***".to_string());
            }
            let token = query.get("token").cloned().unwrap_or_default();
            let state = query.get("state").cloned().unwrap_or_default();
            let href = query.get("href").cloned().unwrap_or_default();
            let url = query.get("url").cloned().unwrap_or_default();
            let email = query.get("email").cloned().unwrap_or_default();
            let password = query.get("password").cloned().unwrap_or_default();

            if !email.trim().is_empty() || !password.is_empty() {
                let mut creds = credentials_route.lock().unwrap();
                if !email.trim().is_empty() {
                    creds.email = Some(email.trim().to_string());
                }
                if !password.is_empty() {
                    creds.password = Some(password);
                }
            }
            if !token.is_empty() {
                if let Some(tx) = token_sender_route.lock().unwrap().take() {
                    let _ = tx.send((token, url));
                }
                if let Some(tx) = shutdown_sender_route.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                warp::reply::html("已收到 Token，可以关闭此页面并返回应用。".to_string())
            } else if state == "logged_in" {
                warp::reply::html(format!("检测到登录完成，等待获取 Token。{href}"))
            } else {
                warp::reply::html("未收到 Token，请重试。".to_string())
            }
        });

    let (addr, server): (std::net::SocketAddr, _) = warp::serve(route)
        .bind_with_graceful_shutdown(([127, 0, 0, 1], 0), async move {
            let _ = shutdown_rx.await;
        });

    tokio::spawn(server);

    let script = build_browser_login_script(addr.port());
    let script_init = script.clone();

    // 关闭已存在的登录窗口
    if let Some(existing) = app.get_webview_window("trae-login") {
        let _ = existing.destroy();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }
    // 再次检查确保窗口已关闭
    if app.get_webview_window("trae-login").is_some() {
        return Err(anyhow::anyhow!("无法关闭已存在的登录窗口，请重启应用后重试").into());
    }

    // 先用 about:blank 建窗、清掉残留会话，再导航到登录页——这个顺序就是本步骤的全部意义，
    // 原因见 clear_login_webview_session
    let webview = WebviewWindowBuilder::new(&app, "trae-login", WebviewUrl::External("about:blank".parse().unwrap()))
        .title("Trae CN 登录")
        .inner_size(1000.0, 720.0)
        .initialization_script(&script_init)
        .build()
        .map_err(|e| anyhow::anyhow!("无法打开登录窗口: {}", e))?;

    clear_login_webview_session(&webview);

    webview
        .navigate(
            Url::parse("https://www.trae.com.cn/login")
                .map_err(|e| anyhow::anyhow!("登录地址非法: {}", e))?,
        )
        .map_err(|e| anyhow::anyhow!("无法打开登录页: {}", e))?;

    let window_close_sender_clone = window_close_sender.clone();
    webview.on_window_event(move |event| {
        if let tauri::WindowEvent::Destroyed = event {
            if let Some(tx) = window_close_sender_clone.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    });

    let _ = webview.set_focus();

    *browser_login = Some(BrowserLoginSession {
        receiver: token_rx,
        shutdown: shutdown_sender,
        cancel: cancel_rx,
        window_close: window_close_rx,
        webview,
        credentials,
    });
    *state.browser_login_cancel.lock().await = Some(cancel_tx);

    Ok(())
}

#[tauri::command]
async fn finish_browser_login(state: State<'_, AppState>) -> Result<Account> {
    let session = {
        let mut browser_login = state.browser_login.lock().await;
        browser_login.take().ok_or_else(|| anyhow::anyhow!("浏览器登录未开始"))?
    };

    let (token, _url) = tokio::select! {
        res = session.receiver => {
            match res {
                Ok(token) => token,
                Err(_) => {
                    let _ = state.browser_login_cancel.lock().await.take();
                    if let Some(tx) = session.shutdown.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    let _ = session.webview.close();
                    // 清理 browser_login 状态
                    let mut browser_login = state.browser_login.lock().await;
                    *browser_login = None;
                    return Err(anyhow::anyhow!("浏览器登录已取消").into());
                }
            }
        }
        _ = session.cancel => {
            let _ = state.browser_login_cancel.lock().await.take();
            if let Some(tx) = session.shutdown.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let _ = session.webview.close();
            // 清理 browser_login 状态
            let mut browser_login = state.browser_login.lock().await;
            *browser_login = None;
            return Err(anyhow::anyhow!("浏览器登录已取消").into());
        }
        _ = session.window_close => {
            let _ = state.browser_login_cancel.lock().await.take();
            if let Some(tx) = session.shutdown.lock().unwrap().take() {
                let _ = tx.send(());
            }
            // 清理 browser_login 状态
            let mut browser_login = state.browser_login.lock().await;
            *browser_login = None;
            return Err(anyhow::anyhow!("浏览器被主动关闭").into());
        }
        _ = tokio::time::sleep(Duration::from_secs(300)) => {
            let _ = state.browser_login_cancel.lock().await.take();
            if let Some(tx) = session.shutdown.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let _ = session.webview.close();
            // 清理 browser_login 状态
            let mut browser_login = state.browser_login.lock().await;
            *browser_login = None;
            return Err(anyhow::anyhow!("等待浏览器登录超时").into());
        }
    };

    if let Some(tx) = session.shutdown.lock().unwrap().take() {
        let _ = tx.send(());
    }
    let _ = state.browser_login_cancel.lock().await.take();

    // 获取 cookies
    let cookies = session.webview.cookies()
        .map(|cookies: Vec<tauri::webview::Cookie>| {
            cookies.iter()
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();

    let mut credentials = session.credentials.lock().unwrap().clone();
    if credentials.email.as_deref().unwrap_or("").trim().is_empty()
        && credentials.password.as_deref().unwrap_or("").is_empty()
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let snapshot = session.credentials.lock().unwrap().clone();
            if !snapshot.email.as_deref().unwrap_or("").trim().is_empty()
                || !snapshot.password.as_deref().unwrap_or("").is_empty()
            {
                credentials = snapshot;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let _ = session.webview.close();
    let cookies = if cookies.is_empty() { None } else { Some(cookies) };

    let mut manager = state.account_manager.lock().await;
    let mut account = manager
        .upsert_account_by_token(token, cookies, None)
        .await
        .map_err(ApiError::from)?;

    let email = credentials.email.unwrap_or_default();
    let password = credentials.password.unwrap_or_default();
    let has_email = !email.trim().is_empty();
    let has_password = !password.is_empty();
    if has_email || has_password {
        account = manager
            .update_account_profile(
                &account.id,
                if has_email { Some(email) } else { None },
                if has_password { Some(password) } else { None },
            )
            .map_err(ApiError::from)?;
    }

    Ok(account)
}

#[tauri::command]
async fn cancel_browser_login(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    if let Some(tx) = state.browser_login_cancel.lock().await.take() {
        let _ = tx.send(());
    }
    let session = {
        let mut browser_login = state.browser_login.lock().await;
        browser_login.take()
    };
    if let Some(session) = session {
        if let Some(tx) = session.shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
        let _ = session.webview.destroy();
    }
    // 关闭自动登录窗口（如果存在）
    if let Some(window) = app.get_webview_window("auto_login") {
        let _ = window.destroy();
    }
    Ok(())
}

#[tauri::command]
async fn browser_auto_login_command(
    app: AppHandle,
    email: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<Account> {
    browser_auto_login::browser_auto_login(app, email, password, &state).await.map_err(|e| e.into())
}

#[tauri::command]
async fn remove_account(account_id: String, state: State<'_, AppState>) -> Result<()> {
    let mut manager = state.account_manager.lock().await;
    manager.remove_account(&account_id).map_err(ApiError::from)
}

/// 获取所有账号
#[tauri::command]
async fn get_accounts(state: State<'_, AppState>) -> Result<Vec<AccountBrief>> {
    let manager = state.account_manager.lock().await;
    Ok(manager.get_accounts())
}

/// 获取单个账号详情
#[tauri::command]
async fn get_account(account_id: String, state: State<'_, AppState>) -> Result<Account> {
    let manager = state.account_manager.lock().await;
    manager.get_account(&account_id).map_err(ApiError::from)
}

/// 切换账号（设置活跃账号并更新机器码）
#[tauri::command]
async fn switch_account(account_id: String, force: Option<bool>, state: State<'_, AppState>) -> Result<()> {
    log::info!("Switching account: {}", account_id);
    {
        let mut manager = state.account_manager.lock().await;
        let force = force.unwrap_or(false);
        if let Err(e) = manager.switch_account(&account_id, force).await {
            log::error!("Failed to switch account: {}", e);
            return Err(ApiError::from(e));
        }
        log::info!("Account switched successfully");
    }

    let settings = state.settings.lock().await.clone();
    if settings.privacy_auto_enable {
        match machine::get_trae_state_db_path() {
            Ok(db_path) => {
                // 后台执行完整隐私流程：全新启动的 Trae 实测需 70 秒以上才生成 state.vscdb，
                // 前台等待会让切换命令长时间不返回（曾导致前端转圈 30 秒、用户重复点击）
                tokio::spawn(async move {
                    let db_path_clone = db_path.clone();
                    let start_result = tokio::task::spawn_blocking(move || {
                        if let Err(e) = machine::open_trae() {
                            log::error!("启动 Trae IDE 失败: {}", e);
                            return Err(e);
                        }
                        let start = std::time::Instant::now();
                        let timeout = std::time::Duration::from_secs(120);
                        while !db_path_clone.exists() {
                            if start.elapsed() > timeout {
                                return Err(anyhow::anyhow!("等待 Trae 数据库超时"));
                            }
                            std::thread::sleep(std::time::Duration::from_millis(200));
                        }
                        Ok(())
                    }).await;

                    match start_result {
                        Ok(Ok(())) => {
                            let result = tokio::task::spawn_blocking(move || {
                                privacy::enable_privacy_mode_at_path_with_restart(db_path, || {
                                    machine::kill_trae()?;
                                    machine::open_trae()
                                })
                            }).await;
                            match result {
                                Ok(Ok(())) => log::info!("隐私模式已写入并重启 Trae"),
                                Ok(Err(e)) => log::error!("写入隐私模式失败: {}", e),
                                Err(e) => log::error!("隐私任务执行异常: {}", e),
                            }
                        }
                        Ok(Err(e)) => log::error!("等待 Trae 数据库失败: {}", e),
                        Err(e) => log::error!("启动任务异常: {}", e),
                    }
                });
            }
            Err(err) => {
                log::error!("Failed to find Trae database: {}", err);
                // 即使查找数据库失败，也尝试启动 Trae
                tokio::spawn(async move {
                    let _ = tokio::task::spawn_blocking(|| {
                        if let Err(e) = machine::open_trae() {
                            // open_trae 只认已保存的路径、不做兜底扫描，失败时界面上毫无反馈，
                            // 用户只会看到「切换成功但 IDE 没打开」——必须留日志可查。
                            log::warn!("切换账号后启动 Trae 失败: {}", e);
                        }
                    }).await;
                });
            }
        }
    } else {
        // 隐私模式未启用，后台启动 Trae（不阻塞切换命令返回）
        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(|| {
                if let Err(e) = machine::open_trae() {
                    log::warn!("切换账号后启动 Trae 失败: {}", e);
                }
            }).await;
        });
    }

    Ok(())
}

/// 获取账号使用量
#[tauri::command]
async fn get_account_usage(account_id: String, state: State<'_, AppState>) -> Result<UsageSummary> {
    // 1. 获取账号信息（持有锁的时间极短）
    let account = {
        let manager = state.account_manager.lock().await;
        manager.get_account(&account_id).map_err(ApiError::from)?
    };

    // 2. 执行网络请求（不持有锁，可并行）
    let (summary, new_token) = fetch_usage_for_account(&account).await.map_err(ApiError::from)?;

    // 3. 更新账号信息（持有锁的时间极短）
    {
        let mut manager = state.account_manager.lock().await;
        // 忽略更新错误（可能账号已被删除），但不影响返回结果
        let _ = manager.update_account_info_after_usage_check(
            &account_id,
            summary.plan_type.clone(),
            new_token,
        );
    }

    Ok(summary)
}

async fn fetch_usage_for_account(account: &Account) -> anyhow::Result<(UsageSummary, Option<(String, String)>)> {
    let mut new_token_info = None;

    let summary = if let Some(token) = &account.jwt_token {
        // 优先使用 Token
        let client = TraeApiClient::new_with_token(token)?;
        match client.get_usage_summary_by_token().await {
            Ok(summary) => summary,
            Err(e) => {
                let error_msg = e.to_string();
                // 如果是 401 错误且有 Cookies，尝试刷新 Token
                if error_msg.contains("401") && !account.cookies.is_empty() {
                    // 使用 Cookies 刷新 Token
                    let mut cookie_client = TraeApiClient::new(&account.cookies)?;
                    let token_result = cookie_client.get_user_token().await?;
                    
                    new_token_info = Some((token_result.token.clone(), token_result.expired_at.clone()));

                    // 使用新 Token 重新获取使用量
                    let new_client = TraeApiClient::new_with_token(&token_result.token)?;
                    new_client.get_usage_summary_by_token().await?
                } else if error_msg.contains("401") {
                    return Err(anyhow::anyhow!("Token 已过期，请更新 Token 或 Cookies"));
                } else {
                    return Err(e);
                }
            }
        }
    } else if !account.cookies.is_empty() {
        // 使用 Cookies
        let mut client = TraeApiClient::new(&account.cookies)?;
        // 先获取 token 以便保存
        let token_result = client.get_user_token().await?;
        new_token_info = Some((token_result.token.clone(), token_result.expired_at.clone()));
        
        client.get_usage_summary().await?
    } else {
        return Err(anyhow::anyhow!("账号没有有效的 Token 或 Cookies"));
    };

    Ok((summary, new_token_info))
}

/// 更新账号 Token
#[tauri::command]
async fn update_account_token(account_id: String, token: String, state: State<'_, AppState>) -> Result<UsageSummary> {
    let mut manager = state.account_manager.lock().await;
    manager.update_account_token(&account_id, token).await.map_err(ApiError::from)
}

/// 刷新 Token（使用 Cookies）
#[tauri::command]
async fn refresh_token(account_id: String, state: State<'_, AppState>) -> Result<()> {
    let mut manager = state.account_manager.lock().await;
    manager.refresh_token(&account_id).await.map_err(ApiError::from)
}

/// 使用密码刷新 Token/Cookies
#[tauri::command]
async fn refresh_token_with_password(
    account_id: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<()> {
    let mut manager = state.account_manager.lock().await;
    manager
        .refresh_token_with_password(&account_id, &password)
        .await
        .map_err(ApiError::from)
}

/// 使用邮箱密码重新登录并更新账号
#[tauri::command]
async fn login_account_with_email(
    account_id: String,
    email: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<UsageSummary> {
    let mut manager = state.account_manager.lock().await;
    manager
        .login_account_with_email(&account_id, email, password)
        .await
        .map_err(ApiError::from)
}

/// 更新账号邮箱/密码
#[tauri::command]
async fn update_account_profile(
    account_id: String,
    email: Option<String>,
    password: Option<String>,
    state: State<'_, AppState>,
) -> Result<Account> {
    let mut manager = state.account_manager.lock().await;
    manager
        .update_account_profile(&account_id, email, password)
        .map_err(ApiError::from)
}

/// 清空账号数据
#[tauri::command]
async fn clear_accounts(state: State<'_, AppState>) -> Result<usize> {
    let mut manager = state.account_manager.lock().await;
    manager.clear_accounts().map_err(ApiError::from)
}

/// 导出账号到指定路径
#[tauri::command]
async fn export_accounts_to_path(path: String, state: State<'_, AppState>) -> Result<()> {
    let manager = state.account_manager.lock().await;
    let content = manager.export_accounts().map_err(ApiError::from)?;
    fs::write(&path, content)
        .map_err(|err| ApiError::from(anyhow::Error::from(err)))?;
    Ok(())
}

/// 导出账号
#[tauri::command]
async fn export_accounts(state: State<'_, AppState>) -> Result<String> {
    let manager = state.account_manager.lock().await;
    manager.export_accounts().map_err(ApiError::from)
}

/// 导入账号
#[tauri::command]
async fn import_accounts(data: String, state: State<'_, AppState>) -> Result<usize> {
    let mut manager = state.account_manager.lock().await;
    manager.import_accounts(&data).await.map_err(ApiError::from)
}

/// 获取使用事件
#[tauri::command]
async fn get_usage_events(
    account_id: String,
    start_time: i64,
    end_time: i64,
    page_num: i32,
    page_size: i32,
    state: State<'_, AppState>
) -> Result<UsageQueryResponse> {
    let mut manager = state.account_manager.lock().await;
    manager.get_usage_events(&account_id, start_time, end_time, page_num, page_size)
        .await
        .map_err(ApiError::from)
}

/// 从 Trae IDE 读取账号
///
/// 返回 `TraeIdeReadOutcome`（新增 / 补全 / 已存在 / 本机无登录态），前端据此给出准确提示
#[tauri::command]
async fn read_trae_account(state: State<'_, AppState>) -> Result<TraeIdeReadOutcome> {
    let mut manager = state.account_manager.lock().await;
    manager.read_trae_ide_account().await.map_err(ApiError::from)
}

/// 获取当前系统机器码
#[tauri::command]
async fn get_machine_id() -> Result<String> {
    machine::get_machine_guid().map_err(ApiError::from)
}

/// 重置系统机器码（生成新的随机机器码）
#[tauri::command]
async fn reset_machine_id() -> Result<String> {
    machine::reset_machine_guid().map_err(ApiError::from)
}

/// 设置系统机器码为指定值
#[tauri::command]
async fn set_machine_id(machine_id: String) -> Result<()> {
    machine::set_machine_guid(&machine_id).map_err(ApiError::from)
}

/// 绑定账号机器码（保存当前系统机器码到账号）
#[tauri::command]
async fn bind_account_machine_id(account_id: String, state: State<'_, AppState>) -> Result<String> {
    let mut manager = state.account_manager.lock().await;
    manager.bind_machine_id(&account_id).map_err(ApiError::from)
}

/// 获取 Trae IDE 的机器码
#[tauri::command]
async fn get_trae_machine_id() -> Result<String> {
    machine::get_trae_machine_id().map_err(ApiError::from)
}

/// 设置 Trae IDE 的机器码
#[tauri::command]
async fn set_trae_machine_id(machine_id: String) -> Result<()> {
    machine::set_trae_machine_id(&machine_id).map_err(ApiError::from)
}

/// 清除 Trae IDE 登录状态（让 IDE 变成全新安装状态）
#[tauri::command]
async fn clear_trae_login_state() -> Result<()> {
    machine::clear_trae_login_state().map_err(ApiError::from)
}

/// 获取保存的 Trae IDE 路径
#[tauri::command]
async fn get_trae_path() -> Result<String> {
    machine::get_saved_trae_path().map_err(ApiError::from)
}

/// 设置 Trae IDE 路径
#[tauri::command]
async fn set_trae_path(path: String) -> Result<()> {
    machine::save_trae_path(&path).map_err(ApiError::from)
}

/// 自动扫描 Trae IDE 路径
#[tauri::command]
async fn scan_trae_path() -> Result<String> {
    machine::scan_trae_path().map_err(ApiError::from)
}

/// 检查更新
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<serde_json::Value>> {
    let updater = app.updater().map_err(|e| {
        ApiError::from(anyhow::anyhow!("获取更新器失败: {}", e))
    })?;
    
    match updater.check().await {
        Ok(Some(update)) => {
            let info = serde_json::json!({
                "version": update.version,
                "current_version": update.current_version,
                "body": update.body,
                "date": update.date.map(|d| d.to_string())
            });
            Ok(Some(info))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(ApiError::from(anyhow::anyhow!("检查更新失败: {}", e)))
    }
}

/// 下载并安装更新
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<()> {
    let updater = app.updater().map_err(|e| ApiError::from(anyhow::anyhow!("获取更新器失败: {}", e)))?;
    
    if let Some(update) = updater.check().await.map_err(|e| ApiError::from(anyhow::anyhow!("检查更新失败: {}", e)))? {
        update.download_and_install(|_, _| {}, || {}).await.map_err(|e| ApiError::from(anyhow::anyhow!("下载安装失败: {}", e)))?;
    }
    
    Ok(())
}

/// 打开购买页面（内置浏览器，携带账号 Cookies）
#[tauri::command]
async fn open_pricing(account_id: String, app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let account = {
        let manager = state.account_manager.lock().await;
        manager.get_account(&account_id).map_err(ApiError::from)?
    };

    // 如果窗口已存在，先关闭它
    if let Some(existing) = app.get_webview_window("trae-pricing") {
        // 使用 destroy 强制销毁窗口
        let _ = existing.destroy();
        // 等待窗口完全销毁
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }
    // 再次检查确保窗口已关闭
    if app.get_webview_window("trae-pricing").is_some() {
        return Err(anyhow::anyhow!("无法关闭已存在的购买窗口，请重启应用后重试").into());
    }

    let cookies = account.cookies.clone();
    let cookies_for_js = cookies.replace('\\', "\\\\").replace('`', "\\`");
    let js_onload = format!(
        r#"
(() => {{
  try {{
    // 只在 trae.com.cn 域名下执行（国内版）
    if (!location.hostname.endsWith('trae.com.cn')) return;

    // 如果已经在 pricing 页面且已注入过，就不再执行
    if (location.href.includes('/pricing') && sessionStorage.getItem('trae_auth_injected')) return;

    console.log('[pricing] Starting auth injection...');

    // 1. 尽力清除旧数据 (JS 能访问到的)
    try {{
        localStorage.clear();
        sessionStorage.clear();
        const oldCookies = document.cookie.split(";");
        for (let i = 0; i < oldCookies.length; i++) {{
            const cookie = oldCookies[i];
            const eqPos = cookie.indexOf("=");
            const name = eqPos > -1 ? cookie.substr(0, eqPos).trim() : cookie.trim();
            document.cookie = name + "=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=/;domain=.trae.com.cn";
            document.cookie = name + "=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=/;domain=www.trae.com.cn";
            document.cookie = name + "=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=/";
        }}
    }} catch (e) {{
        console.warn('[pricing] Clear old data failed', e);
    }}

    // 2. 注入新 Cookie
    const raw = `{cookies}`;
    const parts = raw ? raw.split(';').map(s => s.trim()).filter(Boolean) : [];
    const seen = new Set();
    for (const kv of parts) {{
      const idx = kv.indexOf('=');
      if (idx <= 0) continue;
      const name = kv.slice(0, idx);
      const value = kv.slice(idx + 1);
      if (seen.has(name)) continue;
      seen.add(name);
      document.cookie = `${{name}}=${{value}}; path=/; domain=.trae.com.cn; secure; samesite=lax`;
    }}
    // 补全 IDC cookie
    if (!raw.includes('store-idc=') && !raw.includes('trae-target-idc=')) {{
      document.cookie = `store-idc=alicn; path=/; domain=.trae.com.cn; secure; samesite=lax`;
    }}
    
    // 3. 标记并跳转
    sessionStorage.setItem('trae_auth_injected', 'true');
    
    if (!location.href.includes('/pricing')) {{
        console.log('[pricing] Redirecting to pricing...');
        window.location.href = "https://www.trae.com.cn/pricing";
    }} else {{
        console.log('[pricing] Reloading to apply cookies...');
        location.reload();
    }}
  }} catch (e) {{
    console.error('[pricing] cookie inject error', e);
  }}
}})();
"#,
        cookies = cookies_for_js
    );

    let script_onload = js_onload.clone();
    let webview = WebviewWindowBuilder::new(
        &app,
        "trae-pricing",
        WebviewUrl::External("about:blank".parse().unwrap()),
    )
    .title("Trae 购买 Pro")
    .inner_size(1000.0, 720.0)
    .on_page_load(move |window, payload| {
        if payload.event() == PageLoadEvent::Finished {
            let _ = window.eval(script_onload.clone());
        }
    })
    .build()
    .map_err(|e| anyhow::anyhow!("无法打开购买窗口: {}", e))?;

    // 强制清理数据
    let _ = webview.clear_all_browsing_data();

    // 先导航到一个轻量页(404)来建立域上下文并执行注入，然后再由脚本跳转到 pricing
    // 这样可以确保 Cookie 在请求 pricing 之前就已经准备好
    let _ = webview.navigate(Url::parse("https://www.trae.com.cn/404_auth_init").unwrap());
    let _ = webview.set_focus();
    Ok(())
}

/// 获取用户统计数据
#[tauri::command]
async fn get_user_statistics(account_id: String, state: State<'_, AppState>) -> Result<UserStatisticResult> {
    let manager = state.account_manager.lock().await;
    manager.get_account_statistics(&account_id).await.map_err(ApiError::from)
}

/// 单账号签到（手动触发，右键菜单）
#[tauri::command]
async fn checkin_account(account_id: String, state: State<'_, AppState>) -> Result<api::checkin::CheckinResult> {
    api::checkin::checkin_one_account(&state.account_manager, &account_id)
        .await
        .map_err(ApiError::from)
}

/// 全部账号签到（手动触发，工具栏按钮）
#[tauri::command]
async fn checkin_all_accounts(state: State<'_, AppState>) -> Result<Vec<api::checkin::CheckinResult>> {
    api::checkin::checkin_all(&state.account_manager)
        .await
        .map_err(ApiError::from)
}

/// 全部 TraeWork 账号签到（手动触发，TraeWork 面板按钮）
///
/// 不复用 `checkin_all_accounts`：那个按钮的语义是「只签 TraeCode」，TraeWork 走
/// 自己的批次（凭据按快照解析）。单账号签到复用已注册的 `checkin_account`（按 id
/// 直调，不经过列表过滤），不新增命令。
#[tauri::command]
async fn traework_checkin_all(state: State<'_, AppState>) -> Result<Vec<api::checkin::CheckinResult>> {
    api::checkin::checkin_all_traework(&state.account_manager)
        .await
        .map_err(ApiError::from)
}

/// 自动签到（方案B）：只处理「今日未签到」的账号，启动时静默调用
#[tauri::command]
async fn auto_checkin(state: State<'_, AppState>) -> Result<Vec<api::checkin::CheckinResult>> {
    api::checkin::auto_checkin_pending(&state.account_manager)
        .await
        .map_err(ApiError::from)
}

/// 重置单账号签到设备号（9095「本设备今日已签到」的自救手段）
///
/// 为什么也要走 `try_acquire`：批次签到进行中执行重置，批次末尾的 `apply_checkin_outcomes`
/// 会用重置前收集的旧日期/旧冷却写回、覆盖重置效果；互斥后不存在这个窗口。
/// 红线提醒：重置会清 `last_checkin_date`，但签到流程强制「claim 前先查 status」，
/// 今日已真签到成功的账号重试会被短路为 Already，不会重复获得积分。
#[tauri::command]
async fn reset_account_device_id(account_id: String, state: State<'_, AppState>) -> Result<()> {
    let _guard = api::checkin_guard::try_acquire()
        .ok_or_else(|| ApiError::from(anyhow::anyhow!("签到进行中，请稍候再试")))?;

    let new_device_id = api::device_id::random_device_id();
    let mut manager = state.account_manager.lock().await;
    manager
        .reset_account_device_id(&account_id, new_device_id)
        .map_err(ApiError::from)
}

async fn handle_silent_start() -> anyhow::Result<()> {
    let manager = Mutex::new(AccountManager::new()?);

    // 1. Refresh all accounts
    let account_ids: Vec<String> = {
        let guard = manager.lock().await;
        guard.get_accounts().into_iter().map(|a| a.id).collect()
    };
    for id in account_ids {
        let _ = manager.lock().await.refresh_token(&id).await;
    }

    // 2. 自动签到（方案B）：只处理今日未签到的账号，静默执行，失败不阻断启动流程
    match api::checkin::auto_checkin_pending(&manager).await {
        Ok(results) if !results.is_empty() => {
            log::info!("静默启动自动签到完成，共处理 {} 个账号", results.len());
        }
        Ok(_) => {}
        Err(e) => log::warn!("静默启动自动签到失败: {}", e),
    }

    // 3. Sync with Trae IDE if it's not running
    if !machine::is_trae_running() {
        let accounts = {
            let guard = manager.lock().await;
            guard.get_accounts()
        };
        if let Some(current) = accounts.iter().find(|a| a.is_current) {
            let account = {
                let guard = manager.lock().await;
                guard.get_account(&current.id).ok()
            };
            if let Some(account) = account {
                if let Some(token) = account.jwt_token {
                    let login_info = machine::TraeLoginInfo {
                        token,
                        refresh_token: None,
                        user_id: account.user_id,
                        email: account.email,
                        username: account.name,
                        avatar_url: account.avatar_url,
                        host: String::new(),
                        region: if account.region.is_empty() { "SG".to_string() } else { account.region },
                    };
                    let _ = machine::write_trae_login_info(&login_info);
                }
            }
        }
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logger first
    let _ = logger::init_logger();
    
    // Set up panic handler
    std::panic::set_hook(Box::new(|info| {
        logger::log_panic(info);
        // Also show a message box on Windows
        #[cfg(target_os = "windows")]
        {
            use std::ffi::CString;
            use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxA, MB_ICONERROR, MB_OK};
            let message = format!("应用程序发生错误:\n{}\n\n请查看日志文件获取详细信息。", info);
            if let Ok(c_message) = CString::new(message) {
                if let Ok(c_title) = CString::new("Trae账号管理 - 错误") {
                    unsafe {
                        MessageBoxA(
                            std::ptr::null_mut(),
                            c_message.as_ptr() as *const u8,
                            c_title.as_ptr() as *const u8,
                            MB_OK | MB_ICONERROR,
                        );
                    }
                }
            }
        }
    }));
    
    log::info!("Application starting...");
    
    // Check for silent flag
    let args: Vec<String> = std::env::args().collect();
    if args.contains(&"--silent".to_string()) {
        #[cfg(target_os = "windows")]
        hide_console_window();
        let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        rt.block_on(async {
            let _ = handle_silent_start().await;
        });
        std::process::exit(0);
    }

    log::info!("Initializing account manager...");
    let account_manager = AccountManager::new().expect("无法初始化账号管理器");
    let settings = load_settings_from_disk().unwrap_or_default();
    let _ = autostart::set_auto_start(settings.auto_start_enabled);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState {
            account_manager: Mutex::new(account_manager),
            browser_login: Mutex::new(None),
            browser_login_cancel: Mutex::new(None),
            settings: Mutex::new(settings),
        })
        .invoke_handler(tauri::generate_handler![
            add_account_by_token,
            add_account_by_email,
            get_settings,
            update_settings,
            download_and_run_installer,
            start_browser_login,
            finish_browser_login,
            cancel_browser_login,
            browser_auto_login_command,
            remove_account,
            get_accounts,
            get_account,
            switch_account,
            get_account_usage,
            update_account_token,
            refresh_token,
            refresh_token_with_password,
            login_account_with_email,
            update_account_profile,
            export_accounts,
            export_accounts_to_path,
            import_accounts,
            clear_accounts,
            get_usage_events,
            read_trae_account,
            get_machine_id,
            reset_machine_id,
            set_machine_id,
            bind_account_machine_id,
            get_trae_machine_id,
            set_trae_machine_id,
            clear_trae_login_state,
            get_trae_path,
            set_trae_path,
            scan_trae_path,
            get_user_statistics,
            checkin_account,
            checkin_all_accounts,
            traework_checkin_all,
            auto_checkin,
            reset_account_device_id,
            open_pricing,
            check_update,
            install_update,
            get_logs,
            export_logs_cmd,
            clear_logs_cmd,
            get_log_file_path_cmd,
            get_app_paths,
            get_runtime_status,
            traework::commands::traework_overview,
            traework::commands::traework_discover,
            traework::commands::traework_credits,
            traework::commands::traework_save_current_login,
            traework::commands::traework_switch_account,
            traework::commands::traework_delete_snapshot,
            traework::commands::traework_remove_account,
            traework::commands::traework_set_path,
            traework::commands::traework_scan_path,
            traework::commands::traework_reconcile,
        ])
        .setup(|app| {
            // 获取主窗口：先恢复上次的尺寸/位置（无记录时静默跳过），再显示
            if let Some(window) = app.get_webview_window("main") {
                match window_state::restore(&window.as_ref().window()) {
                    Ok(()) => log::info!("窗口状态已恢复（若为首次启动则无记录）"),
                    Err(e) => log::warn!("窗口状态恢复失败: {e}"),
                }
                window.show().unwrap();
                window.set_focus().unwrap();
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // 仅在主窗口关闭时才退出应用
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "main" {
                    // 此刻窗口还是用户拖完的样子，是保存窗口几何的最后时机：
                    // 下面 process::exit(0) 会直接终止进程，任何挂在退出事件上的
                    // 保存逻辑都不会执行（见 window_state 模块头注）
                    match window_state::save(window) {
                        Ok(()) => log::info!("窗口状态已保存，下次启动恢复"),
                        Err(e) => log::warn!("窗口状态保存失败: {e}"),
                    }
                    api.prevent_close();
                    std::process::exit(0);
                }
            }
            _ => {}
        })

        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// Logger commands
#[tauri::command]
async fn get_logs(count: usize) -> std::result::Result<Vec<String>, String> {
  logger::get_recent_logs(count).map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_logs_cmd(path: String) -> std::result::Result<(), String> {
  let path_buf = std::path::PathBuf::from(path);
  logger::export_logs(&path_buf).map_err(|e| e.to_string())
}

#[tauri::command]
async fn clear_logs_cmd() -> std::result::Result<(), String> {
  logger::clear_logs().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_log_file_path_cmd() -> std::result::Result<String, String> {
  Ok(logger::get_log_file_path().to_string_lossy().to_string())
}

/// 应用四类数据路径（设置页「数据与备份」整组取用）
///
/// 为什么四条路径合成一个命令而不是四个：设置页要一次渲染整组，四次 invoke 既慢，
/// 又会让每个「打开所在目录」按钮各自处理取路径失败；路径口径集中在后端一处，
/// 也避免前端自己拼 `%APPDATA%\hhj\trae-cc` 时把大小写拼错。
/// 单项取不到时返回 `None` 而非整体报错——一个目录缺失不该让整组信息都看不见。
#[derive(serde::Serialize)]
struct AppPaths {
    accounts: Option<String>,
    settings: Option<String>,
    logs: Option<String>,
    traework_profiles: Option<String>,
}

#[tauri::command]
async fn get_app_paths() -> AppPaths {
    AppPaths {
        accounts: AccountManager::get_data_path()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        settings: get_settings_path()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        logs: Some(logger::get_log_file_path().to_string_lossy().to_string()),
        traework_profiles: traework::profile::profiles_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
    }
}

/// 运行时进程状态（设置页状态面板）
///
/// 为什么值得单开一个命令：设置页的操作几乎都有前置条件——清除登录状态要求 Trae 已关闭、
/// 保存/切换 TraeWork 快照要求目标客户端没有进程占用。这些前提此前只写在警告文案里，
/// 用户得自己猜；把「谁在运行」摆在按钮上方比写三行提示有效。
#[derive(serde::Serialize)]
struct RuntimeStatus {
    trae_running: bool,
    traework_running: bool,
}

#[tauri::command]
async fn get_runtime_status() -> Result<RuntimeStatus> {
    // 进程枚举是系统调用（EnumWindows / tasklist，后者要起子进程），
    // 放到阻塞线程里跑，别占着 async 运行时。
    let (trae_running, traework_running) = tauri::async_runtime::spawn_blocking(|| {
        (machine::is_trae_running(), traework::proc::is_running())
    })
    .await
    .map_err(|e| ApiError::from(anyhow::anyhow!("获取进程状态失败: {}", e)))?;

    Ok(RuntimeStatus {
        trae_running,
        traework_running,
    })
}
