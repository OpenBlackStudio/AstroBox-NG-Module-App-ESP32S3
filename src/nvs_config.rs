use esp_idf_svc::sys::{
    nvs_commit, nvs_flash_init, nvs_get_str, nvs_open, nvs_set_str, nvs_close,
    ESP_ERR_NVS_NOT_FOUND, ESP_OK,
};
use std::ffi::CString;
use std::sync::Once;

const NVS_NAMESPACE: &str = "wifi_cfg";
const SSID_KEY: &str = "ssid";
const PASSWORD_KEY: &str = "password";
const MAX_STR_SIZE: usize = 65;
const NVS_READWRITE: u32 = 0x00000002;
const NVS_READONLY: u32 = 0x00000001;

static NVS_INIT: Once = Once::new();
static mut NVS_INIT_OK: bool = false;

pub fn ensure_nvs_initialized() -> bool {
    NVS_INIT.call_once(|| {
        let ret = unsafe { nvs_flash_init() };
        if ret == ESP_OK || ret == ESP_ERR_NVS_NOT_FOUND {
            unsafe {
                NVS_INIT_OK = true;
            }
        } else {
            log::error!("nvs_flash_init failed: {ret}");
        }
    });
    unsafe { NVS_INIT_OK }
}

pub fn load_wifi_credentials() -> (String, String) {
    let default_ssid = env!("DEFAULT_WIFI_SSID", "ASUS_AX86U_2.4G").to_string();
    let default_password = env!("DEFAULT_WIFI_PASSWORD", "reveries2005").to_string();

    if let Ok(ssid) = nvs_get_string(SSID_KEY) {
        if !ssid.is_empty() {
            let password = nvs_get_string(PASSWORD_KEY).unwrap_or(default_password.clone());
            return (ssid, password);
        }
    }

    (default_ssid, default_password)
}

pub fn save_wifi_credentials(ssid: &str, password: &str) -> Result<(), String> {
    nvs_set_string(SSID_KEY, ssid)?;
    nvs_set_string(PASSWORD_KEY, password)?;
    Ok(())
}

fn nvs_get_string(key: &str) -> Result<String, String> {
    if !ensure_nvs_initialized() {
        return Err("NVS not initialized".to_string());
    }

    let c_key = CString::new(key).map_err(|e| e.to_string())?;
    let c_ns = CString::new(NVS_NAMESPACE).map_err(|e| e.to_string())?;

    let mut handle: esp_idf_svc::sys::nvs_handle_t = 0;
    let ret = unsafe { nvs_open(c_ns.as_ptr(), NVS_READONLY, &mut handle) };
    if ret != ESP_OK {
        return Err(format!("nvs_open failed: {ret}"));
    }

    let mut buf = vec![0u8; MAX_STR_SIZE];
    let mut len = buf.len() as u32;
    let ret = unsafe { nvs_get_str(handle, c_key.as_ptr(), buf.as_mut_ptr(), &mut len) };

    unsafe { nvs_close(handle) };

    if ret != ESP_OK {
        return Err(format!("nvs_get_str failed: {ret}"));
    }

    let slice = &buf[..len as usize];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8(slice[..end].to_vec())
        .map_err(|e| format!("invalid UTF-8: {e}"))
}

fn nvs_set_string(key: &str, value: &str) -> Result<(), String> {
    if !ensure_nvs_initialized() {
        return Err("NVS not initialized".to_string());
    }

    let c_key = CString::new(key).map_err(|e| e.to_string())?;
    let c_value = CString::new(value).map_err(|e| e.to_string())?;
    let c_ns = CString::new(NVS_NAMESPACE).map_err(|e| e.to_string())?;

    let mut handle: esp_idf_svc::sys::nvs_handle_t = 0;
    let ret = unsafe { nvs_open(c_ns.as_ptr(), NVS_READWRITE, &mut handle) };
    if ret != ESP_OK {
        return Err(format!("nvs_open failed: {ret}"));
    }

    let ret = unsafe { nvs_set_str(handle, c_key.as_ptr(), c_value.as_ptr()) };
    if ret != ESP_OK {
        unsafe { nvs_close(handle) };
        return Err(format!("nvs_set_str failed: {ret}"));
    }

    let ret = unsafe { nvs_commit(handle) };
    if ret != ESP_OK {
        unsafe { nvs_close(handle) };
        return Err(format!("nvs_commit failed: {ret}"));
    }

    unsafe { nvs_close(handle) };
    Ok(())
}