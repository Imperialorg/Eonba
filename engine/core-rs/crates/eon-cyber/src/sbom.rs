//! Software bill of materials and advisory matching (docs/implementation/03, supply-chain layer).
//!
//! Components are emitted as CycloneDX 1.5 JSON. Advisories are read from OSV-format JSON that is already on disk,
//! so matching works offline. A match needs an explicit version list. An advisory that only gives version ranges
//! cannot be decided here, so it is reported as `Unverified` and never silently treated as clean.

use eon_primitives::{EonError, EonResult};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// OSV ecosystem name, for example `crates.io`, `PyPI`, or `npm`.
    pub ecosystem: String,
    pub name: String,
    pub version: String,
}

impl Component {
    /// Package URL. The purl type follows the ecosystem name, so `crates.io` becomes `cargo`.
    pub fn purl(&self) -> String {
        format!("pkg:{}/{}@{}", purl_type(&self.ecosystem), self.name, self.version)
    }
}

fn purl_type(ecosystem: &str) -> String {
    match ecosystem {
        "crates.io" => "cargo".to_owned(),
        "PyPI" => "pypi".to_owned(),
        "npm" => "npm".to_owned(),
        "Go" => "golang".to_owned(),
        other => other.to_lowercase(),
    }
}

/// Builds a CycloneDX 1.5 JSON document listing the components.
pub fn to_cyclonedx(components: &[Component]) -> Value {
    let listed: Vec<Value> = components
        .iter()
        .map(|c| {
            json!({
                "type": "library",
                "name": c.name,
                "version": c.version,
                "purl": c.purl(),
            })
        })
        .collect();
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "components": listed,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Affected {
    pub ecosystem: String,
    pub name: String,
    /// Explicit affected versions. Empty when the record only gives ranges.
    pub versions: Vec<String>,
    pub ranges_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    pub id: String,
    pub summary: String,
    pub affected: Vec<Affected>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The component's exact version is listed as affected.
    Affected,
    /// The advisory covers this package only by ranges, so a human or a range resolver must decide.
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub purl: String,
    pub advisory: String,
    pub verdict: Verdict,
}

fn invalid(reason: impl std::fmt::Display) -> EonError {
    EonError::ValidationError(format!("invalid OSV advisory: {reason}"))
}

fn text<'a>(value: &'a Value, key: &str) -> EonResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("missing string field '{key}'")))
}

/// Parses an OSV advisory document or an array of them.
pub fn parse_osv(source: &str) -> EonResult<Vec<Advisory>> {
    let root: Value = serde_json::from_str(source).map_err(invalid)?;
    let records = match &root {
        Value::Array(items) => items.clone(),
        Value::Object(_) => vec![root.clone()],
        _ => return Err(invalid("expected an object or an array")),
    };
    records.iter().map(parse_record).collect()
}

fn parse_record(record: &Value) -> EonResult<Advisory> {
    let id = text(record, "id")?.to_owned();
    let summary = record.get("summary").and_then(Value::as_str).unwrap_or_default().to_owned();
    let affected = record
        .get("affected")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let package = entry.get("package").ok_or_else(|| invalid(format!("{id}: affected entry without package")))?;
            let versions: Vec<String> = entry
                .get("versions")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            let has_ranges = entry.get("ranges").is_some();
            Ok(Affected {
                ecosystem: text(package, "ecosystem")?.to_owned(),
                name: text(package, "name")?.to_owned(),
                ranges_only: versions.is_empty() && has_ranges,
                versions,
            })
        })
        .collect::<EonResult<Vec<_>>>()?;
    Ok(Advisory { id, summary, affected })
}

/// Matches components against advisories. Returns one finding per (component, advisory) pair that either lists the
/// exact version or covers the package only by ranges.
pub fn match_advisories(components: &[Component], advisories: &[Advisory]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for component in components {
        for advisory in advisories {
            for affected in &advisory.affected {
                if affected.ecosystem != component.ecosystem || affected.name != component.name {
                    continue;
                }
                let verdict = if affected.versions.iter().any(|v| v == &component.version) {
                    Verdict::Affected
                } else if affected.ranges_only {
                    Verdict::Unverified
                } else {
                    continue;
                };
                findings.push(Finding {
                    purl: component.purl(),
                    advisory: advisory.id.clone(),
                    verdict,
                });
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(ecosystem: &str, name: &str, version: &str) -> Component {
        Component {
            ecosystem: ecosystem.into(),
            name: name.into(),
            version: version.into(),
        }
    }

    const ADVISORIES: &str = r#"[
        {"id":"OSV-EXACT","summary":"bad release","affected":[
            {"package":{"ecosystem":"crates.io","name":"serde"},"versions":["1.0.0"]}]},
        {"id":"OSV-RANGE","summary":"range only","affected":[
            {"package":{"ecosystem":"crates.io","name":"tokio"},"ranges":[{"type":"SEMVER","events":[]}]}]}
    ]"#;

    #[test]
    fn cyclonedx_document_lists_purls_in_the_expected_shape() {
        let bom = to_cyclonedx(&[component("crates.io", "serde", "1.0.0")]);
        assert_eq!(bom["bomFormat"], "CycloneDX");
        assert_eq!(bom["specVersion"], "1.5");
        assert_eq!(bom["components"][0]["purl"], "pkg:cargo/serde@1.0.0");
    }

    #[test]
    fn exact_listed_version_is_affected_and_other_versions_are_clean() {
        let advisories = parse_osv(ADVISORIES).unwrap();
        let findings = match_advisories(&[component("crates.io", "serde", "1.0.0")], &advisories);
        assert!(findings.iter().any(|f| f.advisory == "OSV-EXACT" && f.verdict == Verdict::Affected));

        let clean = match_advisories(&[component("crates.io", "serde", "1.0.1")], &advisories);
        assert!(clean.iter().all(|f| f.advisory != "OSV-EXACT"));
    }

    #[test]
    fn range_only_advisories_are_unverified_not_clean() {
        let advisories = parse_osv(ADVISORIES).unwrap();
        let findings = match_advisories(&[component("crates.io", "tokio", "1.40.0")], &advisories);
        assert_eq!(
            findings,
            vec![Finding {
                purl: "pkg:cargo/tokio@1.40.0".into(),
                advisory: "OSV-RANGE".into(),
                verdict: Verdict::Unverified,
            }]
        );
    }

    #[test]
    fn malformed_advisories_are_refused() {
        assert!(parse_osv("not json").is_err());
        assert!(parse_osv(r#"{"summary":"no id"}"#).is_err());
        assert!(parse_osv(r#"[{"id":"X","affected":[{"versions":["1"]}]}]"#).is_err());
    }
}
