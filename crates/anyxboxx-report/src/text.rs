//! Small helpers that reproduce Python's behaviour byte for byte, so the reports and SVG images
//! written in Rust equal the ones `scarlett.py` writes on the same inputs.

use std::path::Path;

/// `xml.sax.saxutils.escape`: `&`, `<` and `>` only (quotes are left as they are).
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Python 3.12's `sum()` of floats: Neumaier's compensated summation, the compensation added
/// once at the end. A plain left-to-right sum can differ in the last bit, which moves an SVG
/// coordinate printed with one decimal.
pub fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut values = values.into_iter();
    let Some(mut total) = values.next() else {
        return 0.0;
    };
    let mut compensation = 0.0;
    for x in values {
        let t = total + x;
        if total.abs() >= x.abs() {
            compensation += (total - t) + x;
        } else {
            compensation += (x - t) + total;
        }
        total = t;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

/// Python's `round(x)` on a float: nearest integer, ties to even.
pub fn py_round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// `Path.write_text(text, encoding="utf-8")`: text mode, so `\n` becomes `\r\n` on Windows.
pub fn write_text(path: &Path, text: &str) -> std::io::Result<()> {
    let text = if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    std::fs::write(path, text)
}

/// Python's `str.capitalize()` for the ASCII labels of the reports.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// `f"{n:,}"`: thousands separated by commas. jour_j.py then replaces every comma of the line
/// with a space, the ones of the text around included (see `spaced`).
pub fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `line.replace(",", " ")`.
pub fn spaced(line: &str) -> String {
    line.replace(',', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_leaves_quotes() {
        assert_eq!(escape("a<b>&\"c\""), "a&lt;b&gt;&amp;\"c\"");
    }

    #[test]
    fn py_sum_compensates_like_python() {
        // Python 3.12: sum([0.1] * 10) == 1.0, a plain loop gives 0.9999999999999999.
        assert_eq!(py_sum([0.1; 10]), 1.0);
        assert_eq!(py_sum([1e100, 1.0, -1e100, 1.0]), 2.0);
        assert_eq!(py_sum(std::iter::empty()), 0.0);
    }

    #[test]
    fn py_round_ties_to_even() {
        assert_eq!((py_round(0.5), py_round(1.5), py_round(2.5)), (0, 2, 2));
        assert_eq!(py_round(2.9999999999999996), 3);
    }

    #[test]
    fn capitalize_lowers_the_rest() {
        assert_eq!(capitalize("AnyXboxX"), "Anyxboxx");
        assert_eq!(capitalize("paquet"), "Paquet");
    }

    #[test]
    fn thousands_are_grouped_then_spaced() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(
            spaced(&format!("`a,b`, {}", thousands(1234567))),
            "`a b`  1 234 567"
        );
    }
}
