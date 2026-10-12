pub mod driver;
pub mod memory;

// Desktop drivers (built on the `arboard` crate) exist only where that crate does.
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub use driver::{CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError};

use klip_proto::hlc::NodeId;

/// Instantiate the native clipboard driver for the current host OS.
///
/// On Android and iOS the app owns the clipboard and supplies its own driver (see `klip-android`),
/// so there this returns an in-memory placeholder that keeps the function total.
pub fn create_default_driver(local_node: NodeId) -> Box<dyn ClipboardDriver> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsClipboardDriver::new(local_node))
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSClipboardDriver::new(local_node))
    }
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    {
        Box::new(linux::LinuxClipboardDriver::new(local_node))
    }
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        not(any(unix, target_os = "windows"))
    ))]
    {
        Box::new(memory::MemoryClipboardDriver::new(local_node))
    }
}
