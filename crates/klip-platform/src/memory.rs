//! In-memory clipboard driver for tests and headless environments (CI, servers).

use crate::driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};
use klip_proto::clip::{compute_fingerprint, ClipContentType};
use klip_proto::hlc::NodeId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct MemoryClipboardDriver {
    clip: Mutex<Option<CapturedClip>>,
    echo_shield: EchoShield,
    local_node: NodeId,
    fail_writes: AtomicBool,
}

impl MemoryClipboardDriver {
    pub fn new(local_node: NodeId) -> Self {
        Self {
            clip: Mutex::new(None),
            echo_shield: EchoShield::new(),
            local_node,
            fail_writes: AtomicBool::new(false),
        }
    }

    /// Simulate a locked / unavailable OS clipboard: every `write_clip` fails while set.
    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    /// Simulate the user pressing Ctrl+C on text.
    pub fn user_copy(&self, text: &str) {
        let data = text.as_bytes().to_vec();
        let fingerprint = compute_fingerprint(&data, &self.local_node);
        *self.clip.lock().unwrap() = Some(CapturedClip {
            content_type: ClipContentType::PlainTextUtf8,
            data,
            fingerprint,
        });
    }

    /// Simulate the user copying non-text content (an image).
    pub fn user_copy_bytes(&self, content_type: ClipContentType, data: Vec<u8>) {
        let fingerprint = compute_fingerprint(&data, &self.local_node);
        *self.clip.lock().unwrap() = Some(CapturedClip {
            content_type,
            data,
            fingerprint,
        });
    }

    /// Whatever is on the clipboard now, with its content type.
    pub fn current_clip(&self) -> Option<CapturedClip> {
        self.clip.lock().unwrap().clone()
    }

    /// Current clipboard text (what a paste would produce).
    pub fn current_text(&self) -> Option<String> {
        self.clip
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| String::from_utf8(c.data.clone()).ok())
    }
}

impl ClipboardDriver for MemoryClipboardDriver {
    fn environment(&self) -> PlatformEnvironment {
        PlatformEnvironment::Generic
    }

    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        let guard = self.clip.lock().unwrap();
        match guard.as_ref() {
            Some(c) if !self.echo_shield.should_suppress(&c.fingerprint) => Ok(Some(c.clone())),
            _ => Ok(None),
        }
    }

    fn write_clip(&self, content_type: &ClipContentType, data: &[u8]) -> Result<(), PlatformError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(PlatformError::ClipboardBusy);
        }
        let fingerprint = compute_fingerprint(data, &self.local_node);
        self.echo_shield.record_injection(fingerprint);
        *self.clip.lock().unwrap() = Some(CapturedClip {
            content_type: content_type.clone(),
            data: data.to_vec(),
            fingerprint,
        });
        Ok(())
    }

    fn echo_shield(&self) -> &EchoShield {
        &self.echo_shield
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_copy_is_captured_once_and_remote_write_is_not_echoed() {
        let d = MemoryClipboardDriver::new(NodeId::new([1u8; 32]));
        assert!(d.read_clip().unwrap().is_none());
        d.user_copy("hello");
        assert_eq!(d.read_clip().unwrap().unwrap().data, b"hello");
        assert!(
            d.read_clip().unwrap().is_none(),
            "same content must not be re-captured"
        );

        d.write_clip(&ClipContentType::PlainTextUtf8, b"from remote")
            .unwrap();
        assert_eq!(d.current_text().as_deref(), Some("from remote"));
        assert!(
            d.read_clip().unwrap().is_none(),
            "injected clip must not echo back"
        );
    }
}
