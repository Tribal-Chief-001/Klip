use std::fmt;

#[derive(Debug)]
pub enum NetError {
    /// Could not create or bind the QUIC endpoint.
    Bind(String),
    /// Could not reach the peer.
    Connect(String),
    /// Peer closed or a stream failed.
    Stream(String),
    /// Malformed or oversized message.
    Protocol(String),
    /// Ticket is malformed, expired or has an unsupported version.
    Ticket(String),
    /// Pairing was rejected (wrong/expired secret or declined by the host).
    PairingRejected(String),
    /// A clip exceeded the configured size limit.
    TooLarge { size: usize, limit: usize },
    /// The operation needs a paired peer.
    UnknownPeer,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(e) => write!(f, "cannot start network endpoint: {e}"),
            Self::Connect(e) => write!(f, "cannot reach peer: {e}"),
            Self::Stream(e) => write!(f, "connection error: {e}"),
            Self::Protocol(e) => write!(f, "protocol error: {e}"),
            Self::Ticket(e) => write!(f, "invalid pairing ticket: {e}"),
            Self::PairingRejected(e) => write!(f, "pairing rejected: {e}"),
            Self::TooLarge { size, limit } => {
                write!(f, "clip of {size} bytes exceeds the {limit} byte limit")
            }
            Self::UnknownPeer => write!(f, "peer is not paired"),
        }
    }
}

impl std::error::Error for NetError {}
