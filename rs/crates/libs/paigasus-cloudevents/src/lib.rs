// SPDX-License-Identifier: Apache-2.0

//! CloudEvents 1.0 JSON (structured mode): parse, validate and build.
//!
//! This crate is pure. It has no I/O and no async code. It reads and writes the attributes of
//! the CloudEvents core spec, the three typed extension values, and the Paigasus extensions
//! `schemaversion`, `actorprn`, `correlationid` and `threadid`.

mod build;
mod error;
mod event;
mod parse;

pub use error::{ParseError, SchemaError};
pub use event::{CloudEvent, EXTENSION_NAME_MAX_CHARS, EXTENSION_STRING_MAX_CHARS, ExtensionValue, ID_MAX_CHARS, TYPE_MAX_CHARS, URI_MAX_CHARS};
pub use parse::parse;
