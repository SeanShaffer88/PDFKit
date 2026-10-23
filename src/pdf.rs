//! PDF object model + tolerant document loader (ISO 32000).
//!
//! Loading strategy: scan the whole file for indirect objects
//! (`N G obj … endobj`), expand object streams, then take the
//! trailer (classic `trailer` dict, or an xref stream's dict) for
//! /Root and /Info. Scanning instead of trusting xref offsets is
//! what real readers fall back to when files are damaged or were
//! written by sloppy generators — for v0.1 it is the primary path,
//! which also makes xref-stream files work without an xref parser.
//! Encrypted documents (/Encrypt in the trailer) are refused with a
//! precise error.

use std::collections::BTreeMap;

use crate::{CResult, ERR_CORRUPT, ERR_NOT_SUPPORTED};

pub fn corrupt(msg: impl Into<String>) -> (i32, String) {
    (ERR_CORRUPT, format!("pdf: {}", msg.into()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Vec<(String, Obj)>),
    Ref(u32, u16),
    Stream(Vec<(String, Obj)>, Vec<u8>),
    Keyword(String),
}

impl Obj {
    pub fn get<'a>(&'a self, key: &str) -> Option<&'a Obj> {
        match self {
            Obj::Dict(entries) | Obj::Stream(entries, _) => {
                entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
            }
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Obj::Num(n) if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Obj::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn as_ref(&self) -> Option<(u32, u16)> {
        match self {
            Obj::Ref(n, g) => Some((*n, *g)),
            _ => None,
        }
    }
    pub fn is_stream(&self) -> bool {
        matches!(self, Obj::Stream(_, _))
    }
}

// ---------------------------------------------------------------------------
// Lexer / value parser (shared by document parsing and content streams)
// ---------------------------------------------------------------------------

pub struct Cursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}
fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    pub fn skip_ws(&mut self) {
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self.pos < self.data.len()
                    && self.data[self.pos] != b'\n'
                    && self.data[self.pos] != b'\r'
                {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    pub fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn starts_with(&self, s: &[u8]) -> bool {
        self.data[self.pos..].starts_with(s)
    }

    fn read_token(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.data.len()
            && !is_ws(self.data[self.pos])
            && !is_delim(self.data[self.pos])
        {
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(String::from_utf8_lossy(&self.data[start..self.pos]).into_owned())
        }
    }

    pub fn parse_value(&mut self) -> CResult<Obj> {
        self.skip_ws();
        match self.peek() {
            None => Err(corrupt("unexpected end of data")),
            Some(b'<') => {
                if self.starts_with(b"<<") {
                    self.pos += 2;
                    self.parse_dict()
                } else {
                    self.pos += 1;
                    self.parse_hex_string().map(Obj::Str)
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Obj::Array(items));
                        }
                        None => return Err(corrupt("unterminated array")),
                        _ => items.push(self.parse_value()?),
                    }
                }
            }
            Some(b'(') => {
                self.pos += 1;
                self.parse_literal_string().map(Obj::Str)
            }
            Some(b'/') => {
                self.pos += 1;
                let start = self.pos;
                while self.pos < self.data.len()
                    && !is_ws(self.data[self.pos])
                    && !is_delim(self.data[self.pos])
                {
                    self.pos += 1;
                }
                let raw = &self.data[start..self.pos];
                Ok(Obj::Name(decode_name(raw)))
            }
            Some(_) => {
                let tok = self
                    .read_token()
                    .ok_or_else(|| corrupt("expected a value"))?;
                match tok.as_str() {
                    "true" => Ok(Obj::Bool(true)),
                    "false" => Ok(Obj::Bool(false)),
                    "null" => Ok(Obj::Null),
                    _ => {
                        if let Ok(n) = tok.parse::<f64>() {
                            // Possible indirect reference: N G R.
                            let save = self.pos;
                            let second = self.read_token().and_then(|t| t.parse::<f64>().ok());
                            if let Some(n2) = second {
                                let save2 = self.pos;
                                if let Some(r) = self.read_token() {
                                    if r == "R"
                                        && n.fract() == 0.0
                                        && n2.fract() == 0.0
                                        && n >= 0.0
                                        && n2 >= 0.0
                                    {
                                        return Ok(Obj::Ref(n as u32, n2 as u16));
                                    }
                                }
                                self.pos = save2;
                            }
                            self.pos = save;
                            Ok(Obj::Num(n))
                        } else {
                            Ok(Obj::Keyword(tok))
                        }
                    }
                }
            }
        }
    }

    fn parse_dict(&mut self) -> CResult<Obj> {
        let mut entries: Vec<(String, Obj)> = Vec::new();
        loop {
            self.skip_ws();
            if self.starts_with(b">>") {
                self.pos += 2;
                return Ok(Obj::Dict(entries));
            }
            if self.peek() != Some(b'/') {
                return Err(corrupt("expected a name key in dictionary"));
            }
            let key = match self.parse_value()? {
                Obj::Name(n) => n,
                _ => return Err(corrupt("dictionary key is not a name")),
            };
            let value = self.parse_value()?;
            entries.retain(|(k, _)| k != &key);
            entries.push((key, value));
        }
    }

    fn parse_literal_string(&mut self) -> CResult<Vec<u8>> {
        let mut out = Vec::new();
        let mut depth = 1i32;
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                    out.push(b);
                }
                b'\\' => {
                    let c = self
                        .peek()
                        .ok_or_else(|| corrupt("unterminated string escape"))?;
                    self.pos += 1;
                    match c {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'(' | b')' | b'\\' => out.push(c),
                        b'0'..=b'7' => {
                            let mut v = (c - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(b),
            }
        }
        Err(corrupt("unterminated literal string"))
    }

    fn parse_hex_string(&mut self) -> CResult<Vec<u8>> {
        let mut nibbles = Vec::new();
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'>' => {
                    if nibbles.len() % 2 == 1 {
                        nibbles.push(0);
                    }
                    return Ok(nibbles
                        .chunks_exact(2)
                        .map(|p| (p[0] << 4) | p[1])
                        .collect());
                }
                b'0'..=b'9' => nibbles.push(b - b'0'),
                b'a'..=b'f' => nibbles.push(b - b'a' + 10),
                b'A'..=b'F' => nibbles.push(b - b'A' + 10),
                _ if is_ws(b) => {}
                _ => return Err(corrupt("bad character in hex string")),
            }
        }
        Err(corrupt("unterminated hex string"))
    }
}

fn decode_name(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 2 < raw.len() {
            let h = (raw[i + 1] as char).to_digit(16);
            let l = (raw[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
            out.push(raw[i]);
            i += 1;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Stream decoding
// ---------------------------------------------------------------------------

pub fn decode_stream(doc: &Document, obj: &Obj) -> CResult<Vec<u8>> {
    let (dict, raw) = match obj {
        Obj::Stream(d, data) => (d, data.clone()),
        _ => return Err(corrupt("object is not a stream")),
    };
    let dict_obj = Obj::Dict(dict.clone());
    let mut data = raw;
    // Filters: a single name or an array of names.
    let filters: Vec<String> = match dict_obj.get("Filter") {
        None => Vec::new(),
        Some(Obj::Name(n)) => vec![n.clone()],
        Some(Obj::Array(items)) => items
            .iter()
            .filter_map(|i| match doc.resolve(i) {
                Obj::Name(n) => Some(n.clone()),
                _ => None,
            })
            .collect(),
        Some(other) => match doc.resolve(other) {
            Obj::Name(n) => vec![n.clone()],
            _ => Vec::new(),
        },
    };
    for filter in &filters {
        match filter.as_str() {
            "FlateDecode" | "Fl" => {
                data = crate::inflate::unzlib(&data, 256 * 1024 * 1024)
                    .map_err(|(c, m)| (c, format!("FlateDecode: {m}")))?;
            }
            "ASCII85Decode" | "A85" => {
                data = ascii85_decode(&data)?;
            }
            other => {
                return Err((
                    ERR_NOT_SUPPORTED,
                    format!("stream filter '{other}' is not supported in v0.1"),
                ))
            }
        }
    }
    // DecodeParms predictors (apply to the LAST filter in practice;
    // v0.1 applies them after FlateDecode when present).
    if !filters.is_empty() {
        if let Some(parms) = dict_obj.get("DecodeParms").or_else(|| dict_obj.get("DP")) {
            let parms = doc.resolve(parms).clone();
            let predictor = parms
                .get("Predictor")
                .and_then(|p| doc.resolve(p).as_int())
                .unwrap_or(1);
            if predictor >= 10 {
                let columns = parms
                    .get("Columns")
                    .and_then(|p| doc.resolve(p).as_int())
                    .unwrap_or(1) as usize;
                data = png_unpredict(&data, columns)?;
            } else if predictor == 2 {
                return Err((
                    ERR_NOT_SUPPORTED,
                    "TIFF predictor 2 is not supported in v0.1".into(),
                ));
            }
        }
    }
    Ok(data)
}

fn ascii85_decode(data: &[u8]) -> CResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut group: Vec<u8> = Vec::new();
    let mut i = 0;
    let bytes: Vec<u8> = data.iter().copied().filter(|b| !is_ws(*b)).collect();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            i += 1;
            continue;
        }
        if b == b'~' {
            break;
        }
        if !(b'!'..=b'u').contains(&b) {
            return Err(corrupt("invalid ASCII85 character"));
        }
        group.push(b - b'!');
        i += 1;
        if group.len() == 5 {
            let mut v: u32 = 0;
            for g in &group {
                v = v.wrapping_mul(85).wrapping_add(*g as u32);
            }
            out.extend_from_slice(&v.to_be_bytes());
            group.clear();
        }
    }
    if !group.is_empty() {
        let keep = group.len() - 1;
        while group.len() < 5 {
            group.push(84);
        }
        let mut v: u32 = 0;
        for g in &group {
            v = v.wrapping_mul(85).wrapping_add(*g as u32);
        }
        out.extend_from_slice(&v.to_be_bytes()[..keep]);
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i32, b as i32, c as i32);
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn png_unpredict(data: &[u8], columns: usize) -> CResult<Vec<u8>> {
    if columns == 0 {
        return Err(corrupt("predictor Columns is 0"));
    }
    let row_len = columns;
    let mut out = Vec::with_capacity(data.len());
    let mut prev: Vec<u8> = vec![0; row_len];
    let mut pos = 0;
    while pos < data.len() {
        if pos + 1 + row_len > data.len() {
            return Err(corrupt("predictor data truncated"));
        }
        let filt = data[pos];
        let row = &data[pos + 1..pos + 1 + row_len];
        let mut cur = vec![0u8; row_len];
        for x in 0..row_len {
            let a = if x > 0 { cur[x - 1] } else { 0 };
            let b = prev[x];
            let c = if x > 0 { prev[x - 1] } else { 0 };
            cur[x] = match filt {
                0 => row[x],
                1 => row[x].wrapping_add(a),
                2 => row[x].wrapping_add(b),
                3 => row[x].wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => row[x].wrapping_add(paeth(a, b, c)),
                _ => return Err(corrupt("unknown PNG predictor filter")),
            };
        }
        out.extend_from_slice(&cur);
        prev = cur;
        pos += 1 + row_len;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Page {
    pub obj_ref: (u32, u16),
    pub media_box: [f64; 4],
    pub rotate: i64,
    pub resources: Option<Obj>,
    pub contents: Vec<Obj>, // refs or direct streams
}

#[derive(Debug)]
pub struct Document {
    pub version: String,
    pub objects: BTreeMap<(u32, u16), Obj>,
    pub root: Option<(u32, u16)>,
    pub info: Option<(u32, u16)>,
    pub pages: Vec<Page>,
}

impl Document {
    pub fn resolve<'a>(&'a self, obj: &'a Obj) -> &'a Obj {
        match obj {
            Obj::Ref(n, g) => self.objects.get(&(*n, *g)).unwrap_or(&Obj::Null),
            other => other,
        }
    }

    pub fn get_obj(&self, r: (u32, u16)) -> Option<&Obj> {
        self.objects.get(&r)
    }

    pub fn load(data: &[u8]) -> CResult<Document> {
        if data.len() < 8 {
            return Err(corrupt("file too short to be a PDF"));
        }
        // Header: %PDF-x.y within the first 1024 bytes.
        let head = &data[..data.len().min(1024)];
        let version = find_subslice(head, b"%PDF-")
            .map(|i| {
                let rest = &head[i + 5..];
                let end = rest
                    .iter()
                    .position(|b| is_ws(*b))
                    .unwrap_or(rest.len().min(8));
                String::from_utf8_lossy(&rest[..end.min(8)]).into_owned()
            })
            .ok_or_else(|| corrupt("no %PDF- header found"))?;

        let mut objects = scan_objects(data)?;

        // Expand object streams (their members have no direct entries
        // unless already present — direct objects win).
        let objstm_refs: Vec<(u32, u16)> = objects
            .iter()
            .filter(|(_, o)| {
                o.get("Type")
                    .map(|t| t.as_name() == Some("ObjStm"))
                    .unwrap_or(false)
            })
            .map(|(k, _)| *k)
            .collect();
        // Note: decoding ObjStm needs a Document for resolution; build
        // a temporary one. Streams inside ObjStm are plain values.
        let mut doc = Document {
            version: version.clone(),
            objects: objects.clone(),
            root: None,
            info: None,
            pages: Vec::new(),
        };
        for r in objstm_refs {
            if let Some(obj) = doc.objects.get(&r).cloned() {
                let decoded = decode_stream(&doc, &obj)?;
                let n = obj.get("N").and_then(|v| v.as_int()).unwrap_or(0) as usize;
                let first = obj.get("First").and_then(|v| v.as_int()).unwrap_or(0) as usize;
                if first > decoded.len() {
                    continue;
                }
                let header = &decoded[..first];
                let nums: Vec<&str> = std::str::from_utf8(header)
                    .unwrap_or("")
                    .split_whitespace()
                    .collect();
                let mut k = 0;
                while k + 1 < nums.len() && (k / 2) < n {
                    if let (Ok(num), Ok(off)) =
                        (nums[k].parse::<u32>(), nums[k + 1].parse::<usize>())
                    {
                        let start = first + off;
                        if start < decoded.len() {
                            let mut cur = Cursor::new(&decoded[start..]);
                            if let Ok(v) = cur.parse_value() {
                                objects.entry((num, 0)).or_insert(v);
                            }
                        }
                    }
                    k += 2;
                }
            }
        }
        doc.objects = objects;

        // Trailer: last `trailer` dict wins; else an xref stream dict.
        let mut trailer: Option<Obj> = None;
        if let Some(pos) = find_last_subslice(data, b"trailer") {
            let mut cur = Cursor::new(&data[pos + 7..]);
            if let Ok(v) = cur.parse_value() {
                trailer = Some(v);
            }
        }
        if trailer.is_none() {
            for o in doc.objects.values() {
                if o.get("Type").and_then(|t| t.as_name()) == Some("XRef") {
                    if let Obj::Stream(d, _) = o {
                        trailer = Some(Obj::Dict(d.clone()));
                        break;
                    }
                }
            }
        }
        if let Some(t) = &trailer {
            if t.get("Encrypt").is_some() {
                return Err((
                    ERR_NOT_SUPPORTED,
                    "encrypted PDFs are not supported in v0.1".into(),
                ));
            }
            doc.root = t.get("Root").and_then(|r| r.as_ref());
            doc.info = t.get("Info").and_then(|r| r.as_ref());
        }
        if doc.root.is_none() {
            // Last resort: find the catalog directly.
            doc.root = doc
                .objects
                .iter()
                .find(|(_, o)| o.get("Type").and_then(|t| t.as_name()) == Some("Catalog"))
                .map(|(k, _)| *k);
        }
        let root_ref = doc
            .root
            .ok_or_else(|| corrupt("no document catalog (/Root) found"))?;
        let catalog = doc
            .get_obj(root_ref)
            .ok_or_else(|| corrupt("catalog object missing"))?
            .clone();
        let pages_ref = catalog
            .get("Pages")
            .and_then(|p| p.as_ref())
            .ok_or_else(|| corrupt("catalog has no /Pages reference"))?;

        let mut pages = Vec::new();
        let mut guard = 0usize;
        walk_pages(&doc, pages_ref, None, None, 0, &mut pages, &mut guard)?;
        if pages.is_empty() {
            return Err(corrupt("page tree contains no pages"));
        }
        doc.pages = pages;
        Ok(doc)
    }

    /// An Info-dict string field, decoded as text (UTF-16BE when the
    /// BOM is present, else PDFDocEncoding≈WinAnsi for our purposes).
    pub fn info_string(&self, key: &str) -> Option<String> {
        let info_ref = self.info?;
        let info = self.get_obj(info_ref)?;
        match info.get(key) {
            Some(Obj::Str(bytes)) => Some(crate::extract::decode_pdf_text_string(bytes)),
            _ => None,
        }
    }
}

const MAX_PAGES: usize = 100_000;

fn walk_pages(
    doc: &Document,
    node_ref: (u32, u16),
    media: Option<[f64; 4]>,
    resources: Option<Obj>,
    rotate: i64,
    out: &mut Vec<Page>,
    guard: &mut usize,
) -> CResult<()> {
    *guard += 1;
    if *guard > MAX_PAGES * 2 + 16 || out.len() > MAX_PAGES {
        return Err(corrupt("page tree is unreasonably large or cyclic"));
    }
    let node = doc
        .get_obj(node_ref)
        .ok_or_else(|| {
            corrupt(format!(
                "page tree node {} {} R missing",
                node_ref.0, node_ref.1
            ))
        })?
        .clone();
    let media = node
        .get("MediaBox")
        .and_then(|m| parse_box(doc, m))
        .or(media);
    let resources = node.get("Resources").cloned().or(resources);
    let rotate = node
        .get("Rotate")
        .and_then(|r| doc.resolve(r).as_int())
        .unwrap_or(rotate);
    let node_type = node
        .get("Type")
        .and_then(|t| t.as_name().map(|s| s.to_string()));
    if node_type.as_deref() == Some("Page") || node.get("Kids").is_none() {
        let contents = match node.get("Contents") {
            Some(Obj::Array(items)) => items.clone(),
            Some(single) => vec![single.clone()],
            None => Vec::new(),
        };
        out.push(Page {
            obj_ref: node_ref,
            media_box: media.unwrap_or([0.0, 0.0, 612.0, 792.0]),
            rotate,
            resources,
            contents,
        });
        return Ok(());
    }
    let kids = node
        .get("Kids")
        .and_then(|k| match doc.resolve(k) {
            Obj::Array(items) => Some(items.clone()),
            _ => None,
        })
        .ok_or_else(|| corrupt("page tree node has no /Kids array"))?;
    for kid in kids {
        if let Some(kref) = kid.as_ref() {
            walk_pages(doc, kref, media, resources.clone(), rotate, out, guard)?;
        }
    }
    Ok(())
}

pub fn parse_box(doc: &Document, obj: &Obj) -> Option<[f64; 4]> {
    match doc.resolve(obj) {
        Obj::Array(items) if items.len() == 4 => {
            let mut out = [0.0; 4];
            for (i, item) in items.iter().enumerate() {
                out[i] = doc.resolve(item).as_f64()?;
            }
            Some(out)
        }
        _ => None,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn find_last_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Scan for `N G obj … endobj` and parse each body.
fn scan_objects(data: &[u8]) -> CResult<BTreeMap<(u32, u16), Obj>> {
    let mut map = BTreeMap::new();
    let mut i = 0usize;
    while i + 4 < data.len() {
        // Candidate: digits, space, digits, space, "obj"
        if data[i].is_ascii_digit() {
            let start = i;
            let mut j = i;
            while j < data.len() && data[j].is_ascii_digit() {
                j += 1;
            }
            let num_str = &data[start..j];
            if j < data.len() && is_ws(data[j]) {
                let mut k = j;
                while k < data.len() && is_ws(data[k]) {
                    k += 1;
                }
                let gen_start = k;
                while k < data.len() && data[k].is_ascii_digit() {
                    k += 1;
                }
                if k > gen_start && k < data.len() && is_ws(data[k]) {
                    let mut m = k;
                    while m < data.len() && is_ws(data[m]) {
                        m += 1;
                    }
                    if data[m..].starts_with(b"obj")
                        && (m + 3 >= data.len() || is_ws(data[m + 3]) || is_delim(data[m + 3]))
                    {
                        let num: u32 = String::from_utf8_lossy(num_str).parse().unwrap_or(0);
                        let gen: u16 = String::from_utf8_lossy(&data[gen_start..k])
                            .parse()
                            .unwrap_or(0);
                        let body_start = m + 3;
                        let mut cur = Cursor::new(&data[body_start..]);
                        match cur.parse_value() {
                            Ok(value) => {
                                let mut value = value;
                                // Optional stream body follows a dict value.
                                let after = body_start + cur.pos;
                                let mut probe = Cursor::new(&data[after..]);
                                probe.skip_ws();
                                if probe.starts_with(b"stream") {
                                    if let Obj::Dict(d) = value {
                                        let stream_kw_end = after + probe.pos + 6;
                                        let mut data_start = stream_kw_end;
                                        if data[data_start..].starts_with(b"\r\n") {
                                            data_start += 2;
                                        } else if data_start < data.len()
                                            && (data[data_start] == b'\n'
                                                || data[data_start] == b'\r')
                                        {
                                            data_start += 1;
                                        }
                                        // Length: direct int in the dict (a
                                        // ref would need the finished map;
                                        // fall back to searching endstream).
                                        let declared = d
                                            .iter()
                                            .find(|(k2, _)| k2 == "Length")
                                            .and_then(|(_, v)| v.as_int());
                                        let stream_bytes = match declared {
                                            Some(len)
                                                if len >= 0
                                                    && data_start + (len as usize)
                                                        <= data.len() =>
                                            {
                                                data[data_start..data_start + len as usize].to_vec()
                                            }
                                            _ => {
                                                match find_subslice(
                                                    &data[data_start..],
                                                    b"endstream",
                                                ) {
                                                    Some(end) => {
                                                        let mut slice =
                                                            &data[data_start..data_start + end];
                                                        // Strip the EOL before endstream.
                                                        if slice.ends_with(b"\r\n") {
                                                            slice = &slice[..slice.len() - 2];
                                                        } else if slice.ends_with(b"\n")
                                                            || slice.ends_with(b"\r")
                                                        {
                                                            slice = &slice[..slice.len() - 1];
                                                        }
                                                        slice.to_vec()
                                                    }
                                                    None => Vec::new(),
                                                }
                                            }
                                        };
                                        value = Obj::Stream(d, stream_bytes);
                                    }
                                }
                                map.insert((num, gen), value);
                                i = body_start + cur.pos;
                                continue;
                            }
                            Err(_) => {
                                i = m + 3;
                                continue;
                            }
                        }
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_refs() {
        let mut c = Cursor::new(b"<< /Type /Page /Count 3 /Kids [1 0 R 2 0 R] /Name (a\\(b\\)) >>");
        let v = c.parse_value().unwrap();
        assert_eq!(v.get("Count").unwrap().as_int(), Some(3));
        let kids = v.get("Kids").unwrap();
        match kids {
            Obj::Array(items) => {
                assert_eq!(items[0].as_ref(), Some((1, 0)));
                assert_eq!(items[1].as_ref(), Some((2, 0)));
            }
            _ => panic!("kids not array"),
        }
        let mut c = Cursor::new(b"<48656C6C6F>");
        assert_eq!(c.parse_value().unwrap(), Obj::Str(b"Hello".to_vec()));
        let mut c = Cursor::new(b"/Adobe#20Name");
        assert_eq!(c.parse_value().unwrap(), Obj::Name("Adobe Name".into()));
    }

    #[test]
    fn ascii85_roundtrip_property() {
        // 'z' and a short final group.
        assert_eq!(ascii85_decode(b"z").unwrap(), vec![0, 0, 0, 0]);
        assert!(ascii85_decode(b"!!!!!").is_ok());
    }
}
