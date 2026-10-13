#![cfg(target_os = "linux")]
use std::process::Command;

#[test]
fn install_service_writes_the_systemd_unit_into_home_and_says_what_to_run() {
    let home = std::env::temp_dir().join(format!("klip-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_klip"))
        .arg("install-service")
        .env("HOME", &home)
        .env("KLIP_CONFIG_DIR", home.join("cfg"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("Installed systemd unit:"), "{text}");
    assert!(
        text.contains("systemctl --user enable --now klip.service"),
        "{text}"
    );
    let unit = home.join(".config/systemd/user/klip.service");
    let body = std::fs::read_to_string(&unit).expect("unit file written");
    assert!(body.contains("ExecStart="), "{body}");
}
