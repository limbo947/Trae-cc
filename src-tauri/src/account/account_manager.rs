use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use super::types::*;
use crate::api::{TraeApiClient, UsageSummary, UsageQueryResponse, login_with_email};

/// 账号管理器
pub struct AccountManager {
    store: AccountStore,
    data_path: PathBuf,
}

impl AccountManager {
    /// 创建账号管理器
    pub fn new() -> Result<Self> {
        let data_path = Self::get_data_path()?;
        let mut store = Self::load_store(&data_path)?;

        // 确保每个账号都有机器码
        let mut changed = false;
        for account in &mut store.accounts {
            if account.machine_id.is_none() {
                account.machine_id = Some(Uuid::new_v4().to_string());
                changed = true;
            }
            // 签到设备号回填：只补空值，**绝不无条件重算**——重算会把所有账号静默换号，
            // 服务端视为新设备（等价于批量重置），也会让当日已签到判定失效
            if account.device_id.as_deref().map_or(true, |v| v.trim().is_empty()) {
                account.device_id = Some(crate::api::device_id::resolve_device_id(account));
                changed = true;
            }
        }

        let manager = Self { store, data_path };

        if changed {
            manager.save_store()?;
        }

        Ok(manager)
    }

    /// 按 `user_id` 在 **traecode** 账号中查找下标
    ///
    /// 为什么查询必须带 app 条件：`Account.user_id` 对 traecode 存的是 API 返回的用户 id、
    /// 对 TraeWork 存的是快照 uid，而**两者值域相同**（都是同一个 Trae userId），因此同一个账号
    /// 在两个应用下各有一条记录——`user_id` 相同、`app` 不同，这是设计而非重复数据。
    /// 不限定 app 的去重会把 TraeWork 记录误当成 TraeCode 账号，表现为两类事故：
    /// ① 账号管理页看不到该账号却报「已在列表中」（2026-09-20 实测报障）；
    /// ② 把 TraeCode 的 cookies/JWT 写进 TraeWork 记录，让一条记录同时承载两套凭据。
    fn traecode_index_by_user_id(&self, user_id: &str) -> Option<usize> {
        self.store
            .accounts
            .iter()
            .position(|a| a.is_traecode() && a.user_id == user_id)
    }

    /// 获取账号库文件路径（`accounts.json`）
    ///
    /// 为什么公开：设置页「数据与备份」要把这个路径展示给用户并在资源管理器中定位，
    /// 由后端统一给出可以避免前端自己拼 `%APPDATA%\hhj\trae-cc\data` 而拼错大小写。
    pub fn get_data_path() -> Result<PathBuf> {
        let proj_dirs = directories::ProjectDirs::from("com", "hhj", "trae-cc")
            .ok_or_else(|| anyhow!("无法获取应用数据目录"))?;

        let data_dir = proj_dirs.data_dir();
        fs::create_dir_all(data_dir)?;

        Ok(data_dir.join("accounts.json"))
    }

    /// 加载账号存储
    fn load_store(path: &PathBuf) -> Result<AccountStore> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let cleaned = content.trim_start_matches('\u{feff}');
            let trimmed = cleaned.trim();
            if trimmed.is_empty() {
                return Ok(AccountStore::default());
            }
            match serde_json::from_str::<AccountStore>(trimmed) {
                Ok(store) => Ok(store),
                Err(_) => {
                    let store = AccountStore::default();
                    let content = serde_json::to_string_pretty(&store)?;
                    fs::write(path, content)?;
                    Ok(store)
                }
            }
        } else {
            Ok(AccountStore::default())
        }
    }

    /// 保存账号存储
    fn save_store(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.store)?;
        fs::write(&self.data_path, content)?;
        Ok(())
    }

    pub fn update_account_email(&mut self, account_id: &str, email: String) -> Result<()> {
        let email = email.trim();
        if email.is_empty() {
            return Ok(());
        }

        let account = self.store.accounts.iter_mut()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        account.email = email.to_string();
        account.updated_at = chrono::Utc::now().timestamp();
        self.save_store()?;
        Ok(())
    }

    pub fn update_account_profile(
        &mut self,
        account_id: &str,
        email: Option<String>,
        password: Option<String>,
    ) -> Result<Account> {
        let account_index = self.store.accounts
            .iter()
            .position(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let mut changed = false;
        let account_snapshot = {
            let account = &mut self.store.accounts[account_index];
            if let Some(next_email) = email {
                let trimmed = next_email.trim();
                if !trimmed.is_empty() && trimmed != account.email {
                    account.email = trimmed.to_string();
                    changed = true;
                }
            }

            if let Some(next_password) = password {
                let trimmed = next_password.trim();
                let next_value = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                if next_value != account.password {
                    account.password = next_value;
                    changed = true;
                }
            }

            if changed {
                account.updated_at = chrono::Utc::now().timestamp();
            }

            account.clone()
        };

        if changed {
            self.save_store()?;
        }

        Ok(account_snapshot)
    }

    /// 添加账号（通过 cookies）
    /// 如果账号已存在，则更新账号信息
    pub async fn add_account(&mut self, cookies: String, password: Option<String>) -> Result<Account> {
        let mut client = TraeApiClient::new(&cookies)?;

        // 获取 token
        let token_result = client.get_user_token().await?;

        // 获取用户信息
        let user_info = client.get_user_info().await?;

        // 检查是否已存在
        // 只在 traecode 账号中比对：TraeWork 记录可能持有同一个 user_id（见 traecode_index_by_user_id）
        let existing_index = self.traecode_index_by_user_id(&token_result.user_id);
        
        if let Some(index) = existing_index {
            // 账号已存在，更新信息
            {
                let existing_account = &mut self.store.accounts[index];
                existing_account.cookies = cookies;
                existing_account.jwt_token = Some(token_result.token);
                existing_account.token_expired_at = Some(token_result.expired_at);
                existing_account.name = user_info.screen_name.clone();
                existing_account.email = user_info.non_plain_text_email.unwrap_or_default();
                existing_account.avatar_url = user_info.avatar_url.clone();
                existing_account.region = user_info.region.clone();
                existing_account.tenant_id = token_result.tenant_id.clone();
                if let Some(pass) = password {
                    existing_account.password = Some(pass);
                }
                existing_account.updated_at = chrono::Utc::now().timestamp();
            }
            
            self.save_store()?;
            return Ok(self.store.accounts[index].clone());
        }

        let mut account = Account::new(
            user_info.screen_name.clone(),
            user_info.non_plain_text_email.unwrap_or_default(),
            cookies,
            token_result.user_id,
            token_result.tenant_id,
        );

        account.avatar_url = user_info.avatar_url;
        account.region = user_info.region;
        account.jwt_token = Some(token_result.token);
        account.token_expired_at = Some(token_result.expired_at);
        account.password = password;

        self.store.accounts.push(account.clone());

        // 如果是第一个账号，设为活跃账号
        if self.store.active_account_id.is_none() {
            self.store.active_account_id = Some(account.id.clone());
        }

        self.save_store()?;
        Ok(account)
    }

    /// 添加账号（通过 Token，可选 Cookies）
    /// 如果账号已存在，则更新账号信息
    /// 如果 Token 不是 JWT 格式，会尝试使用 Cookies 添加账号
    pub async fn add_account_by_token(&mut self, token: String, cookies: Option<String>, password: Option<String>) -> Result<Account> {
        // 检查 Token 是否是 JWT 格式（包含两个点号）
        let is_jwt = token.split('.').count() == 3;
        
        if !is_jwt {
            // 尝试使用 Cookies 添加账号
            if let Some(ref cookies_str) = cookies {
                if !cookies_str.is_empty() {
                    return self.add_account(cookies_str.clone(), password).await;
                }
            }
            return Err(anyhow!("Token 不是有效的 JWT 格式，且没有提供 Cookies"));
        }
        
        let client = TraeApiClient::new_with_token(&token)?;

        // 通过 Token 获取用户信息
        let user_info = client.get_user_info_by_token().await?;

        // 检查是否已存在
        // 只在 traecode 账号中比对（同上）
        let existing_index = self.traecode_index_by_user_id(&user_info.user_id);
        
        if let Some(index) = existing_index {
            // 账号已存在，更新信息
            println!("[AccountManager] 账号已存在，更新信息: user_id={}", user_info.user_id);
            
            // 如果提供了 Cookies，尝试获取更详细的用户信息
            let (name, email, avatar_url) = if let Some(ref cookies_str) = cookies {
                match self.get_user_info_with_cookies(cookies_str).await {
                    Ok(info) => (
                        info.screen_name,
                        info.non_plain_text_email.unwrap_or_default(),
                        info.avatar_url,
                    ),
                    Err(_) => (
                        user_info.screen_name.clone().unwrap_or_else(|| format!("User_{}", &user_info.user_id[..8.min(user_info.user_id.len())])),
                        user_info.email.clone().unwrap_or_default(),
                        user_info.avatar_url.clone().unwrap_or_default(),
                    ),
                }
            } else {
                (
                    user_info.screen_name.clone().unwrap_or_else(|| format!("User_{}", &user_info.user_id[..8.min(user_info.user_id.len())])),
                    user_info.email.clone().unwrap_or_default(),
                    user_info.avatar_url.clone().unwrap_or_default(),
                )
            };
            
            {
                let existing_account = &mut self.store.accounts[index];
                existing_account.jwt_token = Some(token);
                existing_account.token_expired_at = None;
                if let Some(cookie_str) = cookies.as_ref().filter(|v| !v.is_empty()) {
                    existing_account.cookies = cookie_str.to_string();
                }
                if let Some(pass) = password {
                    existing_account.password = Some(pass);
                }
                if !name.trim().is_empty() {
                    existing_account.name = name;
                }
                if !email.trim().is_empty() {
                    existing_account.email = email;
                }
                if !avatar_url.trim().is_empty() {
                    existing_account.avatar_url = avatar_url;
                }
                if !user_info.tenant_id.trim().is_empty() {
                    existing_account.tenant_id = user_info.tenant_id.clone();
                }
                existing_account.updated_at = chrono::Utc::now().timestamp();
            }
            
            self.save_store()?;
            return Ok(self.store.accounts[index].clone());
        }

        // 如果提供了 Cookies，尝试获取更详细的用户信息
        let (name, email, avatar_url) = if let Some(ref cookies_str) = cookies {
            match self.get_user_info_with_cookies(cookies_str).await {
                Ok(info) => (
                    info.screen_name,
                    info.non_plain_text_email.unwrap_or_default(),
                    info.avatar_url,
                ),
                Err(_) => (
                    user_info.screen_name.unwrap_or_else(|| format!("User_{}", &user_info.user_id[..8.min(user_info.user_id.len())])),
                    user_info.email.unwrap_or_default(),
                    user_info.avatar_url.unwrap_or_default(),
                ),
            }
        } else {
            (
                user_info.screen_name.unwrap_or_else(|| format!("User_{}", &user_info.user_id[..8.min(user_info.user_id.len())])),
                user_info.email.unwrap_or_default(),
                user_info.avatar_url.unwrap_or_default(),
            )
        };

        let mut account = Account::new(
            name,
            email,
            cookies.unwrap_or_default(),
            user_info.user_id.clone(),
            user_info.tenant_id.clone(),
        );

        account.avatar_url = avatar_url;
        account.jwt_token = Some(token);
        account.token_expired_at = None;
        account.password = password;

        self.store.accounts.push(account.clone());

        // 如果是第一个账号，设为活跃账号
        if self.store.active_account_id.is_none() {
            self.store.active_account_id = Some(account.id.clone());
        }

        self.save_store()?;
        Ok(account)
    }

    /// Upsert account by token/cookies and refresh profile when it already exists.
    pub async fn upsert_account_by_token(
        &mut self,
        token: String,
        cookies: Option<String>,
        password: Option<String>,
    ) -> Result<Account> {
        let client = TraeApiClient::new_with_token(&token)?;
        let user_info = client.get_user_info_by_token().await?;

        if let Some(existing_id) = self
            .traecode_index_by_user_id(&user_info.user_id)
            .map(|i| self.store.accounts[i].id.clone())
        {
            // 先准备刷新账号信息（优先使用 cookies）
            let (name, email, avatar_url, region, tenant_id) = if let Some(ref cookies_str) = cookies {
                match self.get_user_info_with_cookies(cookies_str).await {
                    Ok(info) => (
                        info.screen_name,
                        info.non_plain_text_email.unwrap_or_default(),
                        info.avatar_url,
                        info.region,
                        info.tenant_id,
                    ),
                    Err(_) => (
                        user_info.screen_name.clone().unwrap_or_else(|| format!("User_{}", &user_info.user_id[..8.min(user_info.user_id.len())])),
                        user_info.email.clone().unwrap_or_default(),
                        user_info.avatar_url.clone().unwrap_or_default(),
                        String::new(),
                        user_info.tenant_id.clone(),
                    ),
                }
            } else {
                (
                    user_info.screen_name.clone().unwrap_or_else(|| format!("User_{}", &user_info.user_id[..8.min(user_info.user_id.len())])),
                    user_info.email.clone().unwrap_or_default(),
                    user_info.avatar_url.clone().unwrap_or_default(),
                    String::new(),
                    user_info.tenant_id.clone(),
                )
            };

            let updated = if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == existing_id) {
                acc.jwt_token = Some(token.clone());
                acc.token_expired_at = None;
                if let Some(cookie_str) = cookies.as_ref().filter(|v| !v.is_empty()) {
                    acc.cookies = cookie_str.to_string();
                }
                if let Some(pass) = password.as_ref().filter(|v| !v.is_empty()) {
                    acc.password = Some(pass.to_string());
                }
                if !name.trim().is_empty() {
                    acc.name = name;
                }
                if !email.trim().is_empty() {
                    acc.email = email;
                }
                if !avatar_url.trim().is_empty() {
                    acc.avatar_url = avatar_url;
                }
                if !region.trim().is_empty() {
                    acc.region = region;
                }
                if !tenant_id.trim().is_empty() {
                    acc.tenant_id = tenant_id;
                }
                acc.updated_at = chrono::Utc::now().timestamp();
                Some(acc.clone())
            } else {
                None
            };

            if let Some(updated) = updated {
                self.save_store()?;
                return Ok(updated);
            }
        }

        self.add_account_by_token(token, cookies, password).await
    }

    /// 使用 Cookies 获取用户信息
    async fn get_user_info_with_cookies(&self, cookies: &str) -> Result<crate::api::UserInfoResult> {
        let client = TraeApiClient::new(cookies)?;
        client.get_user_info().await
    }

    /// 添加账号（通过邮箱密码登录）
    /// 如果账号已存在，则更新账号信息
    pub async fn add_account_by_email(&mut self, email: String, password: String) -> Result<Account> {
        // 通过邮箱密码登录
        let login_result = login_with_email(&email, &password).await?;

        // 检查是否已存在
        let existing_index = self.traecode_index_by_user_id(&login_result.user_id);
        
        if let Some(index) = existing_index {
            // 账号已存在，更新信息
            
            // 使用 Token 获取完整的用户信息
            let client = TraeApiClient::new_with_token(&login_result.token)?;
            let user_info = client.get_user_info_by_token().await?;
            
            {
                let existing_account = &mut self.store.accounts[index];
                existing_account.cookies = login_result.cookies;
                existing_account.jwt_token = Some(login_result.token);
                existing_account.token_expired_at = Some(login_result.expired_at);
                existing_account.password = Some(password.clone());
                if !email.trim().is_empty() {
                    existing_account.email = email;
                }
                if let Some(name) = user_info.screen_name {
                    if !name.trim().is_empty() {
                        existing_account.name = name;
                    }
                }
                if let Some(avatar) = user_info.avatar_url {
                    if !avatar.trim().is_empty() {
                        existing_account.avatar_url = avatar;
                    }
                }
                if !login_result.tenant_id.trim().is_empty() {
                    existing_account.tenant_id = login_result.tenant_id;
                }
                existing_account.updated_at = chrono::Utc::now().timestamp();
            }
            
            self.save_store()?;
            return Ok(self.store.accounts[index].clone());
        }

        // 使用 Token 获取完整的用户信息
        let client = TraeApiClient::new_with_token(&login_result.token)?;
        let user_info = client.get_user_info_by_token().await?;

        let mut account = Account::new(
            user_info.screen_name.unwrap_or_else(|| email.split('@').next().unwrap_or("User").to_string()),
            email,
            login_result.cookies,
            login_result.user_id,
            login_result.tenant_id,
        );

        account.avatar_url = user_info.avatar_url.unwrap_or_default();
        account.jwt_token = Some(login_result.token);
        account.token_expired_at = Some(login_result.expired_at);
        account.password = Some(password);

        self.store.accounts.push(account.clone());

        // 如果是第一个账号，设为活跃账号
        if self.store.active_account_id.is_none() {
            self.store.active_account_id = Some(account.id.clone());
        }

        self.save_store()?;
        Ok(account)
    }

    /// 删除账号
    pub fn remove_account(&mut self, account_id: &str) -> Result<()> {
        let index = self
            .store
            .accounts
            .iter()
            .position(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        self.store.accounts.remove(index);

        // 如果删除的是活跃账号，重置活跃账号
        if self.store.active_account_id.as_deref() == Some(account_id) {
            self.store.active_account_id = self.store.accounts.first().map(|a| a.id.clone());
        }

        self.save_store()?;
        Ok(())
    }

    /// 清空所有账号
    pub fn clear_accounts(&mut self) -> Result<usize> {
        let count = self.store.accounts.len();
        self.store.accounts.clear();
        self.store.active_account_id = None;
        self.store.current_account_id = None;
        self.save_store()?;
        Ok(count)
    }

    /// 设置活跃账号
    pub fn set_active_account(&mut self, account_id: &str) -> Result<()> {
        if !self.store.accounts.iter().any(|a| a.id == account_id) {
            return Err(anyhow!("账号不存在"));
        }

        self.store.active_account_id = Some(account_id.to_string());
        self.save_store()?;
        Ok(())
    }

    /// 切换账号（设置活跃账号并将登录信息写入 Trae IDE）
    pub async fn switch_account(&mut self, account_id: &str, force: bool) -> Result<()> {
        // 检查是否已经是当前使用的账号
        if !force && self.store.current_account_id.as_deref() == Some(account_id) {
            return Err(anyhow!("该账号已经是当前使用的账号"));
        }

        let account = self.store.accounts.iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .clone();

        // 检查 Token 是否过期，如果过期则尝试刷新
        let token = if let Some(token) = &account.jwt_token {
            if let Some(expired_at) = &account.token_expired_at {
                let expired = chrono::DateTime::parse_from_rfc3339(expired_at)
                    .map(|dt| dt.with_timezone(&chrono::Utc) < chrono::Utc::now())
                    .unwrap_or(true);
                
                if expired && !account.cookies.is_empty() {
                    // Token 已过期，尝试使用 Cookies 刷新
                    let mut cookie_client = TraeApiClient::new(&account.cookies)?;
                    match cookie_client.get_user_token().await {
                        Ok(token_result) => {
                            // 更新存储的 Token
                            if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
                                acc.jwt_token = Some(token_result.token.clone());
                                acc.token_expired_at = Some(token_result.expired_at.clone());
                            }
                            self.save_store()?;
                            token_result.token
                        }
                        Err(e) => {
                            return Err(anyhow!("Token 已过期且刷新失败: {}", e));
                        }
                    }
                } else if expired {
                    return Err(anyhow!("Token 已过期且没有 Cookies 可以刷新"));
                } else {
                    token.clone()
                }
            } else {
                token.clone()
            }
        } else if !account.cookies.is_empty() {
            // 没有 Token 但有 Cookies，尝试获取 Token
            let mut cookie_client = TraeApiClient::new(&account.cookies)?;
            match cookie_client.get_user_token().await {
                Ok(token_result) => {
                    // 更新存储的 Token
                    if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
                        acc.jwt_token = Some(token_result.token.clone());
                        acc.token_expired_at = Some(token_result.expired_at.clone());
                    }
                    self.save_store()?;
                    token_result.token
                }
                Err(e) => {
                    return Err(anyhow!("获取 Token 失败: {}", e));
                }
            }
        } else {
            return Err(anyhow!("账号没有有效的 Token 或 Cookies，无法切换"));
        };

        // 构建 Trae IDE 登录信息
        let login_info = crate::machine::TraeLoginInfo {
            token: token.clone(),
            refresh_token: None, // 如果有 refresh token 可以在这里设置
            user_id: account.user_id.clone(),
            email: account.email.clone(),
            username: account.name.clone(),
            avatar_url: account.avatar_url.clone(),
            host: String::new(), // 根据 region 自动选择
            region: if account.region.is_empty() { "SG".to_string() } else { account.region.clone() },
        };

        // 切换 Trae IDE 到该账号（清除旧登录状态并写入新账号信息，不自动启动）
        crate::machine::switch_trae_account(&login_info, account.machine_id.as_deref(), false)?;

        // 如果账号有绑定的机器码，也更新系统机器码
        if let Some(machine_id) = &account.machine_id {
            let _ = crate::machine::set_machine_guid(machine_id);
        }

        // 设置活跃账号和当前使用的账号
        self.store.active_account_id = Some(account_id.to_string());
        self.store.current_account_id = Some(account_id.to_string());
        self.save_store()?;

        Ok(())
    }

    /// 绑定当前系统机器码到账号
    pub fn bind_machine_id(&mut self, account_id: &str) -> Result<String> {
        // 获取当前系统机器码
        let current_machine_id = crate::machine::get_machine_guid()?;

        // 更新账号的机器码
        let account = self.store.accounts.iter_mut()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        account.machine_id = Some(current_machine_id.clone());
        account.updated_at = chrono::Utc::now().timestamp();
        let email = account.email.clone();

        self.save_store()?;
        println!("[INFO] 已绑定机器码 {} 到账号 {}", current_machine_id, email);

        Ok(current_machine_id)
    }

    /// 获取所有账号列表
    pub fn get_accounts(&self) -> Vec<AccountBrief> {
        let current_id = self.store.current_account_id.as_deref();
        self.store.accounts.iter().map(|account| {
            let is_current = current_id == Some(account.id.as_str());
            AccountBrief::from_account(account, is_current)
        }).collect()
    }

    /// 获取活跃账号
    pub fn get_active_account(&self) -> Option<&Account> {
        self.store
            .active_account_id
            .as_ref()
            .and_then(|id| self.store.accounts.iter().find(|a| &a.id == id))
    }

    /// 获取指定账号
    pub fn get_account(&self, account_id: &str) -> Result<Account> {
        self.store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .cloned()
            .ok_or_else(|| anyhow!("账号不存在"))
    }

    /// 按 uid 落库/更新一个 TraeWork 账号，返回落库后的记录
    ///
    /// 为什么以 uid 为唯一键而不是新建一条：TraeWork 的登录态在 TraeWork 侧，账号库只是
    /// 「槽位索引」。「保存当前登录态」可能被同一账号反复触发，若每次都追加新记录，账号列表
    /// 会迅速被同一 uid 的重复项撑爆，且切换时无法判断该恢复哪个槽。
    ///
    /// 为什么不改 `current_account_id`：TraeWork 的「当前账号」以快照目录里的
    /// `current_account.txt` 为真源（与参考实现一致）。写进 `current_account_id` 会与
    /// traecode 的「Trae IDE 当前账号」语义冲突——两个应用可以同时登录不同账号。
    pub fn upsert_traework_account(&mut self, uid: &str, name: Option<String>) -> Result<Account> {
        let uid = uid.trim();
        if uid.is_empty() {
            return Err(anyhow!("TraeWork uid 不能为空"));
        }

        if let Some(existing) = self
            .store
            .accounts
            .iter_mut()
            .find(|a| a.app == APP_TRAEWORK && a.uid.as_deref() == Some(uid))
        {
            // 槽名缺失就地补齐（历史数据/手工导入可能没有）
            if existing.slot().is_none() {
                existing.snapshot_slot = Some(uid.to_string());
            }
            // 仅当调用方给了更好的名字（例如发现到邮箱）才覆盖，避免把用户手工改的名字打回 uid
            if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
                existing.name = n;
            }
            existing.updated_at = chrono::Utc::now().timestamp();
            let updated = existing.clone();
            self.save_store()?;
            return Ok(updated);
        }

        let mut account = Account::new_traework(
            uid.to_string(),
            name.filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| uid.to_string()),
        );
        account.snapshot_slot = Some(uid.to_string());
        self.store.accounts.push(account.clone());
        self.save_store()?;
        Ok(account)
    }

    /// 获取账号使用量
    pub async fn get_account_usage(&mut self, account_id: &str) -> Result<UsageSummary> {
        let account = self
            .store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .clone();

        // TraeWork 账号没有可查询额度的凭据（登录态在客户端 vscdb 里，本工具不持有）；
        // 直接给出可理解的错误，避免前端收到「Token 无效」这类指向错误方向的提示
        if !account.is_traecode() {
            return Err(anyhow!("TraeWork 账号不支持额度查询"));
        }

        // 根据账号类型选择不同的方式获取使用量
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

                        // 更新存储的 Token
                        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
                            acc.jwt_token = Some(token_result.token.clone());
                            acc.token_expired_at = Some(token_result.expired_at.clone());
                        }
                        self.save_store()?;

                        if self.store.current_account_id.as_deref() == Some(account_id) {
                            let login_info = crate::machine::TraeLoginInfo {
                                token: token_result.token.clone(),
                                refresh_token: None,
                                user_id: account.user_id.clone(),
                                email: account.email.clone(),
                                username: account.name.clone(),
                                avatar_url: account.avatar_url.clone(),
                                host: String::new(),
                                region: if account.region.is_empty() { "SG".to_string() } else { account.region.clone() },
                            };

                            let _ = crate::machine::write_trae_login_info(&login_info);
                            if crate::machine::is_trae_running() {
                                let _ = crate::machine::kill_trae();
                                let _ = crate::machine::open_trae();
                            }
                        }


                        // 使用新 Token 重新获取使用量
                        let new_client = TraeApiClient::new_with_token(&token_result.token)?;
                        new_client.get_usage_summary_by_token().await?
                    } else if error_msg.contains("401") {
                        return Err(anyhow!("登录已过期，请重新登录此账号"));
                    } else {
                        return Err(e);
                    }
                }
            }
        } else if !account.cookies.is_empty() {
            // 使用 Cookies
            let mut client = TraeApiClient::new(&account.cookies)?;
            client.get_usage_summary().await?
        } else {
            return Err(anyhow!("账号没有有效的 Token 或 Cookies"));
        };

        // 更新账号的 plan_type
        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.plan_type = summary.plan_type.clone();
            acc.updated_at = chrono::Utc::now().timestamp();
        }
        self.save_store()?;

        Ok(summary)
    }

    /// 刷新账号 Token
    pub async fn refresh_token(&mut self, account_id: &str) -> Result<()> {
        let account = self
            .store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .clone();

        // TraeWork 账号的登录态在客户端自己的 state.vscdb 里，不经过本工具的 Token 体系；
        // 放行只会得到「账号没有 Cookies」这种误导性错误
        if !account.is_traecode() {
            return Err(anyhow!("TraeWork 账号的登录态由客户端维护，无需刷新 Token"));
        }

        // 检查是否有 cookies
        if account.cookies.trim().is_empty() {
            return Err(anyhow!("账号没有 Cookies，请使用密码重新登录"));
        }

        let mut client = TraeApiClient::new(&account.cookies)?;
        let token_result = client.get_user_token().await?;

        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.jwt_token = Some(token_result.token);
            acc.token_expired_at = Some(token_result.expired_at);
            crate::api::checkin_guard::clear_auth_cooldown(acc);
            acc.updated_at = chrono::Utc::now().timestamp();
        }

        self.save_store()?;
        Ok(())
    }

    /// 使用保存的密码重新登录并刷新 Token/Cookies
    pub async fn refresh_token_with_password(&mut self, account_id: &str, password: &str) -> Result<()> {
        let account = self
            .store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .clone();

        if account.email.is_empty() {
            return Err(anyhow!("账号未绑定邮箱，无法使用密码登录"));
        }

        let login_result = login_with_email(&account.email, password).await?;

        if login_result.user_id != account.user_id {
            return Err(anyhow!("登录账号与当前账号不匹配"));
        }

        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.cookies = login_result.cookies;
            acc.jwt_token = Some(login_result.token);
            acc.token_expired_at = Some(login_result.expired_at);
            acc.password = Some(password.to_string());
            acc.updated_at = chrono::Utc::now().timestamp();
        }

        self.save_store()?;
        Ok(())
    }

    /// 使用用户输入的邮箱密码重新登录并更新账号信息
    pub async fn login_account_with_email(
        &mut self,
        account_id: &str,
        email: String,
        password: String,
    ) -> Result<UsageSummary> {
        let account = self
            .store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .clone();

        let login_result = login_with_email(&email, &password).await?;

        if login_result.user_id != account.user_id {
            return Err(anyhow!("登录账号与当前账号不匹配"));
        }

        let summary = TraeApiClient::new_with_token(&login_result.token)?
            .get_usage_summary_by_token()
            .await?;

        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.email = email;
            acc.password = Some(password);
            acc.cookies = login_result.cookies;
            acc.jwt_token = Some(login_result.token);
            acc.token_expired_at = Some(login_result.expired_at);
            acc.tenant_id = login_result.tenant_id;
            acc.plan_type = summary.plan_type.clone();
            acc.updated_at = chrono::Utc::now().timestamp();
        }

        self.save_store()?;
        Ok(summary)
    }

    /// 使用 Token/Cookies 更新已有账号的登录信息
    pub async fn update_account_credentials(
        &mut self,
        account_id: &str,
        token: String,
        cookies: Option<String>,
        password: Option<String>,
    ) -> Result<()> {
        let client = TraeApiClient::new_with_token(&token)?;
        let user_info = client.get_user_info_by_token().await?;

        let acc = self.store.accounts.iter_mut()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        if acc.user_id != user_info.user_id {
            return Err(anyhow!("Token 对应的用户与当前账号不匹配"));
        }

        let mut token_to_store = token;
        let mut expired_at = None;

        if let Some(cookie_str) = cookies.as_ref().filter(|v| !v.is_empty()) {
            match TraeApiClient::new(cookie_str) {
                Ok(mut cookie_client) => match cookie_client.get_user_token().await {
                    Ok(token_result) => {
                        if token_result.user_id != acc.user_id {
                            return Err(anyhow!("Cookies 对应的用户与当前账号不匹配"));
                        }
                        acc.cookies = cookie_str.to_string();
                        token_to_store = token_result.token;
                        expired_at = Some(token_result.expired_at);
                    }
                    Err(err) => {
                        println!("[WARN] cookies 登录验证失败，仍使用 Token: {}", err);
                    }
                },
                Err(err) => {
                    println!("[WARN] cookies 无效，仍使用 Token: {}", err);
                }
            }
        }

        acc.jwt_token = Some(token_to_store);
        acc.token_expired_at = expired_at;
        if let Some(pass) = password.filter(|v| !v.is_empty()) {
            acc.password = Some(pass);
        }
        acc.updated_at = chrono::Utc::now().timestamp();

        self.save_store()?;
        Ok(())
    }

    /// 更新账号 Token
    pub async fn update_account_token(&mut self, account_id: &str, token: String) -> Result<UsageSummary> {
        let client = TraeApiClient::new_with_token(&token)?;

        // 验证 Token 并获取用户信息
        let user_info = client.get_user_info_by_token().await?;

        // 查找账号
        let acc = self.store.accounts.iter_mut()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        // 确保是同一个用户
        if acc.user_id != user_info.user_id {
            return Err(anyhow!("Token 对应的用户与当前账号不匹配"));
        }

        // 更新 Token
        acc.jwt_token = Some(token.clone());
        acc.updated_at = chrono::Utc::now().timestamp();

        // 获取最新使用量
        let summary = client.get_usage_summary_by_token().await?;
        acc.plan_type = summary.plan_type.clone();

        self.save_store()?;
        Ok(summary)
    }

    /// 更新账号 Cookies
    pub async fn update_cookies(&mut self, account_id: &str, cookies: String) -> Result<()> {
        // 验证新 cookies 是否有效
        let mut client = TraeApiClient::new(&cookies)?;
        let token_result = client.get_user_token().await?;

        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            // 确保是同一个用户
            if acc.user_id != token_result.user_id {
                return Err(anyhow!("Cookies 对应的用户与当前账号不匹配"));
            }

            acc.cookies = cookies;
            acc.jwt_token = Some(token_result.token);
            acc.token_expired_at = Some(token_result.expired_at);
            acc.updated_at = chrono::Utc::now().timestamp();
        } else {
            return Err(anyhow!("账号不存在"));
        }

        self.save_store()?;
        Ok(())
    }

    /// 导出账号数据（只包含邮箱和密码）
    pub fn export_accounts(&self) -> Result<String> {
        let export_data: Vec<serde_json::Value> = self.store.accounts.iter().filter_map(|acc| {
            // 检查邮箱是否脱敏（包含*号），如果是则跳过
            let email = if acc.email.contains('*') || acc.email.trim().is_empty() {
                return None;
            } else {
                acc.email.clone()
            };
            
            // 只导出邮箱和密码
            Some(serde_json::json!({
                "email": email,
                "password": acc.password.clone().unwrap_or_default(),
            }))
        }).collect();

        serde_json::to_string_pretty(&export_data)
            .map_err(|e| anyhow!("导出失败: {}", e))
    }

    /// 导入账号数据（支持邮箱密码自动登录）
    pub async fn import_accounts(&mut self, data: &str) -> Result<usize> {
        #[derive(serde::Deserialize)]
        struct ImportItem {
            email: String,
            password: String,
        }
        
        let import_data: Vec<ImportItem> = serde_json::from_str(data)
            .map_err(|e| anyhow!("JSON 解析失败: {}", e))?;

        println!("[AccountManager] 开始导入 {} 个账号", import_data.len());
        
        // 限制并发数为 3，避免触发限流
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
        let mut tasks = Vec::new();
        
        for item in import_data {
            let email = item.email.trim().to_string();
            let password = item.password;
            
            if email.is_empty() || password.is_empty() {
                println!("[AccountManager] 跳过空邮箱或密码");
                continue;
            }
            
            // 检查是否已存在
            let existing = self.store.accounts.iter()
                .any(|a| a.email.eq_ignore_ascii_case(&email));
            
            if existing {
                println!("[AccountManager] 账号已存在，跳过: {}", email);
                continue;
            }
            
            let semaphore_clone = semaphore.clone();
            
            tasks.push(tokio::spawn(async move {
                let _permit = semaphore_clone.acquire().await.unwrap();
                println!("[AccountManager] 正在登录: {}", email);
                
                // 使用邮箱密码登录
                match login_with_email(&email, &password).await {
                    Ok(login_result) => {
                        println!("[AccountManager] 登录成功: {}", email);
                        Some((login_result, email, password))
                    }
                    Err(e) => {
                        println!("[AccountManager] 登录失败 {}: {}", email, e);
                        None
                    }
                }
            }));
        }

        // 等待所有登录任务完成
        let mut imported_count = 0;
        
        for task in tasks {
            if let Ok(Some((login_result, email, password))) = task.await {
                // 检查是否已存在（再次检查，避免并发重复）；同样只比对 traecode 账号
                if self.traecode_index_by_user_id(&login_result.user_id).is_some() {
                    println!("[AccountManager] 账号已存在（user_id 重复）: {}", email);
                    continue;
                }
                
                // 使用 Token 获取完整用户信息和使用量
                match TraeApiClient::new_with_token(&login_result.token) {
                    Ok(client) => {
                        // 并行获取用户信息和用量
                        let user_info_future = client.get_user_info_by_token();
                        let usage_future = client.get_usage_summary_by_token();
                        
                        match tokio::join!(user_info_future, usage_future) {
                            (Ok(user_info), Ok(usage)) => {
                                let mut account = Account::new(
                                    user_info.screen_name.unwrap_or_else(|| email.split('@').next().unwrap_or("User").to_string()),
                                    email.clone(),
                                    login_result.cookies,
                                    login_result.user_id,
                                    login_result.tenant_id,
                                );
                                
                                account.avatar_url = user_info.avatar_url.unwrap_or_default();
                                account.jwt_token = Some(login_result.token);
                                account.token_expired_at = Some(login_result.expired_at);
                                account.password = Some(password);
                                account.plan_type = usage.plan_type;
                                
                                self.store.accounts.push(account);
                                imported_count += 1;
                                println!("[AccountManager] 账号导入成功: {} (用量已获取)", email);
                            }
                            (Ok(user_info), Err(e)) => {
                                // 获取用量失败，但仍创建账号
                                let mut account = Account::new(
                                    user_info.screen_name.unwrap_or_else(|| email.split('@').next().unwrap_or("User").to_string()),
                                    email.clone(),
                                    login_result.cookies,
                                    login_result.user_id,
                                    login_result.tenant_id,
                                );
                                
                                account.avatar_url = user_info.avatar_url.unwrap_or_default();
                                account.jwt_token = Some(login_result.token);
                                account.token_expired_at = Some(login_result.expired_at);
                                account.password = Some(password);
                                
                                self.store.accounts.push(account);
                                imported_count += 1;
                                println!("[AccountManager] 账号导入成功: {} (用量获取失败: {})", email, e);
                            }
                            (Err(e), _) => {
                                println!("[AccountManager] 获取用户信息失败 {}: {}", email, e);
                            }
                        }
                    }
                    Err(e) => {
                        println!("[AccountManager] 创建 API 客户端失败 {}: {}", email, e);
                    }
                }
            }
        }

        // 设置活跃账号
        if self.store.active_account_id.is_none() && !self.store.accounts.is_empty() {
            self.store.active_account_id = Some(self.store.accounts[0].id.clone());
        }

        if imported_count > 0 {
            self.save_store()?;
        }

        println!("[AccountManager] 导入完成，成功导入 {} 个账号", imported_count);
        Ok(imported_count)
    }

    /// 获取使用事件
    pub async fn get_usage_events(
        &mut self,
        account_id: &str,
        start_time: i64,
        end_time: i64,
        page_num: i32,
        page_size: i32,
    ) -> Result<UsageQueryResponse> {
        let account = self
            .store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .clone();

        // 根据账号类型选择不同的方式调用 API
        if let Some(token) = &account.jwt_token {
            // 优先使用 Token
            let client = TraeApiClient::new_with_token(token)?;
            match client.query_usage(start_time, end_time, page_size, page_num).await {
                Ok(response) => Ok(response),
                Err(e) => {
                    let error_msg = e.to_string();
                    // 如果是 401 错误且有 Cookies，尝试刷新 Token
                    if error_msg.contains("401") && !account.cookies.is_empty() {
                        println!("[INFO] Token 已过期，尝试使用 Cookies 刷新...");
                        // 使用 Cookies 刷新 Token
                        let mut cookie_client = TraeApiClient::new(&account.cookies)?;
                        let token_result = cookie_client.get_user_token().await?;

                        // 更新存储的 Token
                        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
                            acc.jwt_token = Some(token_result.token.clone());
                            acc.token_expired_at = Some(token_result.expired_at.clone());
                        }
                        self.save_store()?;

                        // 使用新 Token 重新查询
                        let new_client = TraeApiClient::new_with_token(&token_result.token)?;
                        new_client.query_usage(start_time, end_time, page_size, page_num).await
                    } else if error_msg.contains("401") {
                        Err(anyhow!("登录已过期，请重新登录此账号"))
                    } else {
                        Err(e)
                    }
                }
            }
        } else if !account.cookies.is_empty() {
            // 使用 Cookies
            let mut client = TraeApiClient::new(&account.cookies)?;
            // 先获取 token
            client.get_user_token().await?;
            client.query_usage(start_time, end_time, page_size, page_num).await
        } else {
            Err(anyhow!("账号没有有效的 Token 或 Cookies"))
        }
    }

    /// 从 Trae IDE 读取当前登录账号
    ///
    /// 返回值区分「新增 / 补全 / 已存在 / 本机无登录态」四种结果，交由前端给出准确提示。
    pub async fn read_trae_ide_account(&mut self) -> Result<TraeIdeReadOutcome> {
        // 获取 Trae IDE 配置文件路径（跨平台支持，仅适配国内版 Trae CN）
        #[cfg(target_os = "windows")]
        let trae_data_path = {
            let appdata = std::env::var("APPDATA")
                .map_err(|_| anyhow!("无法获取 APPDATA 环境变量"))?;
            PathBuf::from(appdata).join("Trae CN")
        };

        #[cfg(target_os = "macos")]
        let trae_data_path = {
            let home = std::env::var("HOME")
                .map_err(|_| anyhow!("无法获取 HOME 环境变量"))?;
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Trae CN")
        };
        
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let trae_data_path: PathBuf = {
            return Err(anyhow!("此功能仅支持 Windows 和 macOS 系统"));
        };

        let storage_path = trae_data_path
            .join("User")
            .join("globalStorage")
            .join("storage.json");

        // 检查文件是否存在
        if !storage_path.exists() {
            // 文件不存在是「客户端还没启动过」的正常状态，不是失败：走 NoLogin 让前端提示怎么做，
            // 而不是抛 Err（Err 会弹红框，用户会以为工具坏了）
            return Ok(TraeIdeReadOutcome::no_login(format!(
                "未找到 Trae IDE 配置文件（{}），请先启动一次 Trae 并登录",
                storage_path.display()
            )));
        }

        // 读取文件内容
        let content = fs::read_to_string(&storage_path)
            .map_err(|e| anyhow!("读取 Trae IDE 配置文件失败: {}", e))?;

        // 解析 JSON（剥离 BOM：客户端写文件时可能带 UTF-8 BOM，serde 会直接判定为非法 JSON）
        let storage: serde_json::Value = serde_json::from_str(content.trim_start_matches('\u{feff}'))
            .map_err(|e| anyhow!("解析 Trae IDE 配置文件失败: {}", e))?;

        // 获取 iCubeAuthInfo 字段：缺失即「本机 Trae 未登录」，与文件不存在同属可预期的状态
        let Some(auth_info_raw) = storage
            .get("iCubeAuthInfo://icube.cloudide")
            .and_then(|v| v.as_str())
        else {
            return Ok(TraeIdeReadOutcome::no_login(format!(
                "Trae IDE 当前未登录（{} 中没有登录信息），请在 Trae 客户端登录后重试",
                storage_path.display()
            )));
        };

        // 解析登录态：新版 Trae CN（≥2.3）把该键从明文 JSON 改成 tc 密文（base64），直接当 JSON 解析
        // 只会得到 "expected value at line 1 column 1"。故先按明文解析（兼容旧客户端），失败再走 tc
        // 解密。判据与 traework::uid::profile_from_storage 保持一致——两条读取路径对同一份文件必须得出
        // 相同结论，否则会出现"识别到的账号"与"实际登录账号"名实不符。
        let auth_info: serde_json::Value = match serde_json::from_str(
            auth_info_raw.trim_start_matches('\u{feff}'),
        ) {
            Ok(v) => v,
            Err(plain_err) => match crate::tc_crypto::decrypt_storage_value(auth_info_raw) {
                Ok(plain) => serde_json::from_str(plain.trim_start_matches('\u{feff}'))
                    .map_err(|e| anyhow!("解析 Trae IDE 认证信息失败: {}", e))?,
                Err(decrypt_err) => {
                    return Err(anyhow!(
                        "解析 Trae IDE 认证信息失败: {}（按 tc 密文解密亦失败: {}，请先启动 Trae 并确认已登录）",
                        plain_err,
                        decrypt_err
                    ))
                }
            },
        };

        // 提取账号信息
        let token = auth_info
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("未找到 Token"))?
            .to_string();

        // userId 在部分客户端版本里是数字而非字符串，两种都接受（与 traework::uid::profile_of 同口径）
        let user_id = match auth_info.get("userId") {
            Some(serde_json::Value::String(s)) => s.trim().to_string(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => return Err(anyhow!("未找到 User ID")),
        };
        if user_id.is_empty() {
            return Err(anyhow!("未找到 User ID"));
        }

        let email = auth_info
            .get("account")
            .and_then(|acc| acc.get("email"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let avatar_url = auth_info
            .get("account")
            .and_then(|acc| acc.get("avatar_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let username = auth_info
            .get("account")
            .and_then(|acc| acc.get("username"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // 账号已存在：不重复添加，改为把 IDE 里更新的字段补进既有记录。
        // 为什么补而不是直接拒绝：实测（2026-09-20）账号库里那条记录只有 user_id，
        // 名字为空且没有 Token——这种「已存在」对用户毫无意义，他点这个按钮就是想让它可用。
        // 这里只用 login 密文里已有的字段（用户名/邮箱/头像/Token），不发网络请求，保证「已存在」是零成本的快路径。
        if let Some(index) = self.traecode_index_by_user_id(&user_id) {
            let ide_expired_at = auth_info
                .get("expiredAt")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let existing = &mut self.store.accounts[index];
            let mut filled: Vec<&str> = Vec::new();

            // Token 仅在本地不可用时回填：IDE 里的 Token 未必比本地新（IDE 可能久未启动），
            // 无条件覆盖会把已刷新出来的有效 Token 打回旧值
            if !token_still_usable(existing) {
                existing.jwt_token = Some(token.clone());
                existing.token_expired_at = ide_expired_at;
                filled.push("登录 Token");
            }
            if existing.name.trim().is_empty() && !username.trim().is_empty() {
                existing.name = username.clone();
                filled.push("用户名");
            }
            if existing.email.trim().is_empty() && !email.trim().is_empty() {
                existing.email = email.clone();
                filled.push("邮箱");
            }
            if existing.avatar_url.trim().is_empty() && !avatar_url.trim().is_empty() {
                existing.avatar_url = avatar_url.clone();
                filled.push("头像");
            }

            let existing = existing.clone();
            let display = format!("{}（{}）", display_name_of(&existing), user_id);

            if filled.is_empty() {
                println!("[INFO] Trae IDE 账号已存在于账号管理中: {}", user_id);
                return Ok(TraeIdeReadOutcome {
                    status: TraeIdeReadStatus::Exists,
                    account: Some(existing),
                    message: format!("该账号已在列表中：{display}，未重复添加"),
                });
            }

            self.save_store()?;
            println!(
                "[INFO] 已用 Trae IDE 登录态补全账号 {}: {}",
                user_id,
                filled.join("、")
            );
            return Ok(TraeIdeReadOutcome {
                status: TraeIdeReadStatus::Updated,
                account: Some(existing),
                message: format!(
                    "该账号已在列表中，已补全{}：{display}",
                    filled.join("、")
                ),
            });
        }

        // 使用 Token 获取完整的用户信息（带超时）
        let client = TraeApiClient::new_with_token(&token)?;
        let user_info = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.get_user_info_by_token()
        ).await {
            Ok(Ok(info)) => info,
            Ok(Err(e)) => return Err(anyhow!("获取用户信息失败: {}", e)),
            Err(_) => return Err(anyhow!("获取用户信息超时，请检查网络连接")),
        };

        // 创建账号对象
        let mut account = Account::new(
            if username.is_empty() {
                user_info.screen_name.unwrap_or_else(|| format!("User_{}", &user_id[..8.min(user_id.len())]))
            } else {
                username
            },
            if email.is_empty() {
                user_info.email.unwrap_or_default()
            } else {
                email
            },
            String::new(), // Trae IDE 不存储 cookies
            user_id,
            user_info.tenant_id,
        );

        account.avatar_url = if avatar_url.is_empty() {
            user_info.avatar_url.unwrap_or_default()
        } else {
            avatar_url
        };
        account.jwt_token = Some(token);

        // 添加到账号列表
        self.store.accounts.push(account.clone());

        // 如果是第一个账号，设为活跃账号
        if self.store.active_account_id.is_none() {
            self.store.active_account_id = Some(account.id.clone());
        }

        self.save_store()?;

        println!("[INFO] 成功从 Trae IDE 读取并添加账号: {}", account.email);

        let display = format!("{}（{}）", display_name_of(&account), account.user_id);
        Ok(TraeIdeReadOutcome {
            status: TraeIdeReadStatus::Added,
            account: Some(account),
            message: format!("已从 Trae IDE 读取并添加账号：{display}"),
        })
    }

    /// 领取生日礼包
    pub async fn claim_birthday_bonus(&mut self, account_id: &str) -> Result<()> {
        let account = self.store.accounts.iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        let token = account.jwt_token.as_ref()
            .ok_or_else(|| anyhow!("账号没有 Token"))?;

        let client = TraeApiClient::new_with_token(token)?;

        // 先查询是否已领取
        let claimed = client.query_birthday_bonus().await?;
        if claimed {
            return Err(anyhow!("该账号已领取过礼包"));
        }

        // 领取礼包
        client.claim_birthday_bonus().await?;

        println!("[INFO] 成功领取礼包: {}", account.email);
        Ok(())
    }

    /// 获取账号统计数据
    pub async fn get_account_statistics(&self, account_id: &str) -> Result<crate::api::UserStatisticResult> {
        let account = self.store.accounts.iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!("账号不存在"))?;

        let token = account.jwt_token.as_ref()
            .ok_or_else(|| anyhow!("账号没有有效的 Token"))?;

        println!("[get_account_statistics] 账号: user_id={}, cookies_length={}", account.user_id, account.cookies.len());

        // 尝试使用 cookies（如果有）
        if !account.cookies.trim().is_empty() {
            let client = TraeApiClient::new_with_token_and_cookies(token, &account.cookies)?;
            
            match client.get_user_statistic_data().await {
                Ok(stats) => {
                    println!("[get_account_statistics] ✅ 使用 cookies 成功获取统计数据");
                    return Ok(stats);
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    println!("[get_account_statistics] 使用 cookies 失败: {}", error_msg);
                    // 如果 cookies 失败，继续尝试只使用 token
                }
            }
        }

        // 尝试只使用 token（不需要 cookies）
        println!("[get_account_statistics] 尝试只使用 token 获取统计数据...");
        let client = TraeApiClient::new_with_token(token)?;
        
        match client.get_user_statistic_data().await {
            Ok(stats) => {
                println!("[get_account_statistics] ✅ 使用 token 成功获取统计数据");
                Ok(stats)
            }
            Err(e) => {
                let error_msg = e.to_string();
                println!("[get_account_statistics] 使用 token 也失败: {}", error_msg);
                if error_msg.contains("403") {
                    Err(anyhow!("统计数据 API 需要账号 Cookies。请使用编辑账号功能输入邮箱密码重新登录，或删除账号后使用浏览器登录重新添加。"))
                } else if error_msg.contains("401") || error_msg.contains("20310") || error_msg.contains("10304") {
                    Err(anyhow!("登录已过期，请重新登录此账号以查看统计数据"))
                } else {
                    Err(anyhow!("获取统计数据失败: {}", error_msg))
                }
            }
        }
    }

    pub fn update_account_info_after_usage_check(
        &mut self,
        account_id: &str,
        plan_type: String,
        new_token: Option<(String, String)>, // (token, expired_at)
    ) -> Result<()> {
        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.plan_type = plan_type;
            if let Some((token, expired_at)) = new_token {
                acc.jwt_token = Some(token);
                acc.token_expired_at = Some(expired_at);
            }
            acc.updated_at = chrono::Utc::now().timestamp();
            self.save_store()?;
        }
        Ok(())
    }

    /// 列出参与签到的账号；`only_pending_today` 为 true 时只返回「今日未签到」的账号
    /// （方案B 自动签到用；手动全量签到传 false）
    pub fn list_accounts_for_checkin(&self, today: &str, only_pending_today: bool) -> Vec<Account> {
        self.store
            .accounts
            .iter()
            // TraeWork 账号没有 Cookies/JWT，签到的网络请求必然失败；不在这里拦掉的话，
            // 每次签到都会给它们各写一条「失败 + 冷却」，把账号列表噪音化
            .filter(|a| a.is_traecode())
            .filter(|a| a.is_active)
            .filter(|a| !only_pending_today || a.last_checkin_date.as_deref() != Some(today))
            .cloned()
            .collect()
    }

    /// 批次签到后统一落盘：成功/Already 写日期、失败项写冷却，一次 persist
    /// 为什么整批一次写：批次运行期间不落盘冷却（防 10 分钟冷却拦死 30 秒后的重试轮），
    /// 全部轮次结束后才统一写，避免每账号一次 `save_store` 的多次写盘窗口。
    pub fn apply_checkin_outcomes(
        &mut self,
        outcomes: &[crate::api::checkin_guard::CheckinOutcome],
    ) -> Result<()> {
        if outcomes.is_empty() {
            return Ok(());
        }
        for outcome in outcomes {
            if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == outcome.account_id) {
                // 日期与冷却互斥：写日期即意味着旧冷却已失效（重试轮内冷却可能刚好到期）
                match (&outcome.date, &outcome.cooldown) {
                    (Some(date), _) => {
                        acc.last_checkin_date = Some(date.clone());
                        acc.checkin_cooldown = None;
                    }
                    (None, Some(cooldown)) => {
                        acc.checkin_cooldown = Some(cooldown.clone());
                    }
                    (None, None) => continue,
                }
                acc.updated_at = chrono::Utc::now().timestamp();
            }
        }
        self.save_store()
    }

    /// 重置单账号签到设备号：换新号 + 清冷却 + 清已签到日期，单次落盘
    ///
    /// 为什么清 `last_checkin_date`：9095 时日期已被写入（Already 语义），不清则重置后
    /// 自动签到仍跳过该账号，自救无效。
    /// 为什么清冷却：让重置后立即可试。注意 9074 是账号级限流、与设备号无关，重置不保证
    /// 解除限流——若复现会重新冷却，代价仅是多打一次请求。
    pub fn reset_account_device_id(&mut self, account_id: &str, new_device_id: String) -> Result<()> {
        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.device_id = Some(new_device_id);
            acc.last_checkin_date = None;
            acc.checkin_cooldown = None;
            acc.updated_at = chrono::Utc::now().timestamp();
            self.save_store()?;
        }
        Ok(())
    }

    /// 保存刷新后的 Token（签到流程复用：取锁读 → 无锁网络 → 取锁写）
    pub fn save_refreshed_token(&mut self, account_id: &str, token: &str, expired_at: &str) -> Result<()> {
        if let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == account_id) {
            acc.jwt_token = Some(token.to_string());
            acc.token_expired_at = Some(expired_at.to_string());
            // Token 刷新成功说明「需重新登录」的冷却解除了，仅清 auth_expired——
            // 刷新 Token 不该解除 9074 限流冷却（见 checkin_guard::clear_auth_cooldown）
            crate::api::checkin_guard::clear_auth_cooldown(acc);
            acc.updated_at = chrono::Utc::now().timestamp();
            self.save_store()?;
        }
        Ok(())
    }
}

/// 本地 Token 是否仍可用（非空，且未过期）
///
/// 与 `switch_account` 的过期判断同口径：`token_expired_at` 缺失视为可用，解析失败视为不可用。
/// 这里只需要一个保守答案——不确定时宁可让 IDE 的 Token 覆盖，也不要抱着一个身份不明的 Token 不放。
fn token_still_usable(account: &Account) -> bool {
    if account
        .jwt_token
        .as_deref()
        .map_or(true, |t| t.trim().is_empty())
    {
        return false;
    }
    match account.token_expired_at.as_deref() {
        None => true,
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc) > chrono::Utc::now())
            .unwrap_or(false),
    }
}

/// 账号展示名：名字为空时兜到 user_id（界面不该出现无名条目）
fn display_name_of(account: &Account) -> String {
    let name = account.name.trim();
    if name.is_empty() {
        account.user_id.clone()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个只含给定账号的 manager（不落盘、不读真实数据目录）
    fn manager_with(accounts: Vec<Account>) -> AccountManager {
        AccountManager {
            store: AccountStore {
                accounts,
                ..Default::default()
            },
            data_path: PathBuf::from("unused-accounts.json"),
        }
    }

    /// 同 user_id 的 TraeWork 记录绝不能被当成 TraeCode 账号
    ///
    /// 复刻 2026-09-20 的实测报障：Trae 账号 `1699216069236448` 只有一条 **traework** 记录，
    /// 账号管理页（只渲染非 traework）看不到它，但按 user_id 去重会把这条 traework 记录当作
    /// 「TraeCode 账号已存在」，于是「读取本地账号」永远只提示已存在、什么也不做。
    #[test]
    fn traecode查找跳过同user_id的traework记录() {
        let manager = manager_with(vec![
            Account::new_traework("1699216069236448".to_string(), "用户3036504228".to_string()),
        ]);
        assert_eq!(manager.traecode_index_by_user_id("1699216069236448"), None);

        // 同一账号在两个应用各有一条时，只应命中 traecode 那条
        let manager = manager_with(vec![
            Account::new_traework("168695880747001".to_string(), "似我".to_string()),
            Account::new(
                "似我".to_string(),
                String::new(),
                String::new(),
                "168695880747001".to_string(),
                String::new(),
            ),
        ]);
        assert_eq!(manager.traecode_index_by_user_id("168695880747001"), Some(1));
    }
}