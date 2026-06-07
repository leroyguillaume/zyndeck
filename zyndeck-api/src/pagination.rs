use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Page size used when the caller does not specify one.
const DEFAULT_PER_PAGE: i64 = 50;
/// Hard cap on the page size, so a client can never request an unbounded page.
const MAX_PER_PAGE: i64 = 100;

/// Offset/limit pagination parameters, taken as query string `?page=&perPage=`.
///
/// Both fields default (rather than being nullable) so the generated schema
/// advertises sensible values instead of `null`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaginationQuery {
    /// 1-based page number.
    #[serde(default = "default_page")]
    pub page: i64,
    /// Number of items per page (clamped to a maximum).
    #[serde(default = "default_per_page")]
    pub per_page: i64,
}

fn default_page() -> i64 {
    1
}

fn default_per_page() -> i64 {
    DEFAULT_PER_PAGE
}

impl PaginationQuery {
    /// The resolved, 1-based page number.
    pub fn page(&self) -> i64 {
        self.page.max(1)
    }

    /// The resolved page size, clamped to `[1, MAX_PER_PAGE]`.
    pub fn per_page(&self) -> i64 {
        self.per_page.clamp(1, MAX_PER_PAGE)
    }

    /// Number of rows to take for this page.
    pub fn limit(&self) -> i64 {
        self.per_page()
    }

    /// Number of rows to skip for this page.
    pub fn offset(&self) -> i64 {
        (self.page() - 1) * self.per_page()
    }
}

/// A single page of results, with the metadata clients need to paginate.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    /// Items on this page.
    pub items: Vec<T>,
    /// 1-based page number this represents.
    pub page: i64,
    /// Page size used.
    pub per_page: i64,
    /// Total number of items across all pages.
    pub total: i64,
}

impl<T> Page<T> {
    /// Assembles a page from its items, the originating query, and the total.
    pub fn new(items: Vec<T>, query: &PaginationQuery, total: i64) -> Self {
        Self {
            items,
            page: query.page(),
            per_page: query.per_page(),
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_advertises_defaults_not_null() {
        let schema = serde_json::to_value(schemars::schema_for!(PaginationQuery)).unwrap();
        assert_eq!(schema["properties"]["page"]["default"], 1);
        assert_eq!(schema["properties"]["perPage"]["default"], DEFAULT_PER_PAGE);
    }

    #[test]
    fn resolves_page_to_offset_and_limit() {
        let query = PaginationQuery {
            page: 3,
            per_page: 20,
        };
        assert_eq!(query.limit(), 20);
        assert_eq!(query.offset(), 40);
    }

    #[test]
    fn per_page_is_clamped_to_the_maximum() {
        let query = PaginationQuery {
            page: 1,
            per_page: 10_000,
        };
        assert_eq!(query.per_page(), MAX_PER_PAGE);
    }
}
