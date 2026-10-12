use klip_proto::hlc::NodeId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAILBOX_TTL_SECS: u64 = 600; // 10 minutes default TTL for sleeping devices

#[derive(Debug, Clone)]
pub struct MailboxEntry {
    pub group_token: [u8; 32],
    pub recipient: NodeId,
    pub encrypted_blob: Vec<u8>,
    pub created_at_secs: u64,
    pub ttl_secs: u64,
}

impl MailboxEntry {
    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs >= self.created_at_secs + self.ttl_secs
    }
}

/// In-memory Ephemeral Store-and-Forward Mailbox Queue for sleeping devices.
/// Relays cannot read the encrypted_blob (Zero-Knowledge).
#[derive(Clone, Default)]
pub struct EphemeralMailbox {
    entries: Arc<Mutex<HashMap<NodeId, Vec<MailboxEntry>>>>,
}

impl EphemeralMailbox {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Deposit an encrypted blob into the recipient's mailbox.
    pub fn deposit(
        &self,
        group_token: [u8; 32],
        recipient: NodeId,
        encrypted_blob: Vec<u8>,
        ttl_secs: u64,
    ) {
        let now = now_secs();
        let entry = MailboxEntry {
            group_token,
            recipient,
            encrypted_blob,
            created_at_secs: now,
            ttl_secs,
        };

        let mut lock = self.entries.lock().unwrap();
        lock.entry(recipient).or_default().push(entry);
    }

    /// Fetch and drain pending mailbox entries for a woken recipient node.
    pub fn fetch_and_drain(&self, recipient: &NodeId) -> Vec<MailboxEntry> {
        let now = now_secs();
        let mut lock = self.entries.lock().unwrap();
        if let Some(list) = lock.remove(recipient) {
            list.into_iter().filter(|e| !e.is_expired(now)).collect()
        } else {
            Vec::new()
        }
    }

    /// Periodically purge expired mailbox items.
    pub fn purge_expired(&self) {
        let now = now_secs();
        let mut lock = self.entries.lock().unwrap();
        lock.retain(|_, list| {
            list.retain(|e| !e.is_expired(now));
            !list.is_empty()
        });
    }

    pub fn pending_count(&self, recipient: &NodeId) -> usize {
        let now = now_secs();
        let lock = self.entries.lock().unwrap();
        lock.get(recipient)
            .map(|list| list.iter().filter(|e| !e.is_expired(now)).count())
            .unwrap_or(0)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_deposit_and_fetch() {
        let mailbox = EphemeralMailbox::new();
        let recipient = NodeId::new([55u8; 32]);
        let token = [1u8; 32];
        let secret_payload = b"encrypted clip while laptop was asleep".to_vec();

        mailbox.deposit(token, recipient, secret_payload.clone(), 60);
        assert_eq!(mailbox.pending_count(&recipient), 1);

        let drained = mailbox.fetch_and_drain(&recipient);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].encrypted_blob, secret_payload);

        // Mailbox is drained
        assert_eq!(mailbox.pending_count(&recipient), 0);
    }
}
