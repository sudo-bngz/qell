//! Language Server Protocol client for Qell.
//!
//! One [`Client`] per running language server: it owns the child process,
//! a writer thread that frames outgoing JSON-RPC messages, and a reader
//! thread that parses incoming ones. Responses to requests are matched by
//! id and delivered as [`Event`]s on a channel the UI polls once per frame,
//! so nothing here ever blocks the render loop except `initialize`, which is
//! done once per server on a background thread anyway.
//!
//! The [`Registry`] maps language ids to clients and starts them lazily the
//! first time a file of that language is opened.

pub mod client;
pub mod convert;
pub mod registry;

pub use client::{Client, Event, RequestId};
pub use registry::Registry;

pub use lsp_types;
