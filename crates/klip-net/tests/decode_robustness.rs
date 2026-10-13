//! Everything a stranger can put on the wire must fail cleanly, never panic or allocate wildly:
//! random bytes, truncations and single-bit corruptions of valid messages.

use klip_net::PairingTicket;
use klip_proto::clip::{ClipContentType, ClipPayload};
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};

fn valid_payload() -> Vec<u8> {
    let node = NodeId::new([7u8; 32]);
    let hlc = HybridLogicalClock::new(node).send_event();
    let p = ClipPayload::new(
        ClipContentType::PlainTextUtf8,
        b"hello \xF0\x9F\x98\x80 world".to_vec(),
        node,
        hlc,
        false,
    );
    postcard::to_stdvec(&p).unwrap()
}

#[test]
fn random_bytes_never_crash_the_clip_decoder() {
    let mut rng = StdRng::seed_from_u64(0xC11B);
    for _ in 0..50_000 {
        let len = rng.gen_range(0..300);
        let mut buf = vec![0u8; len];
        rng.fill_bytes(&mut buf);
        let _ = postcard::from_bytes::<ClipPayload>(&buf);
    }
}

#[test]
fn corrupted_and_truncated_clips_never_crash() {
    let good = valid_payload();
    assert!(postcard::from_bytes::<ClipPayload>(&good).is_ok());
    for cut in 0..good.len() {
        assert!(
            postcard::from_bytes::<ClipPayload>(&good[..cut]).is_err(),
            "a truncated clip (len {cut}) must not decode"
        );
    }
    for pos in 0..good.len() {
        for bit in 0..8 {
            let mut bad = good.clone();
            bad[pos] ^= 1 << bit;
            let _ = postcard::from_bytes::<ClipPayload>(&bad);
        }
    }
    // A length prefix that claims gigabytes must fail without allocating them.
    let mut huge = good.clone();
    huge.splice(0..0, [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    let _ = postcard::from_bytes::<ClipPayload>(&huge);
}

#[test]
fn random_and_mutated_tickets_never_crash() {
    let mut rng = StdRng::seed_from_u64(0x71C4);
    for _ in 0..20_000 {
        let len = rng.gen_range(0..120);
        let text: String = (0..len)
            .map(|_| match rng.gen_range(0..4) {
                0 => char::from(rng.gen_range(b'a'..=b'z')),
                1 => char::from(rng.gen_range(b'2'..=b'7')),
                2 => ' ',
                _ => char::from_u32(rng.gen_range(0..0x3000)).unwrap_or('?'),
            })
            .collect();
        let _ = PairingTicket::decode(&text);
        let _ = PairingTicket::decode(&format!("klip1{text}"));
    }
    for junk in [
        "",
        "klip1",
        "KLIP1",
        "klip1====",
        "klip1\u{0}",
        "ß",
        "klip1ßß",
        &"a".repeat(100_000),
    ] {
        assert!(PairingTicket::decode(junk).is_err(), "{junk:?}");
    }
}
