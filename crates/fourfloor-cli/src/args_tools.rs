//! Argument parsing for the subcommands that wrap the analysis, interchange,
//! library, plugin and agent crates.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::Invocation;
use crate::error::CliError;

/// Most results `library search` prints unless `--limit` says otherwise.
pub const DEFAULT_SEARCH_LIMIT: usize = 25;
const MAX_SEARCH_LIMIT: usize = 10_000;

/// A `library search` filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchOptions {
    /// Library database; the shared default when `None`.
    pub db: Option<PathBuf>,
    /// Words to find in names, folders and tags.
    pub text: Option<String>,
    /// Tempo range in BPM, inclusive.
    pub bpm: Option<(f64, f64)>,
    /// Key, in any form `fourfloor_analysis::Key` parses ("Am", "8A").
    pub key: Option<String>,
    /// File kind: `one-shot`, `loop` or `track`.
    pub kind: Option<String>,
    /// Most rows to print.
    pub limit: usize,
}

/// What the tool subcommands asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolInvocation {
    /// Tempo, key and loudness of an audio file.
    Analyze {
        /// The file to analyse.
        path: PathBuf,
    },
    /// Write a project as a DAWproject file.
    Export {
        /// Project folder to read.
        dir: PathBuf,
        /// `.dawproject` file to write.
        output: PathBuf,
    },
    /// Read a DAWproject file into a new project folder.
    Import {
        /// `.dawproject` file to read.
        file: PathBuf,
        /// Project folder to create.
        dir: PathBuf,
    },
    /// Index folders into the library.
    LibraryScan {
        /// Library database; the shared default when `None`.
        db: Option<PathBuf>,
        /// Folders to scan.
        folders: Vec<PathBuf>,
    },
    /// Search the library.
    LibrarySearch(SearchOptions),
    /// Scan the system's plugin folders and Audio Units into the cache.
    PluginsScan,
    /// List the cached plugin scan.
    PluginsList,
    /// Run an agent command (`serve`, `do`, `tools`).
    Agent(Vec<String>),
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

/// Positional arguments and `--flag value` pairs, in order.
struct Split {
    positional: Vec<OsString>,
    flags: Vec<(String, String)>,
}

fn split(command: &str, args: Vec<OsString>, known: &[&str]) -> Result<Split, CliError> {
    let mut positional = Vec::new();
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if !(text.starts_with('-') && text.len() > 1) {
            positional.push(arg);
            continue;
        }
        let flag = text.into_owned();
        if !known.contains(&flag.as_str()) {
            return Err(usage(format!("unknown option {flag} for `{command}`")));
        }
        let value = args
            .next()
            .ok_or_else(|| usage(format!("{flag} needs a value")))?
            .into_string()
            .map_err(|_| usage(format!("{flag} is not valid UTF-8")))?;
        if flags.iter().any(|(seen, _)| *seen == flag) {
            return Err(usage(format!("{flag} was given twice")));
        }
        flags.push((flag, value));
    }
    Ok(Split { positional, flags })
}

impl Split {
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(flag, _)| flag == name)
            .map(|(_, value)| value.as_str())
    }

    /// Exactly `count` positional paths, or a usage error naming `what`.
    fn paths(self, command: &str, count: usize, what: &str) -> Result<Vec<PathBuf>, CliError> {
        if self.positional.len() != count {
            return Err(usage(format!("`{command}` needs {what}")));
        }
        Ok(self.positional.into_iter().map(PathBuf::from).collect())
    }
}

/// Reads `lo-hi` or a single tempo (a ±2 BPM window).
fn parse_bpm(text: &str) -> Result<(f64, f64), CliError> {
    let bad = || {
        usage(format!(
            "--bpm expects a tempo or a range like 120-128, got `{text}`"
        ))
    };
    let number = |part: &str| part.trim().parse::<f64>().map_err(|_| bad());
    let (low, high) = match text.split_once('-') {
        Some((low, high)) => (number(low)?, number(high)?),
        None => {
            let bpm = number(text)?;
            (bpm - 2.0, bpm + 2.0)
        }
    };
    if !(low.is_finite() && high.is_finite() && low > 0.0 && low <= high) {
        return Err(bad());
    }
    Ok((low, high))
}

fn parse_search(args: Vec<OsString>) -> Result<ToolInvocation, CliError> {
    let known = ["--db", "--text", "--bpm", "--key", "--kind", "--limit"];
    let split = split("library search", args, &known)?;
    let mut words: Vec<String> = Vec::new();
    for word in &split.positional {
        let word = word
            .to_str()
            .ok_or_else(|| usage("search words must be valid UTF-8"))?;
        words.push(word.to_owned());
    }
    let text = match (split.flag("--text"), words.is_empty()) {
        (Some(_), false) => return Err(usage("give search words or --text, not both")),
        (Some(text), true) => Some(text.to_owned()),
        (None, false) => Some(words.join(" ")),
        (None, true) => None,
    };
    let limit = match split.flag("--limit") {
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_SEARCH_LIMIT).contains(n))
            .ok_or_else(|| usage(format!("--limit must be 1 to {MAX_SEARCH_LIMIT}")))?,
        None => DEFAULT_SEARCH_LIMIT,
    };
    Ok(ToolInvocation::LibrarySearch(SearchOptions {
        db: split.flag("--db").map(PathBuf::from),
        text,
        bpm: split.flag("--bpm").map(parse_bpm).transpose()?,
        key: split.flag("--key").map(str::to_owned),
        kind: split.flag("--kind").map(str::to_owned),
        limit,
    }))
}

fn parse_library(mut args: Vec<OsString>) -> Result<ToolInvocation, CliError> {
    if args.is_empty() {
        return Err(usage("`library` needs `scan` or `search`"));
    }
    let action = args.remove(0);
    match action.to_string_lossy().as_ref() {
        "scan" => {
            let split = split("library scan", args, &["--db"])?;
            let db = split.flag("--db").map(PathBuf::from);
            if split.positional.is_empty() {
                return Err(usage("`library scan` needs at least one folder"));
            }
            let folders = split.positional.into_iter().map(PathBuf::from).collect();
            Ok(ToolInvocation::LibraryScan { db, folders })
        }
        "search" => parse_search(args),
        other => Err(usage(format!(
            "unknown library command `{other}`; use `scan` or `search`"
        ))),
    }
}

fn parse_plugins(args: Vec<OsString>) -> Result<ToolInvocation, CliError> {
    let words: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["scan"] => Ok(ToolInvocation::PluginsScan),
        ["list"] => Ok(ToolInvocation::PluginsList),
        _ => Err(usage("`plugins` needs `scan` or `list`")),
    }
}

fn parse_agent(args: Vec<OsString>) -> Result<ToolInvocation, CliError> {
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| usage("agent arguments must be valid UTF-8"))
        })
        .collect::<Result<_, _>>()?;
    match args.first().map(String::as_str) {
        Some("serve" | "do" | "tools") => Ok(ToolInvocation::Agent(args)),
        _ => Err(usage("`agent` needs `serve`, `do` or `tools`")),
    }
}

/// Parses `command` and its arguments if it is one of the tool
/// subcommands; `None` when it is not.
pub fn parse_tool(command: &str, args: Vec<OsString>) -> Option<Result<Invocation, CliError>> {
    let parsed = match command {
        "analyze" | "analyse" => split("analyze", args, &[])
            .and_then(|split| split.paths("analyze", 1, "one audio file"))
            .map(|mut paths| ToolInvocation::Analyze {
                path: paths.remove(0),
            }),
        "export" => split("export", args, &["-o", "--output"]).and_then(|split| {
            let output = match (split.flag("-o"), split.flag("--output")) {
                (Some(_), Some(_)) => return Err(usage("give either -o or --output, not both")),
                (Some(path), None) | (None, Some(path)) => PathBuf::from(path),
                (None, None) => return Err(usage("`export` needs -o <file.dawproject>")),
            };
            let mut paths = split.paths("export", 1, "a project folder")?;
            Ok(ToolInvocation::Export {
                dir: paths.remove(0),
                output,
            })
        }),
        "import" => split("import", args, &[])
            .and_then(|split| split.paths("import", 2, "a .dawproject file and a new folder"))
            .map(|mut paths| {
                let dir = paths.remove(1);
                ToolInvocation::Import {
                    file: paths.remove(0),
                    dir,
                }
            }),
        "library" => parse_library(args),
        "plugins" => parse_plugins(args),
        "agent" => parse_agent(args),
        _ => return None,
    };
    Some(parsed.map(Invocation::Tool))
}
