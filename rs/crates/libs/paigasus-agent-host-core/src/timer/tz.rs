// SPDX-License-Identifier: Apache-2.0

//! The time zone resolver over the time zone database that `jiff` bundles.

use jiff::tz::{TimeZone, TimeZoneDatabase};

use crate::ports::{TzError, TzResolver};

/// Resolves a zone name from the bundled time zone database. The host zoneinfo is not read.
#[derive(Clone, Debug)]
pub struct BundledTzResolver {
    db: TimeZoneDatabase,
}

impl BundledTzResolver {
    #[must_use]
    pub fn new() -> Self {
        Self { db: TimeZoneDatabase::bundled() }
    }
}

impl Default for BundledTzResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TzResolver for BundledTzResolver {
    fn resolve(&self, name: &str) -> Result<TimeZone, TzError> {
        self.db.get(name).map_err(|_| TzError::Unknown { name: name.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_database_has_europe_berlin() {
        assert!(jiff::tz::TimeZoneDatabase::bundled().get("Europe/Berlin").is_ok());
    }

    #[test]
    fn the_resolver_finds_a_known_zone_and_refuses_an_unknown_one() {
        let r = BundledTzResolver::new();
        assert_eq!(r.resolve("europe/berlin").unwrap().iana_name(), Some("Europe/Berlin"));
        assert_eq!(r.resolve("Mars/Olympus"), Err(TzError::Unknown { name: "Mars/Olympus".into() }));
    }
}
