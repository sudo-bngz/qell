//! What a resource browser lists, whatever it is browsing.
//!
//! The cluster view grew its own copy of this first; the AWS view is
//! built on this one, and the cluster view can move onto it.

use serde_json::Value;

/// How a row is doing, for its colour: read from the service's own
/// states, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Warn,
    Bad,
    /// Nothing to say (a bucket has no health).
    Plain,
}

/// One line of the table.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub cells: Vec<String>,
    /// What the row is called, for going into it and for messages.
    pub name: String,
    pub health: Health,
    /// The object itself, for what a drill-down or a describe needs.
    pub object: Value,
}

/// A whole listing, ready to draw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

impl Table {
    pub fn new(columns: &[&str]) -> Self {
        Self { columns: columns.iter().map(|c| c.to_string()).collect(), rows: Vec::new() }
    }

    /// The width each column needs, in characters.
    pub fn widths(&self) -> Vec<usize> {
        let mut w: Vec<usize> = self.columns.iter().map(|c| c.chars().count()).collect();
        for row in &self.rows {
            for (i, cell) in row.cells.iter().enumerate() {
                if let Some(x) = w.get_mut(i) {
                    *x = (*x).max(cell.chars().count());
                }
            }
        }
        w
    }

    /// The rows containing `filter` in any cell, as indexes. Case is
    /// ignored; an empty filter keeps everything.
    pub fn matching(&self, filter: &str) -> Vec<usize> {
        if filter.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let needle = filter.to_lowercase();
        (0..self.rows.len()).filter(|i| self.rows[*i].cells.iter().any(|c| c.to_lowercase().contains(&needle))).collect()
    }
}

/// `2026-11-21T07:04:05Z` (or with an offset, or with fractions) as
/// seconds since the epoch. The date and time are read as written:
/// AWS and Kubernetes both answer in UTC.
pub fn epoch_seconds(timestamp: &str) -> Option<i64> {
    let b = timestamp.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| timestamp.get(a..z)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Days from the civil calendar (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// An age the way kubectl prints it: `12s`, `5m`, `3h20m`, `6d4h`, `21d`.
pub fn age(seconds: i64) -> String {
    let s = seconds.max(0);
    let (m, h, d) = (s / 60, s / 3_600, s / 86_400);
    if d >= 8 {
        format!("{d}d")
    } else if d >= 1 {
        match h - d * 24 {
            0 => format!("{d}d"),
            hours => format!("{d}d{hours}h"),
        }
    } else if h >= 1 {
        match m - h * 60 {
            0 => format!("{h}h"),
            mins => format!("{h}h{mins}m"),
        }
    } else if m >= 1 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

/// How long ago `timestamp` was, as an age, or a dash when it isn't one.
pub fn ago(timestamp: &str, now: i64) -> String {
    match epoch_seconds(timestamp) {
        Some(then) => age(now - then),
        None => "-".to_string(),
    }
}

/// A size as people read it.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut value, mut unit) = (n as f64, 0);
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// An object as indented `key: value` lines, the way `describe` reads:
/// easier on the eye than JSON's quotes and braces, and nothing is left
/// out.
pub fn describe(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    lines_of(value, 0, &mut out);
    out
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn lines_of(value: &Value, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                match v {
                    Value::Object(m) if !m.is_empty() => {
                        out.push(format!("{pad}{key}:"));
                        lines_of(v, depth + 1, out);
                    }
                    Value::Array(a) if !a.is_empty() => {
                        out.push(format!("{pad}{key}:"));
                        lines_of(v, depth + 1, out);
                    }
                    Value::Object(_) | Value::Array(_) => out.push(format!("{pad}{key}: -")),
                    _ => out.push(format!("{pad}{key}: {}", scalar(v))),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        // The first line of an item carries the dash.
                        let mut inner = Vec::new();
                        lines_of(item, depth + 1, &mut inner);
                        for (i, line) in inner.into_iter().enumerate() {
                            if i == 0 {
                                out.push(format!("{pad}- {}", line.trim_start()));
                            } else {
                                out.push(line);
                            }
                        }
                    }
                    _ => out.push(format!("{pad}- {}", scalar(item))),
                }
            }
        }
        _ => out.push(format!("{pad}{}", scalar(value))),
    }
}

/// Seconds since the epoch, now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_in_every_shape_the_services_write() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_seconds("2026-11-21T07:04:05Z"), Some(1_795_244_645));
        // AWS: an offset, or fractions and an offset.
        assert_eq!(epoch_seconds("2026-11-21T07:04:05+00:00"), Some(1_795_244_645));
        assert_eq!(epoch_seconds("2026-11-21T07:04:05.000+0000"), Some(1_795_244_645));
        assert_eq!(epoch_seconds("yesterday"), None);
    }

    #[test]
    fn ages_and_sizes_read_the_usual_way() {
        assert_eq!(age(90), "1m");
        assert_eq!(age(86_400 * 6 + 3_600 * 4), "6d4h");
        assert_eq!(ago("2026-11-21T07:04:05Z", 1_795_244_645 + 7_200), "2h");
        assert_eq!(ago("", 0), "-");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn an_object_reads_as_indented_fields() {
        let v = serde_json::json!({
            "Name": "api",
            "Tags": [{ "Key": "env", "Value": "prod" }],
            "Layers": [],
            "Vpc": { "Subnets": ["a", "b"] },
            "Memory": 512,
        });
        assert_eq!(
            describe(&v),
            [
                "Layers: -",
                "Memory: 512",
                "Name: api",
                "Tags:",
                "  - Key: env",
                "    Value: prod",
                "Vpc:",
                "  Subnets:",
                "    - a",
                "    - b"
            ]
        );
    }

    #[test]
    fn the_filter_matches_any_cell() {
        let mut t = Table::new(&["NAME", "STATE"]);
        for (name, state) in [("api", "running"), ("web", "stopped")] {
            t.rows.push(Row {
                cells: vec![name.into(), state.into()],
                name: name.into(),
                health: Health::Plain,
                object: Value::Null,
            });
        }
        assert_eq!(t.matching(""), vec![0, 1]);
        assert_eq!(t.matching("STOP"), vec![1]);
        assert_eq!(t.widths(), vec![4, 7]);
    }
}
