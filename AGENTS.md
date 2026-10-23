# PDFKit — agent notes

- Zero dependencies is a design rule: the parser (pdf.rs), extractor
  (extract.rs), writer (write.rs), and the zlib inflater (inflate.rs,
  ported from ImageKit) are hand-written. Verification anchors: the
  matplotlib fixture in tests/fixtures/ (regenerate ONLY via
  tools/make_fixture.py; pdfinfo is the external oracle for both the
  fixture and our writer's output), the generate→parse→extract round
  trips, and the ToUnicode unit tests.
- LOADER RULE: scanning + trailer reconstruction is the primary load
  path, not a fallback — don't "optimize" it into trusting xref
  offsets, robustness on real-world files is the point. Direct
  objects win over ObjStm members (or_insert).
- Extraction honesty: text order is content-stream order; line
  breaks are heuristic (Td/Tm/T* + TJ gap threshold −120). Don't
  claim layout analysis. Unknown CID codes become U+FFFD, never
  dropped silently into plausible-looking text.
- ERR_CORRUPT = 7 is a deliberate local extension of the shared 0–6
  codes (house precedent); header and lib.rs stay in sync.
- Writer correctness is byte-offset discipline: every object offset
  is recorded at emit time; poppler's "Suspects: no" is the
  acceptance bar for any change to render().
- Gate: WinDevKit tools/gate.sh (fmt + clippy -D warnings BOTH
  targets + FULL tests). Clippy's collapsible-match / for-kv-map /
  field-assignment lints bite in this crate's match-heavy code —
  prefer match guards and struct-update syntax from the start.
