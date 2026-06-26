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
    /// Maps `q` and optional `seller_wallet` query params to SQL binds.
    /// A full base58 pubkey in `q` (with no `seller_wallet` param) filters by seller exactly.
    pub fn from_query(
        category: Option<&str>,
        agent_friendly: Option<bool>,
        q: Option<String>,
        seller_wallet_param: Option<&str>,
    ) -> Self {
        let normalized = normalize_search(q);
        let explicit_seller = seller_wallet_param.map(str::trim).filter(|s| !s.is_empty());

        let (text_term, seller_wallet) = match (normalized.as_deref(), explicit_seller) {
            (None, None) => (None, None),
            (None, Some(w)) => (None, Some(w.to_string())),
            (Some(term), None) if is_wallet_search_term(term) => (None, Some(term.to_string())),
            (Some(term), None) => (Some(term.to_string()), None),
            (Some(term), Some(w)) => (Some(term.to_string()), Some(w.to_string())),
        };

        Self {
            category: category.map(str::to_string),
            agent_friendly,
            search_pattern: text_term.map(|t| search_like_pattern(&t)),
            seller_wallet,
        }
    }
}

/// True when `term` looks like a full Solana wallet pubkey (base58, 32 bytes).
pub fn is_wallet_search_term(term: &str) -> bool {
    if term.len() < 32 || term.len() > 44 {
        return false;
    }
    bs58::decode(term)
        .into_vec()
        .ok()
        .is_some_and(|bytes| bytes.len() == 32)
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
        let binds = ListingFilterBinds::from_query(None, None, Some("logo".into()), Some("AbC123"));
        let (suffix, next) = listing_filter_suffix(&binds, 1, false);
        assert!(suffix.contains("seller_wallet = $1"));
        assert!(suffix.contains("ILIKE $2"));
        assert_eq!(next, 3);
    }

    #[test]
    fn from_query_treats_full_wallet_as_seller_filter() {
        let wallet = "buyA5hR1Z9KtHQRBTmLkjsFfjAabDwdZtrRC6edqxAJ";
        let binds = ListingFilterBinds::from_query(None, None, Some(wallet.into()), None);
        assert_eq!(binds.seller_wallet.as_deref(), Some(wallet));
        assert!(binds.search_pattern.is_none());
    }

    #[test]
    fn from_query_keeps_text_search_for_non_wallet() {
        let binds = ListingFilterBinds::from_query(None, None, Some("cyberpunk".into()), None);
        assert!(binds.seller_wallet.is_none());
        assert_eq!(binds.search_pattern.as_deref(), Some("%cyberpunk%"));
    }

    #[test]
    fn from_query_combines_explicit_seller_with_text() {
        let wallet = "buyA5hR1Z9KtHQRBTmLkjsFfjAabDwdZtrRC6edqxAJ";
        let binds = ListingFilterBinds::from_query(None, None, Some("logo".into()), Some(wallet));
        assert_eq!(binds.seller_wallet.as_deref(), Some(wallet));
        assert_eq!(binds.search_pattern.as_deref(), Some("%logo%"));
    }
}
