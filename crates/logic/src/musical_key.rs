//! Port of `src/lib/musical-key.ts`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CamelotKey {
    pub camelot: u8,
    /// `true` for A (minor), `false` for B (major).
    pub minor: bool,
}

pub const MAJOR_CAMELOT: [u8; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
pub const MINOR_CAMELOT: [u8; 12] = [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];

fn note_to_semitone(note: &str) -> Option<usize> {
    Some(match note {
        "c" | "b#" => 0,
        "c#" | "db" => 1,
        "d" => 2,
        "d#" | "eb" => 3,
        "e" | "fb" => 4,
        "e#" | "f" => 5,
        "f#" | "gb" => 6,
        "g" => 7,
        "g#" | "ab" => 8,
        "a" => 9,
        "a#" | "bb" => 10,
        "b" | "cb" => 11,
        _ => return None,
    })
}

/// `parseMusicalKey`: `Am`, `A minor`, `F#m`, `Db`, `C major`, `8A`, `8B`.
pub fn parse_musical_key(value: Option<&str>) -> Option<CamelotKey> {
    let normalized: String = value?.to_lowercase().replace('♯', "#").replace('♭', "b").chars().filter(|c| !c.is_whitespace()).collect();
    if normalized.is_empty() {
        return None;
    }
    if let Some(letter) = normalized.chars().last().filter(|c| *c == 'a' || *c == 'b') {
        let digits = &normalized[..normalized.len() - 1];
        if (1..=2).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
            let number: u8 = digits.parse().ok()?;
            if (1..=12).contains(&number) {
                return Some(CamelotKey { camelot: number, minor: letter == 'a' });
            }
        }
    }
    let (minor, root) = if let Some(r) = normalized.strip_suffix("minor") {
        (true, r)
    } else if let Some(r) = normalized.strip_suffix("min") {
        (true, r)
    } else if let Some(r) = normalized.strip_suffix("major") {
        (false, r)
    } else if let Some(r) = normalized.strip_suffix("maj") {
        (false, r)
    } else if normalized.len() > 1 && normalized.ends_with('m') {
        (true, &normalized[..normalized.len() - 1])
    } else {
        (false, normalized.as_str())
    };
    let semitone = note_to_semitone(root)?;
    Some(CamelotKey { camelot: if minor { MINOR_CAMELOT[semitone] } else { MAJOR_CAMELOT[semitone] }, minor })
}

/// `camelotDistance`: 0 identical, 1 adjacent/relative, 2 one step, 3 clash.
pub fn camelot_distance(a: CamelotKey, b: CamelotKey) -> u8 {
    if a == b {
        return 0;
    }
    if a.camelot == b.camelot {
        return 1;
    }
    let forward = (b.camelot as i32 - a.camelot as i32).rem_euclid(12);
    let backward = (a.camelot as i32 - b.camelot as i32).rem_euclid(12);
    let steps = forward.min(backward);
    if a.minor == b.minor {
        match steps {
            1 => 1,
            2 => 2,
            _ => 3,
        }
    } else if steps == 1 {
        2
    } else {
        3
    }
}

pub fn harmonic_compatibility(distance: u8) -> f64 {
    (1.0 - distance as f64 / 3.0).clamp(0.0, 1.0)
}

pub fn fold_tempo_ratio(ratio: f64) -> f64 {
    if !ratio.is_finite() || ratio <= 0.0 {
        return 1.0;
    }
    let mut folded = ratio;
    while folded < 0.75 {
        folded *= 2.0;
    }
    while folded > 1.5 {
        folded /= 2.0;
    }
    folded
}

pub fn tempo_compatibility(from: f64, to: f64) -> f64 {
    if !from.is_finite() || !to.is_finite() || from <= 0.0 || to <= 0.0 {
        return 0.0;
    }
    (1.0 - (fold_tempo_ratio(to / from) - 1.0).abs() / 0.25).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_measures_keys() {
        assert_eq!(parse_musical_key(Some("Am")), Some(CamelotKey { camelot: 8, minor: true }));
        assert_eq!(parse_musical_key(Some("C major")), Some(CamelotKey { camelot: 8, minor: false }));
        assert_eq!(parse_musical_key(Some("8B")), Some(CamelotKey { camelot: 8, minor: false }));
        assert_eq!(parse_musical_key(Some("F♯m")), Some(CamelotKey { camelot: 11, minor: true }));
        assert_eq!(parse_musical_key(Some("x")), None);
        let am = parse_musical_key(Some("Am")).unwrap();
        let c = parse_musical_key(Some("C")).unwrap();
        assert_eq!(camelot_distance(am, c), 1);
        assert_eq!(tempo_compatibility(70.0, 140.0), 1.0);
    }
}
