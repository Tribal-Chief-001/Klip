use crate::error::NetError;
use data_encoding::BASE32_NOPAD;
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const TICKET_PREFIX: &str = "klip1";
const TICKET_VERSION: u8 = 1;
const MAX_TICKET_CHARS: usize = 2048;

/// Out-of-band pairing ticket (shown as text / QR on the host).
///
/// Contains the host's public address (identity + optional relay/direct hints) and a one-time
/// 128-bit secret. The secret is only ever sent over the already-authenticated QUIC connection
/// to the host named in the ticket, so a network attacker cannot learn or replay it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairingTicket {
    version: u8,
    pub addr: EndpointAddr,
    pub secret: [u8; 16],
    pub expires_unix: u64,
}

impl PairingTicket {
    pub(crate) fn new(addr: EndpointAddr, secret: [u8; 16], expires_unix: u64) -> Self {
        Self {
            version: TICKET_VERSION,
            addr,
            secret,
            expires_unix,
        }
    }

    pub fn is_expired(&self) -> bool {
        now_unix() >= self.expires_unix
    }

    pub fn seconds_left(&self) -> u64 {
        self.expires_unix.saturating_sub(now_unix())
    }

    /// `klip1` + lowercase base32 (no padding). Case-insensitive on input.
    pub fn encode(&self) -> String {
        let bytes = postcard::to_stdvec(self).expect("ticket serialization cannot fail");
        format!(
            "{TICKET_PREFIX}{}",
            BASE32_NOPAD.encode(&bytes).to_lowercase()
        )
    }

    pub fn decode(text: &str) -> Result<Self, NetError> {
        // Tolerate whatever a copy/paste brings along: spaces, line breaks, the `│` borders of the
        // terminal card, quotes. Tickets only contain ASCII letters and digits.
        let t: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if t.len() > MAX_TICKET_CHARS {
            return Err(NetError::Ticket("ticket is too long".into()));
        }
        let lower = t.to_lowercase();
        let body = lower
            .strip_prefix(TICKET_PREFIX)
            .ok_or_else(|| NetError::Ticket("missing 'klip1' prefix".into()))?;
        let bytes = BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .map_err(|e| NetError::Ticket(format!("bad encoding: {e}")))?;
        let ticket: PairingTicket = postcard::from_bytes(&bytes)
            .map_err(|e| NetError::Ticket(format!("bad contents: {e}")))?;
        if ticket.version != TICKET_VERSION {
            return Err(NetError::Ticket(format!(
                "unsupported ticket version {}",
                ticket.version
            )));
        }
        Ok(ticket)
    }
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn sample() -> PairingTicket {
        let key = SecretKey::generate();
        PairingTicket::new(EndpointAddr::new(key.public()), [7u8; 16], now_unix() + 120)
    }

    #[test]
    fn roundtrip_and_case_insensitive() {
        let t = sample();
        let s = t.encode();
        assert!(s.starts_with("klip1"));
        let back = PairingTicket::decode(&s).unwrap();
        assert_eq!(back.secret, t.secret);
        assert_eq!(back.addr, t.addr);
        let upper = PairingTicket::decode(&s.to_uppercase()).unwrap();
        assert_eq!(upper.secret, t.secret);
        let spaced = format!("  {}\n", s);
        assert!(PairingTicket::decode(&spaced).is_ok());
    }

    #[test]
    fn survives_being_copied_out_of_the_terminal_card() {
        let t = sample();
        let s = t.encode();
        // The card shows the ticket in 70-character lines inside a box; a mouse selection that
        // spans whole lines brings the borders and indentation along.
        let boxed: String = s
            .as_bytes()
            .chunks(70)
            .map(|c| format!("│   {}   │\n", std::str::from_utf8(c).unwrap()))
            .collect();
        let back = PairingTicket::decode(&boxed).expect("boxed ticket must decode");
        assert_eq!(back.secret, t.secret);
        let quoted = format!("\"{s}\"");
        assert!(PairingTicket::decode(&quoted).is_ok());
    }

    #[test]
    fn rejects_garbage() {
        assert!(PairingTicket::decode("").is_err());
        assert!(PairingTicket::decode("hello").is_err());
        assert!(PairingTicket::decode("klip1!!!!").is_err());
        assert!(PairingTicket::decode(&format!("klip1{}", "a".repeat(5000))).is_err());
        // Truncated ticket must not parse.
        let s = sample().encode();
        assert!(PairingTicket::decode(&s[..s.len() / 2]).is_err());
    }

    #[test]
    fn expiry() {
        let mut t = sample();
        assert!(!t.is_expired());
        t.expires_unix = now_unix().saturating_sub(1);
        assert!(t.is_expired());
        assert_eq!(t.seconds_left(), 0);
    }
}
