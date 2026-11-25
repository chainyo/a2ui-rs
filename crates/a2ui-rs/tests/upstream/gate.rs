use std::collections::BTreeSet;

use serde::Deserialize;

use super::{Record, TestError, scaling};

const APPROVED: &str = include_str!("../../../../fixtures/local/conformance-exceptions.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exception {
    suite: String,
    case: String,
    status: String,
    reason: String,
}

pub(super) struct Exceptions(Vec<Exception>);

impl Exceptions {
    pub(super) fn load() -> Result<Self, TestError> {
        Self::parse(APPROVED)
    }

    fn parse(source: &str) -> Result<Self, TestError> {
        let entries: Vec<Exception> = serde_json::from_str(source)?;
        let mut keys = BTreeSet::new();
        for exception in &entries {
            let Exception {
                suite,
                case,
                status,
                reason,
            } = exception;
            if suite.trim().is_empty()
                || case.trim().is_empty()
                || !matches!(status.as_str(), "skipped" | "xfailed")
                || reason.trim().is_empty()
            {
                return Err(format!("{suite}::{case}: invalid Core conformance exception").into());
            }
            if !keys.insert((suite, case)) {
                return Err(
                    format!("{suite}::{case}: duplicate Core conformance exception").into(),
                );
            }
        }
        Ok(Self(entries))
    }

    pub(super) fn reason_for(&self, suite: &str, case: &str, status: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|entry| entry.suite == suite && entry.case == case && entry.status == status)
            .map(|entry| entry.reason.as_str())
    }

    pub(super) fn verify(&self, records: &[Record]) -> Result<(), TestError> {
        let mut used = BTreeSet::new();
        let mut errors = Vec::new();
        for record in records.iter().filter(|record| record.required_for_release) {
            let suite = record.suite.as_str();
            let case = record.case.as_str();
            match record.status {
                "passed" => {}
                status if self.reason_for(suite, case, status).is_some() => {
                    used.insert((suite, case));
                }
                status => errors.push(format!(
                    "{suite}::{case}: {status} with no matching Core conformance exception"
                )),
            }
        }
        for entry in &self.0 {
            if !used.contains(&(entry.suite.as_str(), entry.case.as_str())) {
                errors.push(format!(
                    "{}::{}: unused Core conformance exception",
                    entry.suite, entry.case
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("v1.0 Core conformance gate failed:\n{}", errors.join("\n")).into())
        }
    }
}

pub(super) fn requires_core_case(suite: &str, action: &str, legacy: bool) -> bool {
    !legacy
        && !matches!(
            (suite, action),
            ("core/accessibility.yaml", "accessibility_check")
                | ("core/adapter_conformance.yaml", "render_surface")
                | (scaling::SUITE, "measure_growth")
        )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Exceptions, Record, TestError, requires_core_case, scaling};

    const SUITE: &str = "core/catalog.yaml";

    fn result(case: &str, status: &'static str) -> Record {
        Record {
            harness: "a2ui-rs-protocol",
            suite: SUITE.to_owned(),
            case: case.to_owned(),
            status,
            reason: Some("Reason from the runner".to_owned()),
            required_for_release: true,
        }
    }

    fn exception(case: &str, status: &str, reason: &str) -> Value {
        json!({"suite": SUITE, "case": case, "status": status, "reason": reason})
    }

    #[test]
    fn rejects_a_new_skip_even_with_a_reason_from_the_runner() -> Result<(), TestError> {
        let exceptions = Exceptions::parse("[]")?;
        let error = exceptions
            .verify(&[result("new_v1_case", "skipped")])
            .err()
            .ok_or("expected the gate to reject the skip")?;
        assert!(
            error
                .to_string()
                .contains("core/catalog.yaml::new_v1_case: skipped")
        );
        Ok(())
    }

    #[test]
    fn exceptions_must_match_the_case_and_result() -> Result<(), TestError> {
        let entries = json!([exception(
            "catalog_case",
            "xfailed",
            "Fixture and schema differ."
        )]);
        let exceptions = Exceptions::parse(&entries.to_string())?;
        exceptions.verify(&[
            result("catalog_case", "xfailed"),
            result("other_case", "passed"),
        ])?;
        for record in [
            result("other_case", "xfailed"),
            result("catalog_case", "skipped"),
            result("catalog_case", "failed"),
            result("catalog_case", "passed"),
        ] {
            let error = exceptions
                .verify(&[record])
                .err()
                .ok_or("expected a gate failure")?;
            assert!(
                error
                    .to_string()
                    .contains("unused Core conformance exception")
            );
        }
        assert!(exceptions.verify(&[]).is_err());
        Ok(())
    }

    #[test]
    fn each_exception_requires_a_reason_and_a_unique_case() -> Result<(), TestError> {
        let approved = exception(
            "catalog_case",
            "skipped",
            "This catalog type requires implementation.",
        );
        for entries in [
            json!([exception("catalog_case", "skipped", "  ")]),
            json!([exception(
                "catalog_case",
                "failed",
                "Cannot waive a failure."
            )]),
            json!([approved, approved]),
        ] {
            assert!(Exceptions::parse(&entries.to_string()).is_err());
        }
        Exceptions::parse(&json!([approved]).to_string())?
            .verify(&[result("catalog_case", "skipped")])?;
        Ok(())
    }

    #[test]
    fn only_known_platform_and_timing_cases_leave_core_scope() {
        assert!(!requires_core_case(SUITE, "validate", true));
        assert!(!requires_core_case(
            "core/adapter_conformance.yaml",
            "render_surface",
            false
        ));
        assert!(!requires_core_case(
            "core/accessibility.yaml",
            "accessibility_check",
            false
        ));
        assert!(!requires_core_case(scaling::SUITE, "measure_growth", false));
        assert!(requires_core_case(
            "core/accessibility.yaml",
            "validate",
            false
        ));
        assert!(requires_core_case(
            "core/new_suite.yaml",
            "new_action",
            false
        ));
    }
}
