# Images both ways Implementation Plan

> **For agentic workers:** executed inline by the author (owner away, blanket permission). Steps use checkbox syntax.

**Goal:** images copied on the laptop reach the phone's clipboard; images shared or copied on the phone reach the laptop, with hostile-input limits on both sides.

**Architecture:** wire format unchanged (PNG/WebP). Header-only validation in `klip-diff`, enforced in `klip-core` before any clipboard write. A new `ClipboardBridge::write_image` and `KlipEngine::send_image` in `klip-android`; the app stores received images in a FileProvider-backed cache and converts outgoing images to PNG.

**Tech Stack:** Rust (image 0.25), UniFFI, Kotlin framework views, androidx.core FileProvider.

**Spec:** `docs/superpowers/specs/2026-10-14-images-both-ways-design.md`

## Global Constraints

- No new dependency. No change to `ClipContentType`, the IPC types or the wire format.
- Pixel limit 36,000,000; Android engine byte limit 12,000,000; laptop 32 MiB unchanged.
- `cargo fmt --check`, `clippy --workspace --all-targets -D warnings` clean; one heavy command at a time (`-j2`, `nice -n 10`, `./gradlew --stop` after).
- Never touch the owner's real phone: emulator only (`adb -s emulator-5554`); isolated test daemon with `KLIP_CONFIG_DIR`.
- Binary size must stay under the CI budget (8,912,896 bytes).

## Review Focus

- Decompression bomb header (60000x60000) and PNG/WebP mislabel: rejected with no decode. (Task 1, 2)
- An image that is exactly at, and one pixel above, the limit. (Task 1)
- Echo: an image written by Klip is not captured back. (Task 2 e2e)
- Phone storage hygiene: pruning keeps at most 4 files and removes old ones. (Task 5, emulator)
- Share activity reading the stream before finishing. (Task 5, emulator)

### Task 1: klip-diff validation
Files: `crates/klip-diff/src/image_transcode.rs`, `lib.rs`. Produces `MAX_IMAGE_PIXELS`, `validate_image(ImageFormat, &[u8]) -> Result<(u32,u32), String>`.
- [ ] Write tests (PNG ok, WebP ok, mislabel, bomb, exactly-limit ok, limit+1 rejected, garbage, empty); run: compile error.
- [ ] Implement with `image::ImageReader::with_format(Cursor::new(bytes), fmt).into_dimensions()` and the `detect_image_format` magic check; tests pass.
- [ ] Commit.

### Task 2: klip-core enforcement and e2e
Files: `crates/klip-core/src/sync.rs` (`apply_remote_clip`), `crates/klip-platform/src/memory.rs` (`user_copy_bytes`, `current_clip`), `crates/klip-daemon/tests/e2e.rs`.
- [ ] Tests: bomb and mislabel -> `ClipOutcome::Rejected`, clipboard unchanged; valid PNG A->B byte-for-byte over the network and not echoed (A's `synced` count / B sends nothing back).
- [ ] Implement validation before the driver write; memory driver helpers.
- [ ] Commit.

### Task 3: Linux read guard
Files: `crates/klip-platform/src/linux.rs`. Pure `fn too_many_pixels(w, h) -> bool` used before `rgba_to_png`; unit test.
- [ ] Test, implement, commit.

### Task 4: klip-android
Files: `crates/klip-android/src/lib.rs`, `tests/engine.rs`. `ClipboardBridge::write_image`, `BridgeDriver`, `max_clip_bytes = 12_000_000`, `KlipEngine::send_image`.
- [ ] Engine tests: image engine1->engine2 bridge (bytes + mime), JPEG refused, 13 MB refused.
- [ ] Implement; clippy; commit.

### Task 5: Android app
Files: `ClipboardWriter.kt`, new `ClipImages.kt`, `ImageSender.kt`, `ShareImageActivity.kt`, `CopyTrampolineActivity.kt`, `SendHelper.kt`, `AndroidManifest.xml`, `res/xml/clip_paths.xml`.
- [ ] Implement per spec; `gradlew assembleRelease` compiles; commit.

### Task 6: Verify, ship
- [ ] Version 0.5.0 / versionCode 15; snapshots; README/docs; fmt, clippy, full tests.
- [ ] Final review by one fresh Sonnet reviewer; fix pass for Critical/Important.
- [ ] Commit, push, tag `v0.5.0-alpha`; local release binary + signed APKs; install the binary with `systemctl --user restart klip.service`.
- [ ] Emulator end-to-end against an isolated desktop daemon; record results honestly.
- [ ] Release notes (rulings + not verified), memory update.
