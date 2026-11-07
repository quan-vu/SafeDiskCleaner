//! Human-readable sizes and rates (decimal units, like Finder).

pub fn bytes(b: i64) -> String {
    let b = b.max(0) as f64;
    if b >= 1e12 {
        format!("{:.2} TB", b / 1e12)
    } else if b >= 1e9 {
        if b >= 1e11 { format!("{:.0} GB", b / 1e9) } else { format!("{:.1} GB", b / 1e9) }
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}

/// Splits a size into (number, unit) for hero numbers: `147`, `GB`.
pub fn bytes_parts(b: i64) -> (String, String) {
    let s = bytes(b);
    let mut it = s.rsplitn(2, ' ');
    let unit = it.next().unwrap_or("").to_string();
    (it.next().unwrap_or("").to_string(), unit)
}

pub fn rate(bps: f64) -> String {
    let b = bps.max(0.0);
    if b >= 1e9 {
        format!("{:.2} GB/s", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB/s", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} KB/s", b / 1e3)
    } else {
        format!("{b:.0} B/s")
    }
}

pub fn rate_parts(bps: f64) -> (String, String) {
    let s = rate(bps);
    let mut it = s.rsplitn(2, ' ');
    let unit = it.next().unwrap_or("").to_string();
    (it.next().unwrap_or("").to_string(), unit)
}
