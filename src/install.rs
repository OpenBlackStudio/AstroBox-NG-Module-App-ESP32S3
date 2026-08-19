use corelib::{
    device::xiaomi::{
        components::{
            install::{InstallComponent, InstallSystem},
            resource::{ResourceComponent, ResourceSystem},
            thirdparty_app::{AppInfo, ThirdpartyAppComponent, ThirdpartyAppSystem},
            watchface::{WatchfaceComponent, WatchfaceSystem},
        },
        packet::mass::MassDataType,
    },
    ecs,
};
use log::info;
use std::path::Path;
use tokio::fs;

/// #19: Generic helper that encapsulates the recurring
/// `ecs::with_rt_mut + oneshot + rt.with_device_mut` access pattern.
///
/// Every `install_* / uninstall_* / list_*` function in this module used to
/// inline ~15 lines of identical `ecs::with_rt_mut(...) + oneshot + rx.await`
/// boilerplate; routing them through `with_device_async` drops the
/// duplication and centralises error surface.
pub(crate) async fn with_device_async<F, T>(addr: &str, f: F) -> anyhow::Result<T>
where
    F: FnOnce(&mut corelib::ecs::World, corelib::ecs::Entity) -> anyhow::Result<T>
        + Send
        + 'static,
    T: Send + 'static,
{
    let addr_owned = addr.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            let result = f(world, entity);
            let _ = tx.send(result);
        });
    })
    .await;

    rx.await?
}

pub(crate) async fn resolve_app_info(addr: &str, package_name: &str) -> anyhow::Result<AppInfo> {
    let pkg_owned = package_name.to_string();
    with_device_async(addr, move |world, entity| {
        let component = world
            .get::<ResourceComponent>(entity)
            .ok_or_else(|| anyhow::anyhow!("ResourceComponent missing on device"))?;
        component
            .quick_apps
            .iter()
            .find(|item| item.package_name == pkg_owned)
            .map(|item| AppInfo {
                package_name: item.package_name.clone(),
                fingerprint: item.fingerprint.clone(),
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "App {} not found in installed list",
                    pkg_owned
                )
            })
    })
    .await
}

pub async fn list_installed_watchfaces(addr: &str) -> anyhow::Result<Vec<String>> {
    let receiver = with_device_async(addr, |world, entity| {
        if world.get::<WatchfaceComponent>(entity).is_none() {
            anyhow::bail!("WatchfaceComponent missing");
        }
        if world.get::<ResourceComponent>(entity).is_none() {
            anyhow::bail!("ResourceComponent missing");
        }
        let mut system = world
            .get_mut::<ResourceSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("ResourceSystem missing"))?;
        Ok(system.request_watchface_list())
    })
    .await?;

    let watchfaces = receiver.await??;
    let names: Vec<String> = watchfaces
        .iter()
        .map(|w| format!("{} (id={})", w.name.clone(), w.id.clone()))
        .collect();
    info!("Device {} has {} watchface(s) installed", addr, names.len());
    Ok(names)
}

pub async fn list_installed_quick_apps(addr: &str) -> anyhow::Result<Vec<String>> {
    let receiver = with_device_async(addr, |world, entity| {
        if world.get::<ResourceComponent>(entity).is_none() {
            anyhow::bail!("ResourceComponent missing");
        }
        let mut system = world
            .get_mut::<ResourceSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("ResourceSystem missing"))?;
        Ok(system.request_quick_app_list())
    })
    .await?;

    let apps = receiver.await??;
    let names: Vec<String> = apps
        .iter()
        .map(|a| format!("{} (pkg={})", a.name.clone(), a.package_name.clone()))
        .collect();
    info!("Device {} has {} quick app(s) installed", addr, names.len());
    Ok(names)
}

pub async fn install_quick_app(
    addr: &str,
    package_name: &str,
    app_data: Vec<u8>,
) -> anyhow::Result<()> {
    info!(
        "Installing quick app {} ({}) on {}...",
        package_name,
        app_data.len(),
        addr
    );

    let pkg_owned = package_name.to_string();
    let future = with_device_async(addr, move |world, entity| {
        if world.get::<InstallComponent>(entity).is_none() {
            anyhow::bail!("InstallComponent missing");
        }
        let mut system = world
            .get_mut::<InstallSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("InstallSystem missing"))?;
        system
            .send_install_request(MassDataType::ThirdPartyApp, app_data, Some(&pkg_owned))
            .map_err(|e| anyhow::anyhow!("Failed to create install request: {e}"))
    })
    .await?;

    future.await.map_err(|e| anyhow::anyhow!(e.to_string()))?;

    info!("Quick app {} installed successfully on {}", package_name, addr);
    Ok(())
}

pub async fn install_quick_app_from_file(
    addr: &str,
    package_name: &str,
    file_path: &str,
) -> anyhow::Result<()> {
    let data = fs::read(Path::new(file_path)).await?;
    install_quick_app(addr, package_name, data).await
}

pub async fn install_watchface(addr: &str, face_data: Vec<u8>) -> anyhow::Result<()> {
    info!(
        "Installing watchface ({}) on {}...",
        face_data.len(),
        addr
    );

    let future = with_device_async(addr, move |world, entity| {
        if world.get::<InstallComponent>(entity).is_none() {
            anyhow::bail!("InstallComponent missing");
        }
        let mut system = world
            .get_mut::<InstallSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("InstallSystem missing"))?;
        system
            .send_install_request(MassDataType::Watchface, face_data, None)
            .map_err(|e| anyhow::anyhow!("Failed to create install request: {e}"))
    })
    .await?;

    future.await.map_err(|e| anyhow::anyhow!(e.to_string()))?;

    info!("Watchface installed successfully on {}", addr);
    Ok(())
}

pub async fn install_watchface_from_file(
    addr: &str,
    file_path: &str,
) -> anyhow::Result<()> {
    let data = fs::read(Path::new(file_path)).await?;
    install_watchface(addr, data).await
}

pub async fn uninstall_quick_app(addr: &str, package_name: &str) -> anyhow::Result<()> {
    info!("Uninstalling quick app {} from {}...", package_name, addr);

    let pkg_owned = package_name.to_string();
    with_device_async(addr, move |world, entity| {
        if world.get::<ThirdpartyAppComponent>(entity).is_none() {
            anyhow::bail!("ThirdpartyAppComponent missing");
        }
        let mut system = world
            .get_mut::<ThirdpartyAppSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("ThirdpartyAppSystem missing"))?;
        let app_info = AppInfo {
            package_name: pkg_owned.clone(),
            fingerprint: vec![],
        };
        system.uninstall_app(&app_info);
        info!("Uninstall request sent for {}", pkg_owned);
        Ok(())
    })
    .await
}

pub async fn uninstall_watchface(addr: &str, watchface_id: &str) -> anyhow::Result<()> {
    info!("Uninstalling watchface {} from {}...", watchface_id, addr);

    let id_owned = watchface_id.to_string();
    with_device_async(addr, move |world, entity| {
        if world.get::<WatchfaceComponent>(entity).is_none() {
            anyhow::bail!("WatchfaceComponent missing");
        }
        let mut system = world
            .get_mut::<WatchfaceSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("WatchfaceSystem missing"))?;
        system.uninstall_watchface(&id_owned);
        info!("Uninstall watchface request sent for {}", id_owned);
        Ok(())
    })
    .await
}

pub async fn set_watchface(addr: &str, watchface_id: &str) -> anyhow::Result<()> {
    info!("Setting watchface {} on {}...", watchface_id, addr);

    let id_owned = watchface_id.to_string();
    with_device_async(addr, move |world, entity| {
        if world.get::<WatchfaceComponent>(entity).is_none() {
            anyhow::bail!("WatchfaceComponent missing");
        }
        let mut system = world
            .get_mut::<WatchfaceSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("WatchfaceSystem missing"))?;
        system.set_watchface(&id_owned);
        info!("Set watchface request sent for {}", id_owned);
        Ok(())
    })
    .await
}

pub async fn launch_quick_app(addr: &str, package_name: &str) -> anyhow::Result<()> {
    info!("Launching quick app {} on {}...", package_name, addr);

    let app_info = resolve_app_info(addr, package_name).await?;

    with_device_async(addr, move |world, entity| {
        if world.get::<ThirdpartyAppComponent>(entity).is_none() {
            anyhow::bail!("ThirdpartyAppComponent missing");
        }
        let mut system = world
            .get_mut::<ThirdpartyAppSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("ThirdpartyAppSystem missing"))?;
        system.launch_app(&app_info, "");
        info!("Launch request sent for {}", package_name);
        Ok(())
    })
    .await
}

pub async fn send_phone_message(
    addr: &str,
    package_name: &str,
    payload: Vec<u8>,
) -> anyhow::Result<()> {
    info!(
        "Sending phone message to app {} on {} ({} bytes)...",
        package_name,
        addr,
        payload.len()
    );

    let app_info = resolve_app_info(addr, package_name).await?;

    with_device_async(addr, move |world, entity| {
        if world.get::<ThirdpartyAppComponent>(entity).is_none() {
            anyhow::bail!("ThirdpartyAppComponent missing");
        }
        let mut system = world
            .get_mut::<ThirdpartyAppSystem>(entity)
            .ok_or_else(|| anyhow::anyhow!("ThirdpartyAppSystem missing"))?;
        system.send_phone_message(&app_info, payload);
        info!("Phone message sent to app {}", package_name);
        Ok(())
    })
    .await
}
