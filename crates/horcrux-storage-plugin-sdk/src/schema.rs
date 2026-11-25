//! The config schema a plugin declares, and its validation.
//!
//! The manager validates every pool config against the plugin's schema
//! BEFORE the plugin sees it (unknown keys, wrong types, out-of-range numbers,
//! pattern mismatches and missing required fields are a 400), fills in
//! defaults, and splits `secret` fields out: secrets are kept apart from the
//! pool record and never returned by the API. The web UI renders the pool
//! form from the same schema (label, hint as tooltip, min/max, pattern,
//! secrets masked), so a plugin needs no UI code.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Field types (`"type"` in the schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    /// Integer (JSON number without fraction). `min`/`max` bound the value.
    Int,
    Bool,
    /// One of `options`.
    Enum,
    /// A string kept in the secrets store, write-only through the API.
    Secret,
    /// An absolute path without `..` components.
    Path,
}

/// A choice of an `enum` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: String,
    #[serde(default)]
    pub label: String,
}

/// A field that only applies when another field has a value: `value` absent
/// means "the other field is set and truthy" (true, non-empty, non-zero).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependsOn {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// One config field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// `[a-z][a-z0-9_]{0,63}`
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub label: String,
    /// One or two sentences; the UI shows it as the field's tooltip and help.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// int: smallest value; string/secret/path: shortest length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    /// int: largest value; string/secret/path: longest length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    /// string/path: a regular expression the WHOLE value must match (same
    /// meaning as the HTML `pattern` attribute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<DependsOn>,
    /// enum choices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<EnumOption>,
    /// Form placeholder text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
}

impl Field {
    /// A field with only key, type and label set (builder start).
    pub fn new(key: &str, field_type: FieldType, label: &str) -> Self {
        Self {
            key: key.into(),
            field_type,
            label: label.into(),
            hint: String::new(),
            default: None,
            min: None,
            max: None,
            pattern: None,
            required: false,
            depends_on: None,
            options: Vec::new(),
            placeholder: String::new(),
        }
    }
    pub fn hint(mut self, h: &str) -> Self {
        self.hint = h.into();
        self
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn default_value(mut self, v: Value) -> Self {
        self.default = Some(v);
        self
    }
    pub fn range(mut self, min: i64, max: i64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
    pub fn pattern(mut self, p: &str) -> Self {
        self.pattern = Some(p.into());
        self
    }
    pub fn placeholder(mut self, p: &str) -> Self {
        self.placeholder = p.into();
        self
    }
    pub fn depends_on(mut self, key: &str, value: Option<Value>) -> Self {
        self.depends_on = Some(DependsOn {
            key: key.into(),
            value,
        });
        self
    }
    pub fn options(mut self, opts: &[(&str, &str)]) -> Self {
        self.options = opts
            .iter()
            .map(|(v, l)| EnumOption {
                value: (*v).into(),
                label: (*l).into(),
            })
            .collect();
        self
    }
}

/// What a plugin's pools can do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Content the pool holds: `images` (VM disks), `rootdir`, `iso`,
    /// `backup`, `snippets`.
    #[serde(default)]
    pub content: Vec<String>,
    #[serde(default)]
    pub snapshots: bool,
    #[serde(default)]
    pub clone: bool,
    #[serde(default)]
    pub resize: bool,
    /// The same storage is reachable from every cluster node.
    #[serde(default)]
    pub shared: bool,
    /// `activate` returns a local directory and VM disks may be created in
    /// it by the manager (like a directory pool).
    #[serde(default)]
    pub vm_disk_dir: bool,
    /// Volume formats the plugin creates.
    #[serde(default)]
    pub formats: Vec<String>,
}

/// One validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// The field key (empty for whole-config errors).
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.key, self.message)
        }
    }
}

/// A config that passed validation, split into plain values and secrets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidatedConfig {
    pub config: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, String>,
}

fn valid_key(k: &str) -> bool {
    let b = k.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => true,
    }
}

#[cfg(feature = "pattern")]
fn pattern_matches(p: &str, s: &str) -> Result<bool, String> {
    regex::Regex::new(&format!("^(?:{p})$"))
        .map(|r| r.is_match(s))
        .map_err(|e| format!("pattern {p:?} is not a valid regular expression: {e}"))
}

#[cfg(not(feature = "pattern"))]
fn pattern_matches(_p: &str, _s: &str) -> Result<bool, String> {
    Ok(true)
}

/// Check a plugin's schema itself (the manager refuses a plugin whose
/// schema is broken, with this message): keys valid and unique, defaults of
/// the field's type and within its limits, enums with options, `depends_on`
/// naming an EARLIER field, min <= max, patterns that compile.
pub fn check_schema(schema: &[Field]) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    for f in schema {
        if !valid_key(&f.key) {
            return Err(format!(
                "field key {:?} must be [a-z][a-z0-9_]{{0,63}}",
                f.key
            ));
        }
        if seen.contains(&f.key.as_str()) {
            return Err(format!("field {} is declared twice", f.key));
        }
        if f.label.trim().is_empty() {
            return Err(format!("field {} has no label", f.key));
        }
        if let (Some(a), Some(b)) = (f.min, f.max) {
            if a > b {
                return Err(format!("field {}: min {a} > max {b}", f.key));
            }
        }
        if f.field_type == FieldType::Enum && f.options.is_empty() {
            return Err(format!("enum field {} has no options", f.key));
        }
        if f.field_type == FieldType::Secret && f.default.is_some() {
            return Err(format!("secret field {} must not have a default", f.key));
        }
        if let Some(p) = &f.pattern {
            pattern_matches(p, "")
                .map_err(|e| format!("field {}: {e}", f.key))
                .map(|_| ())?;
        }
        if let Some(d) = &f.depends_on {
            if !seen.contains(&d.key.as_str()) {
                return Err(format!(
                    "field {} depends on {}, which is not an earlier field",
                    f.key, d.key
                ));
            }
        }
        if let Some(def) = &f.default {
            check_value(f, def).map_err(|m| format!("field {}: default {m}", f.key))?;
        }
        seen.push(&f.key);
    }
    Ok(())
}

/// Normalise one value of field `f` (ints given as decimal strings and bools
/// given as "true"/"false" are accepted, as HTML forms send them).
fn check_value(f: &Field, v: &Value) -> Result<Value, String> {
    let len_ok = |s: &str| -> Result<(), String> {
        let n = s.chars().count() as i64;
        if let Some(min) = f.min {
            if n < min {
                return Err(format!("must be at least {min} characters"));
            }
        }
        if let Some(max) = f.max {
            if n > max {
                return Err(format!("must be at most {max} characters"));
            }
        }
        Ok(())
    };
    let pat_ok = |s: &str| -> Result<(), String> {
        if let Some(p) = &f.pattern {
            if !pattern_matches(p, s)? {
                return Err(format!("does not match the required format ({p})"));
            }
        }
        Ok(())
    };
    match f.field_type {
        FieldType::String | FieldType::Path | FieldType::Enum => {
            let Some(s) = v.as_str() else {
                return Err("must be a string".into());
            };
            if s.chars().any(|c| c.is_control()) {
                return Err("must not contain control characters".into());
            }
            match f.field_type {
                FieldType::Enum => {
                    if !f.options.iter().any(|o| o.value == s) {
                        let all: Vec<&str> = f.options.iter().map(|o| o.value.as_str()).collect();
                        return Err(format!("must be one of {}", all.join(", ")));
                    }
                }
                FieldType::Path => {
                    if !s.starts_with('/') {
                        return Err("must be an absolute path".into());
                    }
                    if s.split('/').any(|c| c == "..") {
                        return Err("must not contain '..'".into());
                    }
                    len_ok(s)?;
                    pat_ok(s)?;
                }
                _ => {
                    len_ok(s)?;
                    pat_ok(s)?;
                }
            }
            Ok(Value::String(s.to_string()))
        }
        FieldType::Secret => {
            let Some(s) = v.as_str() else {
                return Err("must be a string".into());
            };
            if s.contains('\0') {
                return Err("must not contain NUL".into());
            }
            len_ok(s)?;
            Ok(Value::String(s.to_string()))
        }
        FieldType::Int => {
            let n = match v {
                Value::Number(n) => n.as_i64().ok_or("must be a whole number")?,
                Value::String(s) => s
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| "must be a whole number".to_string())?,
                _ => return Err("must be a whole number".into()),
            };
            if let Some(min) = f.min {
                if n < min {
                    return Err(format!("must be at least {min}"));
                }
            }
            if let Some(max) = f.max {
                if n > max {
                    return Err(format!("must be at most {max}"));
                }
            }
            Ok(Value::from(n))
        }
        FieldType::Bool => match v {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if s == "true" || s == "false" => Ok(Value::Bool(s == "true")),
            _ => Err("must be true or false".into()),
        },
    }
}

/// Validate `input` against `schema`: every error is reported (not only the
/// first), each with its field key.
pub fn validate_config(
    schema: &[Field],
    input: &BTreeMap<String, Value>,
) -> Result<ValidatedConfig, Vec<FieldError>> {
    let mut errs = Vec::new();
    for k in input.keys() {
        if !schema.iter().any(|f| &f.key == k) {
            errs.push(FieldError {
                key: k.clone(),
                message: "is not a setting of this plugin".into(),
            });
        }
    }
    // Effective values so far (for depends_on, which names earlier fields).
    let mut effective: BTreeMap<String, Value> = BTreeMap::new();
    let mut out = ValidatedConfig::default();
    for f in schema {
        let active = match &f.depends_on {
            None => true,
            Some(d) => match (effective.get(&d.key), &d.value) {
                (None, _) => false,
                (Some(v), None) => truthy(v),
                (Some(v), Some(want)) => v == want,
            },
        };
        if !active {
            continue;
        }
        let given = input.get(&f.key).filter(|v| match v {
            Value::Null => false,
            // an empty form input means "not set"
            Value::String(s) => !s.is_empty(),
            _ => true,
        });
        let value = match (given, &f.default) {
            (Some(v), _) => match check_value(f, v) {
                Ok(v) => Some(v),
                Err(m) => {
                    errs.push(FieldError {
                        key: f.key.clone(),
                        message: m,
                    });
                    continue;
                }
            },
            (None, Some(d)) => Some(d.clone()),
            (None, None) => None,
        };
        match value {
            None if f.required => errs.push(FieldError {
                key: f.key.clone(),
                message: "is required".into(),
            }),
            None => {}
            Some(v) => {
                effective.insert(f.key.clone(), v.clone());
                if f.field_type == FieldType::Secret {
                    out.secrets
                        .insert(f.key.clone(), v.as_str().unwrap_or_default().to_string());
                } else {
                    out.config.insert(f.key.clone(), v);
                }
            }
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Vec<Field> {
        vec![
            Field::new("host", FieldType::String, "Server")
                .required()
                .pattern("[A-Za-z0-9.:-]+")
                .range(1, 253),
            Field::new("port", FieldType::Int, "Port")
                .default_value(json!(22))
                .range(1, 65535),
            Field::new("auth", FieldType::Enum, "Login")
                .options(&[("key", "SSH key"), ("password", "Password")])
                .default_value(json!("key")),
            Field::new("password", FieldType::Secret, "Password")
                .required()
                .depends_on("auth", Some(json!("password"))),
            Field::new("remote_dir", FieldType::Path, "Remote directory").required(),
            Field::new("compress", FieldType::Bool, "Compression").default_value(json!(false)),
        ]
    }

    fn m(v: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn schema_is_valid() {
        check_schema(&schema()).unwrap();
    }

    #[test]
    fn defaults_fill_in_and_inactive_fields_are_dropped() {
        let v = validate_config(
            &schema(),
            &m(json!({"host": "nas.example", "remote_dir": "/srv/vm", "password": "x"})),
        )
        .unwrap();
        assert_eq!(v.config["port"], json!(22));
        assert_eq!(v.config["auth"], json!("key"));
        assert_eq!(v.config["compress"], json!(false));
        // auth=key: the password field does not apply and is dropped
        assert!(v.secrets.is_empty());
        assert!(!v.config.contains_key("password"));
    }

    #[test]
    fn secrets_are_split_out() {
        let v = validate_config(
            &schema(),
            &m(json!({"host": "h", "remote_dir": "/d", "auth": "password", "password": "s3cret"})),
        )
        .unwrap();
        assert_eq!(v.secrets["password"], "s3cret");
        assert!(!v.config.contains_key("password"));
    }

    #[test]
    fn every_error_is_reported_with_its_key() {
        let e = validate_config(
            &schema(),
            &m(json!({"port": 70000, "auth": "telnet", "bogus": 1, "remote_dir": "rel"})),
        )
        .unwrap_err();
        let keys: Vec<&str> = e.iter().map(|e| e.key.as_str()).collect();
        for k in ["bogus", "host", "port", "auth", "remote_dir"] {
            assert!(keys.contains(&k), "{k} missing in {e:?}");
        }
        let port = e.iter().find(|e| e.key == "port").unwrap();
        assert_eq!(port.message, "must be at most 65535");
    }

    #[test]
    fn required_dependent_field() {
        let e = validate_config(
            &schema(),
            &m(json!({"host": "h", "remote_dir": "/d", "auth": "password"})),
        )
        .unwrap_err();
        assert_eq!(
            e,
            vec![FieldError {
                key: "password".into(),
                message: "is required".into()
            }]
        );
    }

    #[test]
    fn form_strings_are_accepted_for_ints_and_bools() {
        let v = validate_config(
            &schema(),
            &m(json!({"host": "h", "remote_dir": "/d", "port": "2222", "compress": "true"})),
        )
        .unwrap();
        assert_eq!(v.config["port"], json!(2222));
        assert_eq!(v.config["compress"], json!(true));
        let e = validate_config(
            &schema(),
            &m(json!({"host": "h", "remote_dir": "/d", "port": "22.5"})),
        )
        .unwrap_err();
        assert_eq!(e[0].key, "port");
        // empty form input = unset: the default applies
        let v = validate_config(
            &schema(),
            &m(json!({"host": "h", "remote_dir": "/d", "port": ""})),
        )
        .unwrap();
        assert_eq!(v.config["port"], json!(22));
    }

    #[test]
    fn paths_and_control_characters() {
        for bad in ["/a/../b", "relative", "/a\nb"] {
            let e = validate_config(&schema(), &m(json!({"host": "h", "remote_dir": bad})))
                .unwrap_err();
            assert_eq!(e[0].key, "remote_dir", "{bad}");
        }
        let e = validate_config(&schema(), &m(json!({"host": "h\nx", "remote_dir": "/d"})))
            .unwrap_err();
        assert_eq!(e[0].key, "host");
    }

    #[cfg(feature = "pattern")]
    #[test]
    fn pattern_must_match_the_whole_value() {
        let e = validate_config(
            &schema(),
            &m(json!({"host": "bad host", "remote_dir": "/d"})),
        )
        .unwrap_err();
        assert_eq!(e[0].key, "host");
        assert!(e[0].message.contains("required format"), "{e:?}");
        let mut s = schema();
        s[0].pattern = Some("(".into());
        assert!(check_schema(&s)
            .unwrap_err()
            .contains("not a valid regular expression"));
    }

    #[test]
    fn broken_schemas_are_refused() {
        let mut s = schema();
        s.push(Field::new("port", FieldType::Int, "Again"));
        assert!(check_schema(&s).unwrap_err().contains("declared twice"));

        let s = vec![Field::new("Bad Key", FieldType::String, "x")];
        assert!(check_schema(&s).is_err());

        let s = vec![Field::new("e", FieldType::Enum, "x")];
        assert!(check_schema(&s).unwrap_err().contains("no options"));

        let s = vec![Field::new("n", FieldType::Int, "n")
            .range(1, 5)
            .default_value(json!(9))];
        assert!(check_schema(&s)
            .unwrap_err()
            .contains("default must be at most 5"));

        let s = vec![Field::new("a", FieldType::String, "a").depends_on("later", None)];
        assert!(check_schema(&s)
            .unwrap_err()
            .contains("not an earlier field"));

        let s = vec![Field::new("p", FieldType::Secret, "p").default_value(json!("x"))];
        assert!(check_schema(&s)
            .unwrap_err()
            .contains("must not have a default"));

        let s = vec![Field::new("n", FieldType::Int, "n").range(5, 1)];
        assert!(check_schema(&s).unwrap_err().contains("min 5 > max 1"));
    }

    #[test]
    fn field_json_shape() {
        let f = Field::new("port", FieldType::Int, "Port")
            .range(1, 65535)
            .hint("TCP port of the SSH server");
        let j = serde_json::to_value(&f).unwrap();
        assert_eq!(j["type"], "int");
        assert_eq!(j["max"], 65535);
        assert_eq!(j["hint"], "TCP port of the SSH server");
        assert!(j.get("pattern").is_none());
        let back: Field = serde_json::from_value(j).unwrap();
        assert_eq!(back, f);
    }
}
