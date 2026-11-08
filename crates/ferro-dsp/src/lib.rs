//! FerroOS's Sound Enhancer, the part shared by its window (ferro-enhancer)
//! and the plugin ALSA runs in every program that plays sound (ferro-ladspa):
//! the settings, and the processing (volume boost, bass boost, a ten-band
//! equalizer, and a limiter so it never clips).

pub mod enhancer;
pub mod settings;

pub use enhancer::{Enhancer, CEILING};
pub use settings::Settings;

#[cfg(test)]
mod tests;
