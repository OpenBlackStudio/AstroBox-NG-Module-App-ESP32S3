use log::debug;

pub struct OtaManager;

impl OtaManager {
    pub fn new() -> Self {
        // OTA is a stub in the current build. Keep the log at debug so it does
        // not spam production logs once the real implementation lands.
        debug!("OTA manager initialized (stub - not yet implemented)");
        Self
    }

    pub fn check_for_update(&self) -> Option<OtaInfo> {
        debug!("Checking for OTA updates (stub)...");
        None
    }

    pub fn start_ota_update(&self, _url: &str) -> Result<(), String> {
        debug!("OTA update requested (stub) - not yet implemented");
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
