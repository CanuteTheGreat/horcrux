use std::collections::HashMap;

/// Parse a string of "key=value" pairs separated by newlines or commas
pub fn parse_key_value_pairs(input: &str) -> HashMap<String, String> {
    input
        .lines()
        .flat_map(|line| line.split(','))
        .filter_map(|pair| {
            let parts: Vec<&str> = pair.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// Format a HashMap as "key=value" pairs separated by newlines
pub fn format_key_value_pairs(map: &HashMap<String, String>) -> String {
    map.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse HTTP headers from a string format "Header-Name: value"
pub fn parse_headers(input: &str) -> HashMap<String, String> {
    input
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() == 2 {
                Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// Format headers as "Header-Name: value" pairs separated by newlines
pub fn format_headers(headers: &HashMap<String, String>) -> String {
    headers
        .iter()
        .map(|(k, v)| format!("{}: {}", k, v))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render an RFC 3339 timestamp as `YYYY-MM-DD HH:MM:SS UTC`; unparseable
/// input is shown as-is rather than hidden.
pub fn format_rfc3339(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| {
            t.with_timezone(&chrono::Utc)
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string()
        })
        .unwrap_or_else(|_| ts.to_string())
}

/// Render Unix epoch seconds as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn format_unix_secs(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| secs.to_string())
}

/// Options for a `<select>`: the node's own list plus the current value, so a
/// value the list lacks is still shown and selectable (never a blank select).
pub fn choices_with_current(mut list: Vec<String>, current: &str) -> Vec<String> {
    if !current.is_empty() && !list.iter().any(|v| v == current) {
        list.push(current.to_string());
        list.sort();
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps_readably() {
        assert_eq!(
            format_rfc3339("2026-10-16T15:00:39+00:00"),
            "2026-10-16 15:00:39 UTC"
        );
        assert_eq!(
            format_rfc3339("2026-10-16T08:00:39-07:00"),
            "2026-10-16 15:00:39 UTC"
        );
        assert_eq!(format_rfc3339("garbage"), "garbage");
        assert_eq!(format_unix_secs(1792162839), "2026-10-16 15:00:39 UTC");
    }

    #[test]
    fn select_choices_always_include_the_current_value() {
        let got = choices_with_current(vec!["Europe/Berlin".into()], "Etc/UTC");
        assert_eq!(got, vec!["Etc/UTC", "Europe/Berlin"]);
        assert_eq!(choices_with_current(vec!["a".into()], "a"), vec!["a"]);
        assert!(choices_with_current(vec![], "").is_empty());
    }
}
