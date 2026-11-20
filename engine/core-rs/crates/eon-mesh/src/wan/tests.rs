use super::*;

#[test]
fn iroh_and_qos_tests_only_an_explicit_relay_origin_is_valid() {
    for origin in [
        "https://relay.example.invalid.",
        "http://127.0.0.1:8080",
        "http://[::1]:8080",
    ] {
        assert!(validate_relay(&origin.parse().unwrap()).is_ok());
    }
    for origin in [
        "http://localhost",
        "http://relay.example.invalid",
        "ws://127.0.0.1",
        "https://0.0.0.0",
        concat!("https://", "operator@", "relay.example.invalid"),
        "https://relay.example.invalid/path",
        "https://relay.example.invalid/?option=1",
        "https://relay.example.invalid/#fragment",
        "https://relay.example.invalid:0",
        "https://224.0.0.1",
    ] {
        assert!(validate_relay(&origin.parse().unwrap()).is_err());
    }
}

#[test]
fn iroh_and_qos_tests_unknown_and_oversized_streams_are_rejected_at_header() {
    for header in [
        [0, 1, 0, 0, 0],
        [4, 1, 0, 0, 0],
        [1, 1, 0, 16, 0],
        [2, 1, 0, 64, 0],
        [3, 5, 0, 1, 0],
    ] {
        assert!(wire::parse_header(header).is_err());
    }
}
