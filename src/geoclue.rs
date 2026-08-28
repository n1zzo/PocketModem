//! GeoClue2 integration for basic location data
//!
//! GeoClue2 is the standard Linux location service that provides location
//! from various sources (GPS, WiFi, Cell towers). Used as fallback when
//! mmcli/ModemManager is not available.

use zbus::{Connection, proxy};
use zbus::zvariant::OwnedObjectPath;

/// Location data from GeoClue2
#[derive(Debug, Clone)]
pub struct GeoClueLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
    pub accuracy: f64,
}

#[proxy(
    interface = "org.freedesktop.GeoClue2.Manager",
    default_service = "org.freedesktop.GeoClue2",
    default_path = "/org/freedesktop/GeoClue2/Manager"
)]
trait Manager {
    async fn get_client(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.GeoClue2.Client",
    default_service = "org.freedesktop.GeoClue2"
)]
trait Client {
    async fn start(&self) -> zbus::Result<()>;
    async fn location(&self) -> zbus::Result<OwnedObjectPath>;
    async fn desktop_id(&self) -> zbus::Result<String>;
    async fn set_desktop_id(&self, id: &str) -> zbus::Result<()>;
    async fn requested_accuracy_level(&self) -> zbus::Result<u32>;
    async fn set_requested_accuracy_level(&self, level: u32) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.GeoClue2.Location",
    default_service = "org.freedesktop.GeoClue2"
)]
trait Location {
    async fn latitude(&self) -> zbus::Result<f64>;
    async fn longitude(&self) -> zbus::Result<f64>;
    async fn altitude(&self) -> zbus::Result<f64>;
    async fn accuracy(&self) -> zbus::Result<f64>;
}

/// Initialize GeoClue2 - returns true if connection works
pub fn init_geoclue() -> bool {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[geoclue] Failed to create runtime: {}", e);
            return false;
        }
    };
    
    let result = rt.block_on(async_init_geoclue());
    if result {
        eprintln!("[geoclue] GeoClue2 initialized successfully");
    }
    result
}

async fn async_init_geoclue() -> bool {
    let connection = match Connection::system().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("[geoclue] Failed to connect to system D-Bus: {}", e);
            return false;
        }
    };

    match ManagerProxy::new(&connection).await {
        Ok(_manager) => {
            eprintln!("[geoclue] Manager proxy created");
            true
        }
        Err(e) => {
            eprintln!("[geoclue] Failed to create Manager proxy: {}", e);
            false
        }
    }
}

/// Get current location from GeoClue2
/// Returns None if location is not available
pub fn get_location() -> Option<GeoClueLocation> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async_get_location())
}

async fn async_get_location() -> Option<GeoClueLocation> {
    let connection = Connection::system().await.ok()?;
    let manager = ManagerProxy::new(&connection).await.ok()?;
    
    // Get or create client
    let client_path = manager.get_client().await.ok()?;
    
    // Create client proxy - use .path() then .build()
    let client = ClientProxy::builder(&connection)
        .path(client_path.as_str())
        .ok()?
        .build()
        .await
        .ok()?;
    
    // Set desktop ID (required for authorization)
    let _ = client.set_desktop_id("pocket-modem").await;
    
    // Set accuracy level (6 = Exact for GPS)
    let _ = client.set_requested_accuracy_level(6).await;
    
    // Start location tracking
    if let Err(e) = client.start().await {
        eprintln!("[geoclue] start failed: {}", e);
        return None;
    }
    
    // Wait briefly for location to be available
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Get location path
    let location_path = client.location().await.ok()?;
    
    // Create location proxy
    let location = LocationProxy::builder(&connection)
        .path(location_path.as_str())
        .ok()?
        .build()
        .await
        .ok()?;
    
    // Read location data
    let latitude = location.latitude().await.unwrap_or(0.0);
    let longitude = location.longitude().await.unwrap_or(0.0);
    let altitude = location.altitude().await.ok();
    let accuracy = location.accuracy().await.unwrap_or(999.0);
    
    if latitude != 0.0 || longitude != 0.0 {
        Some(GeoClueLocation {
            latitude,
            longitude,
            altitude,
            accuracy,
        })
    } else {
        None
    }
}