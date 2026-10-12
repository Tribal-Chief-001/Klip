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
    fs::write(dest, SYSTEMD_SERVICE_TEMPLATE)
}

pub fn generate_launchd_plist_file(dest: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dest, LAUNCHD_PLIST_TEMPLATE)
}
