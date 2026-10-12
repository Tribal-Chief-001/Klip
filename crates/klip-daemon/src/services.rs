use std::fs;
use std::path::PathBuf;

pub const SYSTEMD_SERVICE_TEMPLATE: &str = r#"[Unit]
Description=Klip Universal Clipboard Daemon
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=klip start --foreground
Restart=always
RestartSec=3
KillMode=process
MemoryMax=32M

[Install]
WantedBy=graphical-session.target
"#;

pub const LAUNCHD_PLIST_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.klip.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>klip</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/klip.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/klip.err</string>
</dict>
</plist>
"#;

pub fn generate_systemd_unit_file(dest: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "klip".to_string());
    let unit = SYSTEMD_SERVICE_TEMPLATE.replace("ExecStart=klip start --foreground", &format!("ExecStart={} start --foreground", exe_path));
    fs::write(dest, unit)
}

pub fn generate_launchd_plist_file(dest: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "klip".to_string());
    let plist = LAUNCHD_PLIST_TEMPLATE.replace("<string>klip</string>", &format!("<string>{}</string>", exe_path));
    fs::write(dest, plist)
}
