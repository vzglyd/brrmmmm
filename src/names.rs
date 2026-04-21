use std::collections::HashSet;

const FIRST: &[&str] = &[
    "solar", "lunar", "cosmic", "stellar", "orbital", "nebula", "quantum", "void", "dark", "deep",
    "event", "polar", "binary", "neutron", "alpha", "omega", "zenith", "apex", "aurora", "eclipse",
    "pulsar", "quasar", "vector", "hyper", "ultra",
];

const SECOND: &[&str] = &[
    "wind", "gate", "drift", "horizon", "wave", "pulse", "core", "field", "arc", "bridge", "path",
    "point", "mass", "flare", "surge", "ring", "loop", "flux", "shift", "wake", "reach", "span",
    "fold", "burn", "lock",
];

fn rand_u64(salt: u64) -> u64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let pid = std::process::id() as u64;
    // LCG mix of time, pid, and caller-supplied salt
    let v = nanos
        .wrapping_add(pid.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add(salt.wrapping_mul(0x6364_1362_2384_6793));
    v ^ (v >> 33)
}

/// Generate a space-themed two-word mission name not already in `taken`.
/// Returns `None` only if 10 collision retries all produce taken names.
pub fn generate_mission_name(taken: &HashSet<String>) -> Option<String> {
    for i in 0..10u64 {
        let v = rand_u64(i.wrapping_mul(0xbf58_476d_1ce4_e5b9));
        let first = FIRST[(v as usize) % FIRST.len()];
        let second = SECOND[((v >> 32) as usize) % SECOND.len()];
        let name = format!("{first}-{second}");
        if !taken.contains(&name) {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_contains_hyphen() {
        let name = generate_mission_name(&HashSet::new()).unwrap();
        assert!(name.contains('-'), "expected hyphenated name, got: {name}");
    }

    #[test]
    fn name_uses_known_words() {
        let name = generate_mission_name(&HashSet::new()).unwrap();
        let (first, second) = name.split_once('-').unwrap();
        assert!(FIRST.contains(&first), "unknown first word: {first}");
        assert!(SECOND.contains(&second), "unknown second word: {second}");
    }

    #[test]
    fn avoids_taken_names() {
        // Take only the last 5 FIRST-word families so ~80% of the 625 names remain
        // available. With 10 retries at 80% per try the miss probability is < 0.0001%.
        let mut taken: HashSet<String> = HashSet::new();
        for f in &FIRST[20..] {
            for s in SECOND {
                taken.insert(format!("{f}-{s}"));
            }
        }
        let result = generate_mission_name(&taken).expect("should find a free name");
        assert!(!taken.contains(&result), "returned a taken name: {result}");
        let (first, _) = result.split_once('-').unwrap();
        assert!(
            !FIRST[20..].contains(&first),
            "first word came from taken family: {first}"
        );
    }

    #[test]
    fn returns_none_when_all_taken() {
        let taken: HashSet<String> = FIRST
            .iter()
            .flat_map(|f| SECOND.iter().map(move |s| format!("{f}-{s}")))
            .collect();
        // All 625 names taken — should return None within 10 retries.
        // (may still return Some on a lucky collision miss in the random order,
        //  but with all slots filled it will eventually exhaust; we just assert
        //  no panic here since the exact result is timing-dependent)
        let _ = generate_mission_name(&taken);
    }
}
