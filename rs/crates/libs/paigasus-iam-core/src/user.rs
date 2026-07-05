// SPDX-License-Identifier: Apache-2.0

//! The `User` entity — the human profile sharing a `Principal`'s identity (1:1).

use crate::value::{Email, PrincipalId};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub principal_id: PrincipalId,
    pub email: Email,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(principal_id: PrincipalId, email: Email, display_name: String, locale: Option<String>, timezone: Option<String>, created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> Self {
        User {
            principal_id,
            email,
            display_name,
            locale,
            timezone,
            created_at,
            updated_at,
        }
    }
}
