use crate::driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};
use klip_proto::clip::{compute_fingerprint, ClipContentType};
use klip_proto::hlc::NodeId;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidSyncMode {
    AccessibilityServiceAuto,
    NotificationTapFallback,
    ImeCompanionStrip,
}

#[allow(dead_code)]
pub struct AndroidClipboardDriver {
    sync_mode: AndroidSyncMode,
    virtual_clip: Mutex<Option<CapturedClip>>,
    echo_shield: EchoShield,
    local_node: NodeId,
}

impl AndroidClipboardDriver {
    pub fn new(local_node: NodeId, sync_mode: AndroidSyncMode) -> Self {
        Self {
            sync_mode,
            virtual_clip: Mutex::new(None),
            echo_shield: EchoShield::new(),
            local_node,
        }
    }
}

impl ClipboardDriver for AndroidClipboardDriver {
    fn environment(&self) -> PlatformEnvironment {
        PlatformEnvironment::AndroidAccessibility
    }

    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        let lock = self.virtual_clip.lock().unwrap();
        if let Some(clip) = lock.as_ref() {
            if self.echo_shield.should_suppress(&clip.fingerprint) {
                return Ok(None);
            }
            return Ok(Some(clip.clone()));
        }
        Ok(None)
    }

    fn write_clip(&self, content_type: &ClipContentType, data: &[u8]) -> Result<(), PlatformError> {
        let fp = compute_fingerprint(data, &self.local_node);
        self.echo_shield.record_injection(fp);

        let mut lock = self.virtual_clip.lock().unwrap();
        *lock = Some(CapturedClip {
            content_type: content_type.clone(),
            data: data.to_vec(),
            fingerprint: fp,
        });
        Ok(())
    }

    fn echo_shield(&self) -> &EchoShield {
        &self.echo_shield
    }
}
