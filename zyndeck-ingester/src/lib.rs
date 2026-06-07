//! Library side of the Zyndeck ingestion service.
//!
//! The ingestion pipeline is split into testable stages: [`pdf`] is the pdfium
//! I/O boundary that turns a PDF into raw text segments, and [`document`] is the
//! pure transformation of those segments into ordered, classified, cleaned text
//! ready for chunking and embedding. The binary ([`main`](../main.rs)) wires
//! these together behind the CLI.

pub mod document;
pub mod pdf;
