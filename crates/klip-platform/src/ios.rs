use crate::driver::{CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError};
use klip_proto::hlc::NodeId;
use klip_proto::wire::{compute_fingerprint, ClipContentType};
use std::sync::Mutex;

pub const IOS_APP_GROUP_ID: &str = "group.com.klip.app";

pub struct IosClipboardDriver {
    app_group_clip: Mutex<Option<CapturedClip>>,
    echo_shield: EchoShield,
    local_node: NodeId,
}

impl IosClipboardDriver {
    pub fn new(local_node: NodeId) -> Self {
        Self {
            app_group_clip: Mutex::new(None),
            echo_shield: EchoShield::new(),
            local_node,
        }
    }
}

impl ClipboardDriver for IosClipboardDriver {
    fn environment(&self) -> PlatformEnvironment {
        PlatformEnvironment::IosAppGroup
    }

    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        let lock = self.app_group_clip.lock().unwrap();
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

        let mut lock = self.app_group_clip.lock().unwrap();
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
