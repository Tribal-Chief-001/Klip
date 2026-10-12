//! Counts how many clipboard-change notifications the X server delivers in 10 s (diagnostics).
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
#[tokio::main]
async fn main() {
    use klip_platform::create_default_driver;
    use klip_proto::hlc::NodeId;
    let driver = create_default_driver(NodeId::new([3u8; 32]));
    let Some(mut rx) = driver.change_signal() else {
        println!("no change signal (no X11?)");
        return;
    };
    let start = std::time::Instant::now();
    let mut n = 0u32;
    while start.elapsed() < std::time::Duration::from_secs(10) {
        if tokio::time::timeout(std::time::Duration::from_millis(200), rx.changed())
            .await
            .is_ok()
        {
            n += 1;
        }
    }
    println!("clipboard change notifications in 10 s: {n}");
}
#[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
fn main() {}
