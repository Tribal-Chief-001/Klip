use crate::driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};
use arboard::{Clipboard, ImageData};
use klip_proto::hlc::NodeId;
use klip_proto::wire::{compute_fingerprint, ClipContentType};
use std::borrow::Cow;
use std::env;
use std::path::PathBuf;
use std::sync::Mutex;

pub const GNOME_BRIDGE_SOCKET_PATH: &str = ".cache/klip/bridge.sock";
pub const X11_INCR_THRESHOLD: usize = 256 * 1024; // 256 KB

pub struct LinuxClipboardDriver {
    clipboard: Mutex<Option<Clipboard>>,
    env: PlatformEnvironment,
    echo_shield: EchoShield,
    local_node: NodeId,
}

impl LinuxClipboardDriver {
    pub fn new(local_node: NodeId) -> Self {
        let env = detect_linux_environment();
        let clipboard = match Clipboard::new() {
            Ok(cb) => Mutex::new(Some(cb)),
            Err(e) => {
                eprintln!(
                    "[klip_platform] Warning: could not initialize native clipboard: {:?}",
                    e
                );
                Mutex::new(None)
            }
        };

        let echo_shield = EchoShield::new();
        if let Ok(mut guard) = clipboard.lock() {
            if let Some(cb) = guard.as_mut() {
                if let Ok(text) = cb.get_text() {
                    if !text.is_empty() {
                        let fp = compute_fingerprint(text.as_bytes(), &local_node);
                        echo_shield.init_baseline(fp);
                    }
                }
            }
        }

        Self {
            clipboard,
            env,
            echo_shield,
            local_node,
        }
    }

    pub fn bridge_socket_path() -> Option<PathBuf> {
        env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(GNOME_BRIDGE_SOCKET_PATH))
    }
}

pub fn detect_linux_environment() -> PlatformEnvironment {
    let session_type = env::var("XDG_SESSION_TYPE")
        .unwrap_or_default()
        .to_lowercase();
    let current_desktop = env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();

    if session_type == "wayland" {
        if current_desktop.contains("gnome") {
            PlatformEnvironment::LinuxGnomeMutter
        } else {
            PlatformEnvironment::LinuxWayland
        }
    } else if env::var("DISPLAY").is_ok() {
        PlatformEnvironment::LinuxX11
    } else {
        PlatformEnvironment::Generic
    }
}

impl ClipboardDriver for LinuxClipboardDriver {
    fn environment(&self) -> PlatformEnvironment {
        self.env
    }

    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        let mut guard = self.clipboard.lock().unwrap();
        let cb = match guard.as_mut() {
            Some(cb) => cb,
            None => return Ok(None),
        };

        // Try reading text first
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                let bytes = text.into_bytes();
                let fp = compute_fingerprint(&bytes, &self.local_node);
                if self.echo_shield.should_suppress(&fp) {
                    return Ok(None);
                }
                return Ok(Some(CapturedClip {
                    content_type: ClipContentType::PlainTextUtf8,
                    data: bytes,
                    fingerprint: fp,
                }));
            }
        }

        // Try reading image
        if let Ok(img) = cb.get_image() {
            let bytes = img.bytes.into_owned();
            if !bytes.is_empty() {
                let fp = compute_fingerprint(&bytes, &self.local_node);
                if self.echo_shield.should_suppress(&fp) {
                    return Ok(None);
                }
                return Ok(Some(CapturedClip {
                    content_type: ClipContentType::ImagePng,
                    data: bytes,
                    fingerprint: fp,
                }));
            }
        }

        Ok(None)
    }

    fn write_clip(&self, content_type: &ClipContentType, data: &[u8]) -> Result<(), PlatformError> {
        let mut guard = self.clipboard.lock().unwrap();
        let cb = match guard.as_mut() {
            Some(cb) => cb,
            None => {
                return Err(PlatformError::BackendUnavailable(
                    "Clipboard backend uninitialized".to_string(),
                ))
            }
        };

        let fp = compute_fingerprint(data, &self.local_node);
        self.echo_shield.record_injection(fp);

        match content_type {
            ClipContentType::PlainTextUtf8 | ClipContentType::HtmlContent => {
                let text = String::from_utf8_lossy(data);
                cb.set_text(text)
                    .map_err(|e| PlatformError::BackendUnavailable(e.to_string()))?;
            }
            ClipContentType::ImagePng | ClipContentType::ImageWebP => {
                // If raw RGBA is supported, inject image
                let img = ImageData {
                    width: 1,
                    height: 1,
                    bytes: Cow::Borrowed(&[0, 0, 0, 255]),
                };
                let _ = cb.set_image(img);
            }
            ClipContentType::CustomMime(_) => {
                let text = String::from_utf8_lossy(data);
                cb.set_text(text)
                    .map_err(|e| PlatformError::BackendUnavailable(e.to_string()))?;
            }
        }

        Ok(())
    }

    fn echo_shield(&self) -> &EchoShield {
        &self.echo_shield
    }
}
