//! Location via XDG Desktop Portal (ashpd)
//!
//! Uses ashpd (flatpak-xdg-utils) to access location through the XDG Location
//! portal. This is the preferred method in Flatpak/sandboxed environments.
//!
//! Falls back gracefully if location access is denied or unavailable.

use ashpd::desktop::location::{Accuracy, Location, LocationProxy};
use futures::StreamExt;
use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::time::sleep;

/// Location data from the XDG Location portal
#[derive(Debug, Clone)]
pub struct GeoClueLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
    pub accuracy: f64,
}

/// Shared runtime for geoclue operations
static GEOFENCE_RUNTIME: OnceCell<Arc<Runtime>> = OnceCell::new();

/// Initialize the location service (creates a portal session)
/// Returns true if the session was created successfully.
/// Note: This doesn't prompt the user yet - that happens on first location request.
pub fn init_geoclue() -> bool {
    // Create or get the tokio runtime
    let rt = GEOFENCE_RUNTIME.get_or_init(|| {
        Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for geoclue"),
        )
    });

    rt.block_on(async_init_geoclue())
}

async fn async_init_geoclue() -> bool {
    LocationProxy::new().await.is_ok()
}

/// Get current location from the XDG Location portal
/// Returns None if location is not available or denied.
pub fn get_location() -> Option<GeoClueLocation> {
    let rt = match GEOFENCE_RUNTIME.get() {
        Some(rt) => rt.clone(),
        None => {
            if !init_geoclue() {
                return None;
            }
            GEOFENCE_RUNTIME.get().unwrap().clone()
        }
    };

    rt.block_on(async_get_location())
}

async fn async_get_location() -> Option<GeoClueLocation> {
    let proxy = match LocationProxy::new().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[geoclue] Failed to create LocationProxy: {}", e);
            return None;
        }
    };

    // Create a session with exact accuracy (GPS)
    let session = match proxy
        .create_session(
            None, // No distance threshold
            None, // No time threshold
            Some(Accuracy::Exact),
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[geoclue] Failed to create location session: {}", e);
            return None;
        }
    };

    // Start the session (this triggers the permission prompt)
    if let Err(e) = proxy.start(&session, &ashpd::WindowIdentifier::default()).await {
        eprintln!("[geoclue] Failed to start location session: {}", e);
        let _ = session.close().await;
        return None;
    }

    // Listen for location updates
    let mut stream = match proxy.receive_location_updated().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[geoclue] Failed to subscribe to location updates: {}", e);
            let _ = session.close().await;
            return None;
        }
    };

    // Wait for first location (with timeout)
    let location = tokio::select! {
        signal = stream.next() => match signal {
            Some(loc) => Some(loc),
            None => {
                eprintln!("[geoclue] Location stream closed unexpectedly");
                None
            }
        },
        _ = sleep(Duration::from_secs(10)) => {
            eprintln!("[geoclue] Location timeout (10s)");
            None
        }
    };

    // Clean up session
    let _ = session.close().await;

    // Convert to our internal type
    location.map(|loc| {
        let altitude = loc.altitude().filter(|&a| a > -1000.0 && a < 50000.0);
        GeoClueLocation {
            latitude: loc.latitude(),
            longitude: loc.longitude(),
            altitude,
            accuracy: loc.accuracy(),
        }
    })
}