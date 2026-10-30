//! Custom-painted controls shared by the views.

pub mod buttons;
pub mod clip;
pub mod knob;
pub mod meter;

pub use knob::{knob, truncate, KnobOutput};
pub use meter::{
    fader, paint_correlation, paint_horizontal_meter, paint_spectrum, paint_stereo_meter,
};
