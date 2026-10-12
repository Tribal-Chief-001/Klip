use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

/// 32-byte unique node identifier (typically Ed25519 public key).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    pub const ZERO: Self = Self([0u8; 32]);

    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(64);
        for b in self.0 {
            hex.push_str(&format!("{:02x}", b));
        }
        hex
    }

    pub fn short_id(&self) -> String {
        let hex = self.to_hex();
        format!("{}...{}", &hex[..6], &hex[hex.len() - 4..])
    }
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeId({})", self.short_id())
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_id())
    }
}

/// Hybrid Logical Clock (HLC) tracking causality and monotonic time.
/// HLC = <physical_time_ms, logical_counter, node_id>
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HybridLogicalClock {
    pub physical_time_ms: u64,
    pub logical_counter: u32,
    pub node_id: NodeId,
}

impl HybridLogicalClock {
    pub fn new(node_id: NodeId) -> Self {
        let now = current_time_ms();
        Self {
            physical_time_ms: now,
            logical_counter: 0,
            node_id,
        }
    }

    /// Advance clock on sending or local event.
    pub fn send_event(&mut self) -> Self {
        let now = current_time_ms();
        if now > self.physical_time_ms {
            self.physical_time_ms = now;
            self.logical_counter = 0;
        } else {
            self.logical_counter = self.logical_counter.saturating_add(1);
        }
        *self
    }

    /// Advance clock on receiving a remote message's HLC.
    pub fn recv_event(&mut self, remote: &HybridLogicalClock) -> Self {
        let now = current_time_ms();
        let max_physical = self.physical_time_ms.max(remote.physical_time_ms).max(now);

        if max_physical == self.physical_time_ms && max_physical == remote.physical_time_ms {
            self.logical_counter = self.logical_counter.max(remote.logical_counter).saturating_add(1);
        } else if max_physical == self.physical_time_ms {
            self.logical_counter = self.logical_counter.saturating_add(1);
        } else if max_physical == remote.physical_time_ms {
            self.logical_counter = remote.logical_counter.saturating_add(1);
        } else {
            self.logical_counter = 0;
        }

        self.physical_time_ms = max_physical;
        *self
    }
}

impl PartialOrd for HybridLogicalClock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HybridLogicalClock {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.physical_time_ms.cmp(&other.physical_time_ms) {
            Ordering::Equal => match self.logical_counter.cmp(&other.logical_counter) {
                Ordering::Equal => self.node_id.cmp(&other.node_id),
                ord => ord,
            },
            ord => ord,
        }
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hlc_monotonicity() {
        let node_a = NodeId::new([1u8; 32]);
        let mut clock_a = HybridLogicalClock::new(node_a);

        let t1 = clock_a.send_event();
        let t2 = clock_a.send_event();
        assert!(t2 > t1);

        let node_b = NodeId::new([2u8; 32]);
        let mut clock_b = HybridLogicalClock::new(node_b);
        let t3 = clock_b.recv_event(&t2);
        assert!(t3 > t2);
    }

    #[test]
    fn test_hlc_lww_ordering() {
        let n1 = NodeId::new([1u8; 32]);
        let n2 = NodeId::new([2u8; 32]);

        let h1 = HybridLogicalClock {
            physical_time_ms: 1000,
            logical_counter: 0,
            node_id: n1,
        };
        let h2 = HybridLogicalClock {
            physical_time_ms: 1000,
            logical_counter: 0,
            node_id: n2,
        };
        // Deterministic tie-breaker by node_id
        assert!(h2 > h1);
    }
}
