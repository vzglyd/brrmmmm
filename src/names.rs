/// Validate a stable daemon mission name shared between producers and watchers.
pub fn validate_mission_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() {
        return Err("must be a non-empty string");
    }
    if name == "." || name == ".." {
        return Err("must not be '.' or '..'");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("must use only ASCII letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_mission_names() {
        assert!(validate_mission_name("vrx64-crypto").is_ok());
        assert!(validate_mission_name("crypto.v1_main").is_ok());
    }

    #[test]
    fn rejects_blank_names() {
        assert_eq!(
            validate_mission_name("   ").unwrap_err(),
            "must be a non-empty string"
        );
    }

    #[test]
    fn rejects_path_like_names() {
        assert_eq!(validate_mission_name("../crypto").unwrap_err(), "must use only ASCII letters, digits, '.', '_' or '-'");
        assert_eq!(validate_mission_name("vrx64/crypto").unwrap_err(), "must use only ASCII letters, digits, '.', '_' or '-'");
    }

    #[test]
    fn rejects_dot_segments() {
        assert_eq!(validate_mission_name(".").unwrap_err(), "must not be '.' or '..'");
        assert_eq!(validate_mission_name("..").unwrap_err(), "must not be '.' or '..'");
    }
}
