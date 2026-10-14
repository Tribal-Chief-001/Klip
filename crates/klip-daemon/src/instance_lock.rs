use crate::config::KlipPaths;
use std::fs::{File, OpenOptions, TryLockError};

/// Exclusive lock that makes a configuration directory single-instance.
///
/// Two daemons with the same keystore would share one device identity (fighting over the relay
/// connection) and the second would replace the first one's control socket. The lock is held for
/// the lifetime of the process and released by the OS when it exits, even after a crash.
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(paths: &KlipPaths) -> Result<Self, String> {
        let path = paths.dir().join("daemon.lock");
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts
            .open(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(format!(
                "Klip is already running with this configuration ({}). Use `klip status`, or `klip stop` first.",
                paths.dir().display()
            )),
            Err(TryLockError::Error(e)) => Err(format!("cannot lock {}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_refused_until_the_first_is_dropped() {
        let dir = std::env::temp_dir().join(format!("klip-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = KlipPaths::from_dir(&dir);
        let first = InstanceLock::acquire(&paths).expect("first lock");
        let err = InstanceLock::acquire(&paths)
            .err()
            .expect("second must fail");
        assert!(err.contains("already running"), "{err}");
        drop(first);
        // Another test thread may have a child process between fork and exec for a moment (it holds a
        // copy of the open file until it execs or exits), so "free again" can take a few ms.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while InstanceLock::acquire(&paths).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "lock is not free again after drop"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
