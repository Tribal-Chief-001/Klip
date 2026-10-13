//! File-based helpers: the daemon's config and log live in the Klip directory and are read directly
//! (same user, private permissions), so these screens work even when the daemon is not running.
use klip_core::{DaemonConfig, KlipPaths};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The log is read from this far back when the Log screen opens.
pub const LOG_INITIAL_BYTES: u64 = 256 * 1024;
/// A line longer than this without a newline is shown anyway instead of waiting forever.
const LOG_MAX_PARTIAL: usize = 64 * 1024;
/// Send's "load a file" refuses anything bigger.
pub const MAX_LOADED_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogChunk {
    /// Where the next read starts.
    pub new_offset: u64,
    /// Complete lines only (each ends with `\n`), lossily decoded.
    pub text: String,
    /// The file became shorter than the previous offset (truncated or rotated): start over.
    pub reset: bool,
    /// There is no log file (yet).
    pub missing: bool,
}

pub fn log_path(paths: &KlipPaths) -> PathBuf {
    paths.dir().join("daemon.log")
}

/// Read what was appended to the log since `offset`; `None` means "first read": the last 256 KiB,
/// starting at a line boundary. An unfinished last line is left for the next read.
pub fn read_log_chunk(path: &Path, offset: Option<u64>) -> Result<LogChunk, String> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LogChunk {
                new_offset: 0,
                text: String::new(),
                reset: offset.is_some(),
                missing: true,
            })
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let (start, reset) = match offset {
        None => (len.saturating_sub(LOG_INITIAL_BYTES), false),
        Some(o) if o > len => (0, true),
        Some(o) => (o, false),
    };
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.take(len - start)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;

    // First read in the middle of a line: drop the partial first line.
    let mut skip = 0;
    if offset.is_none() && start > 0 {
        skip = buf
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(buf.len());
    }
    let data = &buf[skip..];
    let take = match data.iter().rposition(|b| *b == b'\n') {
        Some(i) => i + 1,
        None if data.len() >= LOG_MAX_PARTIAL => data.len(),
        None => 0,
    };
    Ok(LogChunk {
        new_offset: start + (skip + take) as u64,
        text: String::from_utf8_lossy(&data[..take]).into_owned(),
        reset,
        missing: false,
    })
}

/// Validate, then write `config.json`: temp file (mode 0600) -> fsync -> rename over the target.
/// The old file stays untouched when validation or any write step fails.
pub fn write_config_atomic(paths: &KlipPaths, cfg: &DaemonConfig) -> Result<(), String> {
    cfg.validate()?;
    let target = paths.config_file();
    let tmp = paths.dir().join(".config.json.tmp");
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())? + "\n";
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    let written = file
        .write_all(json.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {e}", target.display())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    /// This OS has no unit file the CLI knows how to install.
    Unsupported,
    NotInstalled,
    Installed(PathBuf),
}

/// Where `klip install-service` writes its unit file on this OS.
pub fn service_unit_path(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        Some(home.join(".config/systemd/user/klip.service"))
    } else if cfg!(target_os = "macos") {
        Some(home.join("Library/LaunchAgents/com.klip.daemon.plist"))
    } else {
        None
    }
}

pub fn service_state(home: Option<&Path>) -> ServiceState {
    match home.and_then(service_unit_path) {
        None => ServiceState::Unsupported,
        Some(p) if p.exists() => ServiceState::Installed(p),
        Some(_) => ServiceState::NotInstalled,
    }
}

/// Read a UTF-8 text file for the Send screen. `~/` is expanded; directories, files over `max_bytes`
/// and non-UTF-8 content are refused with a plain message.
pub fn load_text_file(path: &str, max_bytes: u64) -> Result<String, String> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(path),
        }
    } else {
        PathBuf::from(path)
    };
    let meta = fs::metadata(&expanded).map_err(|e| format!("{}: {e}", expanded.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", expanded.display()));
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "{} is too large ({} bytes; the limit is {max_bytes})",
            expanded.display(),
            meta.len()
        ));
    }
    let bytes = fs::read(&expanded).map_err(|e| format!("{}: {e}", expanded.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8 text", expanded.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use klip_core::{DaemonConfig, KlipPaths, RelayMode};
    use std::io::Write;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klip-tui-files-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn config_is_written_atomically_with_mode_0600_and_reads_back() {
        let paths = KlipPaths::from_dir(tmp("cfg"));
        let cfg = DaemonConfig {
            device_name: "Laptop".into(),
            relay: RelayMode::Lan,
            max_history_items: 20,
            ..DaemonConfig::default()
        };
        write_config_atomic(&paths, &cfg).unwrap();
        let back = DaemonConfig::load(&paths).unwrap();
        assert_eq!(back.device_name, "Laptop");
        assert_eq!(back.relay, RelayMode::Lan);
        assert_eq!(back.max_history_items, 20);
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(paths.config_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(
            !paths.dir().join(".config.json.tmp").exists(),
            "temp file must be renamed away"
        );
    }

    #[test]
    fn an_invalid_config_is_refused_and_the_old_file_is_untouched() {
        let paths = KlipPaths::from_dir(tmp("badcfg"));
        write_config_atomic(&paths, &DaemonConfig::default()).unwrap();
        let before = fs::read_to_string(paths.config_file()).unwrap();
        let bad = DaemonConfig {
            max_clip_bytes: 0,
            ..DaemonConfig::default()
        };
        assert!(write_config_atomic(&paths, &bad).is_err());
        assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), before);
    }

    #[test]
    fn log_first_read_returns_the_tail_from_a_line_boundary() {
        let dir = tmp("log1");
        let path = dir.join("daemon.log");
        let mut f = fs::File::create(&path).unwrap();
        for i in 0..20_000 {
            writeln!(f, "line number {i:06} with some padding to make it longer").unwrap();
        }
        let chunk = read_log_chunk(&path, None).unwrap();
        assert!(!chunk.missing && !chunk.reset);
        assert!(chunk.text.len() as u64 <= LOG_INITIAL_BYTES);
        assert!(
            chunk.text.starts_with("line number "),
            "starts mid-line: {:?}",
            &chunk.text[..30]
        );
        assert!(chunk.text.ends_with('\n'));
        assert_eq!(chunk.new_offset, fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn log_follow_returns_only_new_complete_lines() {
        let dir = tmp("log2");
        let path = dir.join("daemon.log");
        fs::write(&path, "one\ntwo\n").unwrap();
        let first = read_log_chunk(&path, None).unwrap();
        assert_eq!(first.text, "one\ntwo\n");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"three\npart").unwrap();
        let second = read_log_chunk(&path, Some(first.new_offset)).unwrap();
        assert_eq!(
            second.text, "three\n",
            "an unfinished line waits for its newline"
        );
        f.write_all(b"ial\n").unwrap();
        let third = read_log_chunk(&path, Some(second.new_offset)).unwrap();
        assert_eq!(third.text, "partial\n");
    }

    #[test]
    fn log_truncation_and_missing_file_are_reported_not_fatal() {
        let dir = tmp("log3");
        let path = dir.join("daemon.log");
        let missing = read_log_chunk(&path, None).unwrap();
        assert!(missing.missing && missing.text.is_empty());
        fs::write(&path, "a long first line that will be cut\n").unwrap();
        let first = read_log_chunk(&path, None).unwrap();
        fs::write(&path, "new\n").unwrap();
        let again = read_log_chunk(&path, Some(first.new_offset)).unwrap();
        assert!(again.reset);
        assert_eq!(again.text, "new\n");
    }

    #[test]
    fn log_with_binary_garbage_is_converted_lossily() {
        let dir = tmp("log4");
        let path = dir.join("daemon.log");
        fs::write(&path, b"ok\n\xff\xfe\x00bad\n").unwrap();
        let chunk = read_log_chunk(&path, None).unwrap();
        assert!(chunk.text.contains("ok\n") && chunk.text.contains("bad"));
    }

    #[test]
    fn service_state_follows_the_unit_file() {
        let home = tmp("svc");
        assert_eq!(service_state(Some(&home)), ServiceState::NotInstalled);
        let unit = service_unit_path(&home).expect("linux/macOS have a unit path");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        fs::write(&unit, "[Unit]\n").unwrap();
        assert_eq!(service_state(Some(&home)), ServiceState::Installed(unit));
        assert_eq!(service_state(None), ServiceState::Unsupported);
    }

    #[test]
    fn text_files_load_with_size_and_encoding_checks() {
        let dir = tmp("load");
        let ok = dir.join("a.txt");
        fs::write(&ok, "héllo\n").unwrap();
        assert_eq!(
            load_text_file(ok.to_str().unwrap(), 1000).unwrap(),
            "héllo\n"
        );
        let big = dir.join("big.txt");
        fs::write(&big, vec![b'x'; 2000]).unwrap();
        assert!(load_text_file(big.to_str().unwrap(), 1000)
            .unwrap_err()
            .contains("too large"));
        let bin = dir.join("bin");
        fs::write(&bin, [0xffu8, 0xfe, 0x00]).unwrap();
        assert!(load_text_file(bin.to_str().unwrap(), 1000)
            .unwrap_err()
            .contains("UTF-8"));
        assert!(load_text_file(dir.join("nope").to_str().unwrap(), 1000).is_err());
        assert!(
            load_text_file(dir.to_str().unwrap(), 1000).is_err(),
            "a directory is not a file"
        );
    }
}
