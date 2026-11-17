//! Property tests for the information-flow lattice and the Rule-of-Two gate (docs/implementation/01, section 5).

use eon_capability::ifc::labels::{ConfidentialityLevel, ExtractedValue, IntegrityLevel, SecurityLabel};
use eon_capability::ifc::rule_of_two::{Capability, ControlParameter, Decision, Reason, TaskProfile, decide, properties};
use proptest::prelude::*;

fn integrity() -> impl Strategy<Value = IntegrityLevel> {
    prop_oneof![
        Just(IntegrityLevel::Untrusted),
        Just(IntegrityLevel::Endorsed),
        Just(IntegrityLevel::Trusted),
    ]
}

fn confidentiality() -> impl Strategy<Value = ConfidentialityLevel> {
    prop_oneof![
        Just(ConfidentialityLevel::Public),
        Just(ConfidentialityLevel::Workspace),
        Just(ConfidentialityLevel::Restricted),
    ]
}

fn label() -> impl Strategy<Value = SecurityLabel> {
    (integrity(), confidentiality(), prop::collection::vec("[a-z]{1,4}", 0..6))
        .prop_map(|(i, c, p)| SecurityLabel::new(i, c, p))
}

fn capability() -> impl Strategy<Value = Capability> {
    prop_oneof![
        Just(Capability::ReadWorkspace),
        Just(Capability::MutateWorkspace),
        Just(Capability::ExecuteProcess),
        Just(Capability::DeviceControl),
        (any::<bool>(), any::<bool>()).prop_map(|(pinned, query_parameterized)| Capability::NetworkAccess {
            pinned,
            query_parameterized,
        }),
    ]
}

fn profile() -> impl Strategy<Value = TaskProfile> {
    (
        label(),
        any::<bool>(),
        any::<bool>(),
        prop::collection::vec(capability(), 0..4),
        prop::collection::vec((label(), "[a-z]{1,6}"), 0..3),
        any::<bool>(),
    )
        .prop_map(|(context, resolves_secret, reads_outside_sandbox, capabilities, params, air_gapped)| TaskProfile {
            context,
            resolves_secret,
            reads_outside_sandbox,
            capabilities,
            control_parameters: params
                .into_iter()
                .map(|(label, name)| ControlParameter { name, label })
                .collect(),
            air_gapped,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn join_is_commutative(a in label(), b in label()) {
        prop_assert_eq!(a.join(&b).integrity, b.join(&a).integrity);
        prop_assert_eq!(a.join(&b).confidentiality, b.join(&a).confidentiality);
    }

    #[test]
    fn join_is_associative(a in label(), b in label(), c in label()) {
        let left = a.join(&b).join(&c);
        let right = a.join(&b.join(&c));
        prop_assert_eq!(left.integrity, right.integrity);
        prop_assert_eq!(left.confidentiality, right.confidentiality);
    }

    #[test]
    fn join_is_idempotent_on_levels(a in label()) {
        let twice = a.join(&a);
        prop_assert_eq!(twice.integrity, a.integrity);
        prop_assert_eq!(twice.confidentiality, a.confidentiality);
    }

    /// The joined label can never claim more trust or less secrecy than either input.
    #[test]
    fn join_never_upgrades_integrity_or_downgrades_secrecy(a in label(), b in label()) {
        let joined = a.join(&b);
        prop_assert!(joined.integrity <= a.integrity && joined.integrity <= b.integrity);
        prop_assert!(joined.confidentiality >= a.confidentiality && joined.confidentiality >= b.confidentiality);
    }

    /// Endorsing a restricted scalar never lowers integrity and never drops provenance.
    #[test]
    fn endorsement_is_monotone(a in label(), value in -1000i64..1000) {
        let endorsed = a.endorse(&ExtractedValue::Integer(value)).unwrap();
        prop_assert!(endorsed.integrity >= a.integrity);
        prop_assert_eq!(endorsed.provenance, a.provenance);
    }

    /// The triad rule is absolute. Whenever all three properties hold, no profile may be allowed.
    #[test]
    fn triad_is_never_allowed(p in profile()) {
        let props = properties(&p);
        if props.processes_untrusted && props.accesses_sensitive && props.mutates_or_egresses {
            prop_assert_eq!(decide(&p), Decision::RequireRawHumanGate(Reason::RuleOfTwoTriadViolation));
        }
    }

    /// An allowed decision must never coexist with an untrusted value steering a mutating or egressing action
    /// outside an air gap.
    #[test]
    fn allowed_profiles_have_no_untrusted_steering_outside_an_air_gap(p in profile()) {
        if decide(&p) == Decision::Allow {
            let props = properties(&p);
            let steered = p.control_parameters.iter().any(|c| c.label.integrity == IntegrityLevel::Untrusted);
            prop_assert!(!(props.processes_untrusted && props.mutates_or_egresses && steered && !p.air_gapped));
        }
    }
}

#[test]
fn every_property_combination_matches_the_truth_table() {
    let trusted = SecurityLabel::new(IntegrityLevel::Trusted, ConfidentialityLevel::Public, vec![]);
    let web = SecurityLabel::external("https://example.test");
    for bits in 0u8..8 {
        let a = bits & 1 != 0;
        let b = bits & 2 != 0;
        let c = bits & 4 != 0;
        let profile = TaskProfile {
            context: if a { web.clone() } else { trusted.clone() },
            resolves_secret: b,
            reads_outside_sandbox: false,
            capabilities: if c { vec![Capability::MutateWorkspace] } else { vec![Capability::ReadWorkspace] },
            control_parameters: vec![],
            air_gapped: false,
        };
        let props = properties(&profile);
        assert_eq!(props.processes_untrusted, a, "A");
        assert_eq!(props.accesses_sensitive, b, "B");
        assert_eq!(props.mutates_or_egresses, c, "C");
        let expected = if a && b && c {
            Decision::RequireRawHumanGate(Reason::RuleOfTwoTriadViolation)
        } else {
            Decision::Allow
        };
        assert_eq!(decide(&profile), expected, "bits {bits:03b}");
    }
}
