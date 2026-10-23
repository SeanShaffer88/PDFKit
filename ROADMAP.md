# PDFKit roadmap

## Next

- **Layout-aware extraction**: group shown text by line geometry
  (y coordinates) and reading order for multi-column pages, in
  addition to today's content-stream order.
- **Full Adobe Glyph List** for Differences resolution (v0.1 ships a
  common-names subset + `uniXXXX`).
- **LZWDecode** streams (small, well-specified) — removes the most
  common "unsupported filter" hit on older documents.

## Later

- Form field (AcroForm) reading: field names, values, flags.
- Outline/bookmark extraction.
- Image XObject inventory (and extraction for Flate/JPEG images).
- Writer growth: multiple fonts, text measurement/word-wrap helper,
  lines/rectangles, and embedding a TrueType font subset.
- Encrypted PDFs (standard security handler, RC4/AES) behind an
  explicit password parameter.

## Non-goals

- No page rendering — PDFKit is a data tool; pair it with ImageKit
  for pixel work.
- No editing-in-place of parsed documents in v0.x (the generator is
  the writing path).
