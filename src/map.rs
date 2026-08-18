//! Map module placeholder

use crate::aprs::APRSMessage;

pub struct MapManager;

impl MapManager {
    pub fn new() -> Self {
        Self
    }

    pub fn initialize(&mut self) {}

    pub fn set_user_position(&mut self, _lat: f64, _lon: f64) {}
    pub fn center_on_user(&mut self) {}
    pub fn update_station(&mut self, _msg: &APRSMessage) {}
    pub fn needs_redraw(&self) -> bool { false }
    pub fn request_redraw(&self) {}
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}