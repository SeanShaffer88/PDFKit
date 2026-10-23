//! C ABI surface — everything declared in include/pdfkit.h.
//! Conventions (house standard):
//! - Documents (parsed) and generators (being built) live behind
//!   u64 handles in separate registries; kind checks are explicit.
//! - `*mut c_char` results are caller-owned; free with `pk_free_string`.
//!   Byte buffers from pk_render are freed with `pk_free_buffer`.
//! - Failures return 0/null/-1 + a thread-local last-error message.
//! - Nothing unwinds across the FFI boundary.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use crate::pdf::Document;
use crate::write::Generator;
use crate::{
    json_escape, ERR_INTERNAL, ERR_INVALID_ARG, PK_OK, VERSION_MAJOR, VERSION_MINOR, VERSION_PATCH,
};

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((0, CString::new("").unwrap()));
}

static DOCS: Mutex<BTreeMap<u64, Document>> = Mutex::new(BTreeMap::new());
static GENS: Mutex<BTreeMap<u64, Generator>> = Mutex::new(BTreeMap::new());
static NEXT_HANDLE: Mutex<u64> = Mutex::new(1);

pub fn set_error(code: i32, msg: &str) {
    let cleaned: String = msg
        .chars()
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect();
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(cleaned).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(PK_OK, "");
}

fn guard<T, F: FnOnce() -> T>(f: F) -> Result<T, (i32, String)> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(_) => Err((ERR_INTERNAL, "internal panic caught at FFI boundary".into())),
    }
}

fn req_str(p: *const c_char, name: &str) -> Result<String, (i32, String)> {
    if p.is_null() {
        return Err((ERR_INVALID_ARG, format!("{name} must not be null")));
    }
    let s = unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|_| (ERR_INVALID_ARG, format!("{name} must be valid UTF-8")))?;
    Ok(s.to_string())
}

fn opt_str(p: *const c_char) -> Result<String, (i32, String)> {
    if p.is_null() {
        return Ok(String::new());
    }
    req_str(p, "string")
}

fn next_handle() -> u64 {
    let mut n = NEXT_HANDLE.lock().unwrap_or_else(|p| p.into_inner());
    let h = *n;
    *n += 1;
    h
}

fn with_doc<T, F: FnOnce(&Document) -> Result<T, (i32, String)>>(
    handle: u64,
    f: F,
) -> Result<T, (i32, String)> {
    let reg = DOCS.lock().unwrap_or_else(|p| p.into_inner());
    let doc = reg
        .get(&handle)
        .ok_or((ERR_INVALID_ARG, format!("unknown document handle {handle}")))?;
    f(doc)
}

fn with_gen_mut<T, F: FnOnce(&mut Generator) -> Result<T, (i32, String)>>(
    handle: u64,
    f: F,
) -> Result<T, (i32, String)> {
    let mut reg = GENS.lock().unwrap_or_else(|p| p.into_inner());
    let g = reg.get_mut(&handle).ok_or((
        ERR_INVALID_ARG,
        format!("unknown generator handle {handle}"),
    ))?;
    f(g)
}

fn out_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => {
            set_error(ERR_INTERNAL, "result contained a NUL byte");
            std::ptr::null_mut()
        }
    }
}

fn finish_str(run: Result<Result<String, (i32, String)>, (i32, String)>) -> *mut c_char {
    match run {
        Ok(Ok(s)) => {
            clear_error();
            out_string(s)
        }
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            std::ptr::null_mut()
        }
    }
}

fn finish_code(run: Result<Result<(), (i32, String)>, (i32, String)>) -> c_int {
    match run {
        Ok(Ok(())) => {
            clear_error();
            PK_OK
        }
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            code
        }
    }
}

#[no_mangle]
pub extern "C" fn pk_version() -> *const c_char {
    static VERSION_C: &[u8] = b"0.1.0\0";
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn pk_version_major() -> c_int {
    VERSION_MAJOR as c_int
}

#[no_mangle]
pub extern "C" fn pk_version_minor() -> c_int {
    VERSION_MINOR as c_int
}

#[no_mangle]
pub extern "C" fn pk_version_patch() -> c_int {
    VERSION_PATCH as c_int
}

#[no_mangle]
pub extern "C" fn pk_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn pk_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0) as c_int
}

/// # Safety (caller contract): `p` is null or a string this library
/// returned and has not been freed yet.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn pk_free_string(p: *mut c_char) {
    if !p.is_null() {
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

/// # Safety (caller contract): `p`/`len` are a buffer pair returned
/// by pk_render and not yet freed.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn pk_free_buffer(p: *mut u8, len: usize) {
    if !p.is_null() && len > 0 {
        unsafe {
            drop(Vec::from_raw_parts(p, len, len));
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn open_bytes(bytes: &[u8]) -> Result<u64, (i32, String)> {
    let doc = Document::load(bytes)?;
    let h = next_handle();
    DOCS.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(h, doc);
    Ok(h)
}

#[no_mangle]
pub extern "C" fn pk_open_file(path: *const c_char) -> u64 {
    let run = guard(|| {
        let path = req_str(path, "path")?;
        let bytes = crate::platform::read_file(&path)?;
        open_bytes(&bytes)
    });
    match run {
        Ok(Ok(h)) => {
            clear_error();
            h
        }
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            0
        }
    }
}

/// # Safety (caller contract): data/len describe a readable buffer.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn pk_open_bytes(data: *const u8, len: usize) -> u64 {
    let run = guard(|| {
        if data.is_null() || len == 0 {
            return Err((ERR_INVALID_ARG, "data must not be null/empty".into()));
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        open_bytes(bytes)
    });
    match run {
        Ok(Ok(h)) => {
            clear_error();
            h
        }
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn pk_close(handle: u64) -> c_int {
    finish_code(guard(|| {
        let mut docs = DOCS.lock().unwrap_or_else(|p| p.into_inner());
        if docs.remove(&handle).is_some() {
            return Ok(());
        }
        let mut gens = GENS.lock().unwrap_or_else(|p| p.into_inner());
        if gens.remove(&handle).is_some() {
            return Ok(());
        }
        Err((ERR_INVALID_ARG, format!("unknown handle {handle}")))
    }))
}

#[no_mangle]
pub extern "C" fn pk_page_count(handle: u64) -> c_int {
    let run = guard(|| {
        if let Ok(n) = with_doc(handle, |d| Ok(d.pages.len() as c_int)) {
            return Ok(n);
        }
        with_gen_mut(handle, |g| Ok(g.pages.len() as c_int))
    });
    match run {
        Ok(Ok(n)) => n,
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            -1
        }
    }
}

fn json_or_null(v: Option<String>) -> String {
    match v {
        Some(s) => format!("\"{}\"", json_escape(&s)),
        None => "null".to_string(),
    }
}

/// {"pages":N,"version":"1.7","title":...,"author":...,...}
#[no_mangle]
pub extern "C" fn pk_info_json(handle: u64) -> *mut c_char {
    finish_str(guard(|| {
        if let Ok(s) = with_doc(handle, |d| {
            Ok(format!(
                "{{\"pages\":{},\"version\":\"{}\",\"title\":{},\"author\":{},\"creator\":{},\"producer\":{}}}",
                d.pages.len(),
                json_escape(&d.version),
                json_or_null(d.info_string("Title")),
                json_or_null(d.info_string("Author")),
                json_or_null(d.info_string("Creator")),
                json_or_null(d.info_string("Producer")),
            ))
        }) {
            return Ok(s);
        }
        with_gen_mut(handle, |g| {
            Ok(format!(
                "{{\"pages\":{},\"version\":\"1.4\",\"title\":{},\"author\":{},\"creator\":\"PDFKit\",\"producer\":\"PDFKit\"}}",
                g.pages.len(),
                if g.title.is_empty() { "null".into() } else { format!("\"{}\"", json_escape(&g.title)) },
                if g.author.is_empty() { "null".into() } else { format!("\"{}\"", json_escape(&g.author)) },
            ))
        })
    }))
}

/// {"width":612,"height":792,"rotate":0} for page (0-based).
#[no_mangle]
pub extern "C" fn pk_page_size(handle: u64, page: u32) -> *mut c_char {
    finish_str(guard(|| {
        with_doc(handle, |d| {
            let p = d.pages.get(page as usize).ok_or((
                ERR_INVALID_ARG,
                format!("no page {page} (document has {})", d.pages.len()),
            ))?;
            let w = (p.media_box[2] - p.media_box[0]).abs();
            let h = (p.media_box[3] - p.media_box[1]).abs();
            Ok(format!(
                "{{\"width\":{},\"height\":{},\"rotate\":{}}}",
                trim_num(w),
                trim_num(h),
                p.rotate
            ))
        })
    }))
}

fn trim_num(n: f64) -> String {
    if n == n.trunc() {
        format!("{}", n as i64)
    } else {
        format!("{n:.2}")
    }
}

/// Extracted text of one page (0-based), or null on failure.
#[no_mangle]
pub extern "C" fn pk_page_text(handle: u64, page: u32) -> *mut c_char {
    finish_str(guard(|| {
        if let Ok(t) = with_doc(handle, |d| {
            let p = d.pages.get(page as usize).ok_or((
                ERR_INVALID_ARG,
                format!("no page {page} (document has {})", d.pages.len()),
            ))?;
            crate::extract::page_text(d, p)
        }) {
            return Ok(t);
        }
        // Generators report the text drawn so far.
        with_gen_mut(handle, |g| {
            let p = g.pages.get(page as usize).ok_or((
                ERR_INVALID_ARG,
                format!("no page {page} (generator has {})", g.pages.len()),
            ))?;
            Ok(p.ops
                .iter()
                .map(|o| o.text.clone())
                .collect::<Vec<_>>()
                .join("\n"))
        })
    }))
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn pk_create() -> u64 {
    let h = next_handle();
    GENS.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(h, Generator::default());
    clear_error();
    h
}

#[no_mangle]
pub extern "C" fn pk_set_info(handle: u64, title: *const c_char, author: *const c_char) -> c_int {
    finish_code(guard(|| {
        let title = opt_str(title)?;
        let author = opt_str(author)?;
        with_gen_mut(handle, |g| {
            g.title = title;
            g.author = author;
            Ok(())
        })
    }))
}

/// Add a blank page (points). Returns the page index, or -1 on error.
#[no_mangle]
pub extern "C" fn pk_add_page(handle: u64, width: f64, height: f64) -> c_int {
    let run = guard(|| with_gen_mut(handle, |g| g.add_page(width, height).map(|i| i as c_int)));
    match run {
        Ok(Ok(i)) => {
            clear_error();
            i
        }
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            -1
        }
    }
}

/// Draw text on a generator page. Coordinates are PDF points with
/// the origin at the BOTTOM-left of the page.
#[no_mangle]
pub extern "C" fn pk_draw_text(
    handle: u64,
    page: u32,
    x: f64,
    y: f64,
    font_size: f64,
    text: *const c_char,
) -> c_int {
    finish_code(guard(|| {
        let text = req_str(text, "text")?;
        with_gen_mut(handle, |g| {
            g.draw_text(page as usize, x, y, font_size, &text)
        })
    }))
}

/// Render the generator to PDF bytes in a caller-owned buffer.
/// # Safety (caller contract): out_ptr/out_len point to writable memory.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn pk_render(handle: u64, out_ptr: *mut *mut u8, out_len: *mut usize) -> c_int {
    let run = guard(|| {
        if out_ptr.is_null() || out_len.is_null() {
            return Err((ERR_INVALID_ARG, "out_ptr/out_len must not be null".into()));
        }
        with_gen_mut(handle, |g| g.render())
    });
    match run {
        Ok(Ok(bytes)) => {
            let mut boxed = bytes.into_boxed_slice();
            let (ptr, len) = (boxed.as_mut_ptr(), boxed.len());
            std::mem::forget(boxed);
            unsafe {
                *out_ptr = ptr;
                *out_len = len;
            }
            clear_error();
            PK_OK
        }
        Ok(Err((code, msg))) | Err((code, msg)) => {
            set_error(code, &msg);
            code
        }
    }
}

/// Render and write to a file.
#[no_mangle]
pub extern "C" fn pk_save(handle: u64, path: *const c_char) -> c_int {
    finish_code(guard(|| {
        let path = req_str(path, "path")?;
        let bytes = with_gen_mut(handle, |g| g.render())?;
        crate::platform::write_file(&path, &bytes)
    }))
}

/// 1 when the handle is a generator, 0 for a document/unknown.
#[no_mangle]
pub extern "C" fn pk_is_generator(handle: u64) -> c_int {
    let gens = GENS.lock().unwrap_or_else(|p| p.into_inner());
    if gens.contains_key(&handle) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    fn take(p: *mut c_char) -> String {
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        pk_free_string(p);
        s
    }

    #[test]
    fn ffi_generate_then_read_back() {
        let g = pk_create();
        assert_eq!(pk_is_generator(g), 1);
        assert_eq!(
            pk_set_info(g, cs("FFI Doc").as_ptr(), cs("Tester").as_ptr()),
            PK_OK
        );
        let page = pk_add_page(g, 612.0, 792.0);
        assert_eq!(page, 0);
        assert_eq!(
            pk_draw_text(g, 0, 72.0, 720.0, 18.0, cs("FFI round trip works").as_ptr()),
            PK_OK
        );
        let (mut ptr, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(pk_render(g, &mut ptr, &mut len), PK_OK);
        assert!(len > 100);
        let d = pk_open_bytes(ptr, len);
        assert!(d != 0, "reopen failed: {}", pk_last_error_code());
        pk_free_buffer(ptr, len);
        assert_eq!(pk_is_generator(d), 0);
        assert_eq!(pk_page_count(d), 1);
        let info = take(pk_info_json(d));
        assert!(info.contains("\"title\":\"FFI Doc\""), "{info}");
        let text = take(pk_page_text(d, 0));
        assert!(text.contains("FFI round trip works"), "{text}");
        let size = take(pk_page_size(d, 0));
        assert!(size.contains("\"width\":612"), "{size}");
        assert_eq!(pk_close(g), PK_OK);
        assert_eq!(pk_close(d), PK_OK);
        assert_eq!(pk_page_count(d), -1);
    }
}
