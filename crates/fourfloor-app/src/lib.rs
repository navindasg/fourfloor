//! The fourfloor desktop application: egui on wgpu, one window.
//!
//! Layers, from the bottom up:
//!
//! - [`geometry`]: pure screen maths (time to pixels, pitch to rows, cell
//!   grids), unit-tested without a display.
//! - [`services`]: the engine host and audio callback, meters, analysis,
//!   background jobs, MIDI input and the device registry.
//! - [`project`]: the built-in demo and file import/export jobs.
//! - [`state`]: the UI-side document (a [`fourfloor_model::CommandBus`]),
//!   UI state and [`state::Intent`]s, the only way views change anything.
//! - [`viewmodel`]: editing logic per view (step patterns, notes, clip
//!   drags, parameter controls), turning gestures into model commands.
//! - [`widgets`] and [`views`]: painting. Each view implements
//!   [`views::View`] and is listed in [`views::registry`].
//! - [`app`]: the shell that wires it all together each frame.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod app;
pub mod geometry;
pub mod project;
pub mod services;
pub mod smoke;
pub mod state;
pub mod theme;
pub mod viewmodel;
pub mod views;
pub mod widgets;

pub use app::{AppConfig, AppError, FourfloorApp};
