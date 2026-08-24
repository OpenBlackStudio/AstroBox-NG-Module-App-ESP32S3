//! Host import `os` 实现（WIT `astrobox:psys-host/os`）
//!
//! Phase 2：把 os interface 的静态信息查询 + 日志 + sleep 真实落地。
//! 这些都是**同步**操作，可直接接入 [`HostCtx`] trait（见 `wit_host.rs`）。
//!
//! WIT `os` interface 字段（host 提供，插件读取）：
//! `arch / hostname / locale / platform / version / astrobox-language /
//!  appearance / timezone-offset-minutes`。外加 `log(level, msg)` 与 `sleep(ms)`。

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

/// 时区偏移（分钟）。默认 UTC+8（+480，中国）。
///
/// 当前固件未提供用户可配的时区设置；ESP-IDF 的 `localtime_r` 依赖 `TZ`
/// 环境变量，固件未设置时返回 GMT。这里先给一个合理的默认值，未来接入
/// NVS 可配时区后用 [`set_timezone_offset`] 覆盖即可。
static TIMEZONE_OFFSET_MIN: AtomicI32 = AtomicI32::new(480);

/// `os` interface 返回的静态/半静态环境信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsInfo {
    pub arch: String,
    pub hostname: String,
    pub locale: String,
    pub platform: String,
    pub version: String,
    pub astrobox_language: String,
    pub appearance: String,
    pub timezone_offset_minutes: i32,
}

/// 覆盖时区偏移（分钟）。供 NVS 配置 / OTA 后修正调用。
pub fn set_timezone_offset(minutes: i32) {
    TIMEZONE_OFFSET_MIN.store(minutes, Ordering::Relaxed);
}

/// 读取当前 os 环境信息。全部为静态值或原子读取，无阻塞、可重入。
pub fn get_os_info() -> OsInfo {
    OsInfo {
        arch: "xtensa".to_string(),
        hostname: "astrobox-ng".to_string(),
        locale: "zh-CN".to_string(),
        platform: "esp32s3".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        astrobox_language: "zh-CN".to_string(),
        appearance: "light".to_string(),
        timezone_offset_minutes: TIMEZONE_OFFSET_MIN.load(Ordering::Relaxed),
    }
}

/// `os.log(level, msg)`：转发到固件 `log` crate，统一加 `[plugin]` 前缀。
///
/// `level` 不区分大小写；未识别的级别按 info 处理并保留原级别字符串。
///
/// 注：本函数名 `log` 与 `log` crate 同名，故内部用绝对路径 `::log::*!`
/// 消除路径前缀歧义（`log::error!` 在此作用域里 `log` 既是本地函数又是
/// 外部 crate，用 `::log::` 显式指向 crate 根）。
pub fn log(level: &str, msg: &str) {
    match level.to_ascii_lowercase().as_str() {
        "error" | "err" | "fatal" => ::log::error!("[plugin] {msg}"),
        "warn" | "warning" => ::log::warn!("[plugin] {msg}"),
        "info" => ::log::info!("[plugin] {msg}"),
        "debug" => ::log::debug!("[plugin] {msg}"),
        "trace" => ::log::trace!("[plugin] {msg}"),
        other => ::log::info!("[plugin][{other}] {msg}"),
    }
}

/// `os.sleep(ms)`：同步阻塞 sleep。
///
/// Phase 3 的 wasm 解释器跑在独立 native 线程上，host import 在该线程的
/// 栈帧里同步回调，故这里用阻塞 sleep（ESP-IDF std 下映射到 `vTaskDelay`，
/// 会交出 CPU）。**不可**在固件 LocalSet 线程上调用（会卡死事件循环）。
/// `ms == 0` 仅做一次 `yield_now`。
pub fn sleep_ms(ms: u64) {
    if ms == 0 {
        std::thread::yield_now();
        return;
    }
    std::thread::sleep(Duration::from_millis(ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_info_defaults() {
        let info = get_os_info();
        assert_eq!(info.platform, "esp32s3");
        assert_eq!(info.arch, "xtensa");
        assert_eq!(info.hostname, "astrobox-ng");
        assert_eq!(info.appearance, "light");
        // version 来自 Cargo.toml，应为语义版本字串
        assert!(!info.version.is_empty());
        assert!(
            info.version.chars().all(|c| c.is_ascii_digit() || c == '.'),
            "version should be numeric: {}",
            info.version
        );
        // 默认时区 UTC+8
        assert_eq!(info.timezone_offset_minutes, 480);
    }

    #[test]
    fn timezone_offset_overridable() {
        let saved = TIMEZONE_OFFSET_MIN.load(Ordering::Relaxed);
        set_timezone_offset(-300); // UTC-5
        assert_eq!(get_os_info().timezone_offset_minutes, -300);
        set_timezone_offset(0); // UTC
        assert_eq!(get_os_info().timezone_offset_minutes, 0);
        // 恢复，避免污染其他测试
        TIMEZONE_OFFSET_MIN.store(saved, Ordering::Relaxed);
    }
}
