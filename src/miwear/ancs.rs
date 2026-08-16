#![allow(unexpected_cfgs)]
use std::time::Duration;

use anyhow::{Context, Result};
#[cfg(not(esp_idf_bt_nimble_ext_adv))]
use esp32_nimble::BLEAdvertisementData;
use esp32_nimble::{
    enums::{AuthReq, PairKeyDist, SecurityIOCap},
    uuid128, BLEDevice, NimbleProperties, NimbleSub, OnWriteArgs,
};
#[cfg(esp_idf_bt_nimble_ext_adv)]
use esp32_nimble::{
    enums::{PrimPhy, SecPhy},
    BLEExtAdvertisement, BLEExtAdvertising,
};
use log::{debug, info, warn};
use tokio::{
    task,
    time::{self, MissedTickBehavior},
};

const DUMMY_APP_IDENTIFIER: &str = "com.astrobox.ghost";
const DUMMY_APP_DISPLAY_NAME: &str = "AstroBox Phantom";
const DUMMY_MESSAGE_TITLE: &str = "Phantom Alert";
const DUMMY_MESSAGE_SUBTITLE: &str = "Faint Signal";
const DUMMY_MESSAGE_BODY: &str = "Spectral notification with no real content.";
const DUMMY_DATE: &str = "19700101T000000";
const ADVERTISED_NAME: &str = "iP";
const APPLE_MANUFACTURER_DATA: [u8; 4] = [0x4C, 0x00, 0x02, 0x15];

pub fn init_fake_ancs_service(ble: &mut BLEDevice) -> Result<()> {
    {
        let security = ble.security();
        // Using KeyboardDisplay I/O capability instead of NoInputNoOutput.
        // NoInputNoOutput allows passive attackers to brute-force the encryption
        // key (up to 1.6M attempts). KeyboardDisplay provides a stronger security
        // model with explicit user confirmation for pairing, which is the
        // recommended approach for ANCS services even though this is a fake
        // (ghost) implementation that only echoes dummy notification data.
        security
            .set_auth(AuthReq::Bond | AuthReq::Sc)
            .set_io_cap(SecurityIOCap::KeyboardDisplay)
            .set_security_init_key(PairKeyDist::ENC | PairKeyDist::ID)
            .set_security_resp_key(PairKeyDist::ENC | PairKeyDist::ID);
    }

    let advertising = ble.get_advertising();
    let server = ble.get_server();

    let service = server.create_service(uuid128!("7905f431-b5ce-4e99-a40f-4b1e122d00d0"));

    let notification_source = service.lock().create_characteristic(
        uuid128!("9fbf120d-6301-42d9-8c58-25e699a21dbd"),
        NimbleProperties::READ | NimbleProperties::READ_ENC | NimbleProperties::NOTIFY,
    );
    {
        let mut chr = notification_source.lock();
        chr.set_value(&build_notification_source_payload(0));
        chr.on_subscribe(|characteristic, desc, sub| {
            if sub.contains(NimbleSub::NOTIFY) {
                info!(
                    "ANCS notification source subscribed: conn={} mtu={} encrypted={}",
                    desc.conn_handle(),
                    desc.mtu(),
                    desc.encrypted()
                );
                if !desc.encrypted() {
                    info!(
                        "ANCS subscription pending encryption; waiting for security upgrade (conn={})",
                        desc.conn_handle()
                    );
                    return;
                }
                let payload = build_notification_source_payload(0);
                if let Err(err) = characteristic.notify_with(&payload, desc.conn_handle()) {
                    warn!(
                        "Failed to send initial ANCS notification to conn {}: {:?}",
                        desc.conn_handle(),
                        err
                    );
                }
            } else {
                info!(
                    "ANCS notification source unsubscribed: conn={}",
                    desc.conn_handle()
                );
            }
        });
    }

    let data_source = service.lock().create_characteristic(
        uuid128!("22eac6e9-24d6-4bb5-be44-b36ace7c7bfb"),
        NimbleProperties::READ | NimbleProperties::READ_ENC | NimbleProperties::NOTIFY,
    );
    {
        let mut chr = data_source.lock();
        chr.set_value(&[]);
        chr.on_subscribe(|_, desc, sub| {
            if sub.contains(NimbleSub::NOTIFY) {
                info!("ANCS data source subscribed: conn={}", desc.conn_handle());
            } else {
                info!("ANCS data source unsubscribed: conn={}", desc.conn_handle());
            }
        });
    }

    let control_point = service.lock().create_characteristic(
        uuid128!("69d1d8f3-45e1-49a8-9821-9bbdfdaad9d9"),
        NimbleProperties::WRITE | NimbleProperties::WRITE_NO_RSP | NimbleProperties::WRITE_ENC,
    );
    {
        let data_source_for_cp = data_source.clone();
        control_point
            .lock()
            .on_write(move |args: &mut OnWriteArgs| {
                let request = args.recv_data();
                debug!("ANCS control point got {:02X?}", request);
                if !args.desc().encrypted() {
                    warn!(
                        "Reject ANCS control write without encryption (conn={})",
                        args.desc().conn_handle()
                    );
                    args.reject();
                    return;
                }
                if let Some(response) = build_control_point_response(request) {
                    let mut target = data_source_for_cp.lock();
                    target.set_value(&response);
                    if target.subscribed_count() > 0 {
                        target.notify();
                    }
                }
            });
    }

    let notification_for_auth = notification_source.clone();
    let advertising_on_connect = advertising;
    let advertising_on_disconnect = advertising;
    let advertising_on_auth = advertising;

    server
        .on_connect(move |server, desc| {
            info!(
                "ANCS client connected: addr={:?} conn={}",
                desc.address(),
                desc.conn_handle()
            );
            let max = esp_idf_svc::sys::CONFIG_BT_NIMBLE_MAX_CONNECTIONS as usize;
            if server.connected_count() < max {
                if let Err(err) = restart_advertising(advertising_on_connect) {
                    warn!(
                        "Failed to keep ANCS advertising after connect (conn={}): {:?}",
                        desc.conn_handle(),
                        err
                    );
                }
            }
        })
        .on_disconnect(move |desc, reason| {
            info!(
                "ANCS client disconnected: conn={} reason={:?}",
                desc.conn_handle(),
                reason
            );
            if let Err(err) = restart_advertising(advertising_on_disconnect) {
                warn!(
                    "Failed to restart ANCS advertising after disconnect (conn={}): {:?}",
                    desc.conn_handle(),
                    err
                );
            }
        })
        .on_authentication_complete(move |_server, desc, status| match status {
            Ok(()) => {
                info!(
                    "ANCS link encrypted: conn={} bonded={} mtu={}",
                    desc.conn_handle(),
                    desc.bonded(),
                    desc.mtu()
                );
                if let Err(err) = restart_advertising(advertising_on_auth) {
                    warn!(
                        "Failed to keep ANCS advertising after encryption (conn={}): {:?}",
                        desc.conn_handle(),
                        err
                    );
                }
                let mut chr = notification_for_auth.lock();
                if chr.subscribed_count() > 0 {
                    let payload = build_notification_source_payload(0);
                    chr.set_value(&payload);
                    if let Err(err) = chr.notify_with(&payload, desc.conn_handle()) {
                        warn!(
                            "Failed to deliver encrypted ANCS notification to conn {}: {:?}",
                            desc.conn_handle(),
                            err
                        );
                    }
                }
            }
            Err(err) => {
                warn!(
                    "ANCS link security failed: conn={} err={:?}",
                    desc.conn_handle(),
                    err
                );
            }
        })
        .advertise_on_disconnect(true);

    server.start().context("start fake ANCS service")?;

    configure_advertising(advertising).context("configure fake ANCS advertising")?;

    let notification_handle = notification_source.clone();
    task::spawn_local(async move {
        let mut counter: u32 = 1;
        let mut ticker = time::interval(Duration::from_secs(120));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            let payload = build_notification_source_payload(counter);
            counter = counter.wrapping_add(1);

            let mut chr = notification_handle.lock();
            if chr.subscribed_count() > 0 {
                chr.set_value(&payload);
                chr.notify();
            }
        }
    });

    Ok(())
}

fn build_notification_source_payload(seq: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0] = 0x00; // EventID: Notification Added
    payload[1] = 0x01; // EventFlags: Silent
    payload[2] = 0x00; // Category: Other
    payload[3] = 1; // Category Count
    payload[4..8].copy_from_slice(&seq.to_le_bytes());
    payload
}

fn build_control_point_response(request: &[u8]) -> Option<Vec<u8>> {
    let command = *request.get(0)?;
    match command {
        0x00 => Some(build_notification_attributes_response(request)),
        0x01 => Some(build_app_attributes_response(request)),
        0x02 => Some(build_action_ack_response(request)),
        other => Some(vec![other, 0x00]),
    }
}

fn build_notification_attributes_response(request: &[u8]) -> Vec<u8> {
    let mut response = Vec::with_capacity(48);
    if request.len() >= 5 {
        response.extend_from_slice(&request[0..5]);
    } else {
        response.push(0x00);
        response.extend_from_slice(&[0, 0, 0, 0]);
    }

    let mut offset = 5;
    let mut appended = false;

    while offset < request.len() {
        let attr_id = request[offset];
        offset += 1;

        let requested_len = if attribute_requires_len(attr_id) {
            if offset + 1 >= request.len() {
                break;
            }
            let len = u16::from_le_bytes([request[offset], request[offset + 1]]);
            offset += 2;
            len as usize
        } else {
            0
        };

        let value = dummy_notification_attribute(attr_id, requested_len);
        response.push(attr_id);
        response.extend_from_slice(&(value.len() as u16).to_le_bytes());
        response.extend_from_slice(&value);
        appended = true;
    }

    if !appended {
        let value = dummy_notification_attribute(0, 0);
        response.push(0);
        response.extend_from_slice(&(value.len() as u16).to_le_bytes());
        response.extend_from_slice(&value);
    }

    response
}

fn build_app_attributes_response(request: &[u8]) -> Vec<u8> {
    let mut response = Vec::with_capacity(48);
    response.push(0x01);

    let (app_id, mut cursor) = extract_app_identifier(request);
    response.extend_from_slice(app_id);
    response.push(0);

    if cursor >= request.len() {
        append_app_attribute(&mut response, 0, dummy_app_attribute(0, 0));
        return response;
    }

    while cursor < request.len() {
        let attr_id = request[cursor];
        cursor += 1;
        if cursor + 1 >= request.len() {
            break;
        }
        let requested_len = u16::from_le_bytes([request[cursor], request[cursor + 1]]) as usize;
        cursor += 2;

        append_app_attribute(
            &mut response,
            attr_id,
            dummy_app_attribute(attr_id, requested_len),
        );
    }

    if response.len() == 1 + app_id.len() + 1 {
        append_app_attribute(&mut response, 0, dummy_app_attribute(0, 0));
    }

    response
}

fn build_action_ack_response(request: &[u8]) -> Vec<u8> {
    let mut response = Vec::with_capacity(6);
    response.push(0x02);
    if request.len() >= 5 {
        response.extend_from_slice(&request[1..5]);
    } else {
        response.extend_from_slice(&[0, 0, 0, 0]);
    }
    response.push(request.get(5).copied().unwrap_or(0));
    response
}

fn extract_app_identifier(request: &[u8]) -> (&[u8], usize) {
    if request.len() <= 1 {
        return (&[], request.len());
    }
    let mut idx = 1;
    while idx < request.len() && request[idx] != 0 {
        idx += 1;
    }
    let app_id = &request[1..idx];
    let cursor = if idx < request.len() {
        idx + 1
    } else {
        request.len()
    };
    (app_id, cursor)
}

fn append_app_attribute(buffer: &mut Vec<u8>, attr_id: u8, value: Vec<u8>) {
    buffer.push(attr_id);
    buffer.extend_from_slice(&(value.len() as u16).to_le_bytes());
    buffer.extend_from_slice(&value);
}

fn attribute_requires_len(attr_id: u8) -> bool {
    matches!(attr_id, 1 | 2 | 3)
}

fn dummy_notification_attribute(attr_id: u8, requested_len: usize) -> Vec<u8> {
    match attr_id {
        0 => truncate_bytes(DUMMY_APP_IDENTIFIER.as_bytes(), requested_len),
        1 => truncate_bytes(DUMMY_MESSAGE_TITLE.as_bytes(), requested_len),
        2 => truncate_bytes(DUMMY_MESSAGE_SUBTITLE.as_bytes(), requested_len),
        3 => truncate_bytes(DUMMY_MESSAGE_BODY.as_bytes(), requested_len),
        4 => truncate_bytes(b"0", requested_len),
        5 => truncate_bytes(DUMMY_DATE.as_bytes(), requested_len),
        6 => truncate_bytes(b"Open", requested_len),
        7 => truncate_bytes(b"Ignore", requested_len),
        _ => truncate_bytes(b"", requested_len),
    }
}

fn dummy_app_attribute(attr_id: u8, requested_len: usize) -> Vec<u8> {
    match attr_id {
        0 => truncate_bytes(DUMMY_APP_DISPLAY_NAME.as_bytes(), requested_len),
        _ => truncate_bytes(b"", requested_len),
    }
}

fn truncate_bytes(data: &[u8], max_len: usize) -> Vec<u8> {
    if max_len == 0 || data.len() <= max_len {
        data.to_vec()
    } else {
        data[..max_len].to_vec()
    }
}

#[cfg(not(esp_idf_bt_nimble_ext_adv))]
fn configure_advertising(
    advertising: &'static esp32_nimble::utilities::mutex::Mutex<esp32_nimble::BLEAdvertising>,
) -> Result<()> {
    let mut adv = advertising.lock();
    adv.reset()
        .context("reset advertising state for fake ANCS")?;
    let mut adv_data = BLEAdvertisementData::new();
    adv_data
        .name(ADVERTISED_NAME)
        .add_service_uuid(uuid128!("7905f431-b5ce-4e99-a40f-4b1e122d00d0"));
    adv_data.manufacturer_data(&APPLE_MANUFACTURER_DATA);
    adv.set_data(&mut adv_data)
        .context("set fake ANCS advertisement payload")?;
    adv.start().context("begin advertising fake ANCS service")?;
    Ok(())
}

#[cfg(esp_idf_bt_nimble_ext_adv)]
fn configure_advertising(
    advertising: &'static esp32_nimble::utilities::mutex::Mutex<BLEExtAdvertising>,
) -> Result<()> {
    let mut adv = advertising.lock();
    let mut payload = BLEExtAdvertisement::new(PrimPhy::Phy1M, SecPhy::Phy1M);
    payload.legacy_advertising(true);
    payload.connectable(true);
    payload.scannable(true);
    payload.name(ADVERTISED_NAME);
    payload.complete_service(&uuid128!("7905f431-b5ce-4e99-a40f-4b1e122d00d0"));
    payload.manufacturer_data(&APPLE_MANUFACTURER_DATA);
    adv.set_instance_data(0, &mut payload)
        .context("set fake ANCS extended advertisement payload")?;
    adv.start(0)
        .context("begin advertising fake ANCS service")?;
    Ok(())
}

#[cfg(not(esp_idf_bt_nimble_ext_adv))]
fn restart_advertising(
    advertising: &'static esp32_nimble::utilities::mutex::Mutex<esp32_nimble::BLEAdvertising>,
) -> Result<(), esp32_nimble::BLEError> {
    advertising.lock().start()
}

#[cfg(esp_idf_bt_nimble_ext_adv)]
fn restart_advertising(
    advertising: &'static esp32_nimble::utilities::mutex::Mutex<BLEExtAdvertising>,
) -> Result<(), esp32_nimble::BLEError> {
    advertising.lock().start(0)
}
