//! Prints what the analysis makes of each factory sample: the numbers the
//! tagging and key thresholds in this crate were set from.
//!
//! `cargo run --release -p fourfloor-library --example calib`

use fourfloor_library::analyse_buffer;
use fourfloor_library::blob::decode_features;
use fourfloor_library::factory::factory_samples;

fn main() -> Result<(), fourfloor_library::LibraryError> {
    for sample in factory_samples() {
        let file = analyse_buffer(sample.filename(), &sample.render())?;
        let features = decode_features(&file.features)?;
        println!(
            "{:40} {:9} bpm {:>7} key {:>4} centroid {:7.0} Hz flatness {:.3} tags {}",
            sample.filename(),
            file.kind.as_str(),
            file.bpm.map_or("-".to_owned(), |bpm| format!("{bpm:.2}")),
            file.key.map_or("-".to_owned(), |key| key.short_name()),
            features.centroid_hz,
            features.flatness,
            file.tags.join(",")
        );
    }
    Ok(())
}
