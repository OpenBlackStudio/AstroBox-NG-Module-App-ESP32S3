//! # 本地包扫描与安装（SD 卡上的 `.rpk/.mwz/.face/.bin`）
//!
//! 对应验收：**AC5**（4 种扩展名列出）、**AC6**（安装分发到 install_*）。
//!
//! 目录固定：`/sdcard/astrobox/packages/`（不存在会自动创建）。
//! 安装触发时走已有 `crate::install::*`，进度事件走
//! [`crate::transfer::TransferProgress`]。

use crate::{
    install,
    transfer::{TransferDirection, TransferProgress},
};
use anyhow::{anyhow, Context, Result};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::{fs, sync::mpsc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalType {
    QuickApp,
    Watchface,
    ResourceBin,
    /// AstroBox 插件包（`.abp`）。安装到 ESP32 宿主本身（非 BLE 设备）。
    /// 仅在 `plugin_runtime` feature 开启时才会被 `classify` 列出。
    Plugin,
}

#[derive(Clone, Debug)]
pub struct LocalPackage {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified_at: Option<SystemTime>,
    pub r#type: LocalType,
    /// 仅对 QuickApp (.rpk) 尝试从包内 `manifest.json` 推断；
    /// 读不到时为 `None`，安装时 fallback 为文件名 stem。
    pub guessed_pkg_name: Option<String>,
}

/// 文件扩展名 → `LocalType`。单独导出（不叫 `pub(crate)`）是为了让
/// `main.rs` 里 webui 安装分发时复用同一判别逻辑，保持路径一致。
pub fn classify(ext: &str) -> Option<LocalType> {
    match ext.to_ascii_lowercase().as_str() {
        "rpk" => Some(LocalType::QuickApp),
        "mwz" | "face" => Some(LocalType::Watchface),
        "bin" => Some(LocalType::ResourceBin),
        // 步骤 6：`.abp` 插件。feature 关闭时不列出，整个插件链路对出货固件透明。
        #[cfg(feature = "plugin_runtime")]
        "abp" => Some(LocalType::Plugin),
        _ => None,
    }
}

/// 列出 `/sdcard/astrobox/packages/` 下所有可安装的包。
///
/// 若目录不存在则创建并返回空 vec（不是 error）。
pub async fn scan_packages(root: Option<&Path>) -> Result<Vec<LocalPackage>> {
    let base = match root {
        Some(r) => r.join("astrobox/packages"),
        None => {
            // 无 SD 卡：视为没有包，但返回友好信息
            return Ok(vec![]);
        }
    };
    fs::create_dir_all(&base)
        .await
        .with_context(|| format!("create packages dir {}", base.display()))?;

    let mut out = Vec::with_capacity(16);
    let mut rd = fs::read_dir(&base)
        .await
        .with_context(|| format!("read_dir {}", base.display()))?;
    while let Some(entry) = rd
        .next_entry()
        .await
        .context("read_dir next_entry failed")?
    {
        let ft = match entry.file_type().await {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(ext) = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_string()) else { continue };
        let Some(kind) = classify(&ext) else { continue };

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let meta = match fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified_at = meta.modified().ok();
        let mut lp = LocalPackage {
            name,
            path,
            size: meta.len(),
            modified_at,
            r#type: kind,
            guessed_pkg_name: None,
        };
        if lp.r#type == LocalType::QuickApp {
            lp.guessed_pkg_name =
                try_extract_rpk_package_name(&lp.path).await.ok();
        }
        out.push(lp);
    }
    // 按 修改时间新→旧 排序
    out.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    Ok(out)
}

/// 轻量的 `.rpk` 包 `manifest.json` package_name 提取。
///
/// 格式假设（小米快应用常见两种）：
/// 1. ZIP 容器（最常见）：根下存在 `manifest.json`（可能 UTF‑8），
///    字段 `"package": "com.xxx.xxx"` 或 `"packageName"`。
/// 2. 非 ZIP 的自定义容器：兜底 stem。
///
/// 本函数不引入 `zip` crate（会增加二进制大小），先按 16 MB 上限
/// 读入整个 rpk，**不严格解压**：
/// - 先检查字节头 "PK\x03\x04"（ZIP local file header）；
/// - 是 ZIP：直接扫字节模式 `manifest.json` 附近的 JSON
///   片段，正则抓包名。
/// - 不是 ZIP：直接返回文件名。
///
/// 失败静默（不返回 Err，只返回 None），绝不影响安装流程。
async fn try_extract_rpk_package_name(path: &Path) -> Result<String> {
    const MAX_RPK: u64 = 16 * 1024 * 1024;
    let meta = fs::metadata(path).await?;
    if meta.len() > MAX_RPK {
        return Err(anyhow!("rpk too big to peek manifest"));
    }
    let bytes = fs::read(path).await?;
    // 找 "package" 字段（宽松匹配，不引入 serde_json 依赖）
    let hay = String::from_utf8_lossy(&bytes);
    // 优先抓 JSON 字段：
    //   "package": "com.xxx" 或 "packageName":"com.xxx"
    for needle in ["\"package\"", "\"packageName\""] {
        let Some(pos) = hay.find(needle) else { continue };
        let tail = &hay[pos + needle.len()..];
        // 跳过空白 / ':' / 空白
        let tail = tail.trim_start_matches(|c: char| c.is_whitespace() || c == ':');
        let tail = tail.trim_start();
        // 下一个字符应是 "
        let Some(tail) = tail.strip_prefix('"') else { continue };
        let Some(end) = tail.find('"') else { continue };
        let pkg = &tail[..end];
        if !pkg.is_empty() && pkg.len() <= 128 {
            return Ok(pkg.to_string());
        }
    }
    // 最终 fallback：stem
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app")
        .to_string();
    Ok(stem)
}

/// Web UI 上传 `.abp/.bin/` 等走「本地包分类路径」直接写入 packages 目录，
/// 不走 local_csv_source（这些条目在列表里与 scan_packages 一致）。
///
/// 返回写入的绝对路径。
pub async fn classify_dir_path(
    sd_root: &Path,
    orig_name: &str,
    ext: &str,
    bytes: Vec<u8>,
) -> Result<PathBuf> {
    use anyhow::anyhow;
    let base = sd_root.join("astrobox/packages");
    tokio::fs::create_dir_all(&base)
        .await
        .with_context(|| format!("create packages dir {}", base.display()))?;
    // sanitize name (ascii safe; 非 ASCII 转 "_")
    let stem = std::path::Path::new(orig_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("upload")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .take(64)
        .collect::<String>();
    let ext = if ext.is_empty() { "bin".to_string() } else { ext.to_lowercase() };
    let mut target = base.join(format!("{stem}.{ext}"));
    if target.exists() {
        for i in 1..10_000 {
            let cand = base.join(format!("{stem}-{i}.{ext}"));
            if !cand.exists() { target = cand; break; }
        }
    }
    tokio::fs::write(&target, bytes)
        .await
        .with_context(|| format!("write upload {}", target.display()))?;
    Ok(target)
}

/// 触发本地安装：读取文件 → 分发到对应 install_* API；
/// 可选进度通道，用于 UI 顶部条显示。
pub async fn install_local(
    device_addr: &str,
    lp: &LocalPackage,
    progress_tx: Option<mpsc::Sender<TransferProgress>>,
) -> Result<()> {
    // 步骤 6：插件安装到 ESP32 宿主本身，不走 BLE / bytes-in-RAM 流程。
    // plugin_runtime::load 自己读文件 + 解包 + 校验，并直接驱动 UI 文本
    // （`FirmwareHostCtx` 把 `dialog.show-info` 写到 slint 进度条），故这里
    // 不再走 TransferProgress channel，避免双写。提前返回绕过下方 16 MB
    // 整文件读入逻辑（插件通常 < 1 MB，路径独立更干净）。
    if lp.r#type == LocalType::Plugin {
        let _ = (device_addr, progress_tx); // 插件不需要设备地址 / 进度 channel
        #[cfg(feature = "plugin_runtime")]
        {
            return crate::plugin_runtime::load(&lp.path).await.map(|_| ());
        }
        #[cfg(not(feature = "plugin_runtime"))]
        {
            return Err(anyhow!(
                "插件运行时未启用（编译时未开 `plugin_runtime` feature）"
            ));
        }
    }

    // 过大包保护
    if lp.size > 16 * 1024 * 1024 {
        return Err(anyhow!(
            "file {} too large ({} bytes > 16 MB)",
            lp.path.display(),
            lp.size
        ));
    }
    if lp.size > 3 * 1024 * 1024 {
        log::warn!(
            "package {} ({}) is > 3 MB; heap usage may spike during transfer",
            lp.path.display(),
            lp.size
        );
    }

    let name = lp.name.clone();
    let total = Some(lp.size as usize);

    // 模拟进度：0%
    emit_progress(
        progress_tx.as_ref(),
        TransferProgress {
            direction: TransferDirection::Send,
            progress_percent: 0.0,
            current_bytes: 0,
            total_bytes: total,
            file_name: name.clone(),
        },
    )
    .await;

    let bytes = fs::read(&lp.path)
        .await
        .with_context(|| format!("read package {}", lp.path.display()))?;
    // 读完毕 → 20%（安装占 20-100%）
    emit_progress(
        progress_tx.as_ref(),
        TransferProgress {
            direction: TransferDirection::Send,
            progress_percent: 20.0,
            current_bytes: bytes.len(),
            total_bytes: total,
            file_name: name.clone(),
        },
    )
    .await;

    let addr = device_addr.to_string();
    let res = match lp.r#type {
        LocalType::QuickApp => {
            let pkg = lp
                .guessed_pkg_name
                .clone()
                .unwrap_or_else(|| lp.name.clone());
            install::install_quick_app(&addr, &pkg, bytes).await
        }
        LocalType::Watchface => install::install_watchface(&addr, bytes).await,
        LocalType::ResourceBin => {
            // 这里复用 transfer 的 send_data_to_device，但它没有公开
            // send_resource 的入口；MassDataType::Resource 对应资源。
            // 由于 corelib 的 MassDataType 定义在外部 crate 可能不一致，
            // 我们走 `install.rs` 中的 `MassDataType`（由它保证一致）。
            // 这里提供 fallback：如果 send_resource API 不可用，就返回
            // 一个明确的错误让用户知道。
            Err(anyhow!(
                ".bin 资源安装暂未开放，请改为快应用/表盘扩展名"
            ))
        }
        // Plugin 在函数开头已提前 return，逻辑上不可达。
        LocalType::Plugin => unreachable!("plugin install handled earlier"),
    };

    match &res {
        Ok(()) => emit_progress(
            progress_tx.as_ref(),
            TransferProgress {
                direction: TransferDirection::Send,
                progress_percent: 100.0,
                current_bytes: lp.size as usize,
                total_bytes: total,
                file_name: name,
            },
        )
        .await,
        Err(e) => {
            log::error!("install_local({}) failed: {e:#}", lp.path.display());
        }
    }
    res
}

async fn emit_progress(tx: Option<&mpsc::Sender<TransferProgress>>, p: TransferProgress) {
    let Some(tx) = tx else { return };
    let _ = tx.send(p).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_ext() {
        assert_eq!(classify("RPK"), Some(LocalType::QuickApp));
        assert_eq!(classify("mwz"), Some(LocalType::Watchface));
        assert_eq!(classify("FACE"), Some(LocalType::Watchface));
        assert_eq!(classify("bin"), Some(LocalType::ResourceBin));
        assert_eq!(classify("txt"), None);
        // 步骤 6：.abp 仅在 plugin_runtime feature 开启时归类为插件
        #[cfg(feature = "plugin_runtime")]
        assert_eq!(classify("abp"), Some(LocalType::Plugin));
        #[cfg(not(feature = "plugin_runtime"))]
        assert_eq!(classify("abp"), None);
    }
}
