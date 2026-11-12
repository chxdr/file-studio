//! The few OS calls the core makes itself.
#![allow(unsafe_code)]

/// The local time zone's offset from UTC, in seconds, at `unix_secs`.
pub(crate) fn utc_offset_secs(unix_secs: i64) -> i32 {
    let t: libc::time_t = unix_secs as libc::time_t;
    // SAFETY: `tm` is plain old data, so all-zero is a valid value; `localtime_r`
    // only writes into it and reads `t`, both of which live on this stack frame.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call (see above).
    let r = unsafe { libc::localtime_r(&t, &mut tm) };
    if r.is_null() { 0 } else { tm.tm_gmtoff as i32 }
}
