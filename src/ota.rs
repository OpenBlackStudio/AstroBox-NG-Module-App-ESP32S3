use log::info;

pub struct OtaManager;

impl OtaManager {
    pub fn new() -> Self {
        info!("OTA manager initialized (stub)");
        Self
    }

    pub fn check_for_update(&self) -> Option<OtaInfo> {
        info!("Checking for OTA updates...");
        None
    }

    pub fn start_ota_update(&self, _url: &str) -> Result<(), String> {
        info!("OTA update requested (stub)");
        Err("OTA not yet implemented".to_string())
    }
}

impl Default for OtaManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct OtaInfo {
    pub version: String,
    pub size: u64,
    pub url: String,
    pub release_notes: String,
}