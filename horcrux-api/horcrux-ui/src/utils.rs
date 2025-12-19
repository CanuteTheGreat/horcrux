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
