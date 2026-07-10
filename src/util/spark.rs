//! Daily cost sparkline helpers shared by the TUI and `status --detailed`.

use chrono::{Datelike, Duration, NaiveDate};
use std::collections::BTreeMap;

/// Unicode block levels for a one-line sparkline (8 buckets).
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Last `days` calendar days of spend ending on `end` (inclusive).
/// Missing days are filled with `0.0`. Dates in `by_day` are `YYYY-MM-DD`.
pub fn daily_series(
    by_day: &BTreeMap<String, f64>,
    end: NaiveDate,
    days: usize,
) -> Vec<(NaiveDate, f64)> {
    let days = days.max(1);
    let start = end - Duration::days((days as i64) - 1);
    let mut out = Vec::with_capacity(days);
    let mut d = start;
    while d <= end {
        let key = d.to_string();
        let v = by_day.get(&key).copied().unwrap_or(0.0);
        out.push((d, v));
        d += Duration::days(1);
    }
    out
}

/// Scale a non-negative value into a bar index `0..8` given the series max.
///
/// - Empty / all-zero / non-positive max → 0 (`▁`)
/// - Values at `max` → 7 (`█`)
pub fn bucket(value: f64, max: f64) -> u8 {
    if max <= 0.0 || !max.is_finite() || value <= 0.0 || !value.is_finite() {
        return 0;
    }
    let ratio = (value / max).clamp(0.0, 1.0);
    // Map (0, 1] onto 1..=7 so tiny non-zero spends still show a blip above ▁.
    if ratio <= 0.0 {
        0
    } else {
        let idx = (ratio * 7.0).ceil() as u8;
        idx.clamp(1, 7)
    }
}

/// One-line Unicode sparkline for the given values.
pub fn unicode_sparkline(values: &[f64]) -> String {
    let max = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0_f64, f64::max);
    values
        .iter()
        .map(|&v| BARS[bucket(v, max) as usize])
        .collect()
}

/// Integer series for `ratatui::widgets::Sparkline` (same scaling as unicode).
pub fn sparkline_data(values: &[f64]) -> Vec<u64> {
    let max = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0_f64, f64::max);
    values.iter().map(|&v| u64::from(bucket(v, max))).collect()
}

/// Caption: `30d cost: $42.13 · today $1.87 · peak $6.20 (Jul 3)`.
pub fn cost_caption(series: &[(NaiveDate, f64)], total_usd: f64, today: NaiveDate) -> String {
    let today_cost = series
        .iter()
        .find(|(d, _)| *d == today)
        .map(|(_, v)| *v)
        .unwrap_or(0.0);

    let (peak_day, peak) = series
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(d, v)| (*d, *v))
        .unwrap_or((today, 0.0));

    if peak > 0.0 {
        format!(
            "30d cost: ${total_usd:.2} · today ${today_cost:.2} · peak ${peak:.2} ({})",
            short_month_day(peak_day)
        )
    } else {
        format!("30d cost: ${total_usd:.2} · today ${today_cost:.2}")
    }
}

fn short_month_day(d: NaiveDate) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m = MONTHS[(d.month0() as usize).min(11)];
    format!("{m} {}", d.day())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn bucket_empty_and_all_zero() {
        assert_eq!(bucket(0.0, 0.0), 0);
        assert_eq!(bucket(1.0, 0.0), 0);
        assert_eq!(bucket(0.0, 5.0), 0);
        let s = unicode_sparkline(&[]);
        assert!(s.is_empty());
        assert_eq!(unicode_sparkline(&[0.0, 0.0, 0.0]), "▁▁▁");
        assert_eq!(sparkline_data(&[0.0, 0.0]), vec![0, 0]);
    }

    #[test]
    fn bucket_single_spike() {
        let vals = [0.0, 0.0, 10.0, 0.0];
        let line = unicode_sparkline(&vals);
        assert_eq!(line.chars().count(), 4);
        let chars: Vec<char> = line.chars().collect();
        assert_eq!(chars[0], '▁');
        assert_eq!(chars[2], '█');
        assert_eq!(bucket(10.0, 10.0), 7);
        assert_eq!(sparkline_data(&vals), vec![0, 0, 7, 0]);
    }

    #[test]
    fn bucket_monotonic_ramp() {
        let vals: Vec<f64> = (0..=7).map(|i| i as f64).collect();
        let max = 7.0;
        let buckets: Vec<u8> = vals.iter().map(|&v| bucket(v, max)).collect();
        // Strictly non-decreasing; ends at full bar.
        for w in buckets.windows(2) {
            assert!(w[1] >= w[0], "buckets={buckets:?}");
        }
        assert_eq!(*buckets.last().unwrap(), 7);
        assert_eq!(buckets[0], 0);
        let line = unicode_sparkline(&vals);
        assert_eq!(line.chars().count(), 8);
        assert!(line.ends_with('█'));
    }

    #[test]
    fn daily_series_fills_missing_days() {
        let mut map = BTreeMap::new();
        map.insert("2026-07-08".into(), 1.5);
        map.insert("2026-07-10".into(), 3.0);
        let end = d(2026, 7, 10);
        let series = daily_series(&map, end, 5);
        assert_eq!(series.len(), 5);
        assert_eq!(series[0], (d(2026, 7, 6), 0.0));
        assert_eq!(series[1], (d(2026, 7, 7), 0.0));
        assert_eq!(series[2], (d(2026, 7, 8), 1.5));
        assert_eq!(series[3], (d(2026, 7, 9), 0.0));
        assert_eq!(series[4], (d(2026, 7, 10), 3.0));
    }

    #[test]
    fn cost_caption_includes_peak_day() {
        let series = vec![
            (d(2026, 7, 1), 1.0),
            (d(2026, 7, 3), 6.2),
            (d(2026, 7, 10), 1.87),
        ];
        let cap = cost_caption(&series, 42.13, d(2026, 7, 10));
        assert!(cap.contains("30d cost: $42.13"), "{cap}");
        assert!(cap.contains("today $1.87"), "{cap}");
        assert!(cap.contains("peak $6.20 (Jul 3)"), "{cap}");
    }
}
