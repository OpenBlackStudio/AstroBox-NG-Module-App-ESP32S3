//! Host import `watchface` 实现（WIT `astrobox:psys-host/watchface`）
//!
//! Phase 2：复用 [`crate::install`] 的表盘安装 / 列表 / 设为当前 / 卸载。
//!
//! 与 `thirdpartyapp` 对称：所有操作需要已配对的 BLE 设备地址。
//! `install_watchface` 接收**文件路径**（`.mwz` / `.face`），固件侧
//! `install::install_watchface_from_file` 会读文件后走 BLE mass install。

use anyhow::Result;

/// `watchface.get-watchface-list(addr)`：列出已装表盘。
/// 每条格式 `"name (id=…)"`（沿用 `install::list_installed_watchfaces`）。
pub async fn list_watchfaces(addr: &str) -> Result<Vec<String>> {
    crate::install::list_installed_watchfaces(addr).await
}

/// `watchface.install(addr, file_path)`：从 SD 卡路径安装 `.mwz` / `.face`。
pub async fn install_watchface(addr: &str, file_path: &str) -> Result<()> {
    crate::install::install_watchface_from_file(addr, file_path).await
}

/// `watchface.set-current-watchface(addr, watchface_id)`：切换当前表盘。
pub async fn set_watchface(addr: &str, watchface_id: &str) -> Result<()> {
    crate::install::set_watchface(addr, watchface_id).await
}

/// `watchface.uninstall(addr, watchface_id)`：卸载表盘。
pub async fn uninstall_watchface(addr: &str, watchface_id: &str) -> Result<()> {
    crate::install::uninstall_watchface(addr, watchface_id).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        let _ = super::list_watchfaces;
        let _ = super::install_watchface;
        let _ = super::set_watchface;
        let _ = super::uninstall_watchface;
    }
}
