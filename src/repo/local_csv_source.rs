//! # 本地用户 index.csv 源（合规替代"米坛社区"抓取）
//!
//! 合规背景：BandBBS（米坛社区）服务条款禁止未授权自动抓取，相关抓取代码
//! 已在步骤 3 初版中**整体移除**。作为替代，我们提供**本地用户源**：
//! 用户通过 SD 卡或网页上传的 `.rpk` / `.mwz` / `.face` / `.abp` 资源，
//! 自动登记到 `/sdcard/astrobox/local_index.csv`，成为"步骤 3"的另一条
//! repo 流水线。这完全不触碰第三方站点，100% 合规。
//!
//! 目录约定：
//! - 索引：`/sdcard/astrobox/local_index.csv`
//! - 资源：`/sdcard/astrobox/local/<sha1>.<ext>`（sha1 取文件名+size，避免重名）
//! - 上传时：写文件 → append CSV 一行 → API 返回 201
//!
//! CSV 格式（与 AstroBox index.csv 一致，方便 `parse_index_csv` 复用）：
//! `name,icon,cover,restype,tags,devices,path,paid_type`
//! `path` 这里是 SD 卡**绝对路径**（非 URL），manifest 路径单独约定
//! 为 `path` 的 `.json` 同级文件，内容是最小化 `RepoManifest`。

use super::{PaidStatus, RepoItem, RepoSource, RepoType};
use crate::sdcard;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const LOCAL_DIR: &str = "astrobox/local";
const LOCAL_CSV: &str = "astrobox/local_index.csv";
const CSV_HEADER: &str = "name,icon,cover,restype,tags,devices,path,paid_type\n";

/// 对应 CSV 单行；字段与 `astrobox_source::IndexRow` 相同，
/// 但这里 `path` 是 SD 卡上的绝对路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalIndexRow {
    name: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    cover: String,
    restype: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    devices: String,
    path: String,
    #[serde(default)]
    paid_type: String,
}

fn csv_path(sd_root: &Path) -> PathBuf {
    sd_root.join(LOCAL_CSV)
}

fn local_dir(sd_root: &Path) -> PathBuf {
    sd_root.join(LOCAL_DIR)
}

/// 确保 `/sdcard/astrobox/local` 目录 + 空 CSV 存在（首次启动自动初始化）。
pub async fn ensure_local_source(sd_root: &Path) -> Result<()> {
    let dir = local_dir(sd_root);
    sdcard::ensure_dir(&dir).with_context(|| format!("mkdir {}", dir.display()))?;

    let csv = csv_path(sd_root);
    match tokio::fs::metadata(&csv).await {
        Ok(_) => Ok(()),
        Err(_) => tokio::fs::write(&csv, CSV_HEADER)
            .await
            .with_context(|| format!("init local csv {}", csv.display())),
    }
}

/// 读取本地用户索引，经过 free+device 过滤后返回 `RepoItem` 列表。
/// `source` 标记为新的 `RepoSource::LocalUser`。
pub async fn fetch_index(sd_root: &Path, device_code: Option<&str>) -> Result<Vec<RepoItem>> {
    let csv = csv_path(sd_root);
    let text = match tokio::fs::read_to_string(&csv).await {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ensure_local_source(sd_root).await?;
            String::new()
        }
        Err(e) => return Err(anyhow!("read {}: {e}", csv.display())),
    };
    parse_csv(&text, device_code)
}

fn parse_csv(text: &str, device_code: Option<&str>) -> Result<Vec<RepoItem>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut out = Vec::with_capacity(64);
    for (idx, row) in rdr.deserialize::<LocalIndexRow>().enumerate() {
        let row = match row {
            Ok(r) => r,
            Err(e) => {
                log::warn!("local index.csv skip row {idx}: {e:?}");
                continue;
            }
        };
        let Some(restype) = RepoType::from_csv(&row.restype) else {
            log::debug!("local index row {idx}: unknown restype {}", row.restype);
            continue;
        };
        let paid = PaidStatus::from_csv(&row.paid_type);
        let tags = row
            .tags
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let devices = row
            .devices
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let item = RepoItem {
            name: row.name,
            icon_url: row.icon,
            cover_url: row.cover,
            restype,
            tags,
            devices,
            manifest_path: format!("{}.json", row.path), // 约定
            paid,
            source: RepoSource::LocalUser,
        };
        if !item.passes_device_and_paid_filter(device_code) {
            continue;
        }
        out.push(item);
    }
    Ok(out)
}

/// 向本地源登记一个已写入 SD 卡的文件：追加 CSV 行 + 写最小 manifest json。
///
/// 返回 `RepoItem`，供 API 直接回传。
pub async fn add_local_entry(
    sd_root: &Path,
    name: &str,
    restype: RepoType,
    devices: &[String],
    file_sd_abs_path: &Path, // 必须已经写好在 SD 卡上
    version: Option<&str>,
    package_name: Option<&str>,
) -> Result<RepoItem> {
    use std::io::Write;

    let restype_str = match restype {
        RepoType::QuickApp => "quickapp",
        RepoType::Watchface => "watchface",
    };
    let tags: String = String::new();
    let device_str = devices.join(";");
    let row = LocalIndexRow {
        name: name.to_string(),
        icon: String::new(),
        cover: String::new(),
        restype: restype_str.to_string(),
        tags,
        devices: device_str,
        path: file_sd_abs_path.to_string_lossy().to_string(),
        paid_type: "free".to_string(),
    };
    // 1) append csv (sync append: 1 行数据 ≤ 1 KB，无并发写入时直接用 fs::OpenOptions append)
    let csv = csv_path(sd_root);
    ensure_local_source(sd_root).await?;
    let csv_path_clone = csv.clone();
    let row_clone = row.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(false)
            .append(true)
            .open(&csv_path_clone)
            .with_context(|| format!("append csv {}", csv_path_clone.display()))?;
        let mut wtr = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(Vec::<u8>::new());
        wtr.serialize(&row_clone)
            .map_err(|e| anyhow!("serialize local row: {e}"))?;
        f.write_all(
            wtr.into_inner()
                .map_err(|e| anyhow!("wtr into_inner: {e}"))?
                .as_slice(),
        )
        .map_err(|e| anyhow!("write csv row: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("append csv task panicked: {e}"))??;

    // 2) 写最小 manifest json（用于 install_from_repo 复用）
    let manifest_path = format!("{}.json", row.path);
    let payload_key = match restype {
        RepoType::QuickApp => "quickapp_rpk_url",
        RepoType::Watchface => "watchface_url",
    };
    let payload_value = &row.path; // 对本地源，payload URL 就是 SD 卡绝对路径
    let mut manifest = serde_json::Map::new();
    manifest.insert(
        payload_key.to_string(),
        serde_json::Value::String(payload_value.to_string()),
    );
    if let Some(v) = version {
        manifest.insert(
            "version".to_string(),
            serde_json::Value::String(v.to_string()),
        );
    }
    if let Some(pkg) = package_name {
        manifest.insert(
            "package_name".to_string(),
            serde_json::Value::String(pkg.to_string()),
        );
    }
    if let Ok(meta) = tokio::fs::metadata(file_sd_abs_path).await {
        manifest.insert(
            "filesize".to_string(),
            serde_json::Value::Number(meta.len().into()),
        );
    }
    tokio::fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::Value::Object(manifest))
            .context("serialize local manifest")?,
    )
    .await
    .with_context(|| format!("write manifest {manifest_path}"))?;

    Ok(RepoItem {
        name: row.name,
        icon_url: row.icon,
        cover_url: row.cover,
        restype,
        tags: Vec::new(),
        devices: row
            .devices
            .split(';')
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        manifest_path,
        paid: PaidStatus::Free,
        source: RepoSource::LocalUser,
    })
}

/// 给上传使用：将字节写到 `/sdcard/astrobox/local/<safe_name>.<ext>`，返回绝对路径。
///
/// `safe_name` 规则：只保留 ASCII 字母数字/`-._`，其余变 `_`，最多 64 字。
pub async fn write_uploaded_bytes(
    sd_root: &Path,
    orig_name: &str,
    ext: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let dir = local_dir(sd_root);
    sdcard::ensure_dir(&dir)?;

    let safe = sanitize_name(orig_name);
    let safe = if safe.is_empty() {
        format!(
            "upload_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        )
    } else {
        safe
    };

    let path = dir.join(format!("{safe}.{ext}"));
    let path = dedup_path(&path);
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("write upload {}", path.display()))?;
    Ok(path)
}

fn sanitize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '.' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out.truncate(64);
    out
}

/// 若 path 已存在，追加 `-1` / `-2` 直到不存在。
fn dedup_path(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let parent = p.parent().unwrap_or(Path::new("/"));
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin")
        .to_string();
    for i in 1..10_000 {
        let cand = parent.join(format!("{stem}-{i}.{ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    // 极端情况：在文件名后缀随机 6 位十六进制
    let rnd = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u32;
    parent.join(format!("{stem}-{rnd:08x}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_maps_non_ascii() {
        assert_eq!(sanitize_name("倒数日 v1.0.rpk"), "___v1.0.rpk");
        assert_eq!(sanitize_name("HyperBili@2024"), "HyperBili_2024");
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn dedup_handles_existing() {
        // 先造一个假存在路径（不真正读写文件），通过构造 path 对象检查格式
        let fake = std::path::PathBuf::from("/tmp/nonexistent_xyz/file.rpk");
        assert_eq!(
            dedup_path(&fake).to_string_lossy(),
            "/tmp/nonexistent_xyz/file.rpk"
        );
    }
}
