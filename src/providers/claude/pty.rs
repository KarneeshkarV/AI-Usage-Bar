use anyhow::{Result, anyhow};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use regex::Regex;
use std::io::{Read, Write};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use super::Window;
use crate::util::path::resolve_binary;

pub struct UsageData {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
}

pub struct PtySession {
    pair: PtyPair,
    writer: Box<dyn Write + Send>,
    reader_buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl PtySession {
    pub fn spawn(binary_override: Option<&str>) -> Result<Self> {
        let bin = resolve_binary("claude", binary_override)
            .ok_or_else(|| anyhow!("claude binary not on PATH"))?;
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: 60,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let cmd = CommandBuilder::new(bin);
        let mut child = pair.slave.spawn_command(cmd)?;
        // We don't need to keep `child` directly — when `pair` drops the master,
        // the child receives SIGHUP. Detach into a reaper thread so it doesn't zombie.
        thread::spawn(move || {
            let _ = child.wait();
        });

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let buf_for_thread = buf.clone();
        thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut g = buf_for_thread.lock().unwrap();
                        g.extend_from_slice(&chunk[..n]);
                        // Cap the buffer so it doesn't grow unbounded.
                        if g.len() > 256 * 1024 {
                            let drop = g.len() - 128 * 1024;
                            g.drain(..drop);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self { pair, writer, reader_buf: buf })
    }

    pub async fn probe_usage(&mut self) -> Result<UsageData> {
        self.drain();
        self.writer.write_all(b"/usage\r")?;
        self.writer.flush()?;

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut accumulated = String::new();
        let mut auto_confirmed = false;

        loop {
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let new = self.take_buf();
            if !new.is_empty() {
                accumulated.push_str(&strip_ansi(&new));
            }
            if !auto_confirmed && accumulated.contains("Show plan") {
                self.writer.write_all(b"\r")?;
                self.writer.flush()?;
                auto_confirmed = true;
            }
            // Heuristic: panel rendered when we see both labels OR a "data not available" line
            if (accumulated.contains("Session limit") || accumulated.contains("session limit"))
                && (accumulated.contains("Weekly limit") || accumulated.contains("weekly limit"))
            {
                break;
            }
        }

        parse_panel(&accumulated)
    }

    fn drain(&self) {
        self.reader_buf.lock().unwrap().clear();
    }

    fn take_buf(&self) -> String {
        let mut g = self.reader_buf.lock().unwrap();
        let bytes = std::mem::take(&mut *g);
        String::from_utf8_lossy(&bytes).to_string()
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Best-effort polite quit.
        let _ = self.writer.write_all(b"/quit\r");
        let _ = self.writer.flush();
        // Closing the master in `pair` happens via field drop order.
        let _ = &self.pair;
    }
}

fn strip_ansi(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").unwrap());
    re.replace_all(s, "").into_owned()
}

fn parse_panel(text: &str) -> Result<UsageData> {
    static SESSION_RE: OnceLock<Regex> = OnceLock::new();
    static WEEKLY_RE: OnceLock<Regex> = OnceLock::new();
    let session_re = SESSION_RE.get_or_init(|| {
        Regex::new(r"(?i)session limit[^0-9]*([\d.]+)%").unwrap()
    });
    let weekly_re = WEEKLY_RE.get_or_init(|| {
        Regex::new(r"(?i)weekly limit[^0-9]*([\d.]+)%").unwrap()
    });

    let session = capture_window(text, session_re);
    let weekly = capture_window(text, weekly_re);

    if session.is_none() && weekly.is_none() {
        if text.to_lowercase().contains("not available") {
            return Err(anyhow!("usage data not available yet"));
        }
        return Err(anyhow!("could not parse /usage panel"));
    }
    Ok(UsageData { session, weekly })
}

fn capture_window(text: &str, re: &Regex) -> Option<Window> {
    let m = re.captures(text)?;
    let pct: f64 = m.get(1)?.as_str().parse().ok()?;
    // Search for a reset clause within ~120 chars after the match.
    let after = &text[m.get(0)?.end()..];
    let slice = &after[..after.len().min(240)];
    let resets_at = parse_reset_time(slice);
    Some(Window { used_percent: pct, resets_at })
}

fn parse_reset_time(slice: &str) -> Option<chrono::DateTime<Utc>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\((\d{1,2}):(\d{2})\)\s*(?:on\s*)?(?:(\d{1,2})\s+(\w{3})|(\w{3})\s+(\d{1,2}))")
            .unwrap()
    });
    let cap = re.captures(slice)?;
    let h: u32 = cap.get(1)?.as_str().parse().ok()?;
    let m: u32 = cap.get(2)?.as_str().parse().ok()?;
    let (day, mon) = if let (Some(d), Some(mo)) = (cap.get(3), cap.get(4)) {
        (d.as_str().parse::<u32>().ok()?, mo.as_str().to_string())
    } else {
        (cap.get(6)?.as_str().parse::<u32>().ok()?, cap.get(5)?.as_str().to_string())
    };
    let month_num = match mon.to_lowercase().as_str() {
        "jan" => 1, "feb" => 2, "mar" => 3, "apr" => 4, "may" => 5, "jun" => 6,
        "jul" => 7, "aug" => 8, "sep" => 9, "oct" => 10, "nov" => 11, "dec" => 12,
        _ => return None,
    };
    let now = Local::now();
    let mut year = now.year();
    let date = NaiveDate::from_ymd_opt(year, month_num, day)?;
    let time = NaiveTime::from_hms_opt(h, m, 0)?;
    let mut naive = NaiveDateTime::new(date, time);
    if naive < now.naive_local() {
        year += 1;
        let date2 = NaiveDate::from_ymd_opt(year, month_num, day)?;
        naive = NaiveDateTime::new(date2, time);
    }
    let local = Local.from_local_datetime(&naive).single()?;
    Some(local.with_timezone(&Utc))
}
