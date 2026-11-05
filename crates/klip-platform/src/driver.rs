use klip_proto::clip::ClipContentType;
use std::fmt;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformEnvironment {
    LinuxWayland,
    LinuxX11,
    LinuxGnomeMutter,
    MacOS,
    Windows,
    /// A mobile app that owns the clipboard and feeds the engine through a bridge.
    MobileApp,
    Generic,
}

impl fmt::Display for PlatformEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LinuxWayland => write!(f, "Linux (Wayland, via XWayland)"),
            Self::LinuxX11 => write!(f, "Linux (X11)"),
            Self::LinuxGnomeMutter => write!(f, "Linux (GNOME Wayland, via XWayland)"),
            Self::MacOS => write!(f, "macOS"),
            Self::Windows => write!(f, "Windows"),
            Self::MobileApp => write!(f, "Mobile app"),
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawImageSignature {
    pub width: usize,
    pub height: usize,
    pub hash: [u8; 32],
}

/// Echo Shield: Caches the fingerprint of the last clip injected by Klip
/// and the last clip read from the host clipboard to prevent:
/// 1. Polling storms (reading identical unmodified clipboard content every 100ms)
/// 2. Local clipboard echo feedback loops (re-broadcasting clips injected by Klip).
/// 3. Raw image deflate/compression thrash on 100ms polling ticks.
pub struct EchoShield {
    last_injected_fingerprint: Mutex<Option<[u8; 32]>>,
    last_read_fingerprint: Mutex<Option<[u8; 32]>>,
    last_injected_raw_image: Mutex<Option<RawImageSignature>>,
    last_read_raw_image: Mutex<Option<RawImageSignature>>,
}

impl EchoShield {
    pub fn new() -> Self {
        Self {
            last_injected_fingerprint: Mutex::new(None),
            last_read_fingerprint: Mutex::new(None),
            last_injected_raw_image: Mutex::new(None),
            last_read_raw_image: Mutex::new(None),
        }
    }

    /// Record baseline content on driver initialization to avoid blasting pre-existing clipboard.
    pub fn init_baseline(&self, fingerprint: [u8; 32]) {
        let mut read = self.last_read_fingerprint.lock().unwrap();
        *read = Some(fingerprint);
    }

    /// Fast check if raw image dimensions and content match the previously read or injected image.
    /// Prevents raw image PNG deflate/compression thrash on 100ms poll ticks.
    pub fn should_suppress_raw_image(&self, width: usize, height: usize, raw_bytes: &[u8]) -> bool {
        let hash = *blake3::hash(raw_bytes).as_bytes();
        let sig = RawImageSignature { width, height, hash };

        let mut read_guard = self.last_read_raw_image.lock().unwrap();
        let inj_guard = self.last_injected_raw_image.lock().unwrap();

        // 1. If identical to last read image, suppress poll storm
        if let Some(last_read) = *read_guard {
            if last_read == sig {
                return true;
            }
        }

        // 2. If identical to last injected image, suppress echo
        if let Some(last_inj) = *inj_guard {
            if last_inj == sig {
                *read_guard = Some(sig);
                return true;
            }
        }

        false
    }

    /// Record a raw image that was read and verified.
    pub fn record_read_raw_image(&self, width: usize, height: usize, raw_bytes: &[u8]) {
        let hash = *blake3::hash(raw_bytes).as_bytes();
        let sig = RawImageSignature { width, height, hash };
        *self.last_read_raw_image.lock().unwrap() = Some(sig);
        *self.last_injected_raw_image.lock().unwrap() = None;
    }

    /// Record a raw image that is being injected into the OS clipboard.
    pub fn record_injected_raw_image(&self, width: usize, height: usize, raw_bytes: &[u8]) {
        let hash = *blake3::hash(raw_bytes).as_bytes();
        let sig = RawImageSignature { width, height, hash };
        *self.last_injected_raw_image.lock().unwrap() = Some(sig);
        *self.last_read_raw_image.lock().unwrap() = Some(sig);
    }

    /// Record a fingerprint that is about to be written to host clipboard.
    pub fn record_injection(&self, fingerprint: [u8; 32]) {
        let mut inj = self.last_injected_fingerprint.lock().unwrap();
        *inj = Some(fingerprint);
        let mut read = self.last_read_fingerprint.lock().unwrap();
        *read = Some(fingerprint);
    }

    /// Check if a captured clip should be suppressed.
    /// Returns true if content has not changed or was injected by Klip.
    /// Returns false if it is a genuinely new user copy that should be broadcast.
    pub fn should_suppress(&self, fingerprint: &[u8; 32]) -> bool {
        let mut read_guard = self.last_read_fingerprint.lock().unwrap();
        let mut inj_guard = self.last_injected_fingerprint.lock().unwrap();

        // 1. If identical to last read clip, suppress (eliminates 100ms poll storm)
        if let Some(last_read) = *read_guard {
            if &last_read == fingerprint {
                return true;
            }
        }

        // 2. If identical to last injected clip, suppress (eliminates echo loop)
        if let Some(last_inj) = *inj_guard {
            if &last_inj == fingerprint {
                *read_guard = Some(*fingerprint);
                return true;
            }
        }

        // 3. New local user copy: update last read and clear injection cache
        *read_guard = Some(*fingerprint);
        *inj_guard = None;
        *self.last_read_raw_image.lock().unwrap() = None;
        *self.last_injected_raw_image.lock().unwrap() = None;
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
    /// Read the active clipboard contents right now without poll suppression filtering.
    /// Default implementation delegates to `read_clip()`.
    fn read_current_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        self.read_clip()
    }
    fn write_clip(&self, content_type: &ClipContentType, data: &[u8]) -> Result<(), PlatformError>;
    /// Like [`Self::write_clip`] but tells the driver the content was flagged as a secret, so it
    /// can ask the OS to keep it out of keyboard suggestions/history where the platform allows
    /// (Android `EXTRA_IS_SENSITIVE`, Windows history formats, Linux `x-kde-passwordManagerHint`).
    fn write_clip_flagged(
        &self,
        content_type: &ClipContentType,
        data: &[u8],
        sensitive: bool,
    ) -> Result<(), PlatformError> {
        let _ = sensitive;
        self.write_clip(content_type, data)
    }
    fn echo_shield(&self) -> &EchoShield;

    /// If the OS can tell us when the clipboard changes, a receiver that ticks on every change.
    /// Hosts then read the clipboard on demand instead of polling it every few milliseconds, which
    /// is what keeps an idle daemon at ~zero wakeups. `None` means "you must poll".
    fn change_signal(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_shield_lifecycle_and_poll_suppression() {
        let shield = EchoShield::new();
        let clip_a = [1u8; 32];
        let clip_b = [2u8; 32];
        let clip_c = [3u8; 32];

        // 1. Initial baseline: content present before start is suppressed
        shield.init_baseline(clip_a);
        assert!(shield.should_suppress(&clip_a));
        assert!(shield.should_suppress(&clip_a)); // subsequent polls suppressed

        // 2. User copies new content clip_b: should NOT be suppressed
        assert!(!shield.should_suppress(&clip_b));
        // Next 50 polling ticks with clip_b still in clipboard: ALL suppressed (no poll storm!)
        for _ in 0..50 {
            assert!(shield.should_suppress(&clip_b));
        }

        // 3. Remote peer injects clip_c:
        shield.record_injection(clip_c);
        // Polling ticks reading injected clip_c: ALL suppressed (no echo loop!)
        for _ in 0..50 {
            assert!(shield.should_suppress(&clip_c));
        }

        // 4. User now copies clip_a: should NOT be suppressed
        assert!(!shield.should_suppress(&clip_a));
        assert!(shield.should_suppress(&clip_a));
    }

    #[test]
    fn test_echo_shield_raw_image_suppression() {
        let shield = EchoShield::new();
        let raw_pixels_a = vec![255u8; 100 * 100 * 4];
        let raw_pixels_b = vec![128u8; 100 * 100 * 4];

        // 1. First sight of raw image: not suppressed
        assert!(!shield.should_suppress_raw_image(100, 100, &raw_pixels_a));
        shield.record_read_raw_image(100, 100, &raw_pixels_a);

        // 2. Subsequent 100ms polling ticks: ALL suppressed without needing PNG encoding!
        for _ in 0..100 {
            assert!(shield.should_suppress_raw_image(100, 100, &raw_pixels_a));
        }

        // 3. Injected image from peer: suppressed without PNG encoding!
        shield.record_injected_raw_image(100, 100, &raw_pixels_b);
        for _ in 0..100 {
            assert!(shield.should_suppress_raw_image(100, 100, &raw_pixels_b));
        }
    }
}

