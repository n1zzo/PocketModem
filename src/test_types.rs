// Test that types exist
pub fn test_map() -> libshumate::Map {
    libshumate::Map::new()
}

pub fn test_viewport() -> libshumate::Viewport {
    libshumate::Viewport::new()
}

pub fn test_marker() -> libshumate::Marker {
    libshumate::Marker::new()
}

pub fn test_marker_layer(vp: &libshumate::Viewport) -> libshumate::MarkerLayer {
    libshumate::MarkerLayer::new(vp)
}
