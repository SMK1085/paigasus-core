// SPDX-License-Identifier: Apache-2.0

/// A CloudEvents attribute that breaks the spec or a limit of this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error("missing required attribute `{0}`")]
    MissingAttribute(&'static str),
    #[error("attribute `{0}` has the wrong type")]
    WrongType(&'static str),
    #[error("specversion must be \"1.0\"")]
    SpecVersion,
    #[error("attribute `{attribute}` is longer than {max} characters")]
    TooLong { attribute: String, max: usize },
    #[error("`{0}` is not a valid extension attribute name")]
    InvalidExtensionName(String),
    #[error("data_base64 is not supported")]
    DataBase64NotSupported,
    #[error("time is not a valid RFC 3339 timestamp")]
    InvalidTime,
    #[error("attribute `{0}` must not be empty")]
    Empty(&'static str),
    #[error("attribute `{0}` contains a character that CloudEvents does not allow in a string")]
    InvalidCharacter(String),
    #[error("attribute `{0}` must be an absolute URI")]
    NotAbsoluteUri(&'static str),
}

/// The reason `parse` rejects a message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error(transparent)]
    Schema(#[from] SchemaError),
}
