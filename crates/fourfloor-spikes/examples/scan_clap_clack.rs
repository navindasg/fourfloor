//! CLAP-only fallback scan through clack-host.
//!
//! ```text
//! cargo run -p fourfloor-spikes --features hosting-clack --example scan_clap_clack
//! ```

use fourfloor_spikes::hosting_clack::{default_clap_dirs, process_silent_block, scan_default};

fn main() {
    println!("CLAP directories:");
    for dir in default_clap_dirs() {
        println!("    {}", dir.display());
    }
    let scan = scan_default();
    println!("{} plugin(s) found", scan.plugins.len());
    for plugin in &scan.plugins {
        println!(
            "    {} - {} [{}] {}",
            plugin.name,
            plugin.vendor,
            plugin.id,
            plugin.features.join(",")
        );
    }
    for failure in &scan.failures {
        println!(
            "    FAILED {}: {}",
            failure.bundle.display(),
            failure.reason
        );
    }
    if let Some(first) = scan.plugins.first() {
        match process_silent_block(first, 48_000.0, 512) {
            Ok(r) => println!(
                "ran {:?}: status {:?}, peak {:.6}, finite {}",
                first.name, r.status, r.peak, r.all_finite
            ),
            Err(error) => println!("FAILED {:?}: {error}", first.name),
        }
    }
}
