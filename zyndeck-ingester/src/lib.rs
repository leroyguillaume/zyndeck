//! Library side of the Zyndeck ingestion service.
//!
//! The ingestion pipeline is split into testable stages: [`pdf`] is the pdfium
//! I/O boundary that turns a PDF into raw text segments, [`document`] is the
//! pure transformation of those segments into ordered, classified, cleaned text,
//! [`chunk`] splits the reviewed transcript into retrieval chunks, and [`embed`]
//! turns those chunks into vectors via a local model. The binary
//! ([`main`](../main.rs)) wires these together behind the CLI.

pub mod chunk;
pub mod document;
pub mod embed;
pub mod pdf;
