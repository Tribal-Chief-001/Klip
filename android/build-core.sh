#!/usr/bin/env bash
# Build the Rust engine for Android and generate the Kotlin bindings.
#
#   android/build-core.sh                 # all ABIs
#   android/build-core.sh arm64-v8a       # one ABI (faster, enough for most phones since 2017)
#
# Needs: rustup targets (aarch64-linux-android armv7-linux-androideabi x86_64-linux-android),
# `cargo install cargo-ndk`, and ANDROID_NDK_HOME pointing at NDK r28+ (16 KB page-size ready).
set -euo pipefail
cd "$(dirname "$0")/.."

: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME, e.g. \$HOME/Android/Sdk/ndk/28.2.13676358}"
ABIS=("$@")
[ ${#ABIS[@]} -eq 0 ] && ABIS=(arm64-v8a armeabi-v7a x86_64)

targets=()
for abi in "${ABIS[@]}"; do targets+=(-t "$abi"); done

JNI_DIR=android/app/src/main/jniLibs
rm -rf "$JNI_DIR"
cargo ndk "${targets[@]}" -o "$JNI_DIR" build -p klip-android --profile android
# iroh's crates are also cdylibs, so cargo-ndk copies their standalone .so files too. They are not
# needed (libklip_android.so already contains everything) and only bloat the APK.
find "$JNI_DIR" -name '*.so' ! -name 'libklip_android.so' -delete

# Bindings are generated from an unstripped host build (UniFFI reads metadata from the symbols).
cargo build -p klip-android
HOST_LIB=target/debug/libklip_android.so
[ -f "$HOST_LIB" ] || HOST_LIB=target/debug/libklip_android.dylib
rm -rf android/app/src/main/java/uniffi
cargo run -q -p klip-android --features bindgen --bin uniffi-bindgen -- \
  generate --library "$HOST_LIB" --language kotlin \
  --out-dir android/app/src/main/java --no-format

echo "Native libraries:"; find "$JNI_DIR" -name '*.so' -exec ls -lh {} \;
echo "Kotlin bindings:"; find android/app/src/main/java/uniffi -name '*.kt'
