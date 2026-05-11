use chrono::{DateTime, Utc};

/// Format a duration-from-now as "2h 14m" / "45m" / "5d 3h" / "now".
pub fn until(target: DateTime<Utc>) -> String {
    let now = Utc::now();
    let dur = target.signed_duration_since(now);
    let secs = dur.num_seconds();
    if secs <= 0 {
        return "now".into();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}
