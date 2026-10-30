//! The agent's command line, shared by the `fourfloor-agent` binary and
//! `fourfloor agent`.
//!
//! ```text
//! serve <project-dir> [--http [ADDR]] [--no-autosave]
//! do <project-dir> <instruction...> [--llm URL --model NAME]
//! tools
//! ```
//!
//! `serve` speaks MCP on stdin/stdout unless `--http` is given, in which
//! case it listens on ADDR (default 127.0.0.1:8765) at `/mcp`. Edits are
//! saved to the project folder after each change unless `--no-autosave`.

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::nl::llm::OpenAiCompatible;
use crate::nl::{instruct, ChatOptions};
use crate::{default_registry, serve_http, serve_stdio, share, Agent, AgentError, AgentSettings};

/// Usage text for the agent commands.
pub const USAGE: &str = "usage:
  fourfloor-agent serve <project-dir> [--http [ADDR]] [--no-autosave]
  fourfloor-agent do <project-dir> <instruction...> [--llm URL --model NAME]
  fourfloor-agent tools";

const DEFAULT_HTTP: &str = "127.0.0.1:8765";

fn open(dir: &str, autosave: bool) -> Result<Agent, AgentError> {
    let registry = Arc::new(default_registry()?);
    let dir = PathBuf::from(dir);
    let settings = AgentSettings {
        autosave,
        ..AgentSettings::default()
    };
    Ok(Agent::open(&dir, registry)?.with_settings(settings))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

async fn serve(args: &[String]) -> Result<(), AgentError> {
    let dir = args
        .first()
        .ok_or_else(|| AgentError::Unavailable(USAGE.to_owned()))?;
    let autosave = !args.iter().any(|arg| arg == "--no-autosave");
    let agent = share(open(dir, autosave)?);
    if args.iter().any(|arg| arg == "--http") {
        let addr: SocketAddr = flag_value(args, "--http")
            .unwrap_or_else(|| DEFAULT_HTTP.to_owned())
            .parse()
            .map_err(|error| AgentError::Unavailable(format!("bad address: {error}")))?;
        let server = serve_http(agent, addr).await?;
        eprintln!("fourfloor-agent: MCP over HTTP at {}", server.url());
        let stop = server.cancel_token();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stop.cancel();
            }
        });
        server.wait().await
    } else {
        serve_stdio(agent).await
    }
}

async fn run_instruction(args: &[String]) -> Result<(), AgentError> {
    let dir = args
        .first()
        .ok_or_else(|| AgentError::Unavailable(USAGE.to_owned()))?;
    let mut text_words = Vec::new();
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--llm" || args[index] == "--model" {
            index += 2;
            continue;
        }
        text_words.push(args[index].clone());
        index += 1;
    }
    let text = text_words.join(" ");
    let backend = match flag_value(args, "--llm") {
        Some(url) => {
            let model = flag_value(args, "--model").unwrap_or_else(|| "local".to_owned());
            Some(OpenAiCompatible::new(&url, &model)?)
        }
        None => None,
    };
    let agent = share(open(dir, true)?);
    let backend_ref = backend
        .as_ref()
        .map(|backend| backend as &dyn crate::nl::llm::ChatBackend);
    let done = instruct(&agent, &text, backend_ref, &ChatOptions::default()).await?;
    for step in &done.steps {
        match &step.result {
            Ok(output) => println!("{}: {}", step.tool, output.text),
            Err(error) => println!("{}: failed: {error}", step.tool),
        }
    }
    if let Some(reply) = &done.reply {
        println!("{reply}");
    }
    Ok(())
}

fn print_tools() -> Result<(), AgentError> {
    let tools = crate::tools::catalog();
    let text = serde_json::to_string_pretty(&tools)
        .map_err(|error| AgentError::Unavailable(error.to_string()))?;
    let mut out = std::io::stdout().lock();
    match writeln!(out, "{text}") {
        // A closed pipe (`| head`) is the reader's choice, not a failure.
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(AgentError::Unavailable(error.to_string()))
        }
        _ => Ok(()),
    }
}

async fn dispatch(args: &[String]) -> Option<Result<(), AgentError>> {
    Some(match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]).await,
        Some("do") => run_instruction(&args[1..]).await,
        Some("tools") => print_tools(),
        _ => return None,
    })
}

/// Runs one agent command (`serve`, `do` or `tools`, then its arguments)
/// on a new Tokio runtime, printing errors to standard error prefixed with
/// `program`.
pub fn run(program: &str, args: &[String]) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{program}: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(dispatch(args)) {
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Some(Ok(())) => ExitCode::SUCCESS,
        Some(Err(error)) => {
            eprintln!("{program}: {error}");
            ExitCode::FAILURE
        }
    }
}
