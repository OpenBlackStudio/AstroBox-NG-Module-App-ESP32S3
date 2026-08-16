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

pub async fn connect_with_retry() -> anyhow::Result<()> {
    let ble = BLEDevice::take();
    ancs::init_fake_ancs_service(&mut *ble)?;

    let mut consecutive_failures: u32 = 0;

    loop {
        let handle = tokio::runtime::Handle::current();

        match connect_once(&ble, handle).await {
            Ok(()) => {
                consecutive_failures = 0;
                log::info!("MiWear session ended normally");
            }
            Err(err) => {
                consecutive_failures += 1;
                log::warn!(
                    "MiWear connection failed (attempt {}): {err:?}",
                    consecutive_failures
                );
                crate::gui::slint_ui::set_device_connected(false);
            }
        }

        let delay = if consecutive_failures > 0 {
            let backoff = (RECONNECT_DELAY_SECS as u64)
                .saturating_mul(1u64 << (consecutive_failures.min(6)));
            Duration::from_secs(backoff.min(120))
        } else {
            Duration::from_secs(RECONNECT_DELAY_SECS)
        };

        log::info!("Reconnecting in {:?}...", delay);
        tokio::time::sleep(delay).await;
    }
}

async fn connect_once(ble: &BLEDevice, handle: tokio::runtime::Handle) -> anyhow::Result<()> {
    crate::gui::slint_ui::set_device_connected(false);

    let mi_service = u16_uuid(0xFE95);
    let uuid_service_flag = u16_uuid(0x0050);
    let uuid_recv = u16_uuid(0x005E);
    let uuid_sent = u16_uuid(0x005F);

    let mut scan = BLEScan::new();
    scan.active_scan(true).interval(80).window(40);

    info!("Start scanning for supported Xiaomi wearables...");
    let (addr, detected_name) = scan
        .start(ble, SCAN_TIMEOUT_MS, |dev, adv| {
            let fe95_match = adv.service_uuids().any(|u| u == mi_service);
            let name = adv.name().map(|n| n.to_string());
            let name_match = name
                .as_deref()
                .map(is_supported_device_name)
                .unwrap_or(false);

            if fe95_match || name_match {
                let display_name = name.clone().unwrap_or_else(|| "<unnamed>".to_string());
                info!(
                    "Found target: {display_name} rssi={} fe95={fe95_match} name_match={name_match}",
                    dev.rssi()
                );
                Some((dev.addr(), name))
            } else {
                None
            }
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("No supported Xiaomi device found"))?;
    info!("Target addr = {addr}");

    let device_name = detected_name.unwrap_or_else(|| "Unknown Xiaomi Device".to_string());

    let mut client: esp32_nimble::BLEClient = ble.new_client();
    client.set_connection_params(12, 24, 0, 400, 16, 16);

    let disconnect_notify = Arc::new(Notify::new());
    let disconnect_reason = Arc::new(Mutex::new(None));
    client.on_disconnect({
        let disconnect_notify = Arc::clone(&disconnect_notify);
        let disconnect_reason = Arc::clone(&disconnect_reason);
        move |reason| {
            log::warn!("BLE disconnected (reason: {})", reason);
            crate::gui::slint_ui::set_device_connected(false);
            if let Ok(mut slot) = disconnect_reason.lock() {
                *slot = Some(reason);
            }
            disconnect_notify.notify_waiters();
        }
    });

    info!("Connecting...");
    client.connect(&addr).await?;
    info!("Connected = {}", client.connected());
    crate::gui::slint_ui::set_device_connected(true);

    let svc = client
        .get_service(mi_service)
        .await
        .map_err(|_| anyhow::anyhow!("Can't found fe95 service"))?;

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

    let mut ch_service_flag = ch_service_flag.ok_or_else(|| anyhow::anyhow!("0x0050 not found"))?;
    let mut ch_recv = ch_recv.ok_or_else(|| anyhow::anyhow!("0x005e not found"))?;
    let ch_sent = ch_sent.ok_or_else(|| anyhow::anyhow!("0x005f not found"))?;

    if ch_service_flag.can_read() {
        if let Ok(v) = ch_service_flag.read_value().await {
            info!("Read 0x0050 = {:02X?}", v);
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
    let auth_key = "fd0ce943010e5112c6a35cb3ea61b968".to_string();
    let sar_version = 2;

    if ch_recv.can_notify() {
        let notify_handle = handle.clone();
        let notify_addr = device_addr.clone();
        ch_recv.on_notify(move |payload| {
            corelib::device::xiaomi::packet::dispatcher::on_packet(
                notify_handle.clone(),
                notify_addr.clone(),
                payload.to_vec(),
            );
        });
        ch_recv.subscribe_notify(true).await?;
        info!("Subscribed notify on 0x005E");
    } else {
        info!("0x005E doesn't support Notify");
    }

    device::create_device(
        handle.clone(),
        DeviceKind::Xiaomi,
        device_name.clone(),
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
        tokio::task::spawn_local(async move {
            time::sleep(Duration::from_secs(AUTO_LAUNCH_DELAY_SECS)).await;
            match launch_watch_app(&addr_for_launch, AUTO_LAUNCH_PACKAGE).await {
                Ok(_) => log::info!(
                    "Auto launched {} on {}",
                    AUTO_LAUNCH_PACKAGE,
                    addr_for_launch
                ),
                Err(err) => {
                    log::warn!(
                        "Failed to auto launch {} on {}: {err:?}",
                        AUTO_LAUNCH_PACKAGE,
                        addr_for_launch
                    );
                }
            }
        });
    }

    info!("MiWear session ready, waiting for disconnect...");
    disconnect_notify.notified().await;
    let reason = match disconnect_reason.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => None,
    };
    info!("Disconnected from {} (reason: {:?})", device_addr, reason);
    crate::gui::slint_ui::set_device_connected(false);

    Ok(())
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