//! # WASM runtime 后端抽象 + Phase 1 `StubBackend`
//!
//! 真正执行 `.abp` 内 wasm 需要选定支持 **WASI Component Model + async**
//! 的 runtime（候选：WAMR `wasm_component`；或 `wasm-tools` 把 component 拍平
//! 到 P1 module 后用 `wasm3`）。这些集成需要构建环境迭代，Phase 1 无法在此完成。
//!
//! Phase 1 策略：定义 `WasmBackend` trait 作为"接入缝"，提供 `StubBackend`
//! ——**不执行 wasm**，只记录 `on_load` 调用并返回 `Ok`，让 `.abp`
//! 加载/校验/list/卸载 全链路在 ESP32 上跑通。stub 下还会代发一条
//! `dialog.show-info`，用以验证 host→UI 文本通道端到端可用（见计划 §9.2）。
//!
//! 详见 `.trae/documents/astrobox_plugin_porting_plan.md` §9.2 的关键技术约束。

use anyhow::Result;

use crate::plugin_runtime::{abp_package::Manifest, wit_host::HostCtx};

/// 一个已实例化插件的句柄。stub 下仅为字符串 id；真 runtime 下应封装
/// runtime 内的 instance/module 引用。
#[derive(Clone, Debug)]
pub struct PluginHandle {
    pub id: String,
    pub entry: String,
}

/// WASM runtime 后端。Phase 1 唯一实现是 [`StubBackend`]；Phase 2 接入
/// WAMR/wasm3 时新增 `WamrBackend` / `Wasm3Backend` 实现同一 trait。
pub trait WasmBackend {
    /// 实例化 wasm（注入 host import 表）。成功返回句柄。
    fn instantiate(&mut self, wasm: &[u8], manifest: &Manifest) -> Result<PluginHandle>;

    /// 调用插件 export `lifecycle.on_load()`（同步）。
    /// `host` 提供插件可能调用的 host import 实现。
    fn call_on_load(&mut self, handle: &PluginHandle, host: &mut dyn HostCtx) -> Result<()>;

    /// 卸载实例（释放 runtime 内资源）。
    fn unload(&mut self, handle: &PluginHandle) -> Result<()>;
}

/// Phase 1 stub 后端：不真正执行 wasm。
#[derive(Default)]
pub struct StubBackend;

impl WasmBackend for StubBackend {
    fn instantiate(&mut self, wasm: &[u8], manifest: &Manifest) -> Result<PluginHandle> {
        let id = format!("stub-{}-{}b", manifest.name, wasm.len());
        log::info!(
            "[plugin_runtime] StubBackend.instantiate: {} (entry={}, {} bytes) — wasm NOT executed",
            manifest.name,
            manifest.entry,
            wasm.len()
        );
        Ok(PluginHandle {
            id,
            entry: manifest.entry.clone(),
        })
    }

    fn call_on_load(&mut self, handle: &PluginHandle, host: &mut dyn HostCtx) -> Result<()> {
        // 真实 runtime 下，这行 `dialog.show-info` 应由 wasm 插件代码调用 host import 触发。
        // stub 下由宿主代发，证明 host→UI 文本通道可用（计划 Phase 1 可见验证点）。
        host.dialog_show_info(&format!("{} on_load (stub runtime)", handle.id));
        log::warn!(
            "[plugin_runtime] StubBackend.call_on_load: {} — wasm NOT executed, returned Ok by stub",
            handle.id
        );
        Ok(())
    }

    fn unload(&mut self, handle: &PluginHandle) -> Result<()> {
        log::info!("[plugin_runtime] StubBackend.unload: {}", handle.id);
        Ok(())
    }
}
