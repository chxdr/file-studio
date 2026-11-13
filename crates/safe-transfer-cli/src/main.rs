//! `safe-transfer`: verified, crash-proof copies from the terminal and from
//! scripts. Milestone 1: single files (`copy`, `plan`), plus `compare` and
//! `volumes`. Jobs, `resume`, `move` and folders come in M2–M3.
//!
//! Exit codes: 0 = every file copied and verified; 1 = some files failed or
//! were skipped (each is listed); 2 = nothing could start (bad arguments).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use safe_transfer::{
    Control, CopyRequest, Failure, FileProgress, FileReport, Options, Outcome, Platform,
    ProgressSink, Symlinks,
};
use serde_json::json;

#[derive(Parser)]
#[command(
    name = "safe-transfer",
    version,
    about = "Crash-proof, verified file copies"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Copy files into a folder. Each file gets its real name only after it
    /// is flushed to the disk and read back from it with matching hashes.
    Copy {
        /// Files to copy (folders come in a later version).
        #[arg(required = true, num_args = 1..)]
        sources: Vec<PathBuf>,
        /// The folder to copy into.
        dest: PathBuf,
        #[command(flatten)]
        opts: CopyOpts,
        /// One JSON object per line (progress, each file, a summary).
        #[arg(long)]
        json: bool,
    },
    /// Show what copying would do (sizes, conflicts, free space). Writes nothing.
    Plan {
        /// Files to copy.
        #[arg(required = true, num_args = 1..)]
        sources: Vec<PathBuf>,
        /// The folder to copy into.
        dest: PathBuf,
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Compare two files or folders by content, reading from the disk (not memory).
    Compare {
        /// The first file or folder.
        a: PathBuf,
        /// The second file or folder.
        b: PathBuf,
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List mounted volumes with the ids Safe Transfer recognises them by.
    Volumes {
        /// JSON output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CopyOpts {
    /// Chunk size for hashing and flushing (e.g. 64M, 16M, 256M).
    #[arg(long, default_value = "64M", value_parser = parse_size)]
    chunk: u64,
    /// Read each original a second time and require both reads to agree.
    /// Catches a failing card or reader; takes about twice the reading time.
    #[arg(long)]
    reread_source: bool,
    /// Copy what symlinks point to instead of the links themselves.
    #[arg(long)]
    follow_symlinks: bool,
    /// Copy byte for byte even on the same APFS volume (no instant clone).
    #[arg(long)]
    no_clone: bool,
}

/// Parses "64M", "1G", "4096", "512K".
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mul) = match s.chars().last().map(|c| c.to_ascii_uppercase()) {
        Some('K') => (&s[..s.len() - 1], 1u64 << 10),
        Some('M') => (&s[..s.len() - 1], 1 << 20),
        Some('G') => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    num.parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mul))
        .ok_or_else(|| format!("“{s}” isn't a size like 64M"))
}

fn human(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", b / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

/// Prints progress: a status line on stderr, or JSON lines on stdout.
struct Printer {
    json: bool,
    name: Mutex<String>,
}

impl ProgressSink for Printer {
    fn progress(&self, p: &FileProgress) {
        if self.json {
            println!(
                "{}",
                json!({"event": "progress", "file": *self.name.lock().unwrap(),
                       "phase": p.phase.as_str(), "done": p.done, "total": p.total})
            );
        } else {
            let pct = (p.done * 100).checked_div(p.total).unwrap_or(100);
            eprint!(
                "\r\x1b[2K{}  {} {pct}%",
                self.name.lock().unwrap(),
                p.phase.as_str()
            );
            let _ = std::io::stderr().flush();
        }
    }
}

fn file_json(r: &FileReport) -> serde_json::Value {
    json!({
        "event": "file_done",
        "source": r.source, "dest": r.dest, "outcome": r.outcome.as_str(),
        "bytes": r.bytes,
        "xxh3_128": matches!(r.outcome, Outcome::Verified | Outcome::VerifiedFromMemory)
            .then(|| r.hashes.file_hex()),
        "chunks": r.hashes.chunks.len(), "source_reread": r.source_reread,
        "flush": format!("{:?}", r.flush_level), "rename": format!("{:?}", r.rename),
        "retries": r.retries,
        "metadata_lost": r.metadata_lost.iter().map(|m| m.describe()).collect::<Vec<_>>(),
        "copy_ms": r.copy_time.as_millis() as u64, "flush_ms": r.flush_time.as_millis() as u64,
        "verify_ms": r.verify_time.as_millis() as u64, "reread_ms": r.reread_time.as_millis() as u64,
    })
}

fn fail_json(src: &Path, f: &Failure) -> serde_json::Value {
    json!({"event": "file_failed", "source": src, "reason": f.to_string(),
           "partial_left": f.partial_left})
}

fn do_copy(sources: &[PathBuf], dest: &Path, o: &CopyOpts, json_out: bool) -> ExitCode {
    if !dest.is_dir() {
        eprintln!("safe-transfer: {} is not a folder", dest.display());
        return ExitCode::from(2);
    }
    #[allow(unused_mut)]
    let mut opts = Options {
        chunk_size: o.chunk,
        reread_source: o.reread_source,
        symlinks: if o.follow_symlinks {
            Symlinks::Follow
        } else {
            Symlinks::CopyLink
        },
        clone_same_volume: !o.no_clone,
        ..Options::default()
    };
    #[cfg(feature = "fault-injection")]
    if let Some(h) = safe_transfer::fault::SpecHook::from_env() {
        opts.faults = Some(std::sync::Arc::new(h));
    }
    let platform = Platform::native();
    let ctl = Control::new();
    let printer = Printer {
        json: json_out,
        name: Mutex::new(String::new()),
    };
    let started = Instant::now();
    let (mut ok, mut failed, mut bytes) = (0u32, 0u32, 0u64);
    for src in sources {
        *printer.name.lock().unwrap() = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let req = match CopyRequest::into_dir(src, dest) {
            Ok(r) => r,
            Err(e) => {
                failed += 1;
                eprintln!("✗ {}: {e}", src.display());
                continue;
            }
        };
        match safe_transfer::copy_file(&platform, &req, &opts, &ctl, &printer) {
            Ok(r) => {
                ok += 1;
                bytes += r.bytes;
                if json_out {
                    println!("{}", file_json(&r));
                } else {
                    eprint!("\r\x1b[2K");
                    let mut line = format!(
                        "✓ {}  {}  {}",
                        r.dest.display(),
                        human(r.bytes),
                        r.outcome.as_str()
                    );
                    if r.source_reread {
                        line.push_str(", original read twice");
                    }
                    if r.flush_level == safe_transfer::FlushLevel::OsOnly {
                        line.push_str(", disk only confirmed an OS-level flush");
                    }
                    println!("{line}");
                    for m in &r.metadata_lost {
                        println!("    couldn't keep: {}", m.describe());
                    }
                }
            }
            Err(f) => {
                failed += 1;
                if json_out {
                    println!("{}", fail_json(src, &f));
                } else {
                    eprint!("\r\x1b[2K");
                    println!("✗ {}: {f}", src.display());
                    if let Some(p) = &f.partial_left {
                        println!("    a hidden partial file was left at {}", p.display());
                    }
                }
            }
        }
    }
    let secs = started.elapsed().as_secs_f64();
    if json_out {
        println!(
            "{}",
            json!({"event": "summary", "copied": ok, "failed": failed, "bytes": bytes, "seconds": secs})
        );
    } else {
        println!(
            "{ok} copied and checked, {failed} failed — {} in {secs:.1} s",
            human(bytes)
        );
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn do_plan(sources: &[PathBuf], dest: &Path, json_out: bool) -> ExitCode {
    let platform = Platform::native();
    let mut bad = false;
    let mut need = 0u64;
    let mut free = None;
    for src in sources {
        let r = CopyRequest::into_dir(src, dest).and_then(|r| safe_transfer::plan(&platform, &r));
        match r {
            Ok(p) => {
                need += p.need;
                free = Some(p.dest_volume.free);
                if json_out {
                    println!(
                        "{}",
                        json!({"source": src, "bytes": p.bytes, "need": p.need, "conflict": p.conflict,
                               "same_volume": p.same_volume, "link": p.is_link, "icloud_placeholder": p.dataless,
                               "dest_fs": p.dest_volume.fs_type})
                    );
                } else {
                    let mut notes = Vec::new();
                    if p.conflict {
                        notes.push("already exists");
                    }
                    if p.dataless {
                        notes.push("iCloud placeholder, would be skipped");
                    }
                    if p.same_volume && p.dest_volume.fs_type == "apfs" {
                        notes.push("instant clone");
                    }
                    println!(
                        "{}  {}  {}",
                        src.display(),
                        human(p.bytes),
                        notes.join(", ")
                    );
                }
                bad |= p.conflict;
            }
            Err(e) => {
                bad = true;
                println!("✗ {}: {}", src.display(), safe_transfer::describe(&e));
            }
        }
    }
    if let Some(free) = free {
        let fits = safe_transfer::space::fits(need, free);
        bad |= !fits;
        if json_out {
            println!(
                "{}",
                json!({"need": need, "margin": safe_transfer::space::margin(need), "free": free, "fits": fits})
            );
        } else {
            println!(
                "needs {} (plus a {} safety margin); {} free — {}",
                human(need),
                human(safe_transfer::space::margin(need)),
                human(free),
                if fits { "fits" } else { "won't fit" }
            );
        }
    }
    if bad {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn do_compare(a: &Path, b: &Path, json_out: bool) -> ExitCode {
    match safe_transfer::compare::compare(&Platform::native(), a, b) {
        Ok(diffs) => {
            use safe_transfer::compare::Difference::*;
            for d in &diffs {
                let (kind, p, why) = match d {
                    OnlyInA(p) => ("only_in_a", p, None),
                    OnlyInB(p) => ("only_in_b", p, None),
                    Differs(p) => ("differs", p, None),
                    Unreadable(p, e) => ("unreadable", p, Some(e.clone())),
                };
                if json_out {
                    println!("{}", json!({"difference": kind, "path": p, "error": why}));
                } else {
                    println!(
                        "{kind}: {}{}",
                        p.display(),
                        why.map(|w| format!(" ({w})")).unwrap_or_default()
                    );
                }
            }
            if diffs.is_empty() {
                if !json_out {
                    println!("identical");
                }
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("safe-transfer: {}", safe_transfer::describe(&e));
            ExitCode::from(2)
        }
    }
}

fn do_volumes(json_out: bool) -> ExitCode {
    match Platform::native().volumes.mounted() {
        Ok(vols) => {
            for v in vols {
                if json_out {
                    println!(
                        "{}",
                        json!({"uuid": v.uuid, "fs": v.fs_type, "name": v.name, "mount": v.mount,
                               "device": v.device, "capacity": v.capacity, "free": v.free, "block_size": v.block_size})
                    );
                } else {
                    println!(
                        "{:<36}  {:<6}  {:>9}  {}",
                        v.uuid.as_deref().unwrap_or("(no id)"),
                        v.fs_type,
                        human(v.capacity),
                        v.mount.display()
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("safe-transfer: {}", safe_transfer::describe(&e));
            ExitCode::from(2)
        }
    }
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::Copy {
            sources,
            dest,
            opts,
            json,
        } => do_copy(&sources, &dest, &opts, json),
        Cmd::Plan {
            sources,
            dest,
            json,
        } => do_plan(&sources, &dest, json),
        Cmd::Compare { a, b, json } => do_compare(&a, &b, json),
        Cmd::Volumes { json } => do_volumes(json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("64M"), Ok(64 << 20));
        assert_eq!(parse_size("1g"), Ok(1 << 30));
        assert_eq!(parse_size("4096"), Ok(4096));
        assert!(parse_size("lots").is_err());
        assert!(parse_size("99999999999999999999G").is_err());
    }
}
