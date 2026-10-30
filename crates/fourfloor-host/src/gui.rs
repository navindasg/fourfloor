//! Plugin editors: **design notes and a stub API. Nothing here opens a
//! window yet.**
//!
//! # Design
//!
//! A plugin editor is a native view the plugin draws into a parent the host
//! supplies. The three formats agree on the shape of it:
//!
//! | Format | How the view is obtained | Parent it wants |
//! | --- | --- | --- |
//! | CLAP | `clap.gui` extension: `create`, `set_parent`, `show` | `NSView*`, `HWND` or X11 window |
//! | AU v2 | `kAudioUnitProperty_CocoaUI` gives a view factory class | the factory returns an `NSView` to add as a subview |
//! | VST3 | `IEditController::createView("editor")` gives an `IPlugView` | `attached(parent, type)` |
//!
//! What the host has to provide:
//!
//! 1. **The UI thread.** Editors must be created and driven from the
//!    operating system's UI thread (the AppKit main thread on macOS). Today
//!    plugins use a worker thread as their main thread
//!    ([`crate::thread::HostThread`]). When `fourfloor-app` exists, its UI
//!    thread takes that role: `HostThread` grows a constructor that wraps
//!    the app's event-loop proxy instead of spawning a worker, and nothing
//!    else in this crate changes, because every main-thread call already
//!    goes through `HostThread::run`.
//! 2. **A parent window.** The app creates one top-level window per open
//!    editor and passes its raw handle as an [`EditorParent`]. Embedding the
//!    editor in the app's own window is avoided on purpose: a plugin view
//!    inside a GPU-rendered UI needs child-window compositing that differs
//!    per platform.
//! 3. **Resize negotiation.** CLAP: `can_resize`, `adjust_size`,
//!    `set_size`, plus the host-side `request_resize`. AU: observe the
//!    view's frame. VST3: `IPlugFrame::resizeView` and `onSize`.
//! 4. **Parameter feedback.** Edits made in the editor arrive as output
//!    events from `process` (CLAP), parameter listener callbacks (AU) or
//!    `IComponentHandler::performEdit` (VST3). They must be forwarded to
//!    the edit model as undoable commands. The CLAP wrapper currently
//!    discards them; that path is the first thing to add.
//! 5. **Idle time.** `HostThread` already calls each plugin's idle hook
//!    about every 20 ms, which covers CLAP's `request_callback` and timers.
//!
//! # Stub API
//!
//! [`PluginEditor`] is the interface a format wrapper will implement, and
//! [`open_editor`] is the entry point the app will call. `open_editor`
//! always returns [`HostError::Unsupported`].

use crate::error::HostError;
use crate::info::PluginController;

/// A native window or view an editor is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorParent {
    /// macOS: an `NSView*`.
    AppKit(*mut std::ffi::c_void),
    /// Windows: an `HWND`.
    Win32(*mut std::ffi::c_void),
    /// X11: a window id.
    X11(u32),
}

/// An open plugin editor.
pub trait PluginEditor {
    /// Current size in logical pixels.
    fn size(&self) -> (u32, u32);

    /// Whether the user may resize the editor.
    fn is_resizable(&self) -> bool;

    /// Asks for a new size; returns the size the plugin settled on.
    fn set_size(&mut self, width: u32, height: u32) -> (u32, u32);

    /// Shows or hides the editor without destroying it.
    fn set_visible(&mut self, visible: bool);
}

/// **Stub.** Opens a plugin's editor inside `parent`.
///
/// Not implemented: always returns [`HostError::Unsupported`]. See the
/// module documentation for the design.
pub fn open_editor(
    plugin: &PluginController,
    _parent: EditorParent,
) -> Result<Box<dyn PluginEditor>, HostError> {
    Err(HostError::Unsupported(format!(
        "plugin editors are not implemented yet (asked for {})",
        plugin.kind()
    )))
}
