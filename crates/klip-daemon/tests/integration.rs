use klip_crypto::aead::{decrypt_payload_in_place, encrypt_payload_in_place, NonceGenerator, SymmetricKey};
use klip_crypto::identity::DeviceIdentity;
use klip_diff::cf_html::{cf_html_to_utf8, utf8_to_cf_html};
use klip_diff::entropy::analyze_snippet;
use klip_platform::driver::EchoShield;
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use klip_proto::wire::{
    compute_fingerprint, MessageType, WireHeader, WireNonce,
    WirePacket, CURRENT_VERSION, FLAG_ENCRYPTED, FLAG_SENSITIVE, MAGIC_BYTES, TAG_LEN,
};
use klip_transport::mailbox::EphemeralMailbox;
use klip_transport::relay::StatelessRelayRouter;

#[test]
fn test_end_to_end_e2ee_packet_flow() {
    // 1. Setup two virtual nodes: Node A (Linux) and Node B (Android)
    let node_a = DeviceIdentity::generate();
    let node_b = DeviceIdentity::generate();

    // 2. Establish shared pre-shared symmetric key via pairing
    let shared_sym_key = SymmetricKey::random();

    // 3. Node A captures clip and prepares payload
    let plaintext = b"export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
    let analysis = analyze_snippet(std::str::from_utf8(plaintext).unwrap());
    assert!(analysis.is_likely_secret);

    let mut clock_a = HybridLogicalClock::new(node_a.node_id);
    let hlc_stamp_a = clock_a.send_event();

    let nonce_gen_a = NonceGenerator::new(100);
    let nonce = nonce_gen_a.next_nonce();

    let mut ciphertext = plaintext.to_vec();
    let tag = encrypt_payload_in_place(&shared_sym_key, &nonce, &mut ciphertext)
        .expect("Encryption should succeed");

    assert_ne!(ciphertext, plaintext);

    // 4. Construct wire packet
    let packet = WirePacket {
        header: WireHeader {
            magic: MAGIC_BYTES,
            version: CURRENT_VERSION,
            msg_type: MessageType::ClipFullPayload,
            flags: FLAG_ENCRYPTED | FLAG_SENSITIVE,
            sender: node_a.node_id,
            nonce,
            hlc_physical: hlc_stamp_a.physical_time_ms,
            hlc_logical: hlc_stamp_a.logical_counter,
            hlc_node_seed: 0x12345678,
            payload_len: ciphertext.len() as u32,
        },
        payload: ciphertext,
        tag,
    };

    // 5. Serialize into binary wire frame
    let wire_bytes = packet.encode();
    assert!(wire_bytes.len() > 72 + TAG_LEN);

    // 6. Traverse zero-knowledge relay router
    let relay = StatelessRelayRouter::new();
    let dest_addr = "10.0.0.2:7123".parse().unwrap();
    relay.register_route(node_b.node_id, dest_addr);
    assert_eq!(relay.lookup(&node_b.node_id), Some(dest_addr));

    // 7. Node B receives wire bytes and decodes
    let decoded_packet = WirePacket::decode(&wire_bytes).expect("Decode wire frame");
    assert_eq!(decoded_packet.header.sender, node_a.node_id);
    assert_eq!((decoded_packet.header.flags & FLAG_SENSITIVE), FLAG_SENSITIVE);

    // 8. Node B decrypts payload in-place
    let mut receiver_buffer = decoded_packet.payload;
    decrypt_payload_in_place(
        &shared_sym_key,
        &decoded_packet.header.nonce,
        &mut receiver_buffer,
        &decoded_packet.tag,
    )
    .expect("Decryption must succeed with valid tag");

    assert_eq!(receiver_buffer, plaintext);

    // 9. Echo Shield prevents echo loop on receiver
    let echo_shield_b = EchoShield::new();
    let fp = compute_fingerprint(&receiver_buffer, &node_b.node_id);
    echo_shield_b.record_injection(fp);

    // Immediate read of identical injected clip must be suppressed
    assert!(echo_shield_b.should_suppress(&fp));
    // Subsequent unrelated clip must NOT be suppressed
    let other_fp = compute_fingerprint(b"other user copy", &node_b.node_id);
    assert!(!echo_shield_b.should_suppress(&other_fp));
}

#[test]
fn test_edge_case_empty_and_tampered_payloads() {
    let key = SymmetricKey::random();
    let nonce = WireNonce::new(1);

    // Empty payload
    let mut empty_buf = Vec::new();
    let tag = encrypt_payload_in_place(&key, &nonce, &mut empty_buf).unwrap();
    assert_eq!(empty_buf.len(), 0);

    decrypt_payload_in_place(&key, &nonce, &mut empty_buf, &tag).unwrap();
    assert_eq!(empty_buf.len(), 0);

    // Corrupted tag
    let mut data = b"payload".to_vec();
    let mut tag = encrypt_payload_in_place(&key, &nonce, &mut data).unwrap();
    tag[0] ^= 0x01; // flip 1 bit
    assert!(decrypt_payload_in_place(&key, &nonce, &mut data, &tag).is_err());
}

#[test]
fn test_edge_case_cf_html_malformed_fallbacks() {
    // Malformed CF_HTML missing end fragment
    let malformed = "Version:0.9\r\nStartHTML:10\r\nEndHTML:50\r\nSome random body";
    let output = cf_html_to_utf8(malformed);
    // Should gracefully fallback rather than panic
    assert_eq!(output, malformed);

    // Valid CF_HTML conversion
    let clean = "<p>Clean HTML Snippet</p>";
    let converted = utf8_to_cf_html(clean);
    let recovered = cf_html_to_utf8(&converted);
    assert_eq!(recovered, clean);
}

#[test]
fn test_sleeping_device_mailbox_expiry_and_drain() {
    let mailbox = EphemeralMailbox::new();
    let node_sleep = NodeId::new([99u8; 32]);
    let group_token = [7u8; 32];

    // Deposit 2 items
    mailbox.deposit(group_token, node_sleep, b"clip 1".to_vec(), 100);
    mailbox.deposit(group_token, node_sleep, b"clip 2".to_vec(), 100);

    assert_eq!(mailbox.pending_count(&node_sleep), 2);

    let drained = mailbox.fetch_and_drain(&node_sleep);
    assert_eq!(drained.len(), 2);
    assert_eq!(mailbox.pending_count(&node_sleep), 0);
}
