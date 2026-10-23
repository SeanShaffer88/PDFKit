//! DEFLATE decompression (RFC 1951) with the zlib wrapper (RFC 1950,
//! Adler-32 verified). Hand-written: ImageKit decodes PNGs without
//! any dependencies. Handles stored, fixed-Huffman, and
//! dynamic-Huffman blocks. Compression lives in deflate.rs.

use crate::{CResult, ERR_CORRUPT};

fn corrupt(msg: &str) -> (i32, String) {
    (ERR_CORRUPT, format!("inflate: {msg}"))
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // byte position
    bit: u32,   // bits buffered (LSB-first)
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bit: 0,
            bit_count: 0,
        }
    }

    fn bits(&mut self, n: u32) -> Result<u32, (i32, String)> {
        while self.bit_count < n {
            if self.pos >= self.data.len() {
                return Err(corrupt("unexpected end of stream"));
            }
            self.bit |= (self.data[self.pos] as u32) << self.bit_count;
            self.pos += 1;
            self.bit_count += 8;
        }
        let v = self.bit & ((1u32 << n) - 1);
        self.bit >>= n;
        self.bit_count -= n;
        Ok(v)
    }

    fn align_byte(&mut self) {
        let drop = self.bit_count % 8;
        self.bit >>= drop;
        self.bit_count -= drop;
    }
}

/// Canonical Huffman decoder built from code lengths.
struct Huffman {
    counts: [u16; 16], // codes per length (index = length)
    symbols: Vec<u16>, // symbols sorted by (length, code order)
}

impl Huffman {
    fn new(lengths: &[u8]) -> CResult<Huffman> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l > 15 {
                return Err(corrupt("code length over 15"));
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        for i in 1..16 {
            offsets[i] = offsets[i - 1] + counts[i - 1];
        }
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        let mut symbols = vec![0u16; total];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offsets[l as usize] as usize] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader) -> CResult<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(corrupt("invalid Huffman code"))
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
) -> CResult<()> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let idx = (sym - 257) as usize;
                if idx >= 29 {
                    return Err(corrupt("length symbol out of range"));
                }
                let len = LENGTH_BASE[idx] as usize + br.bits(LENGTH_EXTRA[idx])? as usize;
                let dsym = dist.decode(br)?;
                if dsym as usize >= 30 {
                    return Err(corrupt("distance symbol out of range"));
                }
                let d = DIST_BASE[dsym as usize] as usize
                    + br.bits(DIST_EXTRA[dsym as usize])? as usize;
                if d > out.len() {
                    return Err(corrupt("distance too far back"));
                }
                let start = out.len() - d;
                for i in 0..len {
                    let b = out[start + i];
                    out.push(b);
                }
            }
            _ => return Err(corrupt("invalid literal/length symbol")),
        }
    }
}

pub fn inflate(data: &[u8], expected_max: usize) -> CResult<Vec<u8>> {
    let mut br = BitReader::new(data);
    let mut out = Vec::new();
    loop {
        let bfinal = br.bits(1)?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                br.align_byte();
                if br.pos + 4 > data.len() {
                    return Err(corrupt("stored block header truncated"));
                }
                let len = u16::from_le_bytes([data[br.pos], data[br.pos + 1]]) as usize;
                let nlen = u16::from_le_bytes([data[br.pos + 2], data[br.pos + 3]]) as usize;
                if len != (!nlen & 0xFFFF) {
                    return Err(corrupt("stored block LEN/NLEN mismatch"));
                }
                br.pos += 4;
                // Any buffered bits beyond the aligned byte were
                // already consumed by position tracking: our reader
                // pulls whole bytes, so after align the stream is at
                // br.pos exactly when bit_count < 8 remainder dropped.
                if br.pos + len > data.len() {
                    return Err(corrupt("stored block truncated"));
                }
                out.extend_from_slice(&data[br.pos..br.pos + len]);
                br.pos += len;
                br.bit = 0;
                br.bit_count = 0;
            }
            1 => {
                let mut lit_lengths = [8u8; 288];
                lit_lengths[144..256].fill(9);
                lit_lengths[256..280].fill(7);
                let lit = Huffman::new(&lit_lengths)?;
                let dist = Huffman::new(&[5u8; 30])?;
                inflate_block(&mut br, &mut out, &lit, &dist)?;
            }
            2 => {
                let hlit = br.bits(5)? as usize + 257;
                let hdist = br.bits(5)? as usize + 1;
                let hclen = br.bits(4)? as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut cl_lengths = [0u8; 19];
                for i in 0..hclen {
                    cl_lengths[ORDER[i]] = br.bits(3)? as u8;
                }
                let cl = Huffman::new(&cl_lengths)?;
                let total = hlit + hdist;
                let mut lengths = vec![0u8; total];
                let mut i = 0;
                while i < total {
                    let sym = cl.decode(&mut br)?;
                    match sym {
                        0..=15 => {
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            if i == 0 {
                                return Err(corrupt("repeat with no previous length"));
                            }
                            let prev = lengths[i - 1];
                            let rep = 3 + br.bits(2)? as usize;
                            for _ in 0..rep {
                                if i >= total {
                                    return Err(corrupt("length repeat overflow"));
                                }
                                lengths[i] = prev;
                                i += 1;
                            }
                        }
                        17 => {
                            let rep = 3 + br.bits(3)? as usize;
                            i += rep;
                            if i > total {
                                return Err(corrupt("zero repeat overflow"));
                            }
                        }
                        18 => {
                            let rep = 11 + br.bits(7)? as usize;
                            i += rep;
                            if i > total {
                                return Err(corrupt("zero repeat overflow"));
                            }
                        }
                        _ => return Err(corrupt("bad code-length symbol")),
                    }
                }
                if hlit > lengths.len() || hdist > lengths.len() - hlit.min(lengths.len()) {
                    return Err(corrupt("header lengths exceed stream"));
                }
                let lit = Huffman::new(&lengths[..hlit])?;
                let dist = Huffman::new(&lengths[hlit..hlit + hdist])?;
                inflate_block(&mut br, &mut out, &lit, &dist)?;
            }
            _ => return Err(corrupt("invalid block type")),
        }
        if out.len() > expected_max {
            return Err(corrupt("decompressed size exceeds the declared limit"));
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Inflate a zlib stream (RFC 1950): 2-byte header, DEFLATE body,
/// Adler-32 trailer (verified).
pub fn unzlib(data: &[u8], expected_max: usize) -> CResult<Vec<u8>> {
    if data.len() < 6 {
        return Err(corrupt("zlib stream too short"));
    }
    let cmf = data[0];
    let flg = data[1];
    if cmf & 0x0F != 8 || ((cmf as u32) << 8 | flg as u32) % 31 != 0 {
        return Err(corrupt("bad zlib header"));
    }
    if flg & 0x20 != 0 {
        return Err(corrupt("preset dictionary not supported"));
    }
    let out = inflate(&data[2..data.len() - 4], expected_max)?;
    let stored = u32::from_be_bytes([
        data[data.len() - 4],
        data[data.len() - 3],
        data[data.len() - 2],
        data[data.len() - 1],
    ]);
    if adler32(&out) != stored {
        return Err(corrupt("Adler-32 mismatch"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_vectors() {
        // "hello" compressed (fixed Huffman) — well-known zlib bytes.
        let hello = [
            0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
        ];
        assert_eq!(unzlib(&hello, 1024).unwrap(), b"hello");
        // Stored block: zlib header + stored "hi" + adler.
        let stored = [
            0x78, 0x01, 0x01, 0x02, 0x00, 0xfd, 0xff, b'h', b'i', 0x01, 0x3b, 0x00, 0xd2,
        ];
        assert_eq!(unzlib(&stored, 1024).unwrap(), b"hi");
        // Tampered adler is caught.
        let mut bad = hello;
        *bad.last_mut().unwrap() ^= 0xFF;
        assert!(unzlib(&bad, 1024).is_err());
        assert!(unzlib(&[0x78, 0x9c], 1024).is_err());
    }
}
