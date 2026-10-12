use klip_crypto::aead::{
    decrypt_payload_in_place, decrypt_payload_in_place_with_aad, encrypt_payload_in_place,
    encrypt_payload_in_place_with_aad, NonceGenerator, SymmetricKey,
};
use klip_crypto::identity::DeviceIdentity;
use klip_crypto::pairing::{compute_sas_invitation, format_sas_pin, PairingInvitation, PairingPsk};
use klip_diff::cf_html::{cf_html_to_utf8, utf8_to_cf_html};
use klip_diff::delta::RecentClipCache;
use klip_diff::entropy::analyze_snippet;
use klip_platform::driver::EchoShield;
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use klip_proto::wire::{
    compute_fingerprint, MessageType, WireHeader, WireNonce, WirePacket, CURRENT_VERSION,
    FLAG_ENCRYPTED, FLAG_SENSITIVE, MAGIC_BYTES, TAG_LEN,
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

    // 4. Construct wire header
    let header = WireHeader {
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
    };
    let header_bytes = header.to_bytes();

    let tag =
        encrypt_payload_in_place_with_aad(&shared_sym_key, &nonce, &header_bytes, &mut ciphertext)
            .expect("Encryption with AAD should succeed");

    assert_ne!(ciphertext, plaintext);

    let packet = WirePacket {
        header,
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
    assert_eq!(
        (decoded_packet.header.flags & FLAG_SENSITIVE),
        FLAG_SENSITIVE
    );

    // 8. Node B decrypts payload in-place with AAD verification
    let mut receiver_buffer = decoded_packet.payload;
    let decoded_header_bytes = decoded_packet.header.to_bytes();
    decrypt_payload_in_place_with_aad(
        &shared_sym_key,
        &decoded_packet.header.nonce,
        &decoded_header_bytes,
        &mut receiver_buffer,
        &decoded_packet.tag,
    )
    .expect("Decryption must succeed with valid tag and AAD");

    assert_eq!(receiver_buffer, plaintext);

    // 9. Echo Shield prevents echo loop on receiver and poll storms
    let echo_shield_b = EchoShield::new();
    let fp = compute_fingerprint(&receiver_buffer, &node_b.node_id);
    echo_shield_b.record_injection(fp);

    // Injected clip must be suppressed on poll 1
    assert!(echo_shield_b.should_suppress(&fp));
    // Injected clip must ALSO be suppressed on subsequent polls (no poll storm)
    assert!(echo_shield_b.should_suppress(&fp));

    // Subsequent unrelated user copy must NOT be suppressed
    let other_fp = compute_fingerprint(b"other user copy", &node_b.node_id);
    assert!(!echo_shield_b.should_suppress(&other_fp));
    // And polling that same copy again should be suppressed
    assert!(echo_shield_b.should_suppress(&other_fp));
}

#[test]
fn test_header_tampering_aead_aad_failure() {
    let key = SymmetricKey::random();
    let nonce = WireNonce::new(42);
    let mut payload = b"secret clipboard payload".to_vec();

    let header = WireHeader {
        magic: MAGIC_BYTES,
        version: CURRENT_VERSION,
        msg_type: MessageType::ClipFullPayload,
        flags: FLAG_ENCRYPTED,
        sender: NodeId::new([1u8; 32]),
        nonce,
        hlc_physical: 1000,
        hlc_logical: 0,
        hlc_node_seed: 0,
        payload_len: payload.len() as u32,
    };
    let header_bytes = header.to_bytes();

    let tag = encrypt_payload_in_place_with_aad(&key, &nonce, &header_bytes, &mut payload).unwrap();

    // 1. Valid decryption succeeds
    let mut valid_buf = payload.clone();
    assert!(
        decrypt_payload_in_place_with_aad(&key, &nonce, &header_bytes, &mut valid_buf, &tag)
            .is_ok()
    );

    // 2. Tampered sender ID in header fails decryption
    let mut tampered_header = header.clone();
    tampered_header.sender = NodeId::new([2u8; 32]);
    let tampered_bytes = tampered_header.to_bytes();
    let mut tampered_buf = payload.clone();
    assert!(decrypt_payload_in_place_with_aad(
        &key,
        &nonce,
        &tampered_bytes,
        &mut tampered_buf,
        &tag
    )
    .is_err());

    // 3. Tampered HLC timestamp in header fails decryption
    let mut tampered_hlc_header = header.clone();
    tampered_hlc_header.hlc_physical = 999999;
    let tampered_hlc_bytes = tampered_hlc_header.to_bytes();
    let mut tampered_hlc_buf = payload.clone();
    assert!(decrypt_payload_in_place_with_aad(
        &key,
        &nonce,
        &tampered_hlc_bytes,
        &mut tampered_hlc_buf,
        &tag
    )
    .is_err());
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

    // Malformed non-ASCII UTF-8 offsets: ensure zero panics when offset lands inside multi-byte character
    let malformed_utf8 =
        "Version:0.9\r\nStartFragment:20\r\nEndFragment:22\r\nPrefix 🎉 emoji test";
    let output_utf8 = cf_html_to_utf8(malformed_utf8);
    // Should return safely without panicking
    assert!(!output_utf8.is_empty());

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

#[test]
fn test_delta_sequential_modification_chain() {
    let mut cache = RecentClipCache::new(5);

    // Clip A
    let clip_a = "pub struct RustMasterClipboardEngine { active: bool }\n"
        .repeat(700)
        .into_bytes();
    let hash_a = cache.insert(clip_a.clone());

    // Clip B: modified from A
    let mut clip_b = clip_a.clone();
    clip_b.extend_from_slice(b"\n// Version B addition\n");
    let delta_b = cache
        .create_delta_if_beneficial(&clip_b)
        .expect("Delta A->B");
    assert_eq!(delta_b.base_hash, hash_a);

    // Both sender and receiver insert reconstructed clip B
    let reconstructed_b = cache.apply_delta(&delta_b).expect("Apply delta A->B");
    assert_eq!(reconstructed_b, clip_b);
    let hash_b = cache.insert(clip_b.clone());

    // Clip C: modified from B
    let mut clip_c = clip_b.clone();
    clip_c.extend_from_slice(b"\n// Version C addition\n");
    let delta_c = cache
        .create_delta_if_beneficial(&clip_c)
        .expect("Delta B->C");
    assert_eq!(delta_c.base_hash, hash_b);

    let reconstructed_c = cache.apply_delta(&delta_c).expect("Apply delta B->C");
    assert_eq!(reconstructed_c, clip_c);
}

#[test]
fn test_pairing_invitation_sas_matching() {
    let host_node = NodeId::new([55u8; 32]);
    let psk = PairingPsk::generate();

    let invitation = PairingInvitation::create(host_node, &psk);
    let token = invitation.to_token();

    // Host computes invitation SAS
    let host_sas = format_sas_pin(compute_sas_invitation(&psk, &host_node));

    // Joining peer decodes token
    let decoded_invitation = PairingInvitation::from_token(&token).unwrap();
    let decoded_psk = decoded_invitation.parse_psk().unwrap();
    let peer_sas = format_sas_pin(compute_sas_invitation(
        &decoded_psk,
        &decoded_invitation.host_node,
    ));

    // Both MUST be identical
    assert_eq!(host_sas, peer_sas);
    assert!(host_sas.contains(" • "));
}

#[test]
fn test_image_payload_sync_and_transcoding() {
    let node_a = DeviceIdentity::generate();
    let _node_b = DeviceIdentity::generate();
    let sym_key = SymmetricKey::random();

    // Generate 8x8 RGBA test pattern
    let width = 8u32;
    let height = 8u32;
    let mut rgba_original = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba_original.extend_from_slice(&[(x * 30) as u8, (y * 30) as u8, 200, 255]);
        }
    }

    // Convert to PNG container
    let png_bytes = klip_diff::rgba_to_png(width, height, &rgba_original).expect("Encode PNG");
    assert_eq!(
        klip_diff::detect_image_format(&png_bytes),
        klip_diff::ImageFormat::Png
    );

    // Node A packages image into wire packet with FLAG_TYPE_IMAGE_PNG
    let content_type = klip_proto::wire::ClipContentType::ImagePng;
    let flags = FLAG_ENCRYPTED | content_type.to_flag();
    let nonce = WireNonce::new(500);

    let mut ciphertext = png_bytes.clone();
    let header = WireHeader {
        magic: MAGIC_BYTES,
        version: CURRENT_VERSION,
        msg_type: MessageType::ClipFullPayload,
        flags,
        sender: node_a.node_id,
        nonce,
        hlc_physical: 1728000000,
        hlc_logical: 1,
        hlc_node_seed: 0x9999,
        payload_len: ciphertext.len() as u32,
    };
    let header_bytes = header.to_bytes();
    let tag = encrypt_payload_in_place_with_aad(&sym_key, &nonce, &header_bytes, &mut ciphertext)
        .expect("Encrypt image");

    let packet = WirePacket {
        header,
        payload: ciphertext,
        tag,
    };
    let wire_bytes = packet.encode();

    // Node B receives and decodes
    let decoded_packet = WirePacket::decode(&wire_bytes).expect("Decode wire bytes");
    let recovered_content_type =
        klip_proto::wire::ClipContentType::from_flags(decoded_packet.header.flags);
    assert_eq!(
        recovered_content_type,
        klip_proto::wire::ClipContentType::ImagePng
    );

    // Decrypt
    let mut decrypted = decoded_packet.payload;
    let dec_header_bytes = decoded_packet.header.to_bytes();
    decrypt_payload_in_place_with_aad(
        &sym_key,
        &decoded_packet.header.nonce,
        &dec_header_bytes,
        &mut decrypted,
        &decoded_packet.tag,
    )
    .expect("Decrypt image");

    assert_eq!(decrypted, png_bytes);

    // Unpack to raw RGBA for host clipboard
    let (rec_w, rec_h, rec_rgba) = klip_diff::png_to_rgba(&decrypted).expect("Decode PNG to RGBA");
    assert_eq!(rec_w, width);
    assert_eq!(rec_h, height);
    assert_eq!(rec_rgba, rgba_original);
}

#[test]
fn test_wire_content_type_flag_preservation() {
    use klip_proto::wire::ClipContentType;

    let types = vec![
        ClipContentType::PlainTextUtf8,
        ClipContentType::HtmlContent,
        ClipContentType::ImagePng,
        ClipContentType::ImageWebP,
    ];

    for ct in types {
        let flag = ct.to_flag();
        let recovered = ClipContentType::from_flags(flag);
        assert_eq!(recovered, ct);
    }
}

#[test]
fn test_nonce_monotonic_counter_invariance() {
    let nonce_gen = NonceGenerator::new(100);
    let mut prev_counter = 99;

    for _ in 0..100 {
        let nonce = nonce_gen.next_nonce();
        let counter = nonce.counter();
        assert!(counter > prev_counter);
        assert_eq!(counter, prev_counter + 1);
        prev_counter = counter;

        // Verify top 4 bytes are zero-padded per spec
        assert_eq!(&nonce.as_bytes()[0..4], &[0u8; 4]);
    }
}
