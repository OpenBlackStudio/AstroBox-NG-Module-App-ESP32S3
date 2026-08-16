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
use log::{error, info, warn};
use std::path::Path;
use tokio::fs;

pub(crate) async fn with_device_async<F, T>(
    addr: &str,
    f: F,
) -> anyhow::Result<T>
where
    F: FnOnce(&mut corelib::ecs::World, corelib::ecs::Entity) -> anyhow::Result<T> + Send + 'static,
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
    let addr_owned = addr.to_string();
    let pkg_owned = package_name.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            let component = match world.get::<ResourceComponent>(entity) {
                Some(c) => c,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ResourceComponent missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            let info = component
                .quick_apps
                .iter()
                .find(|item| item.package_name == pkg_owned)
                .map(|item| AppInfo {
                    package_name: item.package_name.clone(),
                    fingerprint: item.fingerprint.clone(),
                });
            match info {
                Some(i) => {
                    let _ = tx.send(Ok(i));
                }
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "App {} not found on device {}",
                        pkg_owned, addr_owned
                    )));
                }
            }
        });
    })
    .await;

    rx.await??
}

pub async fn list_installed_watchfaces(addr: &str) -> anyhow::Result<Vec<String>> {
    let addr = addr.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr, |world, entity| {
            if world.get::<WatchfaceComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "WatchfaceComponent missing on device {}",
                    addr
                )));
                return;
            }
            if world.get::<ResourceComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "ResourceComponent missing on device {}",
                    addr
                )));
                return;
            }
            let mut system = match world.get_mut::<ResourceSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ResourceSystem missing on device {}",
                        addr
                    )));
                    return;
                }
            };
            let receiver = system.request_watchface_list();
            let _ = tx.send(Ok(receiver));
        });
    })
    .await;

    let receiver = rx.await??;
    let watchfaces = receiver.await??;
    let names: Vec<String> = watchfaces
        .iter()
        .map(|w| format!("{} (id={})", w.name.clone(), w.id.clone()))
        .collect();
    info!("Device {} has {} watchface(s) installed", addr, names.len());
    Ok(names)
}

pub async fn list_installed_quick_apps(addr: &str) -> anyhow::Result<Vec<String>> {
    let addr = addr.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr, |world, entity| {
            if world.get::<ResourceComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "ResourceComponent missing on device {}",
                    addr
                )));
                return;
            }
            let mut system = match world.get_mut::<ResourceSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ResourceSystem missing on device {}",
                        addr
                    )));
                    return;
                }
            };
            let receiver = system.request_quick_app_list();
            let _ = tx.send(Ok(receiver));
        });
    })
    .await;

    let receiver = rx.await??;
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

    let addr_owned = addr.to_string();
    let pkg_owned = package_name.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<InstallComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "InstallComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<InstallSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "InstallSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            let result = system.send_install_request(
                MassDataType::ThirdPartyApp,
                app_data,
                Some(&pkg_owned),
            );
            match result {
                Ok(future) => {
                    let _ = tx.send(Ok(future));
                }
                Err(e) => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "Failed to create install request: {e}"
                    )));
                }
            }
        });
    })
    .await;

    let future = rx.await??;
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

    let addr_owned = addr.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<InstallComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "InstallComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<InstallSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "InstallSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            let result = system.send_install_request(
                MassDataType::Watchface,
                face_data,
                None,
            );
            match result {
                Ok(future) => {
                    let _ = tx.send(Ok(future));
                }
                Err(e) => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "Failed to create install request: {e}"
                    )));
                }
            }
        });
    })
    .await;

    let future = rx.await??;
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

    let addr_owned = addr.to_string();
    let pkg_owned = package_name.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<ThirdpartyAppComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "ThirdpartyAppComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<ThirdpartyAppSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ThirdpartyAppSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            let app_info = AppInfo {
                package_name: pkg_owned.clone(),
                fingerprint: vec![],
            };
            system.uninstall_app(&app_info);
            info!("Uninstall request sent for {}", pkg_owned);
            let _ = tx.send(Ok(()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn uninstall_watchface(addr: &str, watchface_id: &str) -> anyhow::Result<()> {
    info!("Uninstalling watchface {} from {}...", watchface_id, addr);

    let addr_owned = addr.to_string();
    let id_owned = watchface_id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<WatchfaceComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "WatchfaceComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<WatchfaceSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "WatchfaceSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            system.uninstall_watchface(&id_owned);
            info!("Uninstall watchface request sent for {}", id_owned);
            let _ = tx.send(Ok(()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn set_watchface(addr: &str, watchface_id: &str) -> anyhow::Result<()> {
    info!("Setting watchface {} on {}...", watchface_id, addr);

    let addr_owned = addr.to_string();
    let id_owned = watchface_id.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<WatchfaceComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "WatchfaceComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<WatchfaceSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "WatchfaceSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            system.set_watchface(&id_owned);
            info!("Set watchface request sent for {}", id_owned);
            let _ = tx.send(Ok(()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn launch_quick_app(addr: &str, package_name: &str) -> anyhow::Result<()> {
    info!("Launching quick app {} on {}...", package_name, addr);

    let app_info = resolve_app_info(addr, package_name).await?;

    let addr_owned = addr.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<ThirdpartyAppComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "ThirdpartyAppComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<ThirdpartyAppSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ThirdpartyAppSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            system.launch_app(&app_info, "");
            info!("Launch request sent for {}", package_name);
            let _ = tx.send(Ok(()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn send_phone_message(
    addr: &str,
    package_name: &str,
    payload: Vec<u8>,
) -> anyhow::Result<()> {
    info!(
        "Sending phone message to app {} on {}...",
        package_name, addr
    );

    let app_info = resolve_app_info(addr, package_name).await?;

    let addr_owned = addr.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<ThirdpartyAppComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "ThirdpartyAppComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<ThirdpartyAppSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ThirdpartyAppSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            system.send_phone_message(&app_info, payload);
            info!("Phone message sent to app {}", package_name);
            let _ = tx.send(Ok(()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}
