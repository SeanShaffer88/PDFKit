//! PDFKit — PDF parsing, text extraction, and generation behind
//! one clean C ABI, in pure Rust with zero dependencies.
//!
//! Reading: a tolerant object scanner (recovers documents whose
//! xref is damaged, the way real readers do), xref-stream and
//! object-stream support, FlateDecode (+ PNG predictors), a page-tree
//! walk, and text extraction honoring ToUnicode CMaps and WinAnsi
//! encoding. Writing: valid multi-page PDFs with the standard
//! Helvetica font and document metadata.
//!
//! Honest v0.1 limits are in the README (no rendering, no encrypted
//! PDFs, no JPEG image extraction).

pub mod extract;
pub mod ffi;
pub mod inflate;
pub mod pdf;
pub mod platform;
pub mod write;

pub const VERSION: &str = "0.1.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;

// Error codes (mirror include/pdfkit.h).
pub const PK_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_FOUND: i32 = 3;
pub const ERR_ACCESS_DENIED: i32 = 4;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;
/// Extension beyond the shared 0–6 set: the bytes are not a valid
/// (or not a supported) PDF.
pub const ERR_CORRUPT: i32 = 7;

/// The crate-wide result: failures carry a stable code + message,
/// rendered at the FFI boundary into the thread-local last error.
pub type CResult<T> = Result<T, (i32, String)>;

/// Escape a string for inclusion inside JSON double quotes.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_json() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
