use corelib::{
    device::xiaomi::{
        components::{
            mass::{
                MassComponent, MassSystem, SendMassCallbackData,
            },
            thirdparty_app::{AppInfo, ThirdpartyAppComponent, ThirdpartyAppSystem},
        },
        packet::mass::MassDataType,
    },
    ecs, events,
};
use log::{error, info, warn};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub direction: TransferDirection,
    pub progress_percent: f32,
    pub current_bytes: usize,
    pub total_bytes: Option<usize>,
    pub file_name: String,
}

#[derive(Clone, Debug)]
pub enum TransferDirection {
    Send,
    Receive,
}

impl std::fmt::Display for TransferDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferDirection::Send => write!(f, "TX →"),
            TransferDirection::Receive => write!(f, "RX ←"),
        }
    }
}

pub async fn send_data_to_device(
    addr: &str,
    data_type: MassDataType,
    data: Vec<u8>,
) -> anyhow::Result<()> {
    let addr_owned = addr.to_string();
    let (tx, rx) = oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<MassComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "MassComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<MassSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "MassSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            let result = system
                .send_file(data, data_type, |_cb: SendMassCallbackData| {})
                .await;
            let _ = tx.send(result.map(|_| ()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn send_data_to_device_with_progress<F>(
    addr: &str,
    data_type: MassDataType,
    data: Vec<u8>,
    progress_cb: F,
) -> anyhow::Result<()>
where
    F: Fn(SendMassCallbackData) + Send + Sync + 'static,
{
    let addr_owned = addr.to_string();
    let (tx, rx) = oneshot::channel();
    let cb_arc: Arc<dyn Fn(SendMassCallbackData) + Send + Sync> =
        Arc::new(progress_cb);

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<MassComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "MassComponent missing on device {}",
                    addr_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<MassSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "MassSystem missing on device {}",
                        addr_owned
                    )));
                    return;
                }
            };
            let cb = cb_arc.clone();
            let result = system
                .send_file(data, data_type, move |d| cb(d))
                .await;
            let _ = tx.send(result.map(|_| ()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn forward_app_message(
    src_addr: &str,
    dst_addr: &str,
    package_name: &str,
    payload: Vec<u8>,
) -> anyhow::Result<()> {
    info!(
        "[Transfer] Forwarding {} bytes from {} → {} (app: {})",
        payload.len(),
        src_addr,
        dst_addr,
        package_name
    );

    let app_info = resolve_app_info(dst_addr, package_name).await?;

    let dst_owned = dst_addr.to_string();
    let (tx, rx) = oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&dst_owned, |world, entity| {
            if world.get::<ThirdpartyAppComponent>(entity).is_none() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "ThirdpartyAppComponent missing on device {}",
                    dst_owned
                )));
                return;
            }
            let mut system = match world.get_mut::<ThirdpartyAppSystem>(entity) {
                Some(s) => s,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ThirdpartyAppSystem missing on device {}",
                        dst_owned
                    )));
                    return;
                }
            };
            system.send_phone_message(&app_info, payload);
            info!(
                "[Transfer] Message forwarded to {} app on {}",
                package_name, dst_owned
            );
            let _ = tx.send(Ok(()));
        });
    })
    .await;

    rx.await??;
    Ok(())
}

pub async fn relay_interconnect_message(
    src_addr: &str,
    dst_addr: &str,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let src = src_addr.to_string();
    let dst = dst_addr.to_string();

    info!(
        "[Transfer] Starting interconnect relay: {} → {}",
        src, dst
    );

    let mut subscriber = events::subscribe();

    let handle = tokio::task::spawn(async move {
        while let Ok(event) = subscriber.recv().await {
            match event {
                events::CoreEvent::InterconnectMessage(msg) => {
                    if msg.device_addr == src {
                        info!(
                            "[Transfer] Relaying message from {} (app: {}, {} bytes) → {}",
                            src,
                            msg.pkg_name,
                            msg.payload.len(),
                            dst
                        );
                        if let Err(err) =
                            forward_app_message(&src, &dst, &msg.pkg_name, msg.payload).await
                        {
                            warn!(
                                "[Transfer] Failed to relay message {} → {}: {err:?}",
                                src, dst
                            );
                        }
                    }
                }
                events::CoreEvent::DeviceStateChanged(state) => {
                    if state.device_addr == dst {
                        info!(
                            "[Transfer] Target device {} state changed",
                            dst
                        );
                    }
                }
            }
        }
    });

    Ok(handle)
}

pub async fn transfer_quick_app_between_devices(
    src_addr: &str,
    dst_addr: &str,
    package_name: &str,
) -> anyhow::Result<()> {
    info!(
        "[Transfer] Copying quick app {} from {} → {}",
        package_name, src_addr, dst_addr
    );

    let src_owned = src_addr.to_string();
    let (list_tx, list_rx) = oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&src_owned, |world, entity| {
            let component = match world.get::<corelib::device::xiaomi::components::resource::ResourceComponent>(entity) {
                Some(c) => c,
                None => {
                    let _ = list_tx.send(Err(anyhow::anyhow!(
                        "ResourceComponent missing on source device {}",
                        src_owned
                    )));
                    return;
                }
            };
            let app_data = component
                .quick_apps
                .iter()
                .find(|item| item.package_name == package_name)
                .cloned();
            match app_data {
                Some(app) => {
                    let _ = list_tx.send(Ok(app));
                }
                None => {
                    let _ = list_tx.send(Err(anyhow::anyhow!(
                        "App {} not found on source {}",
                        package_name, src_owned
                    )));
                }
            }
        });
    })
    .await;

    let app_item = list_rx.await??;

    info!(
        "[Transfer] Found app {} ({} bytes) on {}, now installing on {}",
        package_name,
        app_item.package_name.len(),
        src_addr,
        dst_addr
    );

    crate::install::install_quick_app(dst_addr, &app_item.package_name, app_item.data.unwrap_or_default()).await?;

    info!(
        "[Transfer] Quick app {} successfully transferred {} → {}",
        package_name, src_addr, dst_addr
    );
    Ok(())
}

pub async fn transfer_watchface_between_devices(
    src_addr: &str,
    dst_addr: &str,
    watchface_id: &str,
) -> anyhow::Result<()> {
    info!(
        "[Transfer] Copying watchface {} from {} → {}",
        watchface_id, src_addr, dst_addr
    );

    let src_owned = src_addr.to_string();
    let id_owned = watchface_id.to_string();
    let (tx, rx) = oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&src_owned, |world, entity| {
            let component = match world.get::<corelib::device::xiaomi::components::resource::ResourceComponent>(entity) {
                Some(c) => c,
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "ResourceComponent missing on source device {}",
                        src_owned
                    )));
                    return;
                }
            };
            let face_data = component
                .watchfaces
                .iter()
                .find(|w| w.id == id_owned)
                .cloned();
            match face_data {
                Some(face) => {
                    let _ = tx.send(Ok(face));
                }
                None => {
                    let _ = tx.send(Err(anyhow::anyhow!(
                        "Watchface {} not found on source {}",
                        id_owned, src_owned
                    )));
                }
            }
        });
    })
    .await;

    let face_item = rx.await??;

    info!(
        "[Transfer] Found watchface {} on {}, now installing on {}",
        watchface_id, src_addr, dst_addr
    );

    crate::install::install_watchface(dst_addr, face_item.data.unwrap_or_default()).await?;

    info!(
        "[Transfer] Watchface {} successfully transferred {} → {}",
        watchface_id, src_addr, dst_addr
    );
    Ok(())
}

pub async fn broadcast_data_to_all_devices(
    data_type: MassDataType,
    data: Vec<u8>,
) -> anyhow::Result<Vec<(String, anyhow::Result<()>)>> {
    let device_ids = ecs::with_rt_mut(|rt| {
        rt.device_ids().cloned().collect::<Vec<_>>()
    })
    .await;

    if device_ids.is_empty() {
        return Ok(vec![]);
    }

    info!(
        "[Transfer] Broadcasting {} bytes to {} device(s)",
        data.len(),
        device_ids.len()
    );

    let mut results = Vec::new();
    for addr in &device_ids {
        let result = send_data_to_device(addr, data_type, data.clone()).await;
        results.push((addr.clone(), result));
        match results.last() {
            Some((a, Ok(_))) => info!("[Transfer] Broadcast to {} OK", a),
            Some((a, Err(e))) => warn!("[Transfer] Broadcast to {} failed: {e:?}", a),
            None => {}
        }
    }

    Ok(results)
}

pub async fn list_connected_devices() -> Vec<String> {
    ecs::with_rt_mut(|rt| rt.device_ids().cloned().collect::<Vec<_>>()).await
}

pub async fn get_device_info(addr: &str) -> anyhow::Result<String> {
    let addr_owned = addr.to_string();
    let (tx, rx) = oneshot::channel();

    ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            let name = world
                .get::<corelib::device::xiaomi::XiaomiDevice>(entity)
                .map(|d| d.name().to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            let _ = tx.send(Ok(name));
        });
    })
    .await;

    rx.await?
}

async fn resolve_app_info(addr: &str, package_name: &str) -> anyhow::Result<AppInfo> {
    crate::install::resolve_app_info(addr, package_name).await
}
