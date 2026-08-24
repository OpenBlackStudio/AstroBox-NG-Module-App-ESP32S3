//! # `.abp` 插件包解包与校验
//!
//! 对应计划 Phase 1：把 `build_dist.py` 产出的 `.abp`（标准 ZIP，
//! 内含 `manifest.json` + entry `.wasm` + 可选 icon）解包到内存并做最小校验。
//!
//! 格式详见 `.trae/documents/astrobox_plugin_porting_plan.md` §9.1。
//!
//! 本模块纯 Rust（`zip` + `serde_json`），不依赖 WASM runtime，
//! 可独立 unit test，也方便后续被 `runtime.rs` 复用。

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Seek};
use std::path::Path;

/// 单个 `.abp` 内 wasm entry 的最大尺寸（32 MB）。
/// ESP32-S3 N16R8 有 8 MB PSRAM，超过此值的插件几乎肯定跑不起来。
const MAX_WASM_BYTES: usize = 32 * 1024 * 1024;
/// `.abp` 整包上限（含 icon / 附加文件）。
const MAX_ABP_BYTES: usize = 48 * 1024 * 1024;

/// wasm 模块/组件的魔数：`\0asm`
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

/// `manifest.json` 反序列化结构。
///
/// 字段命名与上游模板 `manifest.json` 完全一致（snake_case 由 serde 默认处理）。
/// 除 `name`/`version`/`entry` 必需外，其余皆可选，缺失不影响加载。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// wasm 入口文件在包内的相对路径，如 `astrobox_ng_plugin_template_rust.wasm`
    pub entry: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    /// 2 → `psys-world-v2`；3 → `psys-world-v3`（推测）。默认 2。
    #[serde(default = "default_wasi_version")]
    pub wasi_version: u32,
    /// 3 → 使用 `ui-v3` / `event-v3`。默认 3。
    #[serde(default = "default_api_level")]
    pub api_level: u32,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub additional_files: Vec<String>,
}

fn default_wasi_version() -> u32 {
    2
}
fn default_api_level() -> u32 {
    3
}

/// 解包后的插件包：manifest + wasm 字节 + 可选 icon 字节。
#[derive(Clone, Debug)]
pub struct AbpPackage {
    pub manifest: Manifest,
    pub wasm: Vec<u8>,
    pub icon: Option<Vec<u8>>,
    /// 包内附带的额外文件（`additional_files`），name→bytes。
    /// Phase 1 不强求使用，仅保留以便后续 i18n / 资源加载。
    pub extras: Vec<(String, Vec<u8>)>,
}

impl AbpPackage {
    /// 从磁盘路径加载并解包一个 `.abp`。
    pub fn from_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read abp {}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    /// 从内存字节解包一个 `.abp`（也可直接用于 unit test）。
    pub fn from_bytes(abp: &[u8]) -> Result<Self> {
        if abp.len() > MAX_ABP_BYTES {
            return Err(anyhow!(
                "abp too large: {} bytes > {}",
                abp.len(),
                MAX_ABP_BYTES
            ));
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(abp.to_vec()))
            .map_err(|e| anyhow!("open abp zip: {e}"))?;

        // 1. 读 manifest.json（必需）
        let manifest_bytes = read_zip_entry(&mut zip, "manifest.json")
            .map_err(|e| anyhow!("manifest.json missing: {e}"))?;
        let manifest_str = String::from_utf8(manifest_bytes)
            .context("manifest.json not utf-8")?;
        let manifest: Manifest = serde_json::from_str(&manifest_str)
            .with_context(|| format!("parse manifest.json: {manifest_str}"))?;

        // 2. 校验必需字段
        if manifest.name.trim().is_empty() {
            return Err(anyhow!("manifest.name is empty"));
        }
        if manifest.version.trim().is_empty() {
            return Err(anyhow!("manifest.version is empty"));
        }
        if manifest.entry.trim().is_empty() {
            return Err(anyhow!("manifest.entry is empty"));
        }

        // 3. 读 entry wasm（必需）并校验魔数
        let wasm = read_zip_entry(&mut zip, &manifest.entry)
            .map_err(|e| anyhow!("entry wasm '{}' missing: {e}", manifest.entry))?;
        if wasm.len() < 4 || wasm[..4] != WASM_MAGIC {
            return Err(anyhow!(
                "entry '{}' is not a valid wasm (bad magic)",
                manifest.entry
            ));
        }
        if wasm.len() > MAX_WASM_BYTES {
            return Err(anyhow!(
                "wasm too large: {} > {}",
                wasm.len(),
                MAX_WASM_BYTES
            ));
        }

        // 4. 可选 icon
        let icon = manifest
            .icon
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .and_then(|name| read_zip_entry(&mut zip, name).ok());

        // 5. additional_files（尽力而为，单个失败忽略）
        let mut extras: Vec<(String, Vec<u8>)> = Vec::new();
        for rel in &manifest.additional_files {
            if rel.trim().is_empty() {
                continue;
            }
            if let Ok(data) = read_zip_entry(&mut zip, rel) {
                extras.push((rel.clone(), data));
            }
        }

        Ok(Self {
            manifest,
            wasm,
            icon,
            extras,
        })
    }

    /// 插件 ID：用 manifest `name` 做 safe-id（去空白、转小写、
    /// 非字母数字下划线 → `_`）。同一 name 视为同一插件（覆盖加载）。
    pub fn plugin_id(&self) -> String {
        safe_plugin_id(&self.manifest.name)
    }
}

/// 把任意插件显示名规整成可作为注册表 key 的 id。
pub fn safe_plugin_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_whitespace() || ch == '-' || ch == '.' {
            out.push('_');
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "plugin".to_string()
    } else {
        out
    }
}

/// 从 `ZipArchive` 读单个条目到 `Vec<u8>`。
fn read_zip_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| anyhow!("zip entry '{name}': {e}"))?;
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut buf)
        .map_err(|e| anyhow!("read zip entry '{name}': {e}"))?;
    Ok(buf)
}

// Cursor 已经实现了 Read+Seek；trait 已在文件顶部 import。

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 在内存里造一个最小 `.abp`（zip）字节：manifest.json + 一个最小 wasm（仅魔数+0）。
    fn make_minimal_abp(name: &str, entry: &str) -> Vec<u8> {
        let manifest = format!(
            r#"{{"name":"{name}","version":"1.0.0","entry":"{entry}","wasi_version":2,"api_level":3,"permissions":["network"]}}"#
        );
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("manifest.json", opts).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
            // 最小 wasm：魔数 + version=0
            zip.start_file(entry, opts).unwrap();
            zip.write_all(&[0x00, 0x61, 0x73, 0x6d, 0x00, 0x00, 0x00, 0x00])
                .unwrap();
            // 可选 icon
            zip.start_file("icon.png", opts).unwrap();
            zip.write_all(b"fake-png").unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn parses_minimal_abp() {
        let bytes = make_minimal_abp("Demo Plugin", "demo.wasm");
        let pkg = AbpPackage::from_bytes(&bytes).expect("parse");
        assert_eq!(pkg.manifest.name, "Demo Plugin");
        assert_eq!(pkg.manifest.version, "1.0.0");
        assert_eq!(pkg.manifest.entry, "demo.wasm");
        assert_eq!(pkg.wasm[..4], WASM_MAGIC);
        assert_eq!(pkg.plugin_id(), "demo_plugin");
        assert!(pkg.icon.is_none(), "manifest has no icon field → none");
    }

    #[test]
    fn loads_icon_when_manifest_references_it() {
        let manifest = r#"{"name":"WithIcon","version":"0.2.0","entry":"e.wasm","icon":"icon.png"}"#;
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("manifest.json", opts).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
            zip.start_file("e.wasm", opts).unwrap();
            zip.write_all(&[0x00, 0x61, 0x73, 0x6d, 0, 0, 0, 0]).unwrap();
            zip.start_file("icon.png", opts).unwrap();
            zip.write_all(b"PNGDATA").unwrap();
            zip.finish().unwrap();
        }
        let pkg = AbpPackage::from_bytes(&buf).expect("parse");
        assert_eq!(pkg.icon.as_deref(), Some(&b"PNGDATA"[..]));
        assert_eq!(pkg.plugin_id(), "withicon");
    }

    #[test]
    fn rejects_bad_wasm_magic() {
        let manifest = r#"{"name":"Bad","version":"1.0.0","entry":"e.wasm"}"#;
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("manifest.json", opts).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
            zip.start_file("e.wasm", opts).unwrap();
            zip.write_all(b"notawasm").unwrap();
            zip.finish().unwrap();
        }
        assert!(AbpPackage::from_bytes(&buf).is_err());
    }

    #[test]
    fn rejects_missing_manifest() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("e.wasm", opts).unwrap();
            zip.write_all(&[0x00, 0x61, 0x73, 0x6d, 0, 0, 0, 0]).unwrap();
            zip.finish().unwrap();
        }
        assert!(AbpPackage::from_bytes(&buf).is_err());
    }

    #[test]
    fn safe_plugin_id_normalizes() {
        assert_eq!(safe_plugin_id("Demo Plugin"), "demo_plugin");
        assert_eq!(safe_plugin_id("  Weird-Name.v2 "), "weird_name_v2");
        assert_eq!(safe_plugin_id("Alpha123"), "alpha123");
        // 空 / 全空白 → 兜底 "plugin"
        assert_eq!(safe_plugin_id("   "), "plugin");
        assert_eq!(safe_plugin_id(""), "plugin");
    }
}
