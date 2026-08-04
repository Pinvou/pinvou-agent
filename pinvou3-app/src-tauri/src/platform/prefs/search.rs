//! 搜索业务类型:引擎提供商枚举、凭据结构、搜索偏好。
//!
//! 自 prefs 模块抽离——这些类型自包含(不依赖 UserPrefs 的其他字段),
//! 独立成模块便于单测与按业务域聚合。`UserPrefs` 持有 [`SearchPrefs`]
//! 作为字段;凭证迁移逻辑(操作 `UserPrefs` 多个字段的 `impl UserPrefs`
//! 方法)仍留在 [`super::mod`],因它深度耦合 UserPrefs 内部状态。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::platform::credential_store::CredentialReference;

/// Search 后端选择。
/// - `Bing`(默认): HTML scrape,无需 key,但对中文长复合查询相关性差。
///   DDG 在 GFW + 代理 datacenter IP 段下基本恒返 anomaly-modal,
///   所以底座 fork patch #42 已把默认翻成 Bing,这里前端默认对齐。
/// - `Metaso` / `Bocha` / `Baidu`: 国内 AI 搜索 API,中文场景相关性远好于 Bing scrape。
///   Metaso 留空 key 走底座内置共享 key(~100 次/天);Bocha/Baidu 必须填 key。
/// - `Tavily`: 海外 agent 搜索 API(<https://app.tavily.com/> 拿 `tvly-` key,API 实际打
///   `api.tavily.com`)。结果是干净抽取的 content 而非 HTML scrape,质量好;但要稳定外网 +
///   自带额度,key 必填(留空底座直接报 "requires API key")。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SearchProvider {
    #[default]
    Bing,
    Metaso,
    Bocha,
    Baidu,
    Tavily,
}

impl SearchProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchProvider::Bing => "bing",
            SearchProvider::Metaso => "metaso",
            SearchProvider::Bocha => "bocha",
            SearchProvider::Baidu => "baidu",
            SearchProvider::Tavily => "tavily",
        }
    }

    pub fn supports_api_key(self) -> bool {
        !matches!(self, SearchProvider::Bing)
    }

    pub fn env_key_names(self) -> &'static [&'static str] {
        match self {
            SearchProvider::Metaso => &["METASO_API_KEY"],
            SearchProvider::Baidu => &["BAIDU_SEARCH_API_KEY"],
            SearchProvider::Bing | SearchProvider::Bocha | SearchProvider::Tavily => &[],
        }
    }

    pub fn credential_reference(self) -> CredentialReference {
        CredentialReference::for_search_provider(self.as_str())
    }
}

pub(super) fn default_enabled_search_providers() -> Vec<SearchProvider> {
    vec![SearchProvider::Bing]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SearchCredential {
    #[serde(default, skip_serializing)]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialReference>,
    #[serde(default)]
    pub credential_state: crate::platform::credential_store::CredentialState,
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_action: Option<crate::platform::credential_store::CredentialEditAction>,
}

impl SearchCredential {
    pub fn clear_plaintext_key(&mut self) {
        self.api_key.clear();
        self.credential_action = None;
    }

    pub fn mark_configured(&mut self, reference: CredentialReference) {
        self.credential_ref = Some(reference);
        self.credential_state = crate::platform::credential_store::CredentialState::Configured;
        self.has_secret = true;
        self.clear_plaintext_key();
    }

    pub fn mark_missing(&mut self) {
        self.credential_ref = None;
        self.credential_state = crate::platform::credential_store::CredentialState::Missing;
        self.has_secret = false;
        self.clear_plaintext_key();
    }

    pub fn mark_unavailable(&mut self) {
        self.credential_state = crate::platform::credential_store::CredentialState::Unavailable;
        self.has_secret = self.credential_ref.is_some();
        self.clear_plaintext_key();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SearchPrefs {
    pub provider: SearchProvider,
    #[serde(default = "default_enabled_search_providers")]
    pub enabled_providers: Vec<SearchProvider>,
    /// 当 `provider = Metaso` 时:None 走底座内置共享 key。
    /// 当 `provider = Bocha`/`Baidu` 时:None 会让 web_search 直接报错(必填)。
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credentials: BTreeMap<SearchProvider, SearchCredential>,
}

impl Default for SearchPrefs {
    fn default() -> Self {
        Self {
            provider: SearchProvider::Bing,
            enabled_providers: default_enabled_search_providers(),
            api_key: None,
            credentials: BTreeMap::new(),
        }
    }
}

impl SearchPrefs {
    /// 传给底座前归一化 key。
    ///
    /// 空字符串如果透传成 `Some("")`,部分搜索 API 会返回 HTTP 200 + 业务错误体,
    /// 底座旧版本可能误解析成 `No results found`。这里统一把空白 key 当未配置。
    pub fn normalized_api_key(&self) -> Option<String> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
    }

    pub fn normalize(&mut self) {
        self.api_key = None;
        if self.enabled_providers.is_empty() {
            self.enabled_providers.push(SearchProvider::Bing);
        }
        if !self.enabled_providers.contains(&SearchProvider::Bing) {
            self.enabled_providers.insert(0, SearchProvider::Bing);
        }
        if !self.enabled_providers.contains(&self.provider) {
            self.enabled_providers.push(self.provider);
        }
        self.enabled_providers.sort();
        self.enabled_providers.dedup();
        self.credentials
            .retain(|_, credential| credential.has_secret || credential.credential_ref.is_some());
        for credential in self.credentials.values_mut() {
            credential.clear_plaintext_key();
        }
    }
}
