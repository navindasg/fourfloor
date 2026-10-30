//! `fourfloor-agent`: run the MCP server or a one-off instruction. See
//! [`fourfloor_agent::cli`] for the commands.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    fourfloor_agent::cli::run("fourfloor-agent", &args)
}
