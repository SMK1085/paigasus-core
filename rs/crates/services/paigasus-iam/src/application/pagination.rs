// SPDX-License-Identifier: Apache-2.0

//! Pagination parameters with bounds checking.

use crate::application::error::TenancyError;

/// Default page limit when none specified.
pub const DEFAULT_LIMIT: u64 = 50;

/// Maximum allowed page limit.
pub const MAX_LIMIT: u64 = 200;

/// Pagination parameters: limit (page size) and offset (skip count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub limit: u64,
    pub offset: u64,
}

impl Page {
    /// Create a new page with limit and offset, applying bounds checks.
    ///
    /// - `limit`: None → DEFAULT_LIMIT (50); accepts 1..=200; else InvalidPagination
    /// - `offset`: None → 0; accepts >= 0; negative → InvalidPagination
    pub fn new(limit: Option<i64>, offset: Option<i64>) -> Result<Self, TenancyError> {
        let limit = match limit {
            None => DEFAULT_LIMIT,
            Some(l) if l >= 1 && l <= MAX_LIMIT as i64 => l as u64,
            Some(_) => return Err(TenancyError::InvalidPagination),
        };

        let offset = match offset {
            None => 0,
            Some(o) if o >= 0 => o as u64,
            Some(_) => return Err(TenancyError::InvalidPagination),
        };

        Ok(Page { limit, offset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_bounds() {
        assert_eq!(Page::new(None, None).unwrap().limit, 50);
        assert_eq!(Page::new(Some(200), Some(10)).unwrap().offset, 10);
        for (l, o) in [(Some(0), None), (Some(201), None), (Some(-1), None), (None, Some(-1))] {
            assert_eq!(Page::new(l, o).unwrap_err(), TenancyError::InvalidPagination);
        }
    }

    #[test]
    fn page_defaults() {
        let page = Page::new(None, None).unwrap();
        assert_eq!(page.limit, DEFAULT_LIMIT);
        assert_eq!(page.offset, 0);
    }

    #[test]
    fn page_accepts_valid_limits() {
        for limit in [1, 50, 100, 200] {
            let page = Page::new(Some(limit), None).unwrap();
            assert_eq!(page.limit, limit as u64);
        }
    }

    #[test]
    fn page_accepts_valid_offsets() {
        for offset in [0, 1, 10, 100, 1000] {
            let page = Page::new(None, Some(offset)).unwrap();
            assert_eq!(page.offset, offset as u64);
        }
    }

    #[test]
    fn page_rejects_out_of_bounds_limits() {
        assert!(Page::new(Some(0), None).is_err());
        assert!(Page::new(Some(201), None).is_err());
        assert!(Page::new(Some(-1), None).is_err());
        assert!(Page::new(Some(1000), None).is_err());
    }

    #[test]
    fn page_rejects_negative_offsets() {
        assert!(Page::new(None, Some(-1)).is_err());
        assert!(Page::new(Some(50), Some(-100)).is_err());
    }
}
