//! PDF generation: multi-page documents with the standard Type1
//! Helvetica font (never embedded — one of PDF's base-14), text
//! drawn with explicit positions, and document metadata. Output is
//! a classic xref-table file with byte-exact offsets.

use crate::{CResult, ERR_INVALID_ARG};

#[derive(Debug, Clone)]
pub struct DrawText {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct GenPage {
    pub width: f64,
    pub height: f64,
    pub ops: Vec<DrawText>,
}

#[derive(Debug, Default, Clone)]
pub struct Generator {
    pub title: String,
    pub author: String,
    pub pages: Vec<GenPage>,
}

impl Generator {
    pub fn add_page(&mut self, width: f64, height: f64) -> CResult<usize> {
        if width <= 0.0 || height <= 0.0 || width > 14400.0 || height > 14400.0 {
            return Err((
                ERR_INVALID_ARG,
                format!("page size {width}x{height} is out of range (PDF user space max 14400)"),
            ));
        }
        self.pages.push(GenPage {
            width,
            height,
            ops: Vec::new(),
        });
        Ok(self.pages.len() - 1)
    }

    pub fn draw_text(&mut self, page: usize, x: f64, y: f64, size: f64, text: &str) -> CResult<()> {
        let count = self.pages.len();
        let p = self.pages.get_mut(page).ok_or((
            ERR_INVALID_ARG,
            format!("no page {page} (document has {count})"),
        ))?;
        if size <= 0.0 || size > 1000.0 {
            return Err((ERR_INVALID_ARG, "font size out of range".into()));
        }
        p.ops.push(DrawText {
            x,
            y,
            size,
            text: text.to_string(),
        });
        Ok(())
    }

    pub fn render(&self) -> CResult<Vec<u8>> {
        if self.pages.is_empty() {
            return Err((ERR_INVALID_ARG, "cannot write a PDF with no pages".into()));
        }
        // Object plan: 1 Catalog, 2 Pages, 3 Font, 4 Info (optional
        // — always emitted for simplicity), then per page: Page and
        // Contents objects.
        let info_obj = 4u32;
        let mut next = 5u32;
        let mut page_objs: Vec<(u32, u32)> = Vec::new(); // (page, contents)
        for _ in &self.pages {
            page_objs.push((next, next + 1));
            next += 2;
        }
        let total = next; // object numbers 1..total

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = vec![0usize; total as usize];
        let mut emit = |out: &mut Vec<u8>, num: u32, body: &[u8]| {
            offsets[num as usize] = out.len();
            out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        };

        emit(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
        let kids: Vec<String> = page_objs.iter().map(|(p, _)| format!("{p} 0 R")).collect();
        emit(
            &mut out,
            2,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                self.pages.len()
            )
            .as_bytes(),
        );
        emit(
            &mut out,
            3,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        );
        let mut info = String::from("<<");
        if !self.title.is_empty() {
            info.push_str(&format!(" /Title {}", pdf_string(&self.title)));
        }
        if !self.author.is_empty() {
            info.push_str(&format!(" /Author {}", pdf_string(&self.author)));
        }
        info.push_str(" /Creator (PDFKit) /Producer (PDFKit) >>");
        emit(&mut out, info_obj, info.as_bytes());

        for (i, page) in self.pages.iter().enumerate() {
            let (page_num, content_num) = page_objs[i];
            emit(
                &mut out,
                page_num,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
                     /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                    fmt_num(page.width),
                    fmt_num(page.height),
                    content_num
                )
                .as_bytes(),
            );
            let mut content = String::new();
            for op in &page.ops {
                content.push_str(&format!(
                    "BT /F1 {} Tf {} {} Td {} Tj ET\n",
                    fmt_num(op.size),
                    fmt_num(op.x),
                    fmt_num(op.y),
                    pdf_string(&op.text)
                ));
            }
            let stream = format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                content
            );
            emit(&mut out, content_num, stream.as_bytes());
        }

        let xref_pos = out.len();
        out.extend_from_slice(format!("xref\n0 {total}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for num in 1..total {
            out.extend_from_slice(format!("{:010} 00000 n \n", offsets[num as usize]).as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {total} /Root 1 0 R /Info {info_obj} 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n"
            )
            .as_bytes(),
        );
        Ok(out)
    }
}

/// Numbers without trailing noise: integers stay integers.
fn fmt_num(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Encode a Rust string as a PDF literal string in WinAnsi.
/// Characters outside WinAnsi become '?'; the few representable
/// specials are octal-escaped.
fn pdf_string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x100 => {
                let b = c as u8;
                if (0x20..0x7F).contains(&b) {
                    out.push(c);
                } else {
                    out.push_str(&format!("\\{b:03o}"));
                }
            }
            c => {
                // WinAnsi high range characters we can map back.
                let byte = crate::extract::WINANSI_REVERSE
                    .iter()
                    .find(|(_, ch)| *ch == c)
                    .map(|(b, _)| *b);
                match byte {
                    Some(b) => out.push_str(&format!("\\{b:03o}")),
                    None => out.push('?'),
                }
            }
        }
    }
    out.push(')');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::Document;

    #[test]
    fn generated_pdf_roundtrips_through_own_reader() {
        let mut g = Generator {
            title: "Quarterly Report".into(),
            author: "Sean".into(),
            ..Default::default()
        };
        let p = g.add_page(612.0, 792.0).unwrap();
        g.draw_text(p, 72.0, 720.0, 24.0, "Hello, PDFKit!").unwrap();
        g.draw_text(
            p,
            72.0,
            690.0,
            12.0,
            "Second line (with parens) & \\ slash.",
        )
        .unwrap();
        let p2 = g.add_page(612.0, 792.0).unwrap();
        g.draw_text(p2, 72.0, 720.0, 14.0, "Page two here.")
            .unwrap();
        let bytes = g.render().unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));

        let doc = Document::load(&bytes).unwrap();
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(
            doc.info_string("Title").as_deref(),
            Some("Quarterly Report")
        );
        assert_eq!(doc.info_string("Author").as_deref(), Some("Sean"));
        let t0 = crate::extract::page_text(&doc, &doc.pages[0]).unwrap();
        assert!(t0.contains("Hello, PDFKit!"), "{t0}");
        assert!(t0.contains("Second line (with parens)"), "{t0}");
        let t1 = crate::extract::page_text(&doc, &doc.pages[1]).unwrap();
        assert!(t1.contains("Page two here."), "{t1}");
    }

    #[test]
    fn writer_validates() {
        let mut g = Generator::default();
        assert!(g.render().is_err()); // no pages
        assert!(g.add_page(0.0, 100.0).is_err());
        let p = g.add_page(100.0, 100.0).unwrap();
        assert!(g.draw_text(p + 9, 0.0, 0.0, 12.0, "x").is_err());
        assert!(g.draw_text(p, 0.0, 0.0, -1.0, "x").is_err());
    }
}
