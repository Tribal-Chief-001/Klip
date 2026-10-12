use crate::driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};
use arboard::{Clipboard, ImageData};
use klip_proto::clip::{compute_fingerprint, ClipContentType};
use klip_proto::hlc::NodeId;
use std::borrow::Cow;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

pub const MAX_OPEN_CLIPBOARD_RETRIES: u32 = 5;
pub const INITIAL_BACKOFF_MS: u64 = 10;

pub struct WindowsClipboardDriver {
    clipboard: Mutex<Option<Clipboard>>,
    echo_shield: EchoShield,
    local_node: NodeId,
}

impl WindowsClipboardDriver {
    pub fn new(local_node: NodeId) -> Self {
        let clipboard = Mutex::new(Clipboard::new().ok());
        Self {
            clipboard,
            echo_shield: EchoShield::new(),
            local_node,
        }
    }

    /// Execute clipboard operation with exponential backoff for Win32 OpenClipboard contention
    fn with_retry<F, R>(&self, mut f: F) -> Result<R, PlatformError>
    where
        F: FnMut(&mut Clipboard) -> Result<R, arboard::Error>,
    {
        let mut backoff = INITIAL_BACKOFF_MS;
        let mut guard = self.clipboard.lock().unwrap();
        let cb = match guard.as_mut() {
            Some(cb) => cb,
            None => {
                return Err(PlatformError::BackendUnavailable(
                    "Windows clipboard uninitialized".into(),
                ))
            }
        };

        for attempt in 0..MAX_OPEN_CLIPBOARD_RETRIES {
            match f(cb) {
                Ok(res) => return Ok(res),
                Err(e) => {
                    if attempt == MAX_OPEN_CLIPBOARD_RETRIES - 1 {
                        return Err(PlatformError::BackendUnavailable(e.to_string()));
                    }
                    sleep(Duration::from_millis(backoff));
                    backoff = (backoff * 2).min(100);
                }
            }
        }

        Err(PlatformError::ClipboardBusy)
    }
}

impl ClipboardDriver for WindowsClipboardDriver {
    fn environment(&self) -> PlatformEnvironment {
        PlatformEnvironment::Windows
    }

    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        let text_res = self.with_retry(|cb| cb.get_text());
        if let Ok(text) = text_res {
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

        let img_res = self.with_retry(|cb| cb.get_image());
        if let Ok(img) = img_res {
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
        let fp = compute_fingerprint(data, &self.local_node);
        self.echo_shield.record_injection(fp);

        match content_type {
            ClipContentType::PlainTextUtf8 => {
                let text = String::from_utf8_lossy(data).to_string();
                self.with_retry(|cb| cb.set_text(text.clone()))?;
            }
            ClipContentType::HtmlContent => {
                let raw_html = String::from_utf8_lossy(data);
                let text = klip_diff::utf8_to_cf_html(&raw_html);
                self.with_retry(|cb| cb.set_text(text.clone()))?;
            }
            ClipContentType::ImagePng | ClipContentType::ImageWebP => {
                if let Ok((width, height, rgba)) = klip_diff::png_to_rgba(data) {
                    let img = ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: Cow::Owned(rgba),
                    };
                    self.with_retry(|cb| cb.set_image(img.clone()))?;
                }
            }
            ClipContentType::CustomMime(_) => {
                let text = String::from_utf8_lossy(data).to_string();
                self.with_retry(|cb| cb.set_text(text.clone()))?;
            }
        }

        Ok(())
    }

    fn echo_shield(&self) -> &EchoShield {
        &self.echo_shield
    }
}
