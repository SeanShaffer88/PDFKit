//! Platform layer for PDFKit: deliberately thin — the parser,
//! extractor, and writer are pure Rust; only file I/O error mapping
//! lives here.

use crate::{CResult, ERR_ACCESS_DENIED, ERR_NOT_FOUND, ERR_PLATFORM};

pub fn read_file(path: &str) -> CResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => (ERR_NOT_FOUND, format!("file not found: '{path}'")),
        std::io::ErrorKind::PermissionDenied => (
            ERR_ACCESS_DENIED,
            format!("permission denied reading '{path}'"),
        ),
        _ => (ERR_PLATFORM, format!("cannot read '{path}': {e}")),
    })
}

pub fn write_file(path: &str, bytes: &[u8]) -> CResult<()> {
    std::fs::write(path, bytes).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => (
            ERR_ACCESS_DENIED,
            format!("permission denied writing '{path}'"),
        ),
        _ => (ERR_PLATFORM, format!("cannot write '{path}': {e}")),
    })
}
