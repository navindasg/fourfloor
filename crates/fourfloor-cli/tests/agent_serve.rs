//! `fourfloor agent serve` over stdio, the way an MCP client launches it.
//!
//! Regression test: the binary used to hold the standard-output lock on the
//! main thread for its whole run, so the MCP server (which replies from a
//! Tokio worker thread) never wrote a byte and every client hung.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const SET_TEMPO: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"set_tempo","arguments":{"bpm":122}}}"#;

#[test]
fn agent_serve_answers_mcp_on_stdio_and_saves_the_edit() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("song");
    let mut child = Command::new(env!("CARGO_BIN_EXE_fourfloor"))
        .args(["agent", "serve"])
        .arg(&folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the fourfloor binary runs");

    let stdout = child.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{INITIALIZE}").unwrap();
    let reply = lines.recv_timeout(Duration::from_secs(30));
    let reply = match reply {
        Ok(reply) => reply,
        Err(_) => {
            let _ = child.kill();
            panic!("no reply to initialize within 30 s");
        }
    };
    assert!(reply.contains(r#""id":1"#), "{reply}");
    assert!(reply.contains("fourfloor-agent"), "{reply}");

    writeln!(stdin, "{INITIALIZED}").unwrap();
    writeln!(stdin, "{SET_TEMPO}").unwrap();
    let reply = lines.recv_timeout(Duration::from_secs(30));
    let _ = child.kill();
    let _ = child.wait();
    let reply = reply.expect("a reply to set_tempo within 30 s");
    assert!(reply.contains(r#""id":2"#), "{reply}");
    assert!(reply.contains(r#""isError":false"#), "{reply}");

    let saved = fourfloor_model::load_project(&folder).unwrap();
    assert_eq!(saved.tempo.bpm(), 122.0);
}
