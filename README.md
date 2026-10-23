# PDFKit

PDF parsing, text extraction, and generation behind one clean C
ABI — in pure Rust with **zero dependencies**. DLL #15 in the
Windows DLL Collection.

PDFKit opens real-world PDFs (including files with damaged xref
data), walks their page trees, reads metadata and page geometry,
extracts text page by page honoring embedded ToUnicode CMaps — and
writes valid multi-page PDFs of its own with positioned text and
document metadata.

## ABI (`include/pdfkit.h`, prefix `pk_`)

Parsed documents and in-progress generators live behind the same
`uint64_t` handles (`pk_is_generator` tells them apart); page
indices are 0-based.

| Function | Purpose |
|---|---|
| `pk_open_file(path)` / `pk_open_bytes(data, len)` | Parse a PDF → handle, or 0 on failure. |
| `pk_close(handle)` | Release either kind of handle. |
| `pk_page_count(handle)` | Page count (−1 on error). |
| `pk_info_json(handle)` | `{"pages":N,"version":"1.7","title":…,"author":…,"creator":…,"producer":…}`. |
| `pk_page_size(handle, page)` | `{"width":…,"height":…,"rotate":…}` in points. |
| `pk_page_text(handle, page)` | Extracted text of one page (generators report the text drawn so far). |
| `pk_create()` | Start a generator. |
| `pk_set_info(handle, title, author)` | Metadata for the next render. |
| `pk_add_page(handle, w, h)` | Append a page (points); returns its index. |
| `pk_draw_text(handle, page, x, y, size, text)` | Draw text — origin bottom-left, Helvetica. |
| `pk_render(handle, &ptr, &len)` | Render to a caller-owned buffer (`pk_free_buffer`). |
| `pk_save(handle, path)` | Render straight to a file. |
| `pk_version*`, `pk_last_error*`, `pk_free_string` | Version info, thread-local errors, disposal. |

Errors carry the shared 0–6 codes plus `PK_ERR_CORRUPT` (7).

## Reading: what works (v0.1)

- **Tolerant loading**: the loader scans for indirect objects and
  rebuilds the document from the trailer, so broken xref tables
  don't matter. Classic trailers **and** xref streams; **object
  streams** (compressed object collections) are expanded.
- **Streams**: FlateDecode (with PNG predictors) and ASCII85Decode.
  LZW/JPEG image streams and encrypted PDFs are precise
  `NOT_SUPPORTED` errors, never garbage output.
- **Page tree**: full inheritance of MediaBox/Resources/Rotate.
- **Text extraction**: content-stream execution tracking fonts and
  line moves; **ToUnicode CMaps** (bfchar + bfrange, the mechanism
  subset-font PDFs rely on), encoding **Differences** with glyph-name
  resolution, and WinAnsi decoding. Line breaks follow text-position
  operators (`Td`/`Tm`/`T*`); word gaps inside `TJ` arrays become
  spaces by the standard heuristic.
- **Metadata**: Info-dict strings decoded as UTF-16BE (with BOM) or
  PDFDocEncoding.

## Writing: what works (v0.1)

- Multi-page PDFs (PDF 1.4, classic xref with byte-exact offsets),
  page sizes up to the 14400-point spec limit, positioned text in
  the base-14 Helvetica (never embedded), WinAnsi string encoding
  (characters outside WinAnsi are substituted), and Title/Author/
  Creator/Producer metadata.

## Verification

- 11 unit tests: object/value parsing incl. reference folding,
  ToUnicode bfchar/bfrange resolution, WinAnsi and metadata-string
  decoding, writer validation, and a full **generate → parse →
  extract** round trip (plus the same through the raw FFI).
- **Interop, both directions, with mainstream tools**: the committed
  fixture `tests/fixtures/matplotlib.pdf` was produced by
  **matplotlib** (subset CID fonts, ToUnicode CMaps, compressed
  streams); PDFKit reads its pages, metadata, and text exactly
  (asserted in CI). Conversely, a PDFKit-generated file was validated
  by **poppler** (`pdfinfo` reports it cleanly — *Suspects: no* —
  and `pdftotext` extracts its text exactly; verified locally).
- Gate (WinDevKit `tools/gate.sh`): fmt, clippy (host + Windows
  target), full suite — green. Pure logic, no Win32 calls, so the
  host gate and CI's Windows build cover the same paths.

## Not in v0.1

Rendering/pages-as-images, form fields and annotations, digital
signatures, encrypted PDFs, JPEG/DCT stream decoding, LZW, JBIG2 —
see ROADMAP.md.

## License

MIT © 2026 Sean Shaffer.
