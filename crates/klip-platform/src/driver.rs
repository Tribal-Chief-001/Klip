use klip_proto::wire::ClipContentType;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformEnvironment {
    LinuxWayland,
    LinuxX11,
    LinuxGnomeMutter,
    MacOS,
    Windows,
    AndroidAccessibility,
    IosAppGroup,
    Generic,
}

impl fmt::Display for PlatformEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LinuxWayland => write!(f, "Linux (Wayland ext-data-control)"),
            Self::LinuxX11 => write!(f, "Linux (X11 INCR)"),
            Self::LinuxGnomeMutter => write!(f, "Linux (GNOME Mutter Bridge)"),
            Self::MacOS => write!(f, "macOS (NSPasteboard)"),
            Self::Windows => write!(f, "Windows (Win32 HWND_MESSAGE)"),
            Self::AndroidAccessibility => write!(f, "Android (Accessibility Auto-Sync)"),
            Self::IosAppGroup => write!(f, "iOS (App Group)"),
            Self::Generic => write!(f, "Generic OS Clipboard"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedClip {
    pub content_type: ClipContentType,
    pub data: Vec<u8>,
    pub fingerprint: [u8; 32],
}

#[derive(Debug)]
pub enum PlatformError {
    ClipboardBusy,
    UnsupportedFormat,
    BackendUnavailable(String),
    Io(std::io::Error),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClipboardBusy => write!(f, "System clipboard is locked by another process"),
            Self::UnsupportedFormat => write!(f, "Unsupported clipboard format"),
            Self::BackendUnavailable(e) => write!(f, "Clipboard backend unavailable: {}", e),
            Self::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for PlatformError {}

/// Echo Shield: Caches the fingerprint of the last clip injected by Klip
/// to prevent local clipboard echo feedback loops.
pub struct EchoShield {
    last_injected_fingerprint: Mutex<Option<[u8; 32]>>,
    ignore_next_read: AtomicBool,
}

impl EchoShield {
    pub fn new() -> Self {
        Self {
            last_injected_fingerprint: Mutex::new(None),
            ignore_next_read: AtomicBool::new(false),
        }
    }

    /// Record a fingerprint that is about to be written to host clipboard.
    pub fn record_injection(&self, fingerprint: [u8; 32]) {
        let mut lock = self.last_injected_fingerprint.lock().unwrap();
        *lock = Some(fingerprint);
        self.ignore_next_read.store(true, Ordering::SeqCst);
    }

    /// Check if a captured clip should be suppressed because it matches the last injection.
    pub fn should_suppress(&self, fingerprint: &[u8; 32]) -> bool {
        if self.ignore_next_read.swap(false, Ordering::SeqCst) {
            let lock = self.last_injected_fingerprint.lock().unwrap();
            if let Some(injected) = *lock {
                if &injected == fingerprint {
                    return true;
                }
            }
        }
        false
    }
}

impl Default for EchoShield {
    fn default() -> Self {
        Self::new()
    }
}

/// Abstract cross-platform clipboard driver.
pub trait ClipboardDriver: Send + Sync {
    fn environment(&self) -> PlatformEnvironment;
    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError>;
    fn write_clip(&self, content_type: &ClipContentType, data: &[u8]) -> Result<(), PlatformError>;
    fn echo_shield(&self) -> &EchoShield;
}
