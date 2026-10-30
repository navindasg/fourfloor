//! Realtime output backends. Nothing here is opened by the test suite: the
//! tests render through [`crate::offline`] instead.

#[cfg(feature = "cpal")]
pub mod cpal_out;
