//! The SQLite journal (tables `ops_*`). Every step is written here before it
//! touches the disk, so an op can resume after a crash and be undone later.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fstudio_db::rusqlite::{self, OptionalExtension, Row, params};
use fstudio_db::{Db, Migrations};
use fstudio_model::EntryKind;

use crate::meta::{Ident, kind_str, parse_kind};
use crate::types::{EnqueueOptions, OpId, OpPlan, OpRequest, OpState, Result};

/// The ops module's schema. Append-only.
pub const MIGRATIONS: Migrations = Migrations {
    module: "ops",
    steps: &[
        (
            1,
            "CREATE TABLE ops_ops (
            id          INTEGER PRIMARY KEY,
            seq         REAL NOT NULL,
            kind        TEXT NOT NULL,
            request     TEXT NOT NULL,
            options     TEXT NOT NULL,
            plan        TEXT,
            state       TEXT NOT NULL,
            origin      TEXT NOT NULL,
            undo_of     INTEGER,
            title       TEXT NOT NULL,
            created_ns  INTEGER NOT NULL,
            finished_ns INTEGER
        );
        CREATE TABLE ops_items (
            id             INTEGER PRIMARY KEY,
            op_id          INTEGER NOT NULL REFERENCES ops_ops(id),
            parent         INTEGER REFERENCES ops_items(id),
            kind           TEXT NOT NULL,
            source         BLOB NOT NULL,
            dest_dir       BLOB,
            dest_name      TEXT,
            dest           BLOB,
            partial        BLOB,
            state          TEXT NOT NULL,
            created        INTEGER NOT NULL DEFAULT 0,
            merged         INTEGER NOT NULL DEFAULT 0,
            expanded       INTEGER NOT NULL DEFAULT 0,
            bytes          INTEGER NOT NULL DEFAULT 0,
            hash           TEXT,
            error          TEXT,
            src_ident      TEXT,
            dest_ident     TEXT,
            partial_ino    INTEGER,
            replaced_trash BLOB,
            trash_loc      BLOB
        );
        CREATE INDEX ops_items_by_op ON ops_items(op_id, id);
        CREATE INDEX ops_ops_by_state ON ops_ops(state);
        CREATE TABLE ops_undo (
            seq   INTEGER PRIMARY KEY AUTOINCREMENT,
            stack TEXT NOT NULL,
            op_id INTEGER NOT NULL REFERENCES ops_ops(id),
            label TEXT NOT NULL
        );",
        ),
        (
            2,
            // Compound ops (safety audit S12–S14): each step is an op of its own,
            // marked as part `part_step` of the op `part_of`.
            "ALTER TABLE ops_ops ADD COLUMN part_of INTEGER;
        ALTER TABLE ops_ops ADD COLUMN part_step INTEGER;",
        ),
        // The device of the folder a temporary file was written in, so a
        // leftover is only taken as gone once that same volume is back.
        (3, "ALTER TABLE ops_items ADD COLUMN partial_dev INTEGER;"),
    ],
};

/// The state of one item in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemState {
    /// Not started.
    Pending,
    /// Writing to the temporary name (or renaming / trashing).
    InProgress,
    /// The temporary file is complete (its inode is recorded).
    Copied,
    /// The temporary file matched the source hash.
    Verified,
    /// In place at the destination. For a move, the source is not removed yet;
    /// for a folder, its contents are not done yet.
    Placed,
    /// Finished.
    Done,
    Failed,
    Skipped,
    /// Waiting for a conflict decision (treated as pending on resume).
    Conflict,
}

impl ItemState {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemState::Pending => "pending",
            ItemState::InProgress => "in_progress",
            ItemState::Copied => "copied",
            ItemState::Verified => "verified",
            ItemState::Placed => "placed",
            ItemState::Done => "done",
            ItemState::Failed => "failed",
            ItemState::Skipped => "skipped",
            ItemState::Conflict => "conflict",
        }
    }
    pub fn parse(s: &str) -> ItemState {
        match s {
            "in_progress" => ItemState::InProgress,
            "copied" => ItemState::Copied,
            "verified" => ItemState::Verified,
            "placed" => ItemState::Placed,
            "done" => ItemState::Done,
            "failed" => ItemState::Failed,
            "skipped" => ItemState::Skipped,
            "conflict" => ItemState::Conflict,
            _ => ItemState::Pending,
        }
    }
    /// Nothing more will happen to this item.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ItemState::Done | ItemState::Failed | ItemState::Skipped
        )
    }
}

/// One journaled item: a file, folder or symlink of an op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    pub id: i64,
    pub op: OpId,
    /// The folder item this one is inside (`None` for a top-level item).
    pub parent: Option<i64>,
    pub kind: EntryKind,
    pub source: PathBuf,
    /// For top-level items: the folder it goes into.
    pub dest_dir: Option<PathBuf>,
    /// The name it gets at the destination, when not the source's name
    /// (Keep Both, Duplicate, Rename, MoveTo).
    pub dest_name: Option<String>,
    /// The resolved destination, once the item started.
    pub dest: Option<PathBuf>,
    /// Our temporary name for it, while it exists.
    pub partial: Option<PathBuf>,
    pub state: ItemState,
    /// We created the destination (vs. merged into an existing folder).
    pub created: bool,
    /// The destination was an existing folder we merged into.
    pub merged: bool,
    /// Folder: its children are in the journal.
    pub expanded: bool,
    pub bytes: u64,
    pub hash: Option<String>,
    pub error: Option<String>,
    /// The source when the item started.
    pub src_ident: Option<Ident>,
    /// The destination once placed.
    pub dest_ident: Option<Ident>,
    pub partial_ino: Option<u64>,
    /// The volume the folder `partial` was written in: a key from the
    /// volume's UUID (stable when a card is re-attached), or its `st_dev` for
    /// volumes without one. See `recover::folder_dev`.
    pub partial_dev: Option<u64>,
    /// Where "Replace" sent the item that was in the way.
    pub replaced_trash: Option<PathBuf>,
    /// For Trash ops: where the item went.
    pub trash_loc: Option<PathBuf>,
}

/// A new item to insert.
#[derive(Debug, Clone)]
pub struct NewItem {
    pub parent: Option<i64>,
    pub kind: EntryKind,
    pub source: PathBuf,
    pub dest_dir: Option<PathBuf>,
    pub dest_name: Option<String>,
    pub src_ident: Option<Ident>,
}

/// One journaled op.
#[derive(Debug, Clone)]
pub struct OpRow {
    pub id: OpId,
    pub request: OpRequest,
    pub options: EnqueueOptions,
    pub plan: Option<OpPlan>,
    pub state: OpState,
    pub origin: Origin,
    /// The op this one undoes, for undo ops.
    #[allow(dead_code)]
    pub undo_of: Option<OpId>,
    pub title: String,
    /// For a step of a compound op: the compound op and the step's index.
    pub part_of: Option<(OpId, usize)>,
}

/// Why an op exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    User,
    Undo,
    Redo,
    /// A step of a compound op (never on the undo stack by itself).
    Part,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Undo => "undo",
            Origin::Redo => "redo",
            Origin::Part => "part",
        }
    }
    fn parse(s: &str) -> Origin {
        match s {
            "undo" => Origin::Undo,
            "redo" => Origin::Redo,
            "part" => Origin::Part,
            _ => Origin::User,
        }
    }
}

fn p2b(p: &Path) -> &[u8] {
    p.as_os_str().as_bytes()
}

fn b2p(b: Vec<u8>) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&b))
}

fn opt_p2b(p: &Option<PathBuf>) -> Option<&[u8]> {
    p.as_deref().map(p2b)
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn ident_json(i: &Option<Ident>) -> Option<String> {
    i.as_ref().and_then(|i| serde_json::to_string(i).ok())
}

fn parse_ident(s: Option<String>) -> Option<Ident> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}

const ITEM_COLS: &str =
    "id, op_id, parent, kind, source, dest_dir, dest_name, dest, partial, state,
    created, merged, expanded, bytes, hash, error, src_ident, dest_ident, partial_ino,
    replaced_trash, trash_loc, partial_dev";

fn item_from_row(r: &Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok(ItemRow {
        id: r.get(0)?,
        op: OpId(r.get(1)?),
        parent: r.get(2)?,
        kind: parse_kind(&r.get::<_, String>(3)?),
        source: b2p(r.get(4)?),
        dest_dir: r.get::<_, Option<Vec<u8>>>(5)?.map(b2p),
        dest_name: r.get(6)?,
        dest: r.get::<_, Option<Vec<u8>>>(7)?.map(b2p),
        partial: r.get::<_, Option<Vec<u8>>>(8)?.map(b2p),
        state: ItemState::parse(&r.get::<_, String>(9)?),
        created: r.get::<_, i64>(10)? != 0,
        merged: r.get::<_, i64>(11)? != 0,
        expanded: r.get::<_, i64>(12)? != 0,
        bytes: r.get::<_, i64>(13)? as u64,
        hash: r.get(14)?,
        error: r.get(15)?,
        src_ident: parse_ident(r.get(16)?),
        dest_ident: parse_ident(r.get(17)?),
        partial_ino: r.get::<_, Option<i64>>(18)?.map(|v| v as u64),
        replaced_trash: r.get::<_, Option<Vec<u8>>>(19)?.map(b2p),
        trash_loc: r.get::<_, Option<Vec<u8>>>(20)?.map(b2p),
        partial_dev: r.get::<_, Option<i64>>(21)?.map(|v| v as u64),
    })
}

/// An `ops_ops` row before JSON decoding.
type RawOp = (
    i64,
    f64,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<i64>,
    String,
    Option<i64>,
    Option<i64>,
);

/// The journal: typed access to the `ops_*` tables.
#[derive(Clone)]
pub struct Journal {
    db: Db,
}

impl Journal {
    /// Wraps `db`, running the ops migrations.
    pub fn new(db: Db) -> Result<Self> {
        db.migrate(&MIGRATIONS)?;
        Ok(Self { db })
    }

    /// Records a new op and its top-level items, in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_op(
        &self,
        req: &OpRequest,
        opts: &EnqueueOptions,
        origin: Origin,
        undo_of: Option<OpId>,
        title: &str,
        roots: &[NewItem],
        part_of: Option<(OpId, usize)>,
    ) -> Result<OpId> {
        let request = serde_json::to_string(req)?;
        let options = serde_json::to_string(opts)?;
        let kind = format!("{:?}", req.kind());
        let id = self.db.with(|c| {
            let tx = c.transaction()?;
            let seq: f64 = tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM ops_ops", [], |r| {
                r.get(0)
            })?;
            tx.execute(
                "INSERT INTO ops_ops(seq, kind, request, options, state, origin, undo_of, title, created_ns,
                    part_of, part_step)
                 VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    seq,
                    kind,
                    request,
                    options,
                    origin.as_str(),
                    undo_of.map(|o| o.0),
                    title,
                    now_ns(),
                    part_of.map(|p| p.0.0),
                    part_of.map(|p| p.1 as i64),
                ],
            )?;
            let id = tx.last_insert_rowid();
            for it in roots {
                insert_item(&tx, id, it)?;
            }
            tx.commit()?;
            Ok(id)
        })?;
        Ok(OpId(id))
    }

    /// Sets an op's state (and its finish time when it is finished).
    pub fn set_op_state(&self, id: OpId, state: OpState) -> Result<()> {
        let fin = state.is_finished().then(now_ns);
        self.db.with(|c| {
            c.prepare_cached("UPDATE ops_ops SET state = ?2, finished_ns = COALESCE(?3, finished_ns) WHERE id = ?1")?
                .execute(params![id.0, state.as_str(), fin])
        })?;
        Ok(())
    }

    /// Stores the plan computed when the op started.
    pub fn set_op_plan(&self, id: OpId, plan: &OpPlan) -> Result<()> {
        let s = serde_json::to_string(plan)?;
        self.db.with(|c| {
            c.execute(
                "UPDATE ops_ops SET plan = ?2 WHERE id = ?1",
                params![id.0, s],
            )
        })?;
        Ok(())
    }

    /// Stores changed options ("apply to all" rules).
    pub fn set_op_options(&self, id: OpId, opts: &EnqueueOptions) -> Result<()> {
        let s = serde_json::to_string(opts)?;
        self.db.with(|c| {
            c.execute(
                "UPDATE ops_ops SET options = ?2 WHERE id = ?1",
                params![id.0, s],
            )
        })?;
        Ok(())
    }

    /// Changes the queue position of an op.
    pub fn set_op_seq(&self, id: OpId, seq: f64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "UPDATE ops_ops SET seq = ?2 WHERE id = ?1",
                params![id.0, seq],
            )
        })?;
        Ok(())
    }

    fn op_from_row(r: &Row<'_>) -> rusqlite::Result<RawOp> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
        ))
    }

    fn build_op(raw: RawOp) -> Result<OpRow> {
        let (id, _seq, request, options, plan, state, origin, undo_of, title, part_of, part_step) =
            raw;
        Ok(OpRow {
            id: OpId(id),
            request: serde_json::from_str(&request)?,
            options: serde_json::from_str(&options)?,
            plan: plan.map(|p| serde_json::from_str(&p)).transpose()?,
            state: OpState::parse(&state),
            origin: Origin::parse(&origin),
            undo_of: undo_of.map(OpId),
            title,
            part_of: part_of.map(|p| (OpId(p), part_step.unwrap_or(0) as usize)),
        })
    }

    const OP_COLS: &str =
        "id, seq, request, options, plan, state, origin, undo_of, title, part_of, part_step";

    /// Loads one op.
    pub fn load_op(&self, id: OpId) -> Result<Option<OpRow>> {
        let raw = self.db.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM ops_ops WHERE id = ?1", Self::OP_COLS),
                [id.0],
                Self::op_from_row,
            )
            .optional()
        })?;
        raw.map(Self::build_op).transpose()
    }

    /// Ops that were queued, running or paused (or already marked interrupted),
    /// in queue order.
    pub fn unfinished_ops(&self) -> Result<Vec<OpRow>> {
        let raws = self.db.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM ops_ops
                 WHERE state IN ('queued', 'running', 'paused', 'waiting', 'interrupted')
                 ORDER BY seq",
                Self::OP_COLS
            ))?;
            st.query_map([], Self::op_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        raws.into_iter().map(Self::build_op).collect()
    }

    /// The steps of the compound op `op`, in the order they were queued.
    pub fn parts_of(&self, op: OpId) -> Result<Vec<OpRow>> {
        let raws = self.db.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM ops_ops WHERE part_of = ?1 ORDER BY id",
                Self::OP_COLS
            ))?;
            st.query_map([op.0], Self::op_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        raws.into_iter().map(Self::build_op).collect()
    }

    /// The ops queued to undo `op` (origin "undo"), oldest first.
    pub fn undo_ops_of(&self, op: OpId) -> Result<Vec<OpRow>> {
        let raws = self.db.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM ops_ops WHERE undo_of = ?1 AND origin = 'undo' ORDER BY id",
                Self::OP_COLS
            ))?;
            st.query_map([op.0], Self::op_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        raws.into_iter().map(Self::build_op).collect()
    }

    /// Items of finished ops (completed, with errors, cancelled or failed)
    /// that still record a temporary file: the removal after a failure
    /// couldn't reach it (a card pulled out mid-copy). Ops that are running,
    /// queued or interrupted are left to the queue and to resume/cancel.
    pub fn leftover_partials(&self) -> Result<Vec<ItemRow>> {
        Ok(self.db.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM ops_items i JOIN ops_ops o ON o.id = i.op_id
                 WHERE i.partial IS NOT NULL
                   AND o.state IN ('completed', 'completed_with_errors', 'cancelled', 'failed')
                 ORDER BY i.id",
                ITEM_COLS
                    .split(',')
                    .map(|c| format!("i.{}", c.trim()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))?;
            st.query_map([], item_from_row)?.collect()
        })?)
    }

    /// All items of an op, in insertion order (parents before children).
    pub fn load_items(&self, op: OpId) -> Result<Vec<ItemRow>> {
        Ok(self.db.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {ITEM_COLS} FROM ops_items WHERE op_id = ?1 ORDER BY id"
            ))?;
            st.query_map([op.0], item_from_row)?.collect()
        })?)
    }

    /// Writes every mutable field of an item.
    pub fn save_item(&self, it: &ItemRow) -> Result<()> {
        self.db.with(|c| {
            c.prepare_cached(
                "UPDATE ops_items SET dest_name = ?2, dest = ?3, partial = ?4, state = ?5,
                    created = ?6, merged = ?7, expanded = ?8, bytes = ?9, hash = ?10, error = ?11,
                    src_ident = ?12, dest_ident = ?13, partial_ino = ?14, replaced_trash = ?15,
                    trash_loc = ?16, kind = ?17, partial_dev = ?18
                 WHERE id = ?1",
            )?
            .execute(params![
                it.id,
                it.dest_name,
                opt_p2b(&it.dest),
                opt_p2b(&it.partial),
                it.state.as_str(),
                it.created as i64,
                it.merged as i64,
                it.expanded as i64,
                it.bytes as i64,
                it.hash,
                it.error,
                ident_json(&it.src_ident),
                ident_json(&it.dest_ident),
                it.partial_ino.map(|v| v as i64),
                opt_p2b(&it.replaced_trash),
                opt_p2b(&it.trash_loc),
                kind_str(it.kind),
                it.partial_dev.map(|v| v as i64),
            ])
        })?;
        Ok(())
    }

    /// Inserts a folder's children and marks it expanded, in one transaction,
    /// so a crash never leaves a half-listed folder.
    pub fn expand(&self, parent: &ItemRow, children: &[NewItem]) -> Result<Vec<ItemRow>> {
        let op = parent.op;
        let ids = self.db.with(|c| {
            let tx = c.transaction()?;
            let mut ids = Vec::with_capacity(children.len());
            for ch in children {
                ids.push(insert_item(&tx, op.0, ch)?);
            }
            tx.execute(
                "UPDATE ops_items SET expanded = 1 WHERE id = ?1",
                [parent.id],
            )?;
            tx.commit()?;
            Ok(ids)
        })?;
        Ok(children
            .iter()
            .zip(ids)
            .map(|(ch, id)| ItemRow {
                id,
                op,
                parent: ch.parent,
                kind: ch.kind,
                source: ch.source.clone(),
                dest_dir: ch.dest_dir.clone(),
                dest_name: ch.dest_name.clone(),
                dest: None,
                partial: None,
                state: ItemState::Pending,
                created: false,
                merged: false,
                expanded: false,
                bytes: 0,
                hash: None,
                error: None,
                src_ident: ch.src_ident,
                dest_ident: None,
                partial_ino: None,
                partial_dev: None,
                replaced_trash: None,
                trash_loc: None,
            })
            .collect())
    }

    /// Pushes an op onto the undo or redo stack, keeping at most `limit` entries.
    pub fn push_stack(&self, stack: Stack, op: OpId, label: &str, limit: usize) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO ops_undo(stack, op_id, label) VALUES (?1, ?2, ?3)",
                params![stack.as_str(), op.0, label],
            )?;
            c.execute(
                "DELETE FROM ops_undo WHERE stack = ?1 AND seq NOT IN
                   (SELECT seq FROM ops_undo WHERE stack = ?1 ORDER BY seq DESC LIMIT ?2)",
                params![stack.as_str(), limit as i64],
            )
        })?;
        Ok(())
    }

    /// The top of a stack: `(op, label)`.
    pub fn peek_stack(&self, stack: Stack) -> Result<Option<(OpId, String)>> {
        Ok(self.db.with(|c| {
            c.query_row(
                "SELECT op_id, label FROM ops_undo WHERE stack = ?1 ORDER BY seq DESC LIMIT 1",
                [stack.as_str()],
                |r| Ok((OpId(r.get(0)?), r.get(1)?)),
            )
            .optional()
        })?)
    }

    /// Removes the top of a stack.
    pub fn pop_stack(&self, stack: Stack) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "DELETE FROM ops_undo WHERE seq =
                   (SELECT MAX(seq) FROM ops_undo WHERE stack = ?1)",
                [stack.as_str()],
            )
        })?;
        Ok(())
    }

    /// Empties a stack.
    pub fn clear_stack(&self, stack: Stack) -> Result<()> {
        self.db
            .with(|c| c.execute("DELETE FROM ops_undo WHERE stack = ?1", [stack.as_str()]))?;
        Ok(())
    }
}

/// The undo and redo stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stack {
    Undo,
    Redo,
}

impl Stack {
    fn as_str(self) -> &'static str {
        match self {
            Stack::Undo => "undo",
            Stack::Redo => "redo",
        }
    }
}

fn insert_item(tx: &rusqlite::Transaction<'_>, op: i64, it: &NewItem) -> rusqlite::Result<i64> {
    tx.prepare_cached(
        "INSERT INTO ops_items(op_id, parent, kind, source, dest_dir, dest_name, state, src_ident)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
    )?
    .execute(params![
        op,
        it.parent,
        kind_str(it.kind),
        p2b(&it.source),
        opt_p2b(&it.dest_dir),
        it.dest_name,
        ident_json(&it.src_ident),
    ])?;
    Ok(tx.last_insert_rowid())
}
