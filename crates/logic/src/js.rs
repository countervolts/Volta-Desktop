//! Small helpers that reproduce JavaScript semantics the web code relies on.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Characters `encodeURIComponent` leaves alone: `A-Z a-z 0-9 - _ . ! ~ * ' ( )`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub fn encode_uri_component(value: &str) -> String {
    utf8_percent_encode(value, URI_COMPONENT).to_string()
}

/// `Number(string)`: trims whitespace, treats the empty string as 0, and
/// returns NaN for anything that is not a JS numeric literal.
pub fn js_number(value: &str) -> f64 {
    let text = value.trim();
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // Rust accepts "inf"/"nan"; JS does not.
    if text
        .chars()
        .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
    {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// `Math.round`: rounds half toward positive infinity.
pub fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_encode_uri_component() {
        assert_eq!(
            encode_uri_component("https://music.test"),
            "https%3A%2F%2Fmusic.test"
        );
        assert_eq!(encode_uri_component("Listener"), "Listener");
        assert_eq!(encode_uri_component("a b*(c)~'!"), "a%20b*(c)~'!");
        assert_eq!(encode_uri_component("é"), "%C3%A9");
    }

    #[test]
    fn number_semantics() {
        assert_eq!(js_number(""), 0.0);
        assert_eq!(js_number(" 90 "), 90.0);
        assert_eq!(js_number("1e2"), 100.0);
        assert!(js_number("inf").is_nan());
        assert!(js_number("abc").is_nan());
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
    }
}
