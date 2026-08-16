use core::convert::TryInto;

use anyhow::anyhow;
use corelib::device::xiaomi::{
    components::{info::InfoSystem, network::NetworkComponent},
    XiaomiDevice,
};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{gpio::Pins, modem::Modem, prelude::Peripherals},
    io::vfs::MountedEventfs,
    log::EspLogger,
    nvs::EspDefaultNvsPartition,
    sys::link_patches,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::LevelFilter;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod allocator;
pub mod gui;
pub mod install;
pub mod miwear;
pub mod nvs_config;
pub mod ota;
pub mod statlogger;
pub mod touch;

const WIFI_RECONNECT_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const WIFI_INIT_RETRY_DELAY: Duration = Duration::from_secs(5);
const WIFI_INIT_MAX_RETRIES: u32 = 5;
const OTA_CHECK_INTERVAL: Duration = Duration::from_secs(3600);
const ECS_STACK_SIZE: usize = 32 * 1024;
const TARGET_FPS: u64 = 30;
const UI_BATTERY_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

fn main() -> anyhow::Result<()> {
    link_patches();
    EspLogger::initialize_default();
    configure_component_log_levels();

    let _mounted_eventfs = MountedEventfs::mount(5)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let local = tokio::task::LocalSet::new();

    local.block_on(&rt, run_app())
}

async fn run_app() -> anyhow::Result<()> {
    nvs_config::ensure_nvs_initialized();

    let Peripherals {
        pins,
        ledc,
        spi2,
        i2c0,
        modem,
        ..
    } = Peripherals::take()?;

    let (wifi_ssid, wifi_password) = nvs_config::load_wifi_credentials();
    let mut wifi = init_wifi_with_retry(modem, &wifi_ssid, &wifi_password).await?;

    if let Err(e) = nvs_config::save_wifi_credentials(&wifi_ssid, &wifi_password) {
        log::debug!("Initial Wi-Fi credentials save skipped: {e}");
    }

    tokio::task::spawn_local(async move {
        wifi_reconnect_watchdog(wifi, wifi_ssid, wifi_password).await;
    });

    let ota_manager = std::sync::Arc::new(ota::OtaManager::new());
    {
        let mgr = ota_manager.clone();
        tokio::task::spawn_local(async move {
            ota_check_loop(mgr).await;
        });
    }

    if let Some(initial_ota) = ota_manager.check_for_update() {
        info!(
            "OTA update available: v{} ({} bytes)",
            initial_ota.version, initial_ota.size
        );
    }

    corelib::ecs::init_runtime_default_with_stack(ECS_STACK_SIZE);
    gui::slint_ui::set_device_connected(false);

    tokio::task::spawn_local(async {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            statlogger::log_heap_info();
            log_network_meter().await;
        }
    });
    tokio::task::spawn_local(async {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        let mut last_battery_refresh = std::time::Instant::now() - UI_BATTERY_REFRESH_INTERVAL;
        let mut current_device_id = String::new();
        let mut cached_battery_percent = 0i32;
        let mut cached_charge_text = String::from("2天前充电");

        loop {
            ticker.tick().await;

            let Some(snapshot) = read_first_device_snapshot().await else {
                current_device_id.clear();
                cached_battery_percent = 0;
                cached_charge_text = String::from("2天前充电");
                gui::slint_ui::set_device_status(gui::slint_ui::DeviceStatusUi {
                    device_name: String::new(),
                    battery_percent: cached_battery_percent,
                    charge_text: cached_charge_text.clone(),
                    net_up_text: "0 byte/s ↑".to_string(),
                    net_down_text: "0 byte/s ↓".to_string(),
                });
                continue;
            };

            let need_refresh_battery = current_device_id != snapshot.device_id
                || last_battery_refresh.elapsed() >= UI_BATTERY_REFRESH_INTERVAL;
            if need_refresh_battery {
                if let Some((battery_percent, charge_text)) =
                    read_device_battery_status(&snapshot.device_id).await
                {
                    cached_battery_percent = battery_percent.clamp(0, 100);
                }
                last_battery_refresh = std::time::Instant::now();
                current_device_id = snapshot.device_id.clone();
            }

            gui::slint_ui::set_device_status(gui::slint_ui::DeviceStatusUi {
                device_name: snapshot.device_name,
                battery_percent: cached_battery_percent,
                charge_text: cached_charge_text.clone(),
                net_up_text: format_speed_text(snapshot.write_bps, "↑"),
                net_down_text: format_speed_text(snapshot.read_bps, "↓"),
            });
        }
    });

    let Pins {
        gpio0,
        gpio1,
        gpio2,
        gpio3,
        gpio4,
        gpio5,
        gpio6,
        gpio7,
        gpio18,
        gpio16,
        ..
    } = pins;

    let (mut display, mut backlight) = gui::display::init_display_gc9a01(
        spi2,
        ledc,
        gui::display::DisplayPins {
            backlight: gpio2,
            rst: gpio3,
            dc: gpio4,
            cs: gpio5,
            mosi: gpio6,
            sclk: gpio7,
        },
    )?;

    let _ = &mut backlight;

    touch::spawn_touch_task(
        i2c0,
        touch::TouchPins {
            sda: gpio18,
            scl: gpio16,
            interrupt: gpio1,
            reset: gpio0,
        },
    )?;

    tokio::task::spawn_local(async {
        if let Err(err) = miwear::connect_with_retry().await {
            log::error!("miwear connect loop exited: {err:?}");
        }
    });

    tokio::task::spawn_local(async {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        loop {
            ticker.tick().await;
            sync_installed_items().await;
        }
    });

    tokio::task::spawn_local(async move {
        let frame_interval = Duration::from_nanos(1_000_000_000 / TARGET_FPS);
        loop {
            let frame_start = std::time::Instant::now();
            if let Err(err) = gui::slint_ui::render_hello_world(&mut display) {
                log::error!("render loop exited: {err:?}");
                break;
            }

            let elapsed = frame_start.elapsed();
            if elapsed < frame_interval {
                tokio::time::sleep(frame_interval - elapsed).await;
            } else {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    })
    .await?;

    Ok(())
}

async fn init_wifi_with_retry(
    modem: Modem,
    ssid: &str,
    password: &str,
) -> anyhow::Result<BlockingWifi<EspWifi<'static>>> {
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    let wifi_configuration = Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi SSID is too long"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi password is too long"))?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;
    wifi.start()?;
    log::info!("Wi-Fi started");

    for attempt in 1..=WIFI_INIT_MAX_RETRIES {
        match wifi.connect() {
            Ok(()) => match wifi.wait_netif_up() {
                Ok(()) => {
                    log::info!("Wi-Fi connected to {ssid}");
                    return Ok(wifi);
                }
                Err(err) => {
                    log::warn!(
                        "Wi-Fi netif up failed on attempt {attempt}: {err:?}"
                    );
                }
            },
            Err(err) => {
                log::warn!(
                    "Wi-Fi connect attempt {attempt}/{WIFI_INIT_MAX_RETRIES} failed: {err:?}"
                );
            }
        }

        if attempt < WIFI_INIT_MAX_RETRIES {
            let delay = WIFI_INIT_RETRY_DELAY.saturating_mul(attempt as u32);
            log::info!("Retrying Wi-Fi connection in {:?}...", delay);
            tokio::time::sleep(delay).await;
        }
    }

    Err(anyhow!(
        "Wi-Fi connection failed after {WIFI_INIT_MAX_RETRIES} attempts"
    ))
}

async fn ota_check_loop(manager: std::sync::Arc<ota::OtaManager>) {
    let mut ticker = tokio::time::interval(OTA_CHECK_INTERVAL);
    loop {
        ticker.tick().await;
        if let Some(info) = manager.check_for_update() {
            info!(
                "OTA update available: v{} ({} bytes, {}, url: {})",
                info.version, info.size, info.release_notes, info.url
            );
        }
    }
}

fn configure_component_log_levels() {
    let logger = EspLogger::new();

    if let Err(err) = logger.set_target_level("NimBLE", LevelFilter::Warn) {
        log::warn!("failed to set NimBLE log level: {err:?}");
    }

    if let Err(err) = logger.set_target_level(
        "corelib::device::xiaomi::components::network::native",
        LevelFilter::Warn,
    ) {
        log::warn!("failed to set network native log level: {err:?}");
    }
}

async fn log_network_meter() {
    let speeds = corelib::ecs::with_rt_mut(|rt| {
        let ids = rt.device_ids().cloned().collect::<Vec<_>>();
        let world = rt.world();

        ids.into_iter()
            .filter_map(|device_id| {
                let entity = rt.device_entity(&device_id)?;
                let dev = world.get::<XiaomiDevice>(entity)?;
                let name = dev.name().to_string();
                let addr = dev.addr().to_string();
                let speed = world.get::<NetworkComponent>(entity)?.last_speed;
                Some((name, addr, speed))
            })
            .collect::<Vec<_>>()
    })
    .await;

    if speeds.is_empty() {
        log::info!("NET meter: no connected devices");
        return;
    }

    for (name, addr, speed) in speeds {
        log::info!(
            "NET meter {name}({addr}) ↑{:.1} KB/s ↓{:.1} KB/s",
            speed.write / 1024.0,
            speed.read / 1024.0
        );
    }
}

#[derive(Clone)]
struct DeviceSnapshot {
    device_id: String,
    device_name: String,
    write_bps: f64,
    read_bps: f64,
}

async fn read_first_device_snapshot() -> Option<DeviceSnapshot> {
    corelib::ecs::with_rt_mut(|rt| {
        let device_id = rt.device_ids().next()?.to_string();
        let entity = rt.device_entity(&device_id)?;
        let world = rt.world();
        let dev = world.get::<XiaomiDevice>(entity)?;
        let speed = world
            .get::<NetworkComponent>(entity)
            .map(|comp| comp.last_speed)
            .unwrap_or_default();
        Some(DeviceSnapshot {
            device_id,
            device_name: dev.name().to_string(),
            write_bps: speed.write,
            read_bps: speed.read,
        })
    })
    .await
}

async fn read_device_battery_status(device_id: &str) -> Option<(i32, String)> {
    let owner_id = device_id.to_string();
    let status_rx = corelib::ecs::with_rt_mut(move |rt| {
        rt.with_device_mut(&owner_id, |world, entity| {
            let mut info = world.get_mut::<InfoSystem>(entity)?;
            Some(info.request_device_status())
        })
        .flatten()
    })
    .await?;

    let status = tokio::time::timeout(Duration::from_secs(2), status_rx)
        .await
        .ok()?
        .ok()?
        .ok()?;

    let battery = status.battery;
    let percent = battery.capacity.clamp(0, 100) as i32;
    let charge_text = format_charge_text(battery.charge_info.and_then(|info| info.timestamp));
    Some((percent, charge_text))
}

fn format_charge_text(charge_timestamp: Option<u32>) -> String {
    let Some(timestamp) = charge_timestamp else {
        return "充电信息未知".to_string();
    };
    let now_secs = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => return "充电信息未知".to_string(),
    };
    let charge_secs = timestamp as u64;
    if now_secs <= charge_secs {
        return "刚刚充电".to_string();
    }
    let days = (now_secs - charge_secs) / 86_400;
    if days == 0 {
        "今天充电".to_string()
    } else {
        format!("{days} 天前充电")
    }
}

fn format_speed_text(speed_bps: f64, arrow: &str) -> String {
    let speed = speed_bps.max(0.0);
    if speed < 1024.0 {
        format!("{speed:.0} byte/s {arrow}")
    } else if speed < 1024.0 * 1024.0 {
        format!("{:.1} KB/s {arrow}", speed / 1024.0)
    } else {
        format!("{:.2} MB/s {arrow}", speed / (1024.0 * 1024.0))
    }
}

#[allow(dead_code)]
fn init_wifi(modem: Modem, ssid: &str, password: &str) -> anyhow::Result<BlockingWifi<EspWifi<'static>>> {
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;

    let wifi_configuration = Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi SSID is too long"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi password is too long"))?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;
    wifi.start()?;
    log::info!("Wi-Fi started");

    wifi.connect()?;
    log::info!("Wi-Fi connected to {}", ssid);

    wifi.wait_netif_up()?;
    log::info!("Wi-Fi network interface is up");

    Ok(wifi)
}

async fn wifi_reconnect_watchdog(
    wifi: BlockingWifi<EspWifi<'static>>,
    ssid: String,
    password: String,
) {
    let mut ticker = tokio::time::interval(WIFI_RECONNECT_CHECK_INTERVAL);
    loop {
        ticker.tick().await;

        if wifi.is_connected() {
            continue;
        }

        log::warn!("Wi-Fi disconnected, attempting reconnect...");

        let _ = wifi.disconnect();

        match wifi.connect() {
            Ok(()) => {
                match wifi.wait_netif_up() {
                    Ok(()) => {
                        log::info!("Wi-Fi reconnected to {}", ssid);
                        if let Ok(()) = nvs_config::save_wifi_credentials(&ssid, &password) {
                            log::debug!("Wi-Fi credentials saved to NVS");
                        }
                    }
                    Err(err) => {
                        log::warn!("Wi-Fi netif up failed: {err:?}");
                    }
                }
            }
            Err(err) => {
                log::warn!("Wi-Fi reconnect failed: {err:?}");
            }
        }
    }
}

async fn sync_installed_items() {
    let device_ids = corelib::ecs::with_rt_mut(|rt| {
        rt.device_ids().cloned().collect::<Vec<_>>()
    })
    .await;

    if device_ids.is_empty() {
        return;
    }

    for addr in &device_ids {
        match install::list_installed_watchfaces(addr).await {
            Ok(faces) => {
                log::info!(
                    "[Install] Device {} has {} watchface(s): {:?}",
                    addr,
                    faces.len(),
                    faces
                );
            }
            Err(err) => {
                log::debug!(
                    "[Install] Failed to list watchfaces on {}: {err:?}",
                    addr
                );
            }
        }

        match install::list_installed_quick_apps(addr).await {
            Ok(apps) => {
                log::info!(
                    "[Install] Device {} has {} quick app(s): {:?}",
                    addr,
                    apps.len(),
                    apps
                );
            }
            Err(err) => {
                log::debug!(
                    "[Install] Failed to list quick apps on {}: {err:?}",
                    addr
                );
            }
        }
    }
}

#[allow(dead_code)]
pub async fn install_quick_app_on_device(
    addr: &str,
    package_name: &str,
    data: Vec<u8>,
) -> anyhow::Result<()> {
    install::install_quick_app(addr, package_name, data).await
}

#[allow(dead_code)]
pub async fn install_quick_app_file_on_device(
    addr: &str,
    package_name: &str,
    file_path: &str,
) -> anyhow::Result<()> {
    install::install_quick_app_from_file(addr, package_name, file_path).await
}

#[allow(dead_code)]
pub async fn install_watchface_on_device(
    addr: &str,
    data: Vec<u8>,
) -> anyhow::Result<()> {
    install::install_watchface(addr, data).await
}

#[allow(dead_code)]
pub async fn install_watchface_file_on_device(
    addr: &str,
    file_path: &str,
) -> anyhow::Result<()> {
    install::install_watchface_from_file(addr, file_path).await
}

#[allow(dead_code)]
pub async fn uninstall_quick_app_on_device(
    addr: &str,
    package_name: &str,
) -> anyhow::Result<()> {
    install::uninstall_quick_app(addr, package_name).await
}

#[allow(dead_code)]
pub async fn uninstall_watchface_on_device(
    addr: &str,
    watchface_id: &str,
) -> anyhow::Result<()> {
    install::uninstall_watchface(addr, watchface_id).await
}

#[allow(dead_code)]
pub async fn set_watchface_on_device(
    addr: &str,
    watchface_id: &str,
) -> anyhow::Result<()> {
    install::set_watchface(addr, watchface_id).await
}

#[allow(dead_code)]
pub async fn launch_quick_app_on_device(
    addr: &str,
    package_name: &str,
) -> anyhow::Result<()> {
    install::launch_quick_app(addr, package_name).await
}