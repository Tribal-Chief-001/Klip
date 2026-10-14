use std::fs;
use std::path::PathBuf;

pub const SYSTEMD_SERVICE_TEMPLATE: &str = r#"[Unit]
Description=Klip Universal Clipboard Daemon
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=klip start --foreground
Restart=on-failure
RestartSec=3
KillMode=process
# Generous ceiling (the daemon uses a few tens of MB); a leak is stopped instead of eating the machine.
MemoryMax=256M
# Cheap hardening that works in the per-user manager without namespaces.
NoNewPrivileges=yes
LockPersonality=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes

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

pub const WINDOWS_TASK_SCRIPT_TEMPLATE: &str = r#"# Klip Daemon Windows Task Scheduler Registration
$Action = New-ScheduledTaskAction -Execute "__EXE_PATH__" -Argument "start --foreground"
$Trigger = New-ScheduledTaskTrigger -AtLogOn
$Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName "KlipDaemon" -Action $Action -Trigger $Trigger -Settings $Settings -Description "Klip Universal E2EE Universal Clipboard Synchronization Daemon" -Force
Write-Host "✓ KlipDaemon Windows Task registered successfully."
"#;

/// `ExecStart` value: quoted so a path with spaces stays one word; `%` is systemd's specifier character.
fn systemd_exec(exe: &str) -> String {
    format!(
        "\"{}\"",
        exe.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn generate_systemd_unit_file(dest: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "klip".to_string());
    let unit = SYSTEMD_SERVICE_TEMPLATE.replace(
        "ExecStart=klip start --foreground",
        &format!("ExecStart={} start --foreground", systemd_exec(&exe_path)),
    );
    fs::write(dest, unit)
}

pub fn generate_launchd_plist_file(dest: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "klip".to_string());
    let plist = LAUNCHD_PLIST_TEMPLATE.replace(
        "<string>klip</string>",
        &format!("<string>{}</string>", xml_escape(&exe_path)),
    );
    fs::write(dest, plist)
}

pub fn generate_windows_service_script(dest: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "klip.exe".to_string());
    let script = WINDOWS_TASK_SCRIPT_TEMPLATE.replace("__EXE_PATH__", &exe_path);
    fs::write(dest, script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awkward_executable_paths_stay_one_argument() {
        assert_eq!(
            systemd_exec("/home/me/My Tools/klip"),
            "\"/home/me/My Tools/klip\""
        );
        assert_eq!(systemd_exec("/a%b\"c"), "\"/a%%b\\\"c\"");
        assert_eq!(xml_escape("/a&b<c>"), "/a&amp;b&lt;c&gt;");
    }
}
