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

fn read_bridge_socket() -> Option<String> {
    if let Some(sock_path) = LinuxClipboardDriver::bridge_socket_path() {
        if sock_path.exists() {
            #[cfg(unix)]
            {
                use std::io::Read;
                if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(sock_path) {
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(15)));
                    let mut buf = Vec::new();
                    if stream.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                        return String::from_utf8(buf).ok();
                    }
                }
            }
        }
    }
    None
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
        let text_opt = cb.get_text().ok().or_else(read_bridge_socket);
        if let Some(text) = text_opt {
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
            if let Ok(png_bytes) =
                klip_diff::rgba_to_png(img.width as u32, img.height as u32, &img.bytes)
            {
                if !png_bytes.is_empty() {
                    let fp = compute_fingerprint(&png_bytes, &self.local_node);
                    if self.echo_shield.should_suppress(&fp) {
                        return Ok(None);
                    }
                    return Ok(Some(CapturedClip {
                        content_type: ClipContentType::ImagePng,
                        data: png_bytes,
                        fingerprint: fp,
                    }));
                }
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
                if let Ok((width, height, rgba)) = klip_diff::png_to_rgba(data) {
                    let img = ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: Cow::Owned(rgba),
                    };
                    let _ = cb.set_image(img);
                }
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
