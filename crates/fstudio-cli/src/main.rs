//! `fstudio`: the File Studio core from the command line.
//!
//! Every command goes through `fstudio-core` (the same engine the app uses).
//! By default it keeps its own database in
//! `~/Library/Application Support/File Studio CLI`, so it never touches the
//! app's state unless `--data-dir` points there.

use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use crossbeam_channel::{Receiver, unbounded};
use fstudio_core::*;

#[derive(Parser)]
#[command(
    name = "fstudio",
    version,
    about = "File Studio's core, from the command line"
)]
struct Cli {
    /// Where the CLI keeps its database (default: its own folder, not the app's).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Use this folder as the Trash instead of the system Trash.
    #[arg(long, global = true)]
    trash_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum SortArg {
    Name,
    Size,
    Modified,
    Created,
    Kind,
    Extension,
}

#[derive(Clone, Copy, ValueEnum)]
enum OnConflict {
    Ask,
    KeepBoth,
    Replace,
    Skip,
    Merge,
}

#[derive(Subcommand)]
enum Cmd {
    /// List a folder, sorted and formatted by the core.
    Ls {
        /// The folder (default: the current folder).
        path: Option<String>,
        #[arg(long, value_enum, default_value = "name")]
        sort: SortArg,
        /// Largest/newest/Z first.
        #[arg(long)]
        reverse: bool,
        /// Include hidden files.
        #[arg(long)]
        hidden: bool,
        /// Only names matching this text or glob.
        #[arg(long)]
        filter: Option<String>,
        /// One JSON row per line.
        #[arg(long)]
        json: bool,
    },
    /// Watch a folder and print changes as the core's listing is patched.
    Watch {
        dir: String,
        /// Stop after this many seconds (default: run until interrupted).
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// Resolve typed text the way the address bar does (and record a visit).
    Go { text: String },
    /// Parse typed or pasted text without touching history.
    Parse { text: String },
    /// Address-bar completions for the text.
    Complete { text: String },
    /// Remembered folders matching the words (zoxide's rule).
    Z {
        query: Vec<String>,
        /// Print every match with its score.
        #[arg(long, short)]
        list: bool,
    },
    /// Breadcrumbs for a path.
    Crumbs { path: Option<String> },
    /// Copy into a folder through the queue.
    Cp {
        #[arg(required = true, num_args = 2..)]
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value = "ask")]
        on_conflict: OnConflict,
    },
    /// Move into a folder through the queue.
    Mv {
        #[arg(required = true, num_args = 2..)]
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value = "ask")]
        on_conflict: OnConflict,
    },
    /// Move to the Trash (Finder's Put Back works).
    Trash {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Create a folder.
    Mkdir { path: PathBuf },
    /// Rename an item in place.
    Rename { path: PathBuf, new_name: String },
    /// Undo the last file operation.
    Undo,
    /// Redo the last undone operation.
    Redo,
    /// Operations: list them, or resume interrupted ones.
    Ops {
        #[command(subcommand)]
        action: Option<OpsCmd>,
    },
    /// The effective keymap.
    Keys {
        #[command(subcommand)]
        action: Option<KeysCmd>,
    },
    /// Read and change settings.
    Settings {
        #[command(subcommand)]
        action: SettingsCmd,
    },
    /// Import zoxide's history into frecency.
    ImportZoxide {
        /// zoxide's db.zo (default: its usual location).
        path: Option<String>,
    },
}

#[derive(Subcommand)]
enum OpsCmd {
    /// Every operation this run knows, plus interrupted ones.
    List,
    /// Resume an interrupted operation (or all of them).
    Resume { id: Option<i64> },
    /// Cancel an operation (finished items stay).
    Cancel { id: i64 },
}

#[derive(Subcommand)]
enum KeysCmd {
    /// Every binding in force.
    List {
        /// Only commands whose id starts with this.
        prefix: Option<String>,
    },
    /// Clashes and macOS shortcuts.
    Conflicts,
    /// The user's changes as JSON.
    Export,
    /// Replace the user's changes from a JSON file.
    Import { file: PathBuf },
    /// Commands with their primary shortcut.
    Commands,
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// One value by dotted key, or everything.
    Get { key: Option<String> },
    /// Change one value: JSON (`true`, `12`, `"Vim"`) or plain text.
    Set { key: String, value: String },
    /// Every setting as JSON.
    Export,
    /// Replace every setting from an exported file.
    Import { file: PathBuf },
}

struct App {
    engine: Arc<Engine>,
    rx: Receiver<CoreEvent>,
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("File Studio CLI")
}

fn abs(p: &Path) -> Result<PathBuf> {
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    Ok(p)
}

fn cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".into())
}

impl App {
    fn open(cli: &Cli) -> Result<App> {
        let (tx, rx) = unbounded();
        let engine = Engine::new(
            EngineConfig {
                data_dir: cli
                    .data_dir
                    .clone()
                    .unwrap_or_else(default_data_dir)
                    .to_string_lossy()
                    .into_owned(),
                home: None,
                trash_dir: cli
                    .trash_dir
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                poll_watcher: false,
            },
            Arc::new(tx),
        )
        .map_err(|e| anyhow!("opening the core: {e}"))?;
        Ok(App { engine, rx })
    }

    /// A tab showing `path` (relative to the current folder), with its listing finished.
    fn tab_at(&self, path: &str) -> Result<Arc<Tab>> {
        let path = if path.starts_with('/') || path.starts_with('~') {
            path.to_string()
        } else {
            format!("{}/{path}", cwd())
        };
        let path = path.as_str();
        self.engine.new_window(Some(path.to_string()))?;
        let tab = self.engine.active_tab().ok_or_else(|| anyhow!("no tab"))?;
        let id = tab.id();
        let end = Instant::now() + Duration::from_secs(120);
        loop {
            let l = tab.listing();
            if l.state() != ListingState::Loading {
                break;
            }
            match self.rx.recv_deadline(end) {
                Ok(CoreEvent::ListingChanged { tab, kind, .. })
                    if tab == id && matches!(kind, ChangeKind::Sorted | ChangeKind::Error) => {}
                Ok(_) => continue,
                Err(_) => bail!("timed out listing {path}"),
            }
        }
        Ok(tab)
    }

    /// Waits for `ops`, printing progress and asking about conflicts.
    fn follow(&self, ops: &[OpId], ask: bool) -> Result<bool> {
        let mut left: BTreeSet<OpId> = ops.iter().copied().collect();
        let mut ok = true;
        let tty = std::io::stderr().is_terminal();
        while !left.is_empty() {
            let ev = match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(e) => e,
                Err(_) => {
                    // Events can race the enqueue; check the queue directly.
                    let q = self.engine.ops();
                    left.retain(|id| q.status(*id).is_some_and(|s| !s.state.is_finished()));
                    continue;
                }
            };
            match ev {
                CoreEvent::OpProgress { ops } => {
                    for s in ops.iter().filter(|s| left.contains(&s.id)) {
                        let pct = if s.bytes_total > 0 {
                            s.bytes_done as f64 * 100.0 / s.bytes_total as f64
                        } else if s.files_total > 0 {
                            s.files_done as f64 * 100.0 / s.files_total as f64
                        } else {
                            0.0
                        };
                        let line = format!(
                            "{} — {}/{} items, {:.0}%, {}/s",
                            s.title,
                            s.files_done,
                            s.files_total,
                            pct,
                            human(s.speed_bps as u64)
                        );
                        if tty {
                            eprint!("\r\x1b[2K{line}");
                        }
                    }
                }
                CoreEvent::OpConflict { op, conflict } if left.contains(&op) => {
                    if tty {
                        eprintln!();
                    }
                    let (d, all) = if ask {
                        match prompt_conflict(&conflict) {
                            Ok(x) => x,
                            Err(e) => {
                                // Nobody to ask: stop cleanly instead of leaving it waiting.
                                let _ = self.engine.ops().cancel(op);
                                return Err(e);
                            }
                        }
                    } else {
                        (ConflictDecision::Skip, true)
                    };
                    self.engine
                        .ops()
                        .resolve_conflict(op, conflict.id, d, all)?;
                }
                CoreEvent::OpFailed { op, path, error } if left.contains(&op) => {
                    if tty {
                        eprintln!();
                    }
                    eprintln!("error: {path}: {error}");
                    ok = false;
                }
                CoreEvent::OpFinished { op, report } if left.contains(&op) => {
                    if tty {
                        eprint!("\r\x1b[2K");
                    }
                    eprintln!(
                        "{:?} {:?}: {} done, {} failed, {} skipped, {} in {} ms",
                        report.kind,
                        report.state,
                        report.files_done,
                        report.files_failed,
                        report.files_skipped,
                        human(report.bytes_done),
                        report.elapsed_ms
                    );
                    if report.state != OpState::Completed {
                        ok = false;
                    }
                    left.remove(&op);
                }
                _ => {}
            }
        }
        Ok(ok)
    }
}

fn prompt_conflict(c: &ConflictInfo) -> Result<(ConflictDecision, bool)> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!(
            "“{}” already exists; pass --on-conflict to decide without asking",
            c.dest.display()
        );
    }
    loop {
        eprint!(
            "“{}” already exists ({} vs {}). [k]eep both, [r]eplace (old to Trash), [s]kip{}; capital = apply to all: ",
            c.dest.display(),
            human(c.source_size),
            human(c.dest_size),
            if c.can_merge { ", [m]erge" } else { "" }
        );
        std::io::stderr().flush()?;
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let ch = line.trim().chars().next().unwrap_or(' ');
        let all = ch.is_uppercase();
        let d = match ch.to_ascii_lowercase() {
            'k' => ConflictDecision::KeepBoth,
            'r' => ConflictDecision::Replace,
            's' => ConflictDecision::Skip,
            'm' if c.can_merge => ConflictDecision::Merge,
            _ => continue,
        };
        return Ok((d, all));
    }
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn rules(on: OnConflict) -> EnqueueOptions {
    let d = match on {
        OnConflict::Ask => return EnqueueOptions::default(),
        OnConflict::KeepBoth => ConflictDecision::KeepBoth,
        OnConflict::Replace => ConflictDecision::Replace,
        OnConflict::Skip => ConflictDecision::Skip,
        OnConflict::Merge => ConflictDecision::Merge,
    };
    EnqueueOptions {
        rules: ConflictRules::all(d),
        ..EnqueueOptions::default()
    }
}

fn print_location(p: &ParsedLocation) {
    match &p.location {
        Location::Dir(d) => println!("folder  {}", d.display()),
        Location::File { dir, name } => println!("file    {}  (select “{name}”)", dir.display()),
        Location::Missing {
            path,
            nearest_existing,
        } => println!(
            "missing {}  (nearest: {}; offer Create Folder)",
            path.display(),
            nearest_existing.display()
        ),
        Location::Remote(u) => println!("remote  {u}  (not supported yet)"),
    }
    if let Some(chip) = p.chip_text() {
        println!("        {chip}");
    }
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("fstudio: {e}");
            std::process::exit(2);
        }
    }
}

fn run(cli: Cli) -> Result<bool> {
    let app = App::open(&cli)?;
    let e = &app.engine;
    let ok = match cli.cmd {
        Cmd::Ls {
            path,
            sort,
            reverse,
            hidden,
            filter,
            json,
        } => {
            let path = path.unwrap_or_else(cwd);
            let tab = app.tab_at(&path)?;
            let mut v = tab.view();
            v.sort = match sort {
                SortArg::Name => SortBy::Name,
                SortArg::Size => SortBy::Size,
                SortArg::Modified => SortBy::Modified,
                SortArg::Created => SortBy::Created,
                SortArg::Kind => SortBy::Kind,
                SortArg::Extension => SortBy::Extension,
            };
            v.ascending = !reverse;
            v.show_hidden = hidden;
            if v != tab.view() || filter.is_some() {
                tab.set_view(v);
                // Re-sorts and filters on this thread, so the rows are ready to print.
                tab.set_filter_now(filter.as_deref().unwrap_or(""));
            }
            let l = tab.listing();
            if l.state() == ListingState::Error {
                bail!("{}: {}", path, l.error().unwrap_or_default());
            }
            let rows = tab.rows(0, l.len());
            let mut out = std::io::stdout().lock();
            for r in &rows {
                if json {
                    writeln!(out, "{}", serde_json::to_string(r)?)?;
                } else {
                    let name = match r.kind {
                        RowKind::Folder => format!("{}/", r.name),
                        RowKind::Symlink => format!("{}@", r.name),
                        _ => r.name.clone(),
                    };
                    writeln!(
                        out,
                        "{:>10}  {:<20}  {:<16}  {}",
                        r.size_text, r.modified_text, r.kind_text, name
                    )?;
                }
            }
            if !json {
                writeln!(out, "{} shown, {} in folder", l.len(), l.total_len())?;
            }
            true
        }
        Cmd::Watch { dir, seconds } => {
            let tab = app.tab_at(&dir)?;
            let id = tab.id();
            eprintln!(
                "watching {} ({} items); ^C to stop",
                tab.location(),
                tab.listing().total_len()
            );
            let mut known: BTreeSet<String> = tab.listing().names().into_iter().collect();
            let end = seconds.map(|s| Instant::now() + Duration::from_secs(s));
            loop {
                let ev = match end {
                    Some(t) => match app.rx.recv_deadline(t) {
                        Ok(e) => e,
                        Err(_) => break,
                    },
                    None => app.rx.recv()?,
                };
                if let CoreEvent::ListingChanged {
                    tab: t,
                    version,
                    kind,
                } = ev
                    && t == id
                {
                    let now: BTreeSet<String> = tab.listing().names().into_iter().collect();
                    for n in now.difference(&known) {
                        println!("+ {n}");
                    }
                    for n in known.difference(&now) {
                        println!("- {n}");
                    }
                    if now == known && kind == ChangeKind::Error {
                        println!(
                            "! v{version}: {}",
                            tab.listing().error().unwrap_or_default()
                        );
                    }
                    known = now;
                }
            }
            true
        }
        Cmd::Go { text } => {
            let p = e.parse_location(&text, &cwd());
            print_location(&p);
            if let Location::Dir(d) | Location::File { dir: d, .. } = &p.location {
                e.record_visit(&d.to_string_lossy());
            }
            !matches!(p.location, Location::Missing { .. })
        }
        Cmd::Parse { text } => {
            let p = e.parse_location(&text, &cwd());
            print_location(&p);
            if !p.cleanups.is_empty() {
                println!("        cleanups: {:?}", p.cleanups);
            }
            true
        }
        Cmd::Complete { text } => {
            let t0 = Instant::now();
            let c = e.complete(&text, text.len() as u32, &cwd());
            let took = t0.elapsed();
            for i in &c.items {
                println!(
                    "{:<8} {:<30} {}",
                    format!("{:?}", i.kind).to_lowercase(),
                    i.insert_text,
                    i.breadcrumb_text
                );
            }
            if let Some(x) = &c.common_prefix_extension {
                println!("tab → {}", x.new_input);
            }
            if !c.ghost_text.is_empty() {
                println!("ghost: {}", c.ghost_text);
            }
            if c.skipped_slow {
                println!("(a slow or cloud folder was skipped)");
            }
            eprintln!(
                "{} items in {:.2} ms",
                c.items.len(),
                took.as_secs_f64() * 1000.0
            );
            true
        }
        Cmd::Z { query, list } => {
            let q = query.join(" ");
            let hits = e.frecent(&q, if list { 50 } else { 1 });
            if hits.is_empty() {
                eprintln!(
                    "no match for “{q}” ({} folders remembered)",
                    e.frecency_len()
                );
                false
            } else {
                for h in hits {
                    if list {
                        println!("{:>8.1}  {}", h.score, h.path);
                    } else {
                        println!("{}", h.path);
                    }
                }
                true
            }
        }
        Cmd::Crumbs { path } => {
            let p = path.unwrap_or_else(cwd);
            for c in e.breadcrumbs(&p) {
                println!(
                    "{:<12} {:<24} {}{}",
                    format!("{:?}", c.kind),
                    c.name,
                    c.path.display(),
                    if c.is_current { "  ←" } else { "" }
                );
            }
            true
        }
        Cmd::Cp {
            mut paths,
            on_conflict,
        } => {
            let dest = abs(&paths.pop().expect("clap requires two"))?;
            let sources = paths.iter().map(|p| abs(p)).collect::<Result<Vec<_>>>()?;
            let id = e.enqueue_with(
                OpRequest::Copy {
                    sources,
                    dest_dir: dest,
                },
                rules(on_conflict),
            )?;
            app.follow(&[id], matches!(on_conflict, OnConflict::Ask))?
        }
        Cmd::Mv {
            mut paths,
            on_conflict,
        } => {
            let dest = abs(&paths.pop().expect("clap requires two"))?;
            let sources = paths.iter().map(|p| abs(p)).collect::<Result<Vec<_>>>()?;
            let id = e.enqueue_with(
                OpRequest::Move {
                    sources,
                    dest_dir: dest,
                },
                rules(on_conflict),
            )?;
            app.follow(&[id], matches!(on_conflict, OnConflict::Ask))?
        }
        Cmd::Trash { paths } => {
            let paths = paths.iter().map(|p| abs(p)).collect::<Result<Vec<_>>>()?;
            let id = e.enqueue(OpRequest::Trash { paths })?;
            app.follow(&[id], false)?
        }
        Cmd::Mkdir { path } => {
            let p = abs(&path)?;
            let parent = p.parent().ok_or_else(|| anyhow!("no parent folder"))?;
            let name = p
                .file_name()
                .ok_or_else(|| anyhow!("no name"))?
                .to_string_lossy()
                .into_owned();
            let id = e.enqueue(OpRequest::NewFolder {
                parent: parent.to_path_buf(),
                name,
            })?;
            app.follow(&[id], false)?
        }
        Cmd::Rename { path, new_name } => {
            let id = e.enqueue(OpRequest::Rename {
                path: abs(&path)?,
                new_name,
            })?;
            app.follow(&[id], false)?
        }
        Cmd::Undo | Cmd::Redo => {
            let undo = matches!(cli.cmd, Cmd::Undo);
            let before = e.undo_state();
            let out = if undo { e.undo()? } else { e.redo()? };
            if let Some(m) = &out.message {
                eprintln!("{m}");
            } else {
                eprintln!(
                    "{}",
                    if undo { before.undo } else { before.redo }.unwrap_or_default()
                );
            }
            let ok = out.ops.is_empty() || app.follow(&out.ops, false)?;
            ok && out.message.is_none()
        }
        Cmd::Ops { action } => match action.unwrap_or(OpsCmd::List) {
            OpsCmd::List => {
                let q = e.ops();
                let mut all = q.list();
                for s in q.interrupted()? {
                    if !all.iter().any(|a| a.id == s.id) {
                        all.push(s);
                    }
                }
                if all.is_empty() {
                    println!("no operations");
                }
                for s in all {
                    println!(
                        "{:>6}  {:<20} {:>5}/{:<5} {}",
                        s.id.0,
                        format!("{:?}", s.state),
                        s.files_done,
                        s.files_total,
                        s.title
                    );
                }
                let u = e.undo_state();
                if let Some(l) = u.undo {
                    println!("undo: {l}");
                }
                if let Some(l) = u.redo {
                    println!("redo: {l}");
                }
                true
            }
            OpsCmd::Cancel { id } => {
                e.ops().cancel(OpId(id))?;
                println!("cancelled op {id}");
                true
            }
            OpsCmd::Resume { id } => {
                let q = e.ops();
                let ids: Vec<OpId> = match id {
                    Some(i) => vec![OpId(i)],
                    None => q.interrupted()?.into_iter().map(|s| s.id).collect(),
                };
                if ids.is_empty() {
                    println!("nothing to resume");
                }
                for id in &ids {
                    q.resume(*id)?;
                }
                app.follow(&ids, true)?
            }
        },
        Cmd::Keys { action } => match action.unwrap_or(KeysCmd::List { prefix: None }) {
            KeysCmd::List { prefix } => {
                println!("profile: {:?}", e.settings().keymap.profile);
                for b in e.bindings() {
                    if prefix
                        .as_ref()
                        .is_some_and(|p| !b.command.starts_with(p.as_str()))
                    {
                        continue;
                    }
                    println!("{:<14} {:<34} {:<9} {}", b.keys, b.command, b.tier, b.when);
                }
                true
            }
            KeysCmd::Conflicts => {
                let c = e.keymap_conflicts();
                if c.is_empty() {
                    println!("no conflicts");
                }
                for x in &c {
                    println!("{x}");
                }
                c.is_empty()
            }
            KeysCmd::Export => {
                println!("{}", e.export_keymap());
                true
            }
            KeysCmd::Import { file } => {
                let text = std::fs::read_to_string(&file)?;
                for w in e.import_keymap(&text)? {
                    eprintln!("warning: {w}");
                }
                println!("imported {}", file.display());
                true
            }
            KeysCmd::Commands => {
                for c in e.commands() {
                    println!(
                        "{:<12} {:<34} {}",
                        c.shortcut.unwrap_or_default(),
                        c.id,
                        c.title
                    );
                }
                true
            }
        },
        Cmd::Settings { action } => match action {
            SettingsCmd::Get { key } => {
                match key {
                    Some(k) => println!("{}", e.setting(&k)?),
                    None => println!("{}", e.export_settings()),
                }
                true
            }
            SettingsCmd::Set { key, value } => {
                e.set_setting(&key, &value)?;
                println!("{key} = {}", e.setting(&key)?);
                true
            }
            SettingsCmd::Export => {
                println!("{}", e.export_settings());
                true
            }
            SettingsCmd::Import { file } => {
                e.import_settings(&std::fs::read_to_string(&file)?)?;
                println!("imported {}", file.display());
                true
            }
        },
        Cmd::ImportZoxide { path } => {
            let (imported, skipped) = e.import_zoxide(path)?;
            println!("imported {imported} folders, skipped {skipped}");
            true
        }
    };
    e.app_will_quit();
    Ok(ok)
}
