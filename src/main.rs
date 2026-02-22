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
pub mod miwear;
pub mod statlogger;
pub mod touch;

const WIFI_SSID: &str = "ASUS_AX86U_2.4G";
const WIFI_PASSWORD: &str = "reveries2005";
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
    let Peripherals {
        pins,
        ledc,
        spi2,
        i2c0,
        modem,
        ..
    } = Peripherals::take()?;

    let _wifi = init_wifi(modem)?;

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
                    //cached_charge_text = charge_text;
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
        if let Err(err) = miwear::connect().await {
            log::error!("miwear connect failed: {err:?}");
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
                // Keep yielding real CPU time so IDLE0 can run and feed task WDT.
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    })
    .await?;

    Ok(())
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

fn init_wifi(modem: Modem) -> anyhow::Result<BlockingWifi<EspWifi<'static>>> {
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;

    let wifi_configuration = Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi SSID is too long"))?,
        password: WIFI_PASSWORD
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi password is too long"))?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;
    wifi.start()?;
    log::info!("Wi-Fi started");

    wifi.connect()?;
    log::info!("Wi-Fi connected to {}", WIFI_SSID);

    wifi.wait_netif_up()?;
    log::info!("Wi-Fi network interface is up");

    Ok(wifi)
}
