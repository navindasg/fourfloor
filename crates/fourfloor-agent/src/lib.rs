//! The agent API for fourfloor.
//!
//! - [`Agent`] holds one project behind the model's command bus. Every
//!   edit, from any front end, is a [`tools`] call and one undo step.
//! - [`tools`] lists the tools: one per model command (generated from the
//!   commands' JSON Schemas) plus high-level tools such as
//!   `add_house_drum_pattern`, `add_bassline`, `sidechain`,
//!   `add_filter_riser`, `render` and `export_stems`.
//! - [`resources`] serves a project summary, a track/device/parameter
//!   listing, the device registry and a rendered mix report.
//! - [`server`] exposes all of it as an MCP server (rmcp) over stdio and
//!   Streamable HTTP on localhost.
//! - [`generate`] holds the seedable rule-and-Markov generators for drums,
//!   basslines and chords.
//! - [`nl`] is the natural-language layer: a deterministic phrase parser
//!   and a chat-backend trait with an OpenAI-compatible local client.

#![warn(missing_docs)]

pub use fourfloor_core as core;

pub mod agent;
pub mod cli;
pub mod error;
pub mod generate;
pub mod nl;
pub mod resources;
pub mod rng;
pub mod server;
pub mod tools;

pub use agent::{default_registry, Agent, AgentSettings};
pub use error::AgentError;
pub use nl::{share, SharedAgent};
pub use rng::{RandomSource, SeededRng};
pub use server::{serve_http, serve_stdio, FourfloorServer, HttpServer};
pub use tools::{ToolKind, ToolOutput, ToolSpec, TrackRef};
