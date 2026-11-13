//! `safe-transfer`: verified, crash-proof, resumable copies from the
//! terminal and from scripts.
//!
//! Every `copy` is a job with a journal on this Mac
//! (`~/Library/Application Support/Safe Transfer/` unless `--journal-dir`
//! says otherwise), so an interrupted copy can be resumed with
//! `safe-transfer resume <job>`.
//!
//! Exit codes: 0 = every file copied and verified; 1 = some files failed,
//! were skipped or are waiting (each is listed); 2 = nothing could start.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use safe_transfer::{
    Cancel, Engine, EngineConfig, Event, Item, JobId, JobInfo, JobReport, JobSpec, JobState,
    Options, Platform, Symlinks, human,
};
use serde_json::json;

#[derive(Parser)]
#[command(
    name = "safe-transfer",
    version,
    about = "Crash-proof, verified, resumable file copies"
)]
struct Cli {
    /// Where job journals are kept.
    #[arg(long, global = true)]
    journal_dir: Option<PathBuf>,
    /// One JSON object per line (progress, each file, a summary).
    #[arg(long, global = true)]
    json: bool,
    /// Longest the journal goes without a write, in milliseconds (tests).
    #[arg(long, global = true, hide = true, default_value_t = 500)]
    journal_interval_ms: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Copy files and folders into a folder. Each file gets its real name
    /// only after it is flushed to the disk and read back from it with
    /// matching hashes.
    Copy {
        /// Files and folders to copy.
        #[arg(required = true, num_args = 1..)]
        sources: Vec<PathBuf>,
        /// The folder to copy into.
        dest: PathBuf,
        #[command(flatten)]
        opts: CopyOpts,
    },
    /// Show what copying would do (counts, sizes, skips, free space). Writes nothing.
    Plan {
        /// Files and folders to copy.
        #[arg(required = true, num_args = 1..)]
        sources: Vec<PathBuf>,
        /// The folder to copy into.
        dest: PathBuf,
    },
    /// List unfinished (and recently finished) jobs.
    Jobs,
    /// Continue an interrupted, waiting, paused or cancelled job.
    Resume {
        /// The job id.
        job: String,
    },
    /// Cancel a job: its partial files are removed; finished files stay.
    Cancel {
        /// The job id.
        job: String,
        /// Close the job for good instead of keeping it resumable.
        #[arg(long)]
        close: bool,
    },
    /// Show a job's report.
    Report {
        /// The job id.
        job: String,
    },
    /// Compare two files or folders by content, reading from the disk (not memory).
    Compare {
        /// The first file or folder.
        a: PathBuf,
        /// The second file or folder.
        b: PathBuf,
    },
    /// List mounted volumes with the ids Safe Transfer recognises them by.
    Volumes,
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
    /// A label for the job ("Smith wedding → CLIENT_A").
    #[arg(long)]
    label: Option<String>,
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

fn journal_dir(cli: &Option<PathBuf>) -> PathBuf {
    cli.clone().unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Safe Transfer")
    })
}

/// Prints events: status lines on stderr, or JSON lines on stdout.
struct Printer {
    json: bool,
    last: Mutex<Instant>,
}

impl Printer {
    fn on(&self, e: Event) {
        match e {
            Event::Progress(p) => {
                let mut last = self.last.lock().unwrap();
                if last.elapsed() < Duration::from_millis(100) {
                    return;
                }
                *last = Instant::now();
                if self.json {
                    println!(
                        "{}",
                        json!({"event": "progress", "job": p.job.as_str(), "phase": p.phase.as_str(),
                               "file": p.file, "done": p.done, "total": p.total,
                               "files_done": p.files_done, "files_total": p.files_total,
                               "bytes_per_sec": p.bytes_per_sec, "eta_secs": p.eta_secs})
                    );
                } else {
                    let pct = (p.done * 100).checked_div(p.total).unwrap_or(100);
                    let eta = p
                        .eta_secs
                        .map(|s| format!(", {s} s left"))
                        .unwrap_or_default();
                    eprint!(
                        "\r\x1b[2K{}/{} files  {pct}%  {}/s{eta}  {} {}",
                        p.files_done,
                        p.files_total,
                        human(p.bytes_per_sec),
                        p.phase.as_str(),
                        p.file.map(|f| f.display().to_string()).unwrap_or_default()
                    );
                    let _ = std::io::stderr().flush();
                }
            }
            Event::FileDone { path, report, .. } => {
                if self.json {
                    println!(
                        "{}",
                        json!({"event": "file_done", "path": path, "outcome": report.outcome.as_str(),
                               "bytes": report.bytes, "xxh3_128": (!report.hashes.chunks.is_empty()).then(|| report.hashes.file_hex()),
                               "chunks": report.hashes.chunks.len(), "chunks_reused": report.chunks_reused,
                               "source_reread": report.source_reread, "flush": format!("{:?}", report.flush_level),
                               "metadata_lost": report.metadata_lost.iter().map(|m| m.describe()).collect::<Vec<_>>()})
                    );
                }
            }
            Event::FileNotCopied {
                path,
                failed,
                reason,
                ..
            } => {
                if self.json {
                    println!(
                        "{}",
                        json!({"event": "file_not_copied", "path": path, "failed": failed, "reason": reason})
                    );
                } else {
                    eprintln!(
                        "\r\x1b[2K{} {}: {reason}",
                        if failed { "✗" } else { "–" },
                        path.display()
                    );
                }
            }
            Event::JobState { job, state } => {
                if self.json {
                    println!(
                        "{}",
                        json!({"event": "state", "job": job.as_str(), "state": state.as_str()})
                    );
                }
            }
            Event::WaitingForSource { missing, .. } => {
                for m in missing {
                    if self.json {
                        println!(
                            "{}",
                            json!({"event": "waiting_for_source", "volume": m.volume, "uuid": m.uuid, "files": m.files})
                        );
                    } else {
                        eprintln!(
                            "\r\x1b[2K{} files are waiting for the disk “{}” to be connected",
                            m.files.len(),
                            m.volume
                        );
                    }
                }
            }
            Event::WaitingForDestination { volume, .. } => {
                if self.json {
                    println!(
                        "{}",
                        json!({"event": "waiting_for_destination", "volume": volume})
                    );
                } else {
                    eprintln!(
                        "\r\x1b[2KThe destination “{volume}” was disconnected. Connect it and run safe-transfer resume."
                    );
                }
            }
            Event::ResumeAvailable { .. } | Event::JobFinished { .. } => {}
        }
    }
}

fn open_engine(cli: &Cli, printer: Arc<Printer>) -> Result<Engine, ExitCode> {
    #[allow(unused_mut)]
    let mut cfg = EngineConfig::new(journal_dir(&cli.journal_dir));
    cfg.journal_interval = Duration::from_millis(cli.journal_interval_ms);
    #[cfg(feature = "fault-injection")]
    {
        cfg.faults = safe_transfer::fault::SpecHook::from_env()
            .map(|h| Arc::new(h) as Arc<dyn safe_transfer::fault::FaultHook>);
    }
    let sink = move |e: Event| printer.on(e);
    Engine::open(cfg, Platform::native(), Arc::new(sink)).map_err(|e| {
        eprintln!("safe-transfer: {e}");
        ExitCode::from(2)
    })
}

fn report_json(r: &JobReport, info: &JobInfo) -> serde_json::Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    v["event"] = json!("summary");
    v["state"] = json!(info.state.as_str());
    v
}

fn print_report(json_out: bool, r: &JobReport, info: &JobInfo) {
    if json_out {
        println!("{}", report_json(r, info));
        return;
    }
    eprint!("\r\x1b[2K");
    let verified = r.copied_verified + r.verified_from_memory;
    println!(
        "{verified} copied and checked, {} cloned, {} links, {} folders — {} in {:.1} s ({})",
        r.cloned,
        r.links,
        r.folders,
        human(r.bytes),
        r.seconds,
        info.state.as_str()
    );
    if r.resumed > 0 {
        println!(
            "  {} continued from an interrupted copy ({} chunks reused)",
            r.resumed, r.chunks_reused
        );
    }
    if r.source_reread > 0 {
        println!(
            "  {} originals read twice, both reads agreed",
            r.source_reread
        );
    }
    if r.verified_from_memory > 0 {
        println!(
            "  {} checked partly from memory (the cache couldn't be emptied)",
            r.verified_from_memory
        );
    }
    if r.os_flush_only > 0 {
        println!(
            "  {} on a disk that only confirmed an OS-level flush",
            r.os_flush_only
        );
    }
    for n in &r.restarted {
        println!("  restarted {}: {}", n.path.display(), n.reason);
    }
    for n in &r.skipped {
        println!("  skipped {}: {}", n.path.display(), n.reason);
    }
    for n in &r.failed {
        println!("  ✗ {}: {}", n.path.display(), n.reason);
    }
    for n in &r.metadata_lost {
        println!("  couldn't keep for {}: {}", n.path.display(), n.reason);
    }
}

fn exit_for(r: &JobReport, info: &JobInfo) -> ExitCode {
    if info.state == JobState::Finished && r.failed.is_empty() && r.skipped.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Runs `id` to the end (or until it waits) and prints its report.
fn run_job(engine: &Engine, id: JobId, json_out: bool) -> ExitCode {
    if let Some(info) = engine.job(id).filter(|i| i.state.is_final()) {
        let r = engine.report(id).unwrap_or_default();
        print_report(json_out, &r, &info);
        return exit_for(&r, &info);
    }
    if let Err(e) = engine.resume(id) {
        eprintln!("safe-transfer: {e}");
        return ExitCode::from(2);
    }
    let Some(info) = engine.wait(id, Duration::from_secs(365 * 86_400)) else {
        return ExitCode::from(2);
    };
    let r = engine.report(id).unwrap_or_default();
    print_report(json_out, &r, &info);
    exit_for(&r, &info)
}

fn items(sources: &[PathBuf]) -> Vec<Item> {
    sources.iter().cloned().map(Item::Path).collect()
}

fn find_job(engine: &Engine, s: &str) -> Result<JobId, ExitCode> {
    JobId::parse(s)
        .filter(|id| engine.job(*id).is_some())
        .ok_or_else(|| {
            eprintln!("safe-transfer: there is no job {s}");
            ExitCode::from(2)
        })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let printer = Arc::new(Printer {
        json: cli.json,
        last: Mutex::new(Instant::now() - Duration::from_secs(1)),
    });
    let json_out = cli.json;
    match &cli.cmd {
        Cmd::Compare { a, b } => return do_compare(a, b, json_out),
        Cmd::Volumes => return do_volumes(json_out),
        _ => {}
    }
    let engine = match open_engine(&cli, printer) {
        Ok(e) => e,
        Err(c) => return c,
    };
    match &cli.cmd {
        Cmd::Copy {
            sources,
            dest,
            opts,
        } => {
            let mut spec = JobSpec::new(items(sources), dest);
            spec.options = Options {
                chunk_size: opts.chunk,
                reread_source: opts.reread_source,
                symlinks: if opts.follow_symlinks {
                    Symlinks::Follow
                } else {
                    Symlinks::CopyLink
                },
                clone_same_volume: !opts.no_clone,
                ..Options::default()
            };
            if let Some(l) = &opts.label {
                spec.label = l.clone();
            }
            let id = match engine.create(spec) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("safe-transfer: {e}");
                    return ExitCode::from(2);
                }
            };
            if json_out {
                println!("{}", json!({"event": "job_created", "job": id.as_str()}));
            } else {
                eprintln!("job {id}");
            }
            let _ = std::io::stdout().flush();
            run_job(&engine, id, json_out)
        }
        Cmd::Plan { sources, dest } => {
            let spec = JobSpec::new(items(sources), dest);
            match engine.plan(&spec) {
                Ok(p) => {
                    if json_out {
                        println!(
                            "{}",
                            json!({"files": p.files, "folders": p.folders, "links": p.links, "bytes": p.bytes,
                                   "need": p.need, "margin": p.margin, "free": p.free, "fits": p.fits,
                                   "skipped": p.skipped, "dest_fs": p.dest_volume.fs_type})
                        );
                    } else {
                        println!(
                            "{} files, {} folders, {} links, {}",
                            p.files,
                            p.folders,
                            p.links,
                            human(p.bytes)
                        );
                        for n in &p.skipped {
                            println!("  would skip {}: {}", n.path.display(), n.reason);
                        }
                        println!(
                            "needs {} (plus a {} safety margin); {} free — {}",
                            human(p.need),
                            human(p.margin),
                            human(p.free),
                            if p.fits { "fits" } else { "won't fit" }
                        );
                    }
                    if p.fits && p.skipped.is_empty() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    eprintln!("safe-transfer: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Cmd::Jobs => {
            for j in engine.jobs() {
                if json_out {
                    println!(
                        "{}",
                        json!({"job": j.id.as_str(), "label": j.label, "state": j.state.as_str(),
                               "dest": j.dest, "dest_volume": j.dest_volume, "files_total": j.files_total,
                               "files_done": j.files_done, "files_failed": j.files_failed,
                               "files_skipped": j.files_skipped, "bytes_total": j.bytes_total,
                               "bytes_done": j.bytes_done})
                    );
                } else {
                    println!(
                        "{}  {:<24} {}/{} files  {} left  {}",
                        j.id,
                        j.state.as_str(),
                        j.files_done,
                        j.files_total,
                        human(j.bytes_total - j.bytes_done),
                        j.label
                    );
                }
            }
            for d in engine.damaged() {
                eprintln!("damaged journal (left alone): {}", d.display());
            }
            ExitCode::SUCCESS
        }
        Cmd::Resume { job } => match find_job(&engine, job) {
            Ok(id) => run_job(&engine, id, json_out),
            Err(c) => c,
        },
        Cmd::Cancel { job, close } => match find_job(&engine, job) {
            Ok(id) => {
                let how = if *close {
                    Cancel::Close
                } else {
                    Cancel::KeepResumable
                };
                match engine.cancel(id, how) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("safe-transfer: {e}");
                        ExitCode::from(2)
                    }
                }
            }
            Err(c) => c,
        },
        Cmd::Report { job } => match find_job(&engine, job) {
            Ok(id) => {
                let info = engine.job(id).expect("found above");
                let r = engine.report(id).unwrap_or_default();
                print_report(json_out, &r, &info);
                exit_for(&r, &info)
            }
            Err(c) => c,
        },
        Cmd::Compare { .. } | Cmd::Volumes => unreachable!("handled above"),
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
