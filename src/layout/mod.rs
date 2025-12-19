pub mod tiling;
use smithay::desktop::Window;

pub enum LayoutType {
    Tiling,
}

pub struct GapConfig {
    pub inner_horizontal: u32,
    pub inner_vertical: u32,
    pub outer_horizontal: u32,
    pub outer_vertical: u32,
}

pub trait Layout {
    fn arrange(
        &self,
        windows: &[Window],
        screen_width: u32,
        screen_height: u32,
        gaps: &GapConfig,
        master_factor: f32,
        num_master: i32,
        smartgaps_enabled: bool,
    ) -> Vec<WindowGeometry>;
}

#[derive(Clone)]
pub struct WindowGeometry {
    pub x_coordinate: i32,
    pub y_coordinate: i32,
    pub width: u32,
    pub height: u32,
}
