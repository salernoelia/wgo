#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));


// The C++ default library loader calls this function. The bytes stay in the
// executable's read-only data for its entire lifetime; no runtime file is needed.
/// # Safety
/// `size` must point to a writable `usize`.
#[cfg(feature = "metal")]
#[no_mangle]
pub unsafe extern "C" fn wgo_mlx_metallib(size: *mut usize) -> *const u8 {
    static METALLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/build/lib/mlx.metallib"));
    // SAFETY: the only caller is the native loader, which passes a valid size_t.
    unsafe { *size = METALLIB.len(); }
    METALLIB.as_ptr()
}
