//! # 联网仓库通用模型（RepoItem / RepoType / PaidStatus / RepoManifest）
//!
//! 被 AstroBox 官方源 + 本地用户源 + `install_from_repo` 流水线共用。
//!
//! 付费过滤 + 设备型号过滤在此模块定义，避免漏掉。
//!
//! 合规注：此前曾规划"米坛社区源"作为 Secondary source 接入，
//! 但由于 BandBBS 服务条款禁止未授权自动抓取，相关抓取代码已从本仓库
//! 彻底移除。作为合规替代，新增 `RepoSource::LocalUser`：用户通过 SD 卡
//! 或**网页控制台**上传的资源登记到 `/sdcard/astrobox/local_index.csv`
//! 作为本地源。既满足"步骤 3 另一个源"目标，又完全不触碰 BandBBS。

#[cfg(feature = "repo_net")]
pub mod astrobox_source;

#[cfg(feature = "sdcard")]
pub mod local_csv_source;

use serde::{Deserialize, Serialize};
use std::fmt;

/// 资源来源：AstroBox 官方源 / 本地用户上传源（BandBBS 抓取因合规移除）
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoSource {
    AstroBoxOfficial,
    /// 用户上传到 SD 卡的本地源（`.rpk/.mwz/.face`），合规替代"米坛社区抓取"。
    LocalUser,
}

impl fmt::Display for RepoSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoSource::AstroBoxOfficial => f.write_str("AstroBox"),
            RepoSource::LocalUser => f.write_str("本地"),
        }
    }
}

/// 资源类型：快应用 / 表盘
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoType {
    QuickApp,
    Watchface,
}

impl RepoType {
    pub fn from_csv(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "quickapp" | "quick_app" | "mini_app" => Some(RepoType::QuickApp),
            "watchface" | "watch_face" | "face" => Some(RepoType::Watchface),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaidStatus {
    /// 免费（`paid_type` 空 或 "免费"）
    Free,
    /// 付费（非强制）
    Paid,
    /// 强制付费（force_paid / 大会员专属 / 商城明码标价）
    ForcePaid,
}

impl PaidStatus {
    /// 从 CSV 字符串推断；匹配不到时为 **Free** 是偏激进安全（宁可展示免费）。
    /// 另在 AstroBox 官方源解析路径外，还对 `name/title` 跑 `PaidKeywordFilter::default_generic()`
    /// 做二次兜底，避免 `paid_type` 列留空但标题写 "¥5" / "VIP only" 的漏网。
    pub fn from_csv(s: &str) -> Self {
        let t = s.trim().to_ascii_lowercase();
        if t.is_empty() {
            return PaidStatus::Free;
        }
        if t.contains("force_paid") || t.contains("force-paid") || t.contains("强制") {
            PaidStatus::ForcePaid
        } else if t.contains("paid")
            || t.contains("付费")
            || t.contains("大会员")
            || t.contains("¥")
            || t.contains("vip")
            || t.starts_with('¥')
        {
            PaidStatus::Paid
        } else if t == "免费" || t == "free" {
            PaidStatus::Free
        } else {
            PaidStatus::Free
        }
    }

    #[must_use]
    pub fn is_free(&self) -> bool {
        matches!(self, PaidStatus::Free)
    }
}

/// 索引列表条目（对应 AstroBox index.csv 的一行）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoItem {
    pub name: String,
    pub icon_url: String,
    pub cover_url: String,
    pub restype: RepoType,
    pub tags: Vec<String>,
    pub devices: Vec<String>,
    /// CSV 里 `path`（清单 JSON）
    pub manifest_path: String,
    pub paid: PaidStatus,
    pub source: RepoSource,
}

impl RepoItem {
    /// 基础过滤：Free + 指定设备型号
    #[must_use]
    pub fn passes_device_and_paid_filter(&self, device_code: Option<&str>) -> bool {
        if !self.paid.is_free() {
            return false;
        }
        if let Some(code) = device_code {
            if !self.devices.iter().any(|d| d == code) {
                return false;
            }
        }
        true
    }
}

/// Manifest 解析产物（每个提交的 release repo JSON）。
///
/// 对应 AstroBox‑Repo 中每个 `{path}.json` 字段结构：通常含有
/// `repo_url`、`payloads` / `rpk_url` / `watchface_url` 等。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RepoManifest {
    #[serde(default)]
    pub repo_url: Option<String>,
    /// 快应用 payload URL（.rpk）
    #[serde(default)]
    pub quickapp_rpk_url: Option<String>,
    /// 表盘 payload URL（.mwz / .face）
    #[serde(default)]
    pub watchface_url: Option<String>,
    /// 版本标识（用于缓存 key）
    #[serde(default)]
    pub version: Option<String>,
    /// 文件大小（bytes），可能缺省
    #[serde(default)]
    pub filesize: Option<u64>,
    /// 包名（对 quickapp）
    #[serde(default)]
    pub package_name: Option<String>,
    /// 表盘 ID（对 watchface，可选）
    #[serde(default)]
    pub watchface_id: Option<String>,
}

impl RepoManifest {
    /// 按类型拿 payload URL。若类型与 URL 不匹配返回 None。
    #[must_use]
    pub fn payload_url(&self, ty: RepoType) -> Option<&str> {
        match ty {
            RepoType::QuickApp => self.quickapp_rpk_url.as_deref(),
            RepoType::Watchface => self.watchface_url.as_deref(),
        }
    }
}

// ================= 付费关键词过滤（AstroBox 源 CSV / 未来用户自定义标题） =================
//
// 合规保留：付费关键词过滤本身不涉及米坛；它是一个通用"疑似付费标记"检测，
// 用于防止 AstroBox 源 CSV 中 paid_type 为空但标题含明显付费信息的条目漏过滤。
// （相关 BandBBS 抓取代码已整体移除，见文件顶部 doc comment。）

/// 通用付费关键词过滤器。可用于过滤 CSV/文本标题中疑似付费内容。
pub struct PaidKeywordFilter {
    needles: Vec<&'static str>,
}

impl PaidKeywordFilter {
    /// 默认关键词：覆盖 AstroBox 官方源 `paid_type` 中常见的文字变体
    /// （避免 CSV 某行 paid_type 列留空但 name 里写 "¥5" 的漏网）。
    pub fn default_generic() -> Self {
        Self {
            needles: vec![
                "¥",
                "付费",
                "购买",
                "大会员",
                "paid",
                "force_paid",
                "price",
                "money",
                "vip",
            ],
        }
    }
    pub fn contains_paid_indicator(&self, s: &str) -> bool {
        let lower = s.to_ascii_lowercase();
        self.needles
            .iter()
            .any(|n| lower.contains(&n.to_ascii_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_csv_parses() {
        assert_eq!(PaidStatus::from_csv(""), PaidStatus::Free);
        assert_eq!(PaidStatus::from_csv("paid"), PaidStatus::Paid);
        assert_eq!(PaidStatus::from_csv("force_paid"), PaidStatus::ForcePaid);
        assert_eq!(PaidStatus::from_csv("大会员"), PaidStatus::Paid);
        assert_eq!(PaidStatus::from_csv("免费"), PaidStatus::Free);
    }

    #[test]
    fn item_filter_works() {
        let item = RepoItem {
            name: "a".into(),
            icon_url: String::new(),
            cover_url: String::new(),
            restype: RepoType::QuickApp,
            tags: vec![],
            devices: vec!["n67".into()],
            manifest_path: String::new(),
            paid: PaidStatus::Free,
            source: RepoSource::AstroBoxOfficial,
        };
        assert!(item.passes_device_and_paid_filter(Some("n67")));
        assert!(!item.passes_device_and_paid_filter(Some("o66")));
        let mut paid = item.clone();
        paid.paid = PaidStatus::ForcePaid;
        assert!(!paid.passes_device_and_paid_filter(Some("n67")));
    }
}
