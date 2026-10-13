//! Small formatting helpers shared by the screens.

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Exact below 10 KiB (`1,234 B`), then one decimal in KiB/MiB/GiB.
pub fn bytes(n: usize) -> String {
    const K: f64 = 1024.0;
    if n < 10 * 1024 {
        return format!("{} B", group(n));
    }
    let f = n as f64;
    if f < K * K {
        format!("{:.1} KiB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MiB", f / (K * K))
    } else {
        format!("{:.1} GiB", f / (K * K * K))
    }
}

pub fn ago(secs: u64) -> String {
    match secs {
        0..=1 => "just now".to_string(),
        2..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

pub fn ago_ms(now_ms: u64, then_ms: u64) -> String {
    ago(now_ms.saturating_sub(then_ms) / 1000)
}

pub fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A countdown: `1:30`, `1 h 05 min`, or `until you resume` for the "no end" pause.
pub fn remaining(secs: u64) -> String {
    if secs > 365 * 24 * 3600 {
        "until you resume".to_string()
    } else if secs >= 3600 {
        format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60)
    } else {
        mmss(secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1234), "1,234 B");
        assert_eq!(bytes(10 * 1024), "10.0 KiB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(bytes(32 * 1024 * 1024), "32.0 MiB");
    }

    #[test]
    fn ages() {
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(4), "4s ago");
        assert_eq!(ago(125), "2 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(200_000), "2 d ago");
        assert_eq!(ago_ms(10_000, 4_000), "6s ago");
        assert_eq!(ago_ms(1_000, 5_000), "just now");
    }

    #[test]
    fn clocks() {
        assert_eq!(mmss(102), "1:42");
        assert_eq!(remaining(90), "1:30");
        assert_eq!(remaining(3900), "1 h 05 min");
        assert_eq!(remaining(315_360_000), "until you resume");
    }
}
