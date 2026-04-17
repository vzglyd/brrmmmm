use brrmmmm::events::{ms_to_iso8601, EnvVarStatus};

#[test]
fn ms_to_iso8601_unix_epoch() {
    assert_eq!(ms_to_iso8601(0), "1970-01-01T00:00:00.000Z");
}

#[test]
fn ms_to_iso8601_known_date() {
    // 946684800000 ms = 2000-01-01T00:00:00.000Z (Y2K timestamp)
    assert_eq!(ms_to_iso8601(946684800000), "2000-01-01T00:00:00.000Z");
}

#[test]
fn ms_to_iso8601_preserves_milliseconds() {
    // 1123 ms = 1.123s after epoch
    let result = ms_to_iso8601(1123);
    assert!(result.ends_with(".123Z"), "expected .123Z suffix, got: {result}");
}

#[test]
fn ms_to_iso8601_leap_year_feb_29() {
    // 951782400000 ms = 2000-02-29T00:00:00.000Z (2000 is a leap year)
    assert_eq!(ms_to_iso8601(951782400000), "2000-02-29T00:00:00.000Z");
}

#[test]
fn ms_to_iso8601_output_matches_format() {
    let result = ms_to_iso8601(1_700_000_000_000);
    // YYYY-MM-DDTHH:MM:SS.mmmZ is always 24 characters
    assert_eq!(result.len(), 24, "unexpected length: {result}");
    assert_eq!(&result[4..5], "-");
    assert_eq!(&result[7..8], "-");
    assert_eq!(&result[10..11], "T");
    assert_eq!(&result[13..14], ":");
    assert_eq!(&result[16..17], ":");
    assert_eq!(&result[19..20], ".");
    assert_eq!(&result[23..24], "Z");
}

#[test]
fn from_raw_env_empty() {
    let result = EnvVarStatus::from_raw_env(&[]);
    assert!(result.is_empty());
}

#[test]
fn from_raw_env_single_var_is_set_and_not_required() {
    let input = [("MY_VAR".to_string(), "hello".to_string())];
    let result = EnvVarStatus::from_raw_env(&input);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "MY_VAR");
    assert!(result[0].set);
    assert!(!result[0].required);
}

#[test]
fn from_raw_env_preserves_key_names_in_order() {
    let input = [
        ("FIRST".to_string(), "a".to_string()),
        ("SECOND".to_string(), "b".to_string()),
    ];
    let result = EnvVarStatus::from_raw_env(&input);
    assert_eq!(result[0].name, "FIRST");
    assert_eq!(result[1].name, "SECOND");
}
