//! Spike 5: case sensitivity and Unicode normalisation behaviour of a volume. Usage: s5_case <dir>
use fstudio_spikes::list::*;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let d = Path::new(&dir).join("s5");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let c = CString::new(d.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid C string; pathconf only reads filesystem properties.
    let cs = unsafe { libc::pathconf(c.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    println!("volume {dir}: pathconf(_PC_CASE_SENSITIVE) = {cs}");
    std::fs::write(d.join("File"), b"A").unwrap();
    println!("create 'File' then 'file': {:?}", std::fs::OpenOptions::new().write(true).create_new(true).open(d.join("file")).map(|_| "both exist").map_err(|e| e.kind()));
    println!("stat 'FILE' (third spelling): {}", d.join("FILE").exists());
    let nfc = "caf\u{e9}"; let nfd = "cafe\u{301}";
    std::fs::write(d.join(nfc), b"C").unwrap();
    println!("created NFC name; open by NFD spelling: {}", d.join(nfd).exists());
    println!("create NFD spelling with create_new: {:?}", std::fs::OpenOptions::new().write(true).create_new(true).open(d.join(nfd)).map(|_| "created second file").map_err(|e| e.kind()));
    let nfd_only = "u\u{308}ber"; // stored as NFD
    std::fs::write(d.join(nfd_only), b"N").unwrap();
    for e in list_bulk(&d, &mut || {}) {
        let n = String::from_utf8_lossy(&e.name);
        let is_ascii = n.is_ascii();
        println!("  listed (getattrlistbulk): {:?} bytes={:02x?} ascii={is_ascii}", n, e.name);
    }
    for e in std::fs::read_dir(&d).unwrap() {
        println!("  read_dir: {:?}", e.unwrap().file_name());
    }
    println!("lookup 'uber' with NFC ü ('\u{fc}ber'): {}", d.join("\u{fc}ber").exists());
    println!("lookup 'CAFÉ' (upper, NFD): {}", d.join("CAFE\u{301}").exists());
    let _ = std::fs::remove_dir_all(&d);
}
