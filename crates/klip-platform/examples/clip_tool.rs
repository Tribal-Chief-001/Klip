//! Tiny manual-testing helper for the OS clipboard driver (Linux/macOS/Windows).
//!
//!   cargo run -p klip-platform --example clip_tool -- get
//!   cargo run -p klip-platform --example clip_tool -- set "some text"   # stays alive 15 s on X11
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() {
    use klip_platform::create_default_driver;
    use klip_proto::clip::ClipContentType;
    use klip_proto::hlc::NodeId;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let driver = create_default_driver(NodeId::new([9u8; 32]));
    match args.first().map(String::as_str) {
        Some("get") => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => print!("{t}"),
            Err(e) => {
                eprintln!("cannot read clipboard: {e}");
                std::process::exit(1);
            }
        },
        Some("set") => {
            let text = args.get(1).cloned().unwrap_or_default();
            driver
                .write_clip(&ClipContentType::PlainTextUtf8, text.as_bytes())
                .expect("write clipboard");
            // On X11 the selection is served by this process, so it must stay alive.
            std::thread::sleep(std::time::Duration::from_secs(15));
        }
        _ => eprintln!("usage: clip_tool get | set <text>"),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() {}
