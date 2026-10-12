use klip_proto::hlc::NodeId;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// Stateless Zero-Knowledge Relay Router.
/// Relays packets between nodes solely by inspecting the 32-byte destination `NodeId`.
/// Payloads remain strictly opaque and end-to-end encrypted.
#[derive(Clone, Default)]
pub struct StatelessRelayRouter {
    routes: Arc<Mutex<HashMap<NodeId, SocketAddr>>>,
}

impl StatelessRelayRouter {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_route(&self, node_id: NodeId, addr: SocketAddr) {
        let mut lock = self.routes.lock().unwrap();
        lock.insert(node_id, addr);
    }

    pub fn remove_route(&self, node_id: &NodeId) {
        let mut lock = self.routes.lock().unwrap();
        lock.remove(node_id);
    }

    pub fn lookup(&self, destination: &NodeId) -> Option<SocketAddr> {
        let lock = self.routes.lock().unwrap();
        lock.get(destination).copied()
    }

    pub fn active_nodes(&self) -> Vec<NodeId> {
        let lock = self.routes.lock().unwrap();
        lock.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stateless_relay_routing() {
        let router = StatelessRelayRouter::new();
        let node_a = NodeId::new([1u8; 32]);
        let addr_a: SocketAddr = "192.168.1.50:7123".parse().unwrap();

        router.register_route(node_a, addr_a);
        assert_eq!(router.lookup(&node_a), Some(addr_a));

        router.remove_route(&node_a);
        assert_eq!(router.lookup(&node_a), None);
    }
}
