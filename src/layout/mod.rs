pub mod tiling;

use smithay::desktop::Window;

pub type LayoutBox = Box<dyn Layout>;

pub struct GapConfig {
    pub inner_horizontal: u32,
    pub inner_vertical: u32,
    pub outer_horizontal: u32,
    pub outer_vertical: u32,
}

pub enum LayoutType {
    Tiling,
}

impl LayoutType {
    pub fn new(&self) -> LayoutBox {
        match self {
            LayoutType::Tiling => Box::new(tiling::Tiling),
        }
    }
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "tiling" => Ok(Self::Tiling),
            _ => Err(format!("Invalid Layout Type: {}", s)),
        }
    }
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
