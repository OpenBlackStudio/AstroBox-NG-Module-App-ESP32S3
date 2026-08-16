use corelib::device::{
    self,
    xiaomi::{
        components::{
            resource::{ResourceComponent, ResourceSystem},
            thirdparty_app::{AppInfo, ThirdpartyAppComponent, ThirdpartyAppSystem},
        },
        r#type::ConnectType,
        SendError,
    },
    DeviceKind,
};
use esp32_nimble::{utilities::BleUuid, utilities::BleUuid::Uuid16, BLEDevice, BLEScan};
use log::info;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, Notify},
    time,
};

pub mod ancs;

const AUTO_LAUNCH_PACKAGE: &str = "com.searchstars.hyperbilibili";
const AUTO_LAUNCH_DELAY_SECS: u64 = 10;
const RECONNECT_DELAY_SECS: u64 = 5;
const SCAN_TIMEOUT_MS: u64 = 10_000;

const SUPPORTED_DEVICE_KEYWORDS: &[&str] = &[
    "Xiaomi Watch S5",
    "Xiaomi Watch S4",
    "Xiaomi Watch S3",
    "Mi Band 10 Pro",
    "Mi Band 10",
    "Mi Band 9 Pro",
    "Mi Band 9",
    "Redmi Watch 6",
    "Redmi Watch 5 eSIM",
    "Redmi Watch 5",
    "REDMI Watch 6",
    "REDMI Watch 5 eSIM",
    "REDMI Watch 5",
];

const SUPPORTED_GENERIC_KEYWORDS: &[&str] = &[
    "Xiaomi Watch",
    "Mi Band",
    "Redmi Watch",
    "REDMI Watch",
    "Band",
];

struct ConnectedDevice {
    addr: String,
    name: String,
}

pub async fn connect_with_retry() -> anyhow::Result<()> {
    let ble = BLEDevice::take();
    ancs::init_fake_ancs_service(&mut *ble)?;

    let handle = tokio::runtime::Handle::current();

    let shared_disconnect_addr = Arc::new(Mutex::new(None::<String>));
    let shared_notify = Arc::new(Notify::new());

    let mut sessions: Vec<ConnectedDevice> = Vec::new();

    loop {
        info!("Scanning for supported Xiaomi wearables...");
        let found = match scan_all_supported_devices(&ble).await {
            Ok(list) => list,
            Err(err) => {
                log::warn!("Scan failed: {err:?}");
                tokio::time::sleep(Duration::from_secs(RECONNECT_DELAY_SECS)).await;
                continue;
            }
        };

        if found.is_empty() {
            log::warn!("No supported devices found");
            crate::gui::slint_ui::set_device_connected(false);
            crate::gui::slint_ui::set_connected_device_count(0);
            tokio::time::sleep(Duration::from_secs(RECONNECT_DELAY_SECS)).await;
            continue;
        }

        log!("Found {} device(s)", found.len());

        for (addr, name) in &found {
            if sessions.iter().any(|s| s.addr == *addr) {
                log::info!("Already connected to {name} ({addr}), skipping");
                continue;
            }

            match connect_one_device(
                &ble,
                addr,
                name,
                &handle,
                &shared_disconnect_addr,
                &shared_notify,
            )
            .await
            {
                Ok(()) => {
                    sessions.push(ConnectedDevice {
                        addr: addr.clone(),
                        name: name.clone(),
                    });
                    log::info!("Connected to {} ({addr})", name);
                }
                Err(err) => {
                    log::warn!("Failed to connect to {} ({addr}): {err:?}", name);
                }
            }
        }

        let count = sessions.len();
        crate::gui::slint_ui::set_device_connected(count > 0);
        crate::gui::slint_ui::set_connected_device_count(count);

        if sessions.is_empty() {
            log::warn!("All connection attempts failed");
            tokio::time::sleep(Duration::from_secs(RECONNECT_DELAY_SECS)).await;
            continue;
        }

        log!("{} device(s) connected, waiting for events...", count);

        shared_notify.notified().await;

        let disconnected_addr = shared_disconnect_addr.lock().unwrap().take();

        if let Some(daddr) = disconnected_addr {
            if let Some(pos) = sessions.iter().position(|s| s.addr == daddr) {
                let session = sessions.remove(pos);
                let remaining = sessions.len();

                log::warn!(
                    "Device {} ({}) disconnected, {} device(s) remaining",
                    session.name,
                    session.addr,
                    remaining
                );

                crate::gui::slint_ui::set_connected_device_count(remaining);
                crate::gui::slint_ui::set_device_connected(remaining > 0);

                tokio::time::sleep(Duration::from_secs(RECONNECT_DELAY_SECS)).await;

                log!("Attempting to reconnect to {} ({})", session.name, session.addr);

                match connect_one_device(
                    &ble,
                    &session.addr,
                    &session.name,
                    &handle,
                    &shared_disconnect_addr,
                    &shared_notify,
                )
                .await
                {
                    Ok(()) => {
                        sessions.push(ConnectedDevice {
                            addr: session.addr.clone(),
                            name: session.name.clone(),
                        });
                        log::info!("Reconnected to {} ({})", session.name, session.addr);
                    }
                    Err(err) => {
                        log::warn!(
                            "Failed to reconnect to {} ({}): {err:?}",
                            session.name,
                            session.addr
                        );
                    }
                }

                crate::gui::slint_ui::set_connected_device_count(sessions.len());
                crate::gui::slint_ui::set_device_connected(!sessions.is_empty());
            }
        }
    }
}

async fn scan_all_supported_devices(
    ble: &BLEDevice,
) -> anyhow::Result<Vec<(String, String)>> {
    let mi_service = u16_uuid(0xFE95);
    let mut scan = BLEScan::new();
    scan.active_scan(true).interval(80).window(40);

    let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let results_ref = Arc::clone(&results);

    let mut discovered: Vec<(String, String)> = Vec::new();

    scan.start(ble, SCAN_TIMEOUT_MS, |dev, adv| {
        let fe95_match = adv.service_uuids().any(|u| u == mi_service);
        let name = adv.name().map(|n| n.to_string());
        let name_match = name
            .as_deref()
            .map(is_supported_device_name)
            .unwrap_or(false);

        if fe95_match || name_match {
            let addr = dev.addr().to_string();
            let display_name = name.clone().unwrap_or_else(|| {
                if fe95_match {
                    "Unknown Xiaomi Device".to_string()
                } else {
                    "Unknown Device".to_string()
                }
            });

            log::info!(
                "Found target: {} rssi={} fe95={fe95_match} name_match={name_match}",
                display_name,
                dev.rssi()
            );

            let mut vec = results_ref.lock().unwrap();
            vec.push((addr, display_name));
        }
        None::<()>
    })
    .await?;

    let mut vec = results.lock().unwrap();
    std::mem::swap(&mut *vec, &mut discovered);

    discovered.sort_by(|a, b| {
        let a_priority = if a.1.starts_with("Unknown") { 1 } else { 0 };
        let b_priority = if b.1.starts_with("Unknown") { 1 } else { 0 };
        a_priority.cmp(&b_priority)
    });

    Ok(discovered)
}

async fn connect_one_device(
    ble: &BLEDevice,
    addr: &str,
    device_name: &str,
    handle: &tokio::runtime::Handle,
    shared_disconnect_addr: &Arc<Mutex<Option<String>>>,
    shared_notify: &Arc<Notify>,
) -> anyhow::Result<()> {
    let mi_service = u16_uuid(0xFE95);
    let uuid_service_flag = u16_uuid(0x0050);
    let uuid_recv = u16_uuid(0x005E);
    let uuid_sent = u16_uuid(0x005F);

    let mut client: esp32_nimble::BLEClient = ble.new_client();
    client.set_connection_params(12, 24, 0, 400, 16, 16);

    let device_addr_owned = addr.to_string();
    client.on_disconnect({
        let shared_addr = Arc::clone(shared_disconnect_addr);
        let shared_notify = Arc::clone(shared_notify);
        let disconnect_addr = device_addr_owned.clone();
        let device_name = device_name.to_string();
        move |reason| {
            log::warn!(
                "BLE disconnected from {} (reason: {})",
                device_name,
                reason
            );
            if let Ok(mut slot) = shared_addr.lock() {
                *slot = Some(disconnect_addr);
            }
            shared_notify.notify_waiters();
        }
    });

    info!("Connecting to {} ({})...", device_name, addr);
    client.connect(addr).await?;
    info!("Connected to {} (connected={})", device_name, client.connected());

    let svc = client
        .get_service(mi_service)
        .await
        .map_err(|_| anyhow::anyhow!("Can't find FE95 service on {}", device_name))?;

    let mut ch_service_flag = None;
    let mut ch_recv = None;
    let mut ch_sent = None;

    let chars: Vec<_> = svc.get_characteristics().await?.collect();

    for c in &chars {
        let u = c.uuid();
        if ch_recv.is_none() && u == uuid_recv {
            ch_recv = Some((*c).clone());
            continue;
        }
        if ch_sent.is_none() && u == uuid_sent {
            ch_sent = Some((*c).clone());
            continue;
        }
        if ch_service_flag.is_none() && u == uuid_service_flag {
            ch_service_flag = Some((*c).clone());
            continue;
        }
    }

    if ch_service_flag.is_none() || ch_recv.is_none() || ch_sent.is_none() {
        for c in &chars {
            let u = c.uuid();
            if ch_recv.is_none() && uuid_contains(&u, "005e") {
                ch_recv = Some((*c).clone());
                continue;
            }
            if ch_sent.is_none() && uuid_contains(&u, "005f") {
                ch_sent = Some((*c).clone());
                continue;
            }
            if ch_service_flag.is_none() && uuid_contains(&u, "0050") {
                ch_service_flag = Some((*c).clone());
                continue;
            }
        }
    }

    let mut ch_service_flag = ch_service_flag
        .ok_or_else(|| anyhow::anyhow!("0x0050 not found on {}", device_name))?;
    let mut ch_recv = ch_recv
        .ok_or_else(|| anyhow::anyhow!("0x005e not found on {}", device_name))?;
    let ch_sent = ch_sent
        .ok_or_else(|| anyhow::anyhow!("0x005f not found on {}", device_name))?;

    if ch_service_flag.can_read() {
        if let Ok(v) = ch_service_flag.read_value().await {
            info!("Read 0x0050 = {:02X?} on {}", v, device_name);
        }
    }

    let (send_tx, mut rx) =
        mpsc::unbounded_channel::<(Vec<u8>, oneshot::Sender<Result<(), SendError>>)>();
    let mut ch_sent_worker = ch_sent;
    let _send_task = tokio::task::spawn_local(async move {
        while let Some((data, responder)) = rx.recv().await {
            let result: Result<(), SendError> = async {
                if ch_sent_worker.can_write() {
                    ch_sent_worker
                        .write_value(&data, true)
                        .await
                        .map_err(|e| SendError::Io(e.to_string()))?;
                } else if ch_sent_worker.can_write_no_response() {
                    ch_sent_worker
                        .write_value(&data, false)
                        .await
                        .map_err(|e| SendError::Io(e.to_string()))?;
                } else {
                    return Err(SendError::Io("0x005F can't write".to_string()));
                }
                Ok(())
            }
            .await;
            let _ = responder.send(result);
        }
    });

    let send_queue = Arc::new(send_tx);
    let send_cb = {
        let tx = Arc::clone(&send_queue);
        move |data: Vec<u8>| {
            let tx = Arc::clone(&tx);
            async move {
                let (resp_tx, resp_rx) = oneshot::channel();
                tx.send((data, resp_tx))
                    .map_err(|_| SendError::Io("send queue closed".to_string()))?;
                resp_rx
                    .await
                    .map_err(|_| SendError::Io("send task dropped".to_string()))?
            }
        }
    };

    let device_addr = addr.to_string();
    // BLE authentication key for Xiaomi wearable protocol.
    // Configure at compile time via MIWEAR_AUTH_KEY env var (32-char hex string).
    // Example: MIWEAR_AUTH_KEY=fd0ce943010e5112c6a35cb3ea61b968
    // An empty string means "no auth key" and will likely fail device authentication.
    let auth_key = env!("MIWEAR_AUTH_KEY", "").to_string();
    if auth_key.is_empty() {
        log::warn!(
            "No MiWear auth key configured. Set MIWEAR_AUTH_KEY at compile time \
             or configure via NVS for production use."
        );
    }
    let sar_version = 2;

    if ch_recv.can_notify() {
        let notify_handle = handle.clone();
        let notify_addr = device_addr.clone();
        let notify_device = device_name.to_string();
        ch_recv.on_notify(move |payload| {
            corelib::device::xiaomi::packet::dispatcher::on_packet(
                notify_handle.clone(),
                notify_addr.clone(),
                payload.to_vec(),
            );
        });
        ch_recv.subscribe_notify(true).await?;
        info!("Subscribed notify on 0x005E for {}", notify_device);
    } else {
        info!("0x005E doesn't support Notify on {}", device_name);
    }

    device::create_device(
        handle.clone(),
        DeviceKind::Xiaomi,
        device_name.to_string(),
        device_addr.clone(),
        auth_key,
        sar_version,
        ConnectType::BLE,
        false,
        move |data| {
            let fut = send_cb(data);
            async move {
                fut.await.map_err(|err| {
                    log::error!("send failed: {:?}", err);
                    err
                })
            }
        },
    )
    .await?;

    {
        let addr_for_launch = device_addr.clone();
        let device_for_launch = device_name.to_string();
        tokio::task::spawn_local(async move {
            time::sleep(Duration::from_secs(AUTO_LAUNCH_DELAY_SECS)).await;
            match launch_watch_app(&addr_for_launch, AUTO_LAUNCH_PACKAGE).await {
                Ok(_) => log::info!(
                    "Auto launched {} on {}",
                    AUTO_LAUNCH_PACKAGE,
                    device_for_launch
                ),
                Err(err) => {
                    log::warn!(
                        "Failed to auto launch {} on {}: {err:?}",
                        AUTO_LAUNCH_PACKAGE,
                        device_for_launch
                    );
                }
            }
        });
    }

    info!("Device {} ({}) session ready", device_name, addr);
    Ok(())
}

fn u16_uuid(u: u16) -> BleUuid {
    BleUuid::from(Uuid16(u))
}

fn uuid_contains(u: &BleUuid, needle: &str) -> bool {
    let s = format!("{u:?}").replace('-', "").to_ascii_lowercase();
    s.contains(&needle.to_ascii_lowercase())
}

fn is_supported_device_name(name: &str) -> bool {
    let name_lower = name.to_ascii_lowercase();
    SUPPORTED_DEVICE_KEYWORDS
        .iter()
        .any(|kw| name_lower.contains(&kw.to_ascii_lowercase()))
        || SUPPORTED_GENERIC_KEYWORDS
            .iter()
            .any(|kw| name_lower.contains(&kw.to_ascii_lowercase()))
}

async fn launch_watch_app(addr: &str, package: &str) -> anyhow::Result<()> {
    let app_info = resolve_app_info(addr, package).await?;
    let addr_owned = addr.to_string();
    let info = app_info.clone();
    corelib::ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<ThirdpartyAppComponent>(entity).is_none() {
                return Err(anyhow::anyhow!("third-party component missing"));
            }
            let mut system = world
                .get_mut::<ThirdpartyAppSystem>(entity)
                .ok_or_else(|| anyhow::anyhow!("third-party system missing"))?;
            system.launch_app(&info, "");
            Ok(())
        })
        .unwrap_or_else(|| Err(anyhow::anyhow!("device {} not found", addr_owned)))
    })
    .await
}

async fn resolve_app_info(addr: &str, package: &str) -> anyhow::Result<AppInfo> {
    if let Some(info) = lookup_cached_app_info(addr, package).await? {
        return Ok(info);
    }

    refresh_quick_app_list(addr).await?;

    lookup_cached_app_info(addr, package)
        .await?
        .ok_or_else(|| anyhow::anyhow!("package {} not installed on {}", package, addr))
}

async fn lookup_cached_app_info(addr: &str, package: &str) -> anyhow::Result<Option<AppInfo>> {
    let addr_owned = addr.to_string();
    let package_owned = package.to_string();
    corelib::ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            let component = match world.get::<ResourceComponent>(entity) {
                Some(comp) => comp,
                None => return Ok(None),
            };
            let info = component
                .quick_apps
                .iter()
                .find(|item| item.package_name == package_owned)
                .map(|item| AppInfo {
                    package_name: item.package_name.clone(),
                    fingerprint: item.fingerprint.clone(),
                });
            Ok(info)
        })
        .unwrap_or_else(|| Err(anyhow::anyhow!("device {} not found", addr_owned)))
    })
    .await
}

async fn refresh_quick_app_list(addr: &str) -> anyhow::Result<()> {
    let addr_owned = addr.to_string();
    let rx = corelib::ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&addr_owned, |world, entity| {
            if world.get::<ResourceComponent>(entity).is_none() {
                return Err(anyhow::anyhow!("resource component missing"));
            }
            let mut system = world
                .get_mut::<ResourceSystem>(entity)
                .ok_or_else(|| anyhow::anyhow!("resource system missing"))?;
            Ok::<_, anyhow::Error>(system.request_quick_app_list())
        })
        .unwrap_or_else(|| Err(anyhow::anyhow!("device {} not found", addr_owned)))
    })
    .await?;

    rx.await
        .map_err(|err| anyhow::anyhow!("quick app response dropped: {err:?}"))??;
    Ok(())
}
