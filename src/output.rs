use crate::state::Oxwc;
use smithay::{
    output::{Mode, Output, PhysicalProperties, Subpixel},
    utils::Transform,
};

impl Oxwc {
    pub fn add_output(&mut self, name: String, width: i32, height: i32) -> Output {
        let physical_properties = PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "oxwc".to_string(),
            model: name.clone(),
        };

        let output = Output::new(name, physical_properties);
        let mode = Mode {
            size: (width, height).into(),
            refresh: 60_000,
        };

        output.change_current_state(Some(mode), Some(Transform::Normal), None, None);
        output.set_preferred(mode);
        output.create_global::<Oxwc>(&self.display_handle);
        self.space.map_output(&output, (0, 0));

        output
    }
}
