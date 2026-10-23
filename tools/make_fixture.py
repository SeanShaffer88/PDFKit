#!/usr/bin/env python3
"""Generate the committed test fixture with matplotlib.

A real-world PDF from a mainstream producer (embedded subset fonts,
ToUnicode CMaps, compressed streams) that PDFKit's hand-written
parser must read: 2 pages, known text, known metadata. Cross-checked
with pdfinfo at build time. Regenerate with:
    python3 tools/make_fixture.py
"""
import os
from matplotlib.backends.backend_pdf import PdfPages
from matplotlib import pyplot as plt

OUT = os.path.join(os.path.dirname(__file__), "..", "tests", "fixtures", "matplotlib.pdf")

with PdfPages(OUT) as pdf:
    fig = plt.figure(figsize=(8.5, 11))
    fig.text(0.1, 0.9, "PDFKit Fixture Page One", fontsize=20)
    fig.text(0.1, 0.8, "The quick brown fox jumps over the lazy dog.", fontsize=12)
    fig.text(0.1, 0.7, "Pack my box with five dozen liquor jugs 0123456789.", fontsize=12)
    pdf.savefig(fig)
    plt.close(fig)

    fig = plt.figure(figsize=(8.5, 11))
    fig.text(0.1, 0.9, "Second Page Heading", fontsize=18)
    fig.text(0.1, 0.8, "Final page of the fixture document.", fontsize=12)
    pdf.savefig(fig)
    plt.close(fig)

    d = pdf.infodict()
    d["Title"] = "PDFKit Test Fixture"
    d["Author"] = "PDFKit Tests"

print("fixture written to", OUT)
