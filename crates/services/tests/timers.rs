use std::time::Duration;
use rind_services::parse_duration;

#[test]
fn parse_duration_supports_units_and_raw_seconds() {
  assert_eq!(parse_duration("5s"), Some(Duration::from_secs(5)));
  assert_eq!(parse_duration("3m"), Some(Duration::from_secs(180)));
  assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
  assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86400)));
  assert_eq!(parse_duration("12"), Some(Duration::from_secs(12)));
}

#[test]
fn parse_duration_rejects_invalid_values() {
  assert_eq!(parse_duration(""), None);
  assert_eq!(parse_duration("foo"), None);
  assert_eq!(parse_duration("xs"), None);
}

#[test]
fn parse_duration_property_seconds_equivalence() {
  for n in 0u64..500 {
    let raw = parse_duration(&n.to_string());
    let with_suffix = parse_duration(&format!("{n}s"));
    assert_eq!(raw, Some(Duration::from_secs(n)));
    assert_eq!(with_suffix, raw);
  }
}
