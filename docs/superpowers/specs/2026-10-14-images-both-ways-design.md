# Images both ways, including Android

Status: written 2026-10-14 while the owner was away. The owner gave blanket permission ("full
permission, one after another, I will not reply") for brainstorming, spec, plan, build and release
without asking. Every decision below that the owner would normally make is marked **Ruling** and
repeated in the release notes.

## Problem

Klip syncs text. Image clips are half supported: the laptop can send and receive PNG/WebP (since
v0.4.2 without silent failures), but the Android app refuses every image (`BridgeDriver` returns
`UnsupportedFormat`) and cannot send one. A screenshot copied on the laptop never reaches the phone;
a photo or screenshot on the phone cannot be sent to the laptop except by saving and moving a file.

## Success criteria

1. Copy an image on the laptop: it is on the phone's clipboard and pastes into other apps
   (messaging, notes) without extra steps.
2. On the phone, share an image to Klip (Share sheet), or copy an image and tap the Klip tile or the
   notification button: it is on the laptop's clipboard.
3. Hostile or broken images from a paired device cannot exhaust memory or disk on either side.
4. Nothing leaks: no EXIF/GPS from phone photos, no files left behind on the phone.
5. A failure is reported truthfully to the sender (no "delivered" for an image that was not applied).
6. No change to the wire format or the control-socket protocol; older devices keep working for text.

## Decisions

- **Wire format unchanged.** Only `ClipContentType::ImagePng` and `ImageWebP` are used. JPEG, HEIC and
  everything else are converted to PNG on the phone before sending. Adding a JPEG decoder to the
  Rust side would cost binary size (the build is at 8.26 MB of an 8.5 MB budget) and a new enum
  variant would be an unknown value for older receivers.
- **Ruling: size limits.** The Android engine accepts and sends up to 12,000,000 bytes (was 1,000,000
  since v0.4.2, which existed because images were refused). Text over about 900 KB is still refused
  by the phone, after being downloaded (cost: at most 12 MB of wasted transfer, only for giant text).
  The laptop keeps 32 MiB.
- **Ruling: pixel limit.** Any image with more than 36,000,000 pixels (about 8K) is rejected before it
  is decoded or written, on every platform (a 36 Mpx image is 144 MB as RGBA). Header-only check
  (`image::ImageReader::into_dimensions`), so a decompression bomb costs a few bytes of reading.
- **Ruling: phone photos are downscaled, not refused.** If the PNG of a converted image exceeds the
  limit, the image is scaled by 0.75 and re-encoded, up to 6 times, then the send fails with a clear
  message. The longest side is first capped at 4096 px while decoding (`inSampleSize`) so a 50 MP
  photo cannot out-of-memory the app. A PNG/WebP that already fits is sent unchanged (no quality
  loss, no re-encode).
- **EXIF/GPS.** Decoding to a `Bitmap` and re-encoding drops all metadata. A PNG/WebP sent
  unchanged keeps whatever it contains; screenshots (the common case) contain none.
- **Receive validation (core, all platforms).** Before any driver write, an incoming
  `ImagePng`/`ImageWebP` must have the matching magic bytes and a header whose dimensions are
  within the pixel limit; otherwise the outcome is `Rejected` (the sender sees "rejected").
- **Android receive.** The engine calls a new bridge method
  `ClipboardBridge.write_image(data, mime) -> bool`. The app writes the bytes to
  `cacheDir/clips/klip-<millis>.<png|webp>` (temp file then rename), exposes it through a
  `FileProvider` (`<applicationId>.clips`, `grantUriPermissions`), and puts `ClipData.newUri(...)` on
  the clipboard. The clipboard holds a URI, so the Binder size limit that restricts text does not
  apply. Files older than 60 minutes are deleted on every write and at service start, and at most
  4 are kept.
- **Ruling: no tap-to-copy fallback for images.** If the system refuses the clipboard write the
  sender is told "could not write its clipboard" (as for text without the notification fallback,
  which is limited to small text).
- **Android send.** Rust: `KlipEngine::send_image(data) -> SendReport` (magic + pixel check, then
  `broadcast_local_clip`). Kotlin: `ImageSender` prepares bytes from a `Uri`; entry points are a new
  Share-sheet activity for `image/*` (`ACTION_SEND`, first image only), and the existing clipboard
  trampoline (tile and notification button), which now also accepts an image URI on the clipboard when
  there is no text. Automatic sending of images is not done (Android does not allow reading the
  clipboard in the background).
- **Reading the shared stream.** The Share activity reads the stream bytes inside `onCreate` (the
  read permission granted by the sharing app can end when the activity finishes), capped at 32 MB raw,
  then hands the bytes to a worker thread.
- **Laptop read guard.** `LinuxClipboardDriver::read_clip` skips an image above the pixel limit
  instead of encoding it (saves a 150 MB PNG encode of something that would be rejected anyway).
- **Not done (and why).** Animated GIF, several images at once, files, thumbnails in history, JPEG
  on the wire, macOS/Windows image-write changes (never run), tap-to-copy for images, automatic
  phone-to-laptop image capture.

## Design

### klip-diff
`pub const MAX_IMAGE_PIXELS: u64 = 36_000_000;`
`pub fn validate_image(expected: ImageFormat, bytes: &[u8]) -> Result<(u32, u32), String>` using
`detect_image_format` (magic) and `image::ImageReader::with_format(..).into_dimensions()`.
Errors: "not a PNG/WebP image", "image is W x H pixels; the limit is 36000000".

### klip-core (`sync.rs`)
`apply_remote_clip`: for `ImagePng`/`ImageWebP` call `validate_image` before the driver write; on error
return `ClipOutcome::Rejected`. New helper `image_format_of(&ClipContentType) -> Option<ImageFormat>`.

### klip-platform
`MemoryClipboardDriver`: `user_copy_bytes(content_type, data)` and `current_clip()` for tests.
`LinuxClipboardDriver::read_clip`: skip when `width*height > MAX_IMAGE_PIXELS`.

### klip-android
`ClipboardBridge::write_image(&self, data: Vec<u8>, mime: String) -> bool`; `BridgeDriver` writes
`ImagePng`/`ImageWebP` through it (mime `image/png`/`image/webp`) after recording the echo
fingerprint; `KlipEngine::start` sets `max_clip_bytes = 12_000_000`; `KlipEngine::send_image`.

### Android app
`ClipboardWriter.writeImage`; `ClipImages` (cache dir, prune, URI); `ImageSender` (prepare/convert);
`ShareImageActivity`; `CopyTrampolineActivity` image branch; `SendHelper.sendImage`; manifest
`<provider>` + activity; `res/xml/clip_paths.xml`; README/Privacy text.

## Errors and edge cases

- Huge dimensions in a tiny file: rejected by header check, no decode.
- PNG bytes labelled WebP (or the reverse): rejected.
- Truncated image: header parses but the pasting app fails; acceptable (the paired device is
  trusted; the pixel limit bounds the damage).
- Phone storage full: write fails, sender is told.
- Two images in quick succession: the newer file replaces the clipboard; the older stays until
  pruned (still readable by an app that already holds the URI).
- Share activity killed before sending: nothing is sent, nothing is stored.
- Older phone app (v0.4.x) receiving an image: refuses with "could not write its clipboard" as
  before.

## Tests

1. klip-diff: PNG ok; WebP ok; WebP bytes declared PNG rejected; 60000x60000 IHDR rejected; garbage
   and empty rejected.
2. klip-core/daemon e2e: bomb and mislabelled image sent to a daemon are `Rejected` and never reach the
   clipboard; a valid PNG copied on A arrives byte-for-byte on B over the real network (and is not
   echoed back).
3. klip-platform: pixel guard function.
4. klip-android: image from engine 1 arrives at engine 2's bridge with the same bytes and mime;
   JPEG bytes and a 13 MB blob are refused with clear errors.
5. Android app: compiled by Gradle; run on the emulator against an isolated desktop daemon
   (`KLIP_CONFIG_DIR`): share an image from the emulator to the laptop, and send a laptop image to the
   emulator and check the clipboard URI and the cached file. Never against the owner's real phone.

## Not verified until built

Pasting the received image into a third-party app on a real phone; behaviour on Android 15/16; the
Vivo's own clipboard handling of URI clips.
