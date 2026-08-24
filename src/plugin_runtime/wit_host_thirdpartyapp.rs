//! Host import `thirdpartyapp` 实现（WIT `astrobox:psys-host/thirdpartyapp`）
//!
//! Phase 2：复用 [`crate::install`] 的快应用安装 / 列表 / 卸载 / 启动。
//!
//! 所有操作需要**已配对的 BLE 设备地址**（`addr`）。插件调用时由 runtime
//! 从 `device.get-connected-device-list` 取当前设备后传入，或在插件
//! manifest 声明 `permissions: ["device"]` 后由 host 注入默认设备。
//!
//! `app-info` record（WIT）含 `name / package_name / fingerprint / version`；
//! 固件侧 [`crate::install`] 现返回 `Vec<String>`（"name (pkg=…)" 格式），
//! Phase 2 直接透传字符串列表，Phase 3 视需要解析回结构化 record。

use anyhow::Result;

/// `thirdpartyapp.get-thirdparty-app-list(addr)`：列出已装快应用。
/// 每条格式 `"name (pkg=package_name)"`（沿用固件 `install::list_installed_quick_apps`）。
pub async fn list_quick_apps(addr: &str) -> Result<Vec<String>> {
    crate::install::list_installed_quick_apps(addr).await
}

/// `thirdpartyapp.launch-qa(addr, package_name)`：启动指定快应用。
pub async fn launch_quick_app(addr: &str, package_name: &str) -> Result<()> {
    crate::install::launch_quick_app(addr, package_name).await
}

/// `thirdpartyapp.install(addr, package_name, file_path)`：从 SD 卡路径安装 `.rpk`。
///
/// 复用 `install::install_quick_app_from_file`（读文件 → BLE mass install）。
/// `package_name` 由调用方提供（缺失时 install 侧会用文件名兜底）。
pub async fn install_quick_app(
    addr: &str,
    package_name: &str,
    file_path: &str,
) -> Result<()> {
    crate::install::install_quick_app_from_file(addr, package_name, file_path).await
}

/// `thirdpartyapp.uninstall(addr, package_name)`：卸载快应用。
pub async fn uninstall_quick_app(addr: &str, package_name: &str) -> Result<()> {
    crate::install::uninstall_quick_app(addr, package_name).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        let _ = super::list_quick_apps;
        let _ = super::launch_quick_app;
        let _ = super::install_quick_app;
        let _ = super::uninstall_quick_app;
    }
}
