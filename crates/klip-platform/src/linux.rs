use crate::driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};
use arboard::{Clipboard, ImageData};
use klip_proto::clip::{compute_fingerprint, ClipContentType};
use klip_proto::hlc::NodeId;
use std::borrow::Cow;
use std::env;
use std::sync::Mutex;

pub const X11_INCR_THRESHOLD: usize = 256 * 1024; // 256 KB

pub struct LinuxClipboardDriver {
    clipboard: Mutex<Option<Clipboard>>,
    env: PlatformEnvironment,
    echo_shield: EchoShield,
    local_node: NodeId,
    changes: Option<tokio::sync::watch::Receiver<u64>>,
}

/// Watch the X11 CLIPBOARD selection owner with XFIXES: the X server pushes an event whenever any
/// application takes over the clipboard, so no polling is needed. Returns `None` when there is
/// no X server (pure Wayland) or XFIXES is unavailable; the caller then falls back to polling.
fn spawn_x11_change_watcher() -> Option<tokio::sync::watch::Receiver<u64>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xfixes::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    env::var_os("DISPLAY")?;
    let (conn, screen) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
    let clipboard = conn
        .intern_atom(false, b"CLIPBOARD")
        .ok()?
        .reply()
        .ok()?
        .atom;
    conn.xfixes_select_selection_input(
        root,
        clipboard,
        xfixes::SelectionEventMask::SET_SELECTION_OWNER
            | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
            | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )
    .ok()?
    .check()
    .ok()?;
    conn.flush().ok()?;

    let (tx, rx) = tokio::sync::watch::channel(0u64);
    std::thread::Builder::new()
        .name("klip-x11-watch".into())
        .spawn(move || {
            // Blocks inside the kernel until the X server sends something: zero idle cost.
            while let Ok(event) = conn.wait_for_event() {
                if matches!(event, Event::XfixesSelectionNotify(_)) {
                    tx.send_modify(|n| *n = n.wrapping_add(1));
                }
            }
        })
        .ok()?;
    Some(rx)
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
            changes: spawn_x11_change_watcher(),
        }
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
        let text_opt = cb.get_text().ok();
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
            // Would be refused by every receiver anyway: do not spend a PNG encode on it.
            if klip_diff::too_many_pixels(img.width as u32, img.height as u32) {
                return Ok(None);
            }
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

        // The echo shield must know what the clipboard will hold *before* the write, or the change
        // event could be read as a new copy. It also must match what `read_clip` will see.
        match content_type {
            ClipContentType::PlainTextUtf8
            | ClipContentType::HtmlContent
            | ClipContentType::CustomMime(_) => {
                let text = String::from_utf8_lossy(data);
                self.echo_shield
                    .record_injection(compute_fingerprint(text.as_bytes(), &self.local_node));
                cb.set_text(text)
                    .map_err(|e| PlatformError::BackendUnavailable(e.to_string()))?;
            }
            ClipContentType::ImagePng | ClipContentType::ImageWebP => {
                let (width, height, rgba) =
                    klip_diff::png_to_rgba(data).map_err(PlatformError::BackendUnavailable)?;
                // `read_clip` re-encodes the pixels as PNG, so that is what must be remembered, not
                // the incoming bytes (a WebP, or a PNG from another encoder, would never match).
                let png = klip_diff::rgba_to_png(width, height, &rgba)
                    .map_err(PlatformError::BackendUnavailable)?;
                self.echo_shield
                    .record_injection(compute_fingerprint(&png, &self.local_node));
                cb.set_image(ImageData {
                    width: width as usize,
                    height: height as usize,
                    bytes: Cow::Owned(rgba),
                })
                .map_err(|e| PlatformError::BackendUnavailable(e.to_string()))?;
            }
        }

        Ok(())
    }

    fn echo_shield(&self) -> &EchoShield {
        &self.echo_shield
    }

    fn change_signal(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.changes.clone()
    }
}
