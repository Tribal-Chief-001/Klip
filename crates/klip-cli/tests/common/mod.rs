#![allow(dead_code)]
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

pub const LAN_ONLY: &str = r#"{"relay":"lan","lan_discovery":false}"#;

pub struct Isolated {
    pub dir: PathBuf,
}

impl Isolated {
    /// A private Klip directory with this `config.json`; no daemon is started.
    pub fn create(tag: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("klip-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), config).unwrap();
        Self { dir }
    }

    /// `create`, then `klip start`.
    pub fn start(tag: &str, config: &str) -> Self {
        let me = Self::create(tag, config);
        let out = me.run(&["start"], None);
        assert!(out.status.success(), "start failed: {}", text(&out));
        me
    }

    pub fn env_pairs(&self) -> Vec<(String, String)> {
        vec![
            (
                "KLIP_CONFIG_DIR".to_string(),
                self.dir.display().to_string(),
            ),
            ("KLIP_CLIPBOARD".to_string(), "memory".to_string()),
        ]
    }

    pub fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_klip"));
        cmd.args(args)
            .env("KLIP_CONFIG_DIR", &self.dir)
            .env("KLIP_CLIPBOARD", "memory")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match stdin {
            Some(bytes) => {
                cmd.stdin(Stdio::piped());
                let mut child = cmd.spawn().unwrap();
                let mut sin = child.stdin.take().unwrap();
                let data = bytes.to_vec();
                let writer = std::thread::spawn(move || {
                    let _ = sin.write_all(&data); // the CLI may stop reading early on an error
                });
                let out = child.wait_with_output().unwrap();
                let _ = writer.join();
                out
            }
            None => {
                cmd.stdin(Stdio::null());
                cmd.output().unwrap()
            }
        }
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = self.run(&["stop"], None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}
