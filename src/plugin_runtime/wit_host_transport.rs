//! Host import `transport` 实现（WIT `astrobox:psys-host/transport`）
//!
//! Phase 2：HTTP GET / POST 最小子集，复用 [`crate::net_http`]。
//!
//! WIT `transport` interface 完整面含 device-to-device `send` / `request`
//! （protobuf over BLE）与 `to-json` / `from-json`；Phase 2 只覆盖**网络
//! HTTP fetch**（插件最常见需求：拉天气、拉配置、上报数据）。设备间
//! `send`/`request` 与 WS 留待 Phase 3。
//!
//! 所有函数 `async`：Phase 3 的 WASM runtime 通过 `wit-bindgen` 生成的
//! `future<list<u8>>` 绑定调用它们；返回值经 runtime 的 async 机制回 wasm。

use anyhow::Result;

/// `transport` HTTP GET 文本：返回响应体字符串。
///
/// 复用 `net_http::get_text`（阻塞线程 + oneshot 回 async，自动重试 2 次）。
pub async fn http_get_text(url: &str) -> Result<String> {
    crate::net_http::get_text(url).await
}

/// `transport` HTTP GET 字节：返回响应体 `Vec<u8>`（≤ 16 MB）。
///
/// 复用 `net_http::get_bytes_with_progress`；进度回调此处置空（插件若需
/// 进度，Phase 3 通过 host→wasm 的事件回调单独推）。
pub async fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    crate::net_http::get_bytes_with_progress(url, |_, _| {}).await
}

/// `transport` HTTP POST：`headers` 至少含 `Content-Type`，`body` 为原始字节。
/// 返回 `(status, body_bytes)`。复用 `net_http::post_raw`。
pub async fn http_post(
    url: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Result<(u16, Vec<u8>)> {
    crate::net_http::post_raw(url, headers, body).await
}

#[cfg(test)]
mod tests {
    // 网络函数依赖 esp-idf HTTP 栈 + Wi-Fi，无法在 host 单测里跑；
    // 这里只占位保证模块可编译、`cfg(test)` 路径覆盖。
    #[test]
    fn module_compiles() {
        // 引用三个 async fn 保证它们没被 dead_code 优化掉时报错路径可定位
        let _f1 = super::http_get_text;
        let _f2 = super::http_get_bytes;
        let _f3 = super::http_post;
    }
}
