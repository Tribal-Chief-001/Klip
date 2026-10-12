pub mod android;
pub mod driver;
pub mod ios;
pub mod linux;
pub mod macos;
pub mod windows;

pub use driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};

use klip_proto::hlc::NodeId;

/// Instantiate the optimal native clipboard driver for the current host OS.
pub fn create_default_driver(local_node: NodeId) -> Box<dyn ClipboardDriver> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxClipboardDriver::new(local_node))
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOSClipboardDriver::new(local_node))
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsClipboardDriver::new(local_node))
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
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        Box::new(linux::LinuxClipboardDriver::new(local_node))
    }
}
