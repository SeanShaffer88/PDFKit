//! Text extraction: walk a page's content stream, track the current
//! font, decode shown strings through the font's ToUnicode CMap
//! (when present), encoding Differences, or WinAnsi, and reassemble
//! lines approximately (a newline is emitted when text moves to a
//! new line). This is extraction, not layout analysis: column order
//! follows the content stream, as in most tools.

use std::collections::BTreeMap;

use crate::pdf::{Cursor, Document, Obj, Page};
use crate::CResult;

/// WinAnsiEncoding for the 0x80..=0x9F range (elsewhere it matches
/// Latin-1 / ASCII).
const WINANSI_HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// Reverse map for the writer: WinAnsi byte for the high-range
/// characters (only the printable ones).
pub const WINANSI_REVERSE: &[(u8, char)] = &[
    (0x80, '€'),
    (0x82, '‚'),
    (0x83, 'ƒ'),
    (0x84, '„'),
    (0x85, '…'),
    (0x86, '†'),
    (0x87, '‡'),
    (0x88, 'ˆ'),
    (0x89, '‰'),
    (0x8A, 'Š'),
    (0x8B, '‹'),
    (0x8C, 'Œ'),
    (0x8E, 'Ž'),
    (0x91, '\u{2018}'),
    (0x92, '\u{2019}'),
    (0x93, '\u{201C}'),
    (0x94, '\u{201D}'),
    (0x95, '•'),
    (0x96, '–'),
    (0x97, '—'),
    (0x98, '˜'),
    (0x99, '™'),
    (0x9A, 'š'),
    (0x9B, '›'),
    (0x9C, 'œ'),
    (0x9E, 'ž'),
    (0x9F, 'Ÿ'),
];

pub fn winansi_char(b: u8) -> char {
    match b {
        0x80..=0x9F => WINANSI_HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Decode an Info-dict / metadata text string: UTF-16BE with BOM,
/// else PDFDocEncoding — approximated by WinAnsi, which agrees with
/// PDFDocEncoding on the printable range documents actually use.
pub fn decode_pdf_text_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|p| ((p[0] as u16) << 8) | p[1] as u16)
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| winansi_char(b)).collect()
    }
}

/// Map an encoding-Differences glyph name to Unicode. Handles the
/// `uniXXXX` / `uXXXX` conventions plus a compact table of common
/// names; unknown names fall back to None.
pub fn glyph_name_to_unicode(name: &str) -> Option<String> {
    if let Some(hex) = name.strip_prefix("uni") {
        if hex.len() == 4 {
            if let Ok(cp) = u32::from_str_radix(hex, 16) {
                return char::from_u32(cp).map(|c| c.to_string());
            }
        }
    }
    if let Some(hex) = name.strip_prefix('u') {
        if (4..=6).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            if let Ok(cp) = u32::from_str_radix(hex, 16) {
                return char::from_u32(cp).map(|c| c.to_string());
            }
        }
    }
    if name.chars().count() == 1 {
        return Some(name.to_string());
    }
    let mapped = match name {
        "bullet" => '•',
        "endash" => '–',
        "emdash" => '—',
        "quoteleft" => '\u{2018}',
        "quoteright" => '\u{2019}',
        "quotedblleft" => '\u{201C}',
        "quotedblright" => '\u{201D}',
        "ellipsis" => '…',
        "dagger" => '†',
        "Euro" => '€',
        "trademark" => '™',
        "copyright" => '©',
        "registered" => '®',
        "degree" => '°',
        "plusminus" => '±',
        "multiply" => '×',
        "divide" => '÷',
        "minus" => '−',
        "fi" => return Some("fi".to_string()),
        "fl" => return Some("fl".to_string()),
        "space" => ' ',
        "hyphen" => '-',
        "period" => '.',
        "comma" => ',',
        "colon" => ':',
        "semicolon" => ';',
        "question" => '?',
        "exclam" => '!',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        _ => return None,
    };
    Some(mapped.to_string())
}

#[derive(Debug, Default, Clone)]
pub struct FontInfo {
    pub to_unicode: BTreeMap<u32, String>,
    pub code_len: usize, // bytes per character code (1 simple, 2 typical CID)
    pub differences: BTreeMap<u8, String>,
}

impl FontInfo {
    fn decode(&self, bytes: &[u8]) -> String {
        let mut out = String::new();
        let len = if self.code_len == 0 { 1 } else { self.code_len };
        let mut i = 0;
        while i < bytes.len() {
            let take = len.min(bytes.len() - i);
            let mut code: u32 = 0;
            for k in 0..take {
                code = (code << 8) | bytes[i + k] as u32;
            }
            i += take;
            if let Some(s) = self.to_unicode.get(&code) {
                out.push_str(s);
            } else if take == 1 {
                if let Some(name) = self.differences.get(&(code as u8)) {
                    if let Some(u) = glyph_name_to_unicode(name) {
                        out.push_str(&u);
                    } else {
                        out.push(winansi_char(code as u8));
                    }
                } else {
                    out.push(winansi_char(code as u8));
                }
            } else if code < 0x100 {
                out.push(winansi_char(code as u8));
            } else {
                out.push('\u{FFFD}');
            }
        }
        out
    }
}

/// Parse a ToUnicode CMap stream into (code → unicode) + code length.
fn parse_tounicode(data: &[u8]) -> (BTreeMap<u32, String>, usize) {
    let text = String::from_utf8_lossy(data).into_owned();
    let mut map = BTreeMap::new();
    let mut code_len = 1usize;
    // codespace range: <lo> <hi> — width from the hex length.
    for section in text.split("begincodespacerange").skip(1) {
        let end = section.find("endcodespacerange").unwrap_or(section.len());
        let body = &section[..end];
        if let Some(lo) = first_hex(body) {
            code_len = (lo.len() / 2).max(1);
        }
    }
    fn hex_val(s: &str) -> u32 {
        u32::from_str_radix(s.trim(), 16).unwrap_or(0)
    }
    fn hex_bytes(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        let mut b = Vec::new();
        let mut i = 0;
        while i + 1 < s.len() {
            b.push(u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0));
            i += 2;
        }
        b
    }
    fn utf16(bytes: &[u8]) -> String {
        if bytes.len() % 2 == 0 && !bytes.is_empty() {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| ((p[0] as u16) << 8) | p[1] as u16)
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|&b| winansi_char(b)).collect()
        }
    }
    // bfchar: <src> <dst> pairs.
    for section in text.split("beginbfchar").skip(1) {
        let end = section.find("endbfchar").unwrap_or(section.len());
        let hexes = all_hex(&section[..end]);
        for pair in hexes.chunks_exact(2) {
            map.insert(hex_val(&pair[0]), utf16(&hex_bytes(&pair[1])));
        }
    }
    // bfrange: <lo> <hi> <dstStart>  or  <lo> <hi> [<d1> <d2> ...]
    for section in text.split("beginbfrange").skip(1) {
        let end = section.find("endbfrange").unwrap_or(section.len());
        let body = &section[..end];
        let lines: Vec<&str> = body.lines().collect();
        for line in lines {
            let hexes = all_hex(line);
            if hexes.len() >= 3 {
                let lo = hex_val(&hexes[0]);
                let hi = hex_val(&hexes[1]);
                if line.contains('[') {
                    // Array form: dst values follow in order.
                    for (off, dst) in hexes.iter().skip(2).enumerate() {
                        map.insert(lo + off as u32, utf16(&hex_bytes(dst)));
                    }
                } else if hi >= lo && hi - lo <= 10000 {
                    let base = hex_bytes(&hexes[2]);
                    let base_is_utf16 = base.len() >= 2 && base.len() % 2 == 0;
                    for code in lo..=hi {
                        if base_is_utf16 {
                            // Increment the last UTF-16 unit.
                            let mut units: Vec<u16> = base
                                .chunks_exact(2)
                                .map(|p| ((p[0] as u16) << 8) | p[1] as u16)
                                .collect();
                            if let Some(last) = units.last_mut() {
                                *last = last.wrapping_add((code - lo) as u16);
                            }
                            map.insert(code, String::from_utf16_lossy(&units));
                        } else if let Some(&b0) = base.first() {
                            let v = (b0 as u32 + (code - lo)) as u8;
                            map.insert(code, winansi_char(v).to_string());
                        }
                    }
                }
            }
        }
    }
    (map, code_len)
}

fn first_hex(s: &str) -> Option<String> {
    let start = s.find('<')?;
    let end = s[start..].find('>')? + start;
    Some(s[start + 1..end].to_string())
}

fn all_hex(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        match after.find('>') {
            Some(end) => {
                out.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}

fn build_fonts(doc: &Document, page: &Page) -> BTreeMap<String, FontInfo> {
    let mut fonts = BTreeMap::new();
    let resources = match &page.resources {
        Some(r) => doc.resolve(r).clone(),
        None => return fonts,
    };
    let font_dict = match resources.get("Font") {
        Some(f) => doc.resolve(f).clone(),
        None => return fonts,
    };
    let entries = match &font_dict {
        Obj::Dict(e) => e.clone(),
        _ => return fonts,
    };
    for (name, font_obj) in entries {
        let font = doc.resolve(&font_obj).clone();
        let mut info = FontInfo {
            code_len: 1,
            ..Default::default()
        };
        let subtype = font
            .get("Subtype")
            .and_then(|s| s.as_name().map(|x| x.to_string()));
        if subtype.as_deref() == Some("Type0") {
            info.code_len = 2;
        }
        if let Some(tu) = font.get("ToUnicode") {
            let tu_obj = doc.resolve(tu).clone();
            if tu_obj.is_stream() {
                if let Ok(data) = crate::pdf::decode_stream(doc, &tu_obj) {
                    let (map, len) = parse_tounicode(&data);
                    info.to_unicode = map;
                    if len > 0 {
                        info.code_len = len;
                    }
                }
            }
        }
        if let Some(enc) = font.get("Encoding") {
            let enc = doc.resolve(enc).clone();
            if let Some(diffs) = enc.get("Differences") {
                if let Obj::Array(items) = doc.resolve(diffs) {
                    let mut current: i64 = -1;
                    for item in items {
                        match doc.resolve(item) {
                            Obj::Num(n) => current = *n as i64,
                            Obj::Name(gname) if current >= 0 => {
                                info.differences.insert(current as u8, gname.clone());
                                current += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        fonts.insert(name, info);
    }
    fonts
}

pub fn page_text(doc: &Document, page: &Page) -> CResult<String> {
    let fonts = build_fonts(doc, page);
    let default_font = FontInfo::default();
    let mut out = String::new();
    let mut operands: Vec<Obj> = Vec::new();
    let mut current_font: Option<String> = None;
    let mut pending_newline = false;

    let show = |text: &str, out: &mut String, pending_newline: &mut bool| {
        if text.is_empty() {
            return;
        }
        if *pending_newline && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        *pending_newline = false;
        out.push_str(text);
    };

    for content_ref in &page.contents {
        let obj = doc.resolve(content_ref).clone();
        if !obj.is_stream() {
            continue;
        }
        let data = crate::pdf::decode_stream(doc, &obj)?;
        let mut cur = Cursor::new(&data);
        loop {
            cur.skip_ws();
            if cur.pos >= data.len() {
                break;
            }
            let value = match cur.parse_value() {
                Ok(v) => v,
                Err(_) => break,
            };
            match value {
                Obj::Keyword(op) => {
                    match op.as_str() {
                        "Tf" => {
                            if let Some(Obj::Name(n)) = operands.first() {
                                current_font = Some(n.clone());
                            }
                        }
                        "Tj" | "TJ" | "'" | "\"" => {
                            if op == "'" || op == "\"" {
                                pending_newline = true;
                            }
                            let font = current_font
                                .as_ref()
                                .and_then(|n| fonts.get(n))
                                .unwrap_or(&default_font);
                            if op == "TJ" {
                                if let Some(Obj::Array(items)) = operands.first() {
                                    for item in items {
                                        match item {
                                            Obj::Str(bytes) => {
                                                let t = font.decode(bytes);
                                                show(&t, &mut out, &mut pending_newline);
                                            }
                                            Obj::Num(n)
                                                if *n < -120.0
                                                    && !out.ends_with(' ')
                                                    && !out.ends_with('\n') =>
                                            {
                                                out.push(' ');
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            } else if let Some(Obj::Str(bytes)) =
                                operands.iter().find(|o| matches!(o, Obj::Str(_)))
                            {
                                let t = font.decode(bytes);
                                show(&t, &mut out, &mut pending_newline);
                            }
                        }
                        "Td" | "TD" => {
                            // ty (second operand) non-zero → new line.
                            if let Some(Obj::Num(ty)) = operands.get(1) {
                                if *ty != 0.0 {
                                    pending_newline = true;
                                }
                            }
                        }
                        "Tm" | "T*" => {
                            pending_newline = true;
                        }
                        _ => {}
                    }
                    operands.clear();
                }
                other => operands.push(other),
            }
            // Keep the operand stack bounded against malformed streams.
            if operands.len() > 64 {
                operands.clear();
            }
        }
    }
    Ok(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tounicode_bfchar_and_bfrange() {
        let cmap = b"begincodespacerange\n<00> <FF>\nendcodespacerange\n\
                     beginbfchar\n<01> <0041>\n<02> <00E9>\nendbfchar\n\
                     beginbfrange\n<10> <12> <0041>\nendbfrange";
        let (map, len) = parse_tounicode(cmap);
        assert_eq!(len, 1);
        assert_eq!(map.get(&1).map(|s| s.as_str()), Some("A"));
        assert_eq!(map.get(&2).map(|s| s.as_str()), Some("é"));
        assert_eq!(map.get(&0x10).map(|s| s.as_str()), Some("A"));
        assert_eq!(map.get(&0x12).map(|s| s.as_str()), Some("C"));
    }

    #[test]
    fn winansi_high_bytes() {
        assert_eq!(winansi_char(0x93), '\u{201C}');
        assert_eq!(winansi_char(0x85), '…');
        assert_eq!(winansi_char(b'A'), 'A');
        assert_eq!(winansi_char(0xE9), 'é');
    }

    #[test]
    fn text_string_decoding() {
        assert_eq!(decode_pdf_text_string(b"Plain"), "Plain");
        assert_eq!(
            decode_pdf_text_string(&[0xFE, 0xFF, 0x00, 0x48, 0x00, 0x69]),
            "Hi"
        );
    }

    #[test]
    fn glyph_names() {
        assert_eq!(glyph_name_to_unicode("uni0041").as_deref(), Some("A"));
        assert_eq!(glyph_name_to_unicode("bullet").as_deref(), Some("•"));
        assert_eq!(glyph_name_to_unicode("fi").as_deref(), Some("fi"));
        assert_eq!(glyph_name_to_unicode("A").as_deref(), Some("A"));
    }
}
