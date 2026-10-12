pub mod android;
pub mod driver;
pub mod ios;
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
/// On Android and iOS the clipboard is owned by the app UI layer, so the returned driver is a
/// bridge that the app feeds; see `android.rs` / `ios.rs`.
pub fn create_default_driver(local_node: NodeId) -> Box<dyn ClipboardDriver> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsClipboardDriver::new(local_node))
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSClipboardDriver::new(local_node))
    }
    #[cfg(target_os = "android")]
    {
        Box::new(android::AndroidClipboardDriver::new(
            local_node,
            android::AndroidSyncMode::AccessibilityServiceAuto,
        ))
    }
    #[cfg(target_os = "ios")]
    {
        Box::new(ios::IosClipboardDriver::new(local_node))
    }
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    {
        Box::new(linux::LinuxClipboardDriver::new(local_node))
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        Box::new(memory::MemoryClipboardDriver::new(local_node))
    }
}
