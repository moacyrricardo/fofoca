//! Meeting labels (spec 007): free-form, many per meeting, normalized on save.

/// Longest label kept, in characters.
pub const MAX_LEN: usize = 32;

/// Trimmed, lower-case, inner whitespace → `-`, at most [`MAX_LEN`] characters.
/// `"Cliente BH"` → `cliente-bh`. Accents are kept. Empty → `None`.
pub fn normalize(s: &str) -> Option<String> {
    let joined = s.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase();
    let capped: String = joined.chars().take(MAX_LEN).collect();
    let capped = capped.trim_end_matches('-').to_string();
    (!capped.is_empty()).then_some(capped)
}

/// Normalize every label and drop empties and duplicates, keeping the first occurrence's order.
pub fn normalize_all<S: AsRef<str>>(labels: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        if let Some(n) = normalize(l.as_ref()) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("Cliente BH").as_deref(), Some("cliente-bh"));
        assert_eq!(normalize("  A  b ").as_deref(), Some("a-b"));
        assert_eq!(normalize("Reunião Diária").as_deref(), Some("reunião-diária"));
        assert_eq!(normalize("   "), None);
        assert_eq!(normalize(""), None);
        let long = "x".repeat(40);
        assert_eq!(normalize(&long).unwrap().chars().count(), MAX_LEN);
        // a cut that lands on a separator doesn't leave a dangling '-'
        assert_eq!(normalize(&format!("{} y", "x".repeat(31))).unwrap(), "x".repeat(31));
    }

    #[test]
    fn dedups_after_normalizing() {
        assert_eq!(normalize_all(&["Trabalho", "trabalho ", "1:1", "", "Cliente  BH"]), vec!["trabalho", "1:1", "cliente-bh"]);
    }
}
