//! Safe construction of the declared FTS5 query subset.

use std::fmt::{Display, Formatter};

/// Keyword term combination requested by the Worker API.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeywordMatchMode {
    /// Every normalized term must match.
    #[default]
    And,
    /// At least one normalized term must match.
    Or,
}

/// Failure to construct a bounded keyword query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeywordQueryError {
    /// The query contained no searchable alphanumeric terms.
    Empty,
    /// The query exceeded the configured term bound.
    TooManyTerms,
}

impl Display for KeywordQueryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("keyword query is empty"),
            Self::TooManyTerms => formatter.write_str("keyword query has too many terms"),
        }
    }
}

impl std::error::Error for KeywordQueryError {}

/// Tokenize user text and construct only quoted FTS5 literals joined by the
/// declared `AND` or `OR` operator.
///
/// Raw user text never becomes FTS5 syntax, so column selectors, `NEAR`,
/// prefixes, parentheses, and wildcard operators cannot be injected.
pub fn build_fts_query(
    input: &str,
    mode: KeywordMatchMode,
    max_terms: usize,
) -> Result<String, KeywordQueryError> {
    let mut terms = Vec::new();
    for term in input
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
    {
        if terms.len() == max_terms {
            return Err(KeywordQueryError::TooManyTerms);
        }
        terms.push(term.to_lowercase());
    }
    if terms.is_empty() {
        return Err(KeywordQueryError::Empty);
    }
    let operator = match mode {
        KeywordMatchMode::And => " AND ",
        KeywordMatchMode::Or => " OR ",
    };
    Ok(terms
        .into_iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(operator))
}
