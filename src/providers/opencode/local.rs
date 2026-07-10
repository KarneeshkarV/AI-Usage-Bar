//! Local OpenCode usage from `opencode.db` (read-only), ported from
//! CodexBar's OpenCodeGoLocalUsageReader — simplified to a last-30d cost total.

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use rusqlite::Connection;
use std::path::Path;

const MESSAGE_USAGE_SQL: &str = r#"
SELECT
  CAST(COALESCE(json_extract(data, '$.time.created'), time_created) AS INTEGER) AS createdMs,
  CAST(json_extract(data, '$.cost') AS REAL) AS cost
FROM message
WHERE json_valid(data)
  AND json_extract(data, '$.providerID') = 'opencode-go'
  AND json_extract(data, '$.role') = 'assistant'
  AND json_type(data, '$.cost') IN ('integer', 'real')
"#;

const MESSAGE_AND_PART_USAGE_SQL: &str = r#"
WITH message_costs AS (
  SELECT
    id AS messageID,
    CAST(COALESCE(json_extract(data, '$.time.created'), time_created) AS INTEGER) AS createdMs,
    CAST(json_extract(data, '$.cost') AS REAL) AS cost
  FROM message
  WHERE json_valid(data)
    AND json_extract(data, '$.providerID') = 'opencode-go'
    AND json_extract(data, '$.role') = 'assistant'
    AND json_type(data, '$.cost') IN ('integer', 'real')
)
SELECT createdMs, cost
FROM message_costs
UNION ALL
SELECT
  CAST(COALESCE(json_extract(p.data, '$.time.created'), p.time_created, m.time_created) AS INTEGER)
    AS createdMs,
  CAST(json_extract(p.data, '$.cost') AS REAL) AS cost
FROM part p
JOIN message m ON m.id = p.message_id
WHERE json_valid(p.data)
  AND json_valid(m.data)
  AND json_extract(p.data, '$.type') = 'step-finish'
  AND json_type(p.data, '$.cost') IN ('integer', 'real')
  AND json_extract(m.data, '$.providerID') = 'opencode-go'
  AND json_extract(m.data, '$.role') = 'assistant'
  AND NOT EXISTS (
    SELECT 1
    FROM message_costs
    WHERE message_costs.messageID = p.message_id
  )
"#;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalUsage {
    /// Sum of opencode-go assistant costs in the last 30 days (USD).
    pub last_30d_cost_usd: f64,
}

/// Read last-30d cost from a local opencode.db. Missing/locked DB → error
/// (caller degrades to balance-only).
pub fn read_local_usage(db_path: &Path) -> Result<LocalUsage> {
    if !db_path.is_file() {
        bail!("database not found at {}", db_path.display());
    }

    let conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .context("sqlite busy_timeout")?;

    let sql = if has_table(&conn, "part") {
        MESSAGE_AND_PART_USAGE_SQL
    } else {
        MESSAGE_USAGE_SQL
    };

    let mut stmt = conn.prepare(sql).context("prepare usage SQL")?;
    let rows = stmt
        .query_map([], |row| {
            let created_ms: i64 = row.get(0)?;
            let cost: f64 = row.get(1)?;
            Ok((created_ms, cost))
        })
        .context("query usage rows")?;

    let now_ms = Utc::now().timestamp_millis();
    let start_ms = (Utc::now() - Duration::days(30)).timestamp_millis();

    let mut total = 0.0_f64;
    for row in rows {
        let (created_ms, cost) = row.context("read usage row")?;
        if created_ms <= 0 || !cost.is_finite() || cost < 0.0 {
            continue;
        }
        if created_ms >= start_ms && created_ms < now_ms {
            total += cost;
        }
    }

    Ok(LocalUsage {
        last_30d_cost_usd: total,
    })
}

fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn sums_last_30d_from_message_table() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE message (
              id TEXT PRIMARY KEY,
              time_created INTEGER,
              data TEXT
            );
            "#,
        )
        .unwrap();

        let now_ms = Utc::now().timestamp_millis();
        let recent = now_ms - 86_400_000; // 1 day ago
        let old = now_ms - 40_i64 * 86_400_000; // 40 days ago

        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params![
                "m1",
                recent,
                r#"{"providerID":"opencode-go","role":"assistant","cost":1.25,"time":{"created":0}}"#
            ],
        )
        .unwrap();
        // Use time.created for the second row
        let data2 = format!(
            r#"{{"providerID":"opencode-go","role":"assistant","cost":0.75,"time":{{"created":{recent}}}}}"#
        );
        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params!["m2", 0, data2],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params![
                "m3",
                old,
                format!(
                    r#"{{"providerID":"opencode-go","role":"assistant","cost":99.0,"time":{{"created":{old}}}}}"#
                )
            ],
        )
        .unwrap();
        // Different provider — ignored
        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params![
                "m4",
                recent,
                r#"{"providerID":"openai","role":"assistant","cost":5.0,"time":{"created":0}}"#
            ],
        )
        .unwrap();

        // Fix m1 to use recent created via time_created (COALESCE path)
        // m1 has time.created = 0 so it uses time_created = recent → included
        // Actually COALESCE(0, recent) = 0 in SQL? json_extract returns 0 which is not null.
        // So m1 would be excluded (createdMs=0). Re-insert properly.
        conn.execute("DELETE FROM message", []).unwrap();
        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params![
                "m1",
                recent,
                format!(
                    r#"{{"providerID":"opencode-go","role":"assistant","cost":1.25,"time":{{"created":{recent}}}}}"#
                )
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params![
                "m2",
                recent,
                format!(
                    r#"{{"providerID":"opencode-go","role":"assistant","cost":0.75,"time":{{"created":{recent}}}}}"#
                )
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
            params![
                "m3",
                old,
                format!(
                    r#"{{"providerID":"opencode-go","role":"assistant","cost":99.0,"time":{{"created":{old}}}}}"#
                )
            ],
        )
        .unwrap();

        let usage = read_local_usage(&db).unwrap();
        assert!((usage.last_30d_cost_usd - 2.0).abs() < 1e-9);
    }
}
