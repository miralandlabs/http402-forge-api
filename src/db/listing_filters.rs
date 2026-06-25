/// Trim and sanitize free-text listing search (title / description `ILIKE`).
pub fn normalize_search(raw: Option<String>) -> Option<String> {
    let term = raw.as_deref()?.trim();
    if term.is_empty() {
        return None;
    }
    let cleaned: String = term
        .chars()
        .filter(|c| *c != '%' && *c != '_')
        .take(80)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

pub fn search_like_pattern(term: &str) -> String {
    format!("%{term}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_strips_wildcards() {
        assert_eq!(
            normalize_search(Some("  cyber%punk_  ".into())),
            Some("cyberpunk".into())
        );
    }

    #[test]
    fn normalize_empty_is_none() {
        assert_eq!(normalize_search(Some("   ".into())), None);
        assert_eq!(normalize_search(None), None);
    }
}
