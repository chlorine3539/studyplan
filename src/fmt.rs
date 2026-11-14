//! Small formatting helpers shared by the CLI and the TUI.

/// Video length: `3:07` or `1:02:45`.
pub fn dur(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Totals: `12h 05m` or `42m`.
pub fn hm(secs: u64) -> String {
    let total_min = (secs + 30) / 60; // round to nearest minute
    let (h, m) = (total_min / 60, total_min % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// Text progress bar, e.g. `████████░░░░ 66.7%`.
pub fn bar(percent: f64, width: usize) -> String {
    let p = percent.clamp(0.0, 100.0);
    let filled = ((p / 100.0) * width as f64).round() as usize;
    format!(
        "{}{} {:>5.1}%",
        "█".repeat(filled),
        "░".repeat(width - filled),
        p
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(dur(187), "3:07");
        assert_eq!(dur(3765), "1:02:45");
        assert_eq!(hm(7500), "2h 05m");
        assert_eq!(hm(1500), "25m");
        assert_eq!(bar(50.0, 10), "█████░░░░░  50.0%");
    }
}
