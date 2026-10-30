//! `fourfloor` command-line entry point. See the `fourfloor_cli` library
//! for what each subcommand does.

use std::process::ExitCode;

// Makes the engine's allocation guard live: in a debug build, anything that
// allocates inside the audio callback (offline or realtime) aborts.
#[global_allocator]
static ALLOCATOR: fourfloor_engine::AllocDisabler = fourfloor_engine::AllocDisabler;

fn main() -> ExitCode {
    // Not `stdout().lock()`: `agent serve` writes MCP replies from a Tokio
    // worker thread, which would wait forever on a lock held by this one.
    let mut out = std::io::stdout();
    match fourfloor_cli::run(std::env::args_os().skip(1), &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(if error.is_usage() { 2 } else { 1 })
        }
    }
}
