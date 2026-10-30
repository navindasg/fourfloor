//! Stretch a synthesized sine and print how long each ratio takes.
//!
//! ```text
//! cargo run -p fourfloor-spikes --release --features stretch --example stretch_sine
//! ```

use fourfloor_spikes::core::SampleRate;
use fourfloor_spikes::stretch::{default_preset_latency, stretch_mono};

fn main() {
    let rate = SampleRate::HZ_48000;
    let seconds = 10.0;
    let input: Vec<f32> = (0..(seconds * rate.as_f64()) as usize)
        .map(|i| {
            (0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate.as_f64()).sin()) as f32
        })
        .collect();

    let (input_latency, output_latency) = default_preset_latency(rate);
    println!("default preset latency: {input_latency} in + {output_latency} out frames");

    for ratio in [0.9, 1.03, 1.1, 2.0] {
        let start = std::time::Instant::now();
        match stretch_mono(&input, rate, ratio) {
            Ok(out) => {
                let elapsed = start.elapsed().as_secs_f64();
                let out_seconds = out.len() as f64 / rate.as_f64();
                println!(
                    "ratio {ratio}: {} -> {} frames in {:.3}s ({:.0}x realtime)",
                    input.len(),
                    out.len(),
                    elapsed,
                    out_seconds / elapsed
                );
            }
            Err(error) => println!("ratio {ratio}: failed: {error}"),
        }
    }
}
