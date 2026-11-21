//! Qell editor core.
//!
//! This crate knows nothing about windows, pixels or language servers. It owns
//! the text ([`Buffer`]), where the user is in it ([`Cursor`], [`Selection`]),
//! how to undo what they did ([`history`]) and how to colour it
//! ([`syntax`]). Everything else in Qell builds on top of these types.

pub mod alea;
pub mod buffer;
pub mod comment;
pub mod conf;
pub mod history;
pub mod position;
pub mod structure;
pub mod syntax;
pub mod wrap;

pub use buffer::Buffer;
pub use position::{Cursor, Position, Selection};
pub use syntax::{HighlightSpan, Language, LanguageRegistry, Theme, TokenKind};
