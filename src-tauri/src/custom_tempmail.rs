//! 可配置的临时邮箱配置
//!
//! 支持用户自定义 Cloudflare Worker 地址和密钥。
//! 客户端实现（CustomTempMailClient 及验证码查询逻辑）已随死代码清理移除，
//! 如需恢复可查看 git 历史中本文件的旧版本。

use serde::{Deserialize, Serialize};

/// 自定义临时邮箱配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTempMailConfig {
    /// Cloudflare Worker 地址，例如：https://your-worker.your-subdomain.workers.dev
    pub api_url: String,
    /// API 密钥
    pub secret_key: String,
    /// 邮箱域名
    pub email_domain: String,
}

impl Default for CustomTempMailConfig {
    fn default() -> Self {
        Self {
            api_url: String::new(),
            secret_key: String::new(),
            email_domain: String::new(),
        }
    }
}

impl CustomTempMailConfig {
    /// 检查配置是否有效
    pub fn is_valid(&self) -> bool {
        !self.api_url.is_empty()
            && !self.secret_key.is_empty()
            && !self.email_domain.is_empty()
    }
}