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

#[derive(Debug, Clone)]
pub struct ListingFilterBinds {
    pub category: Option<String>,
    pub agent_friendly: Option<bool>,
    pub search_pattern: Option<String>,
    pub seller_wallet: Option<String>,
}

impl ListingFilterBinds {
    pub fn new(
        category: Option<&str>,
        agent_friendly: Option<bool>,
        search: Option<&str>,
        seller_wallet: Option<&str>,
    ) -> Self {
        Self {
            category: category.map(str::to_string),
            agent_friendly,
            search_pattern: search.map(search_like_pattern),
            seller_wallet: seller_wallet.map(str::to_string),
        }
    }
}

/// Builds ` AND …` for postgres (`$n`) or sqlite (`?n`), starting at `start_idx`.
/// Returns (suffix, next placeholder index).
pub fn listing_filter_suffix(
    binds: &ListingFilterBinds,
    start_idx: usize,
    sqlite: bool,
) -> (String, usize) {
    let mut parts = Vec::new();
    let mut idx = start_idx;
    let ph = |n: usize| {
        if sqlite {
            format!("?{n}")
        } else {
            format!("${n}")
        }
    };

    if binds.category.is_some() {
        parts.push(format!("category = {}", ph(idx)));
        idx += 1;
    }
    if binds.agent_friendly.is_some() {
        parts.push(format!("agent_friendly = {}", ph(idx)));
        idx += 1;
    }
    if binds.seller_wallet.is_some() {
        parts.push(format!("seller_wallet = {}", ph(idx)));
        idx += 1;
    }
    if binds.search_pattern.is_some() {
        let p = ph(idx);
        if sqlite {
            parts.push(format!(
                "(title LIKE {p} ESCAPE '\\' OR description LIKE {p} ESCAPE '\\')"
            ));
        } else {
            parts.push(format!("(title ILIKE {p} OR description ILIKE {p})"));
        }
        idx += 1;
    }

    if parts.is_empty() {
        (String::new(), idx)
    } else {
        (format!(" AND {}", parts.join(" AND ")), idx)
    }
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

    #[test]
    fn suffix_includes_seller_wallet_before_search() {
        let binds = ListingFilterBinds::new(None, None, Some("logo"), Some("AbC123"));
        let (suffix, next) = listing_filter_suffix(&binds, 1, false);
        assert!(suffix.contains("seller_wallet = $1"));
        assert!(suffix.contains("ILIKE $2"));
        assert_eq!(next, 3);
    }
}
