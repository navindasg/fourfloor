//! Enumerate installed CLAP, VST3 and AU plugins through truce-rack and try to
//! push one silent block through one plugin of each format.
//!
//! ```text
//! cargo run -p fourfloor-spikes --features hosting-truce --example scan_plugins
//! cargo run -p fourfloor-spikes --features hosting-truce --example scan_plugins -- --all
//! ```
//!
//! `--all` instantiates every plugin found instead of one per format. That
//! loads third-party code in-process, so a misbehaving plugin can crash it.

use fourfloor_spikes::hosting_truce::{load_and_process_silence, scan_all};

const SAMPLE_RATE: f64 = 48_000.0;
const FRAMES: usize = 512;
const LISTED_PER_FORMAT: usize = 15;

fn main() {
    let run_all = std::env::args().any(|a| a == "--all");

    for report in scan_all() {
        let name = report.format.name();
        let plugins = match report.plugins {
            Ok(plugins) => plugins,
            Err(error) => {
                println!("[{name}] scan failed: {error}");
                continue;
            }
        };
        println!("[{name}] {} plugin(s) found", plugins.len());
        for info in plugins.iter().take(LISTED_PER_FORMAT) {
            println!("    {info}  id={}", info.unique_id);
        }
        if plugins.len() > LISTED_PER_FORMAT {
            println!("    ... and {} more", plugins.len() - LISTED_PER_FORMAT);
        }

        let to_run = if run_all { plugins.len() } else { 1 };
        let mut ok = 0_usize;
        let mut failed = 0_usize;
        for info in plugins.iter().take(to_run) {
            match load_and_process_silence(info, SAMPLE_RATE, FRAMES) {
                Ok(r) => {
                    ok += 1;
                    println!(
                        "    ran {:?}: {} in / {} out, {} params, status {:?}, peak {:.6}, finite {}",
                        info.name,
                        r.input_channels,
                        r.output_channels,
                        r.parameter_count,
                        r.status,
                        r.peak,
                        r.all_finite
                    );
                }
                Err(error) => {
                    failed += 1;
                    println!("    FAILED {:?}: {error}", info.name);
                }
            }
        }
        if !plugins.is_empty() {
            println!("[{name}] silent block: {ok} ok, {failed} failed");
        }
    }
}
