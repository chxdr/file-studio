//! The usage tree: one node per scanned item, in a flat arena indexed by
//! [`NodeId`]. Folders hold their totals (sizes, item counts, newest change)
//! so every question the view asks is answered without walking the disk.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Index of a node in a [`Tree`].
pub type NodeId = u32;

/// "No node" (the root's parent).
pub const NO_NODE: NodeId = u32::MAX;

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A regular file.
    File,
    /// A folder that can be expanded.
    Dir,
    /// A package (an app, a Photos library…): sized in full, shown as one item.
    Package,
    /// A symbolic link, sized as the link itself and never followed.
    Symlink,
    /// A socket, pipe or device.
    Other,
}

impl NodeKind {
    /// True for folders (not packages).
    pub fn is_dir(self) -> bool {
        self == NodeKind::Dir
    }
}

/// Bits in [`Node::flags`].
pub mod flags {
    /// An iCloud / File Provider placeholder file: counted at its logical
    /// size, never read (F-016).
    pub const DATALESS: u16 = 1;
    /// The folder could not be read (permission denied).
    pub const DENIED: u16 = 2;
    /// A folder on another volume, not entered.
    pub const OTHER_VOLUME: u16 = 4;
    /// A further hard link to a file already counted: its size counts once.
    pub const HARDLINK: u16 = 8;
    /// Reading this item failed for another reason.
    pub const ERROR: u16 = 16;
    /// A folder that is itself an iCloud placeholder: not listed, because
    /// listing it may download it (F-016).
    pub const CLOUD_DIR: u16 = 32;
    /// The scan was cancelled before this folder was finished.
    pub const INCOMPLETE: u16 = 64;
    /// Removed from the tree (trashed, or replaced by a rescan).
    pub const REMOVED: u16 = 128;
    /// Skipped on purpose (the Data volume's own mount when scanning `/`,
    /// whose contents appear through the firmlinks).
    pub const SKIPPED: u16 = 256;
}

/// Which size the view shows, sorts and lays out by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeMode {
    /// Bytes allocated on disk (what deleting would free).
    #[default]
    Allocated,
    /// Logical size (what Finder shows; iCloud placeholders count in full).
    Logical,
}

/// One scanned item. Folder totals include everything below them.
#[derive(Debug, Clone)]
pub struct Node {
    /// The name exactly as stored; the root holds the whole path.
    pub name: Box<[u8]>,
    /// The parent, or [`NO_NODE`] for the root.
    pub parent: NodeId,
    /// Children, largest first in the current [`SizeMode`].
    pub children: Vec<NodeId>,
    /// The kind.
    pub kind: NodeKind,
    /// [`flags`] bits.
    pub flags: u16,
    /// The file type: an index into [`Tree::ext_name`] (0 = no extension).
    pub ext: u16,
    /// Logical bytes (a folder's total).
    pub logical: u64,
    /// Allocated bytes (a folder's total).
    pub allocated: u64,
    /// Logical bytes of iCloud placeholders at or below this node.
    pub cloud: u64,
    /// Items below this folder (0 for files and packages).
    pub items: u64,
    /// Unreadable folders at or below this node.
    pub denied: u32,
    /// The newest modification time at or below this node (ns).
    pub mtime_ns: i64,
}

impl Node {
    /// The size in `mode`.
    pub fn size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Allocated => self.allocated,
            SizeMode::Logical => self.logical,
        }
    }
    /// How many items this node adds to its parent's count.
    pub fn weight(&self) -> u64 {
        1 + if self.kind.is_dir() { self.items } else { 0 }
    }
    /// The name as an `OsStr`.
    pub fn name_os(&self) -> &OsStr {
        OsStr::from_bytes(&self.name)
    }
    /// The name for display (lossless on APFS, where names are UTF-8).
    pub fn name_lossy(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }
    /// True when the node has this flag.
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

/// The scanned tree.
#[derive(Debug, Clone)]
pub struct Tree {
    pub(crate) nodes: Vec<Node>,
    pub(crate) root: NodeId,
    exts: Vec<Box<str>>,
    ext_index: HashMap<Box<str>, u16>,
    mode: SizeMode,
}

/// The type key of a name: its extension, lower-cased, or "" when it has
/// none (dot-files, no dot, or an implausibly long tail).
pub fn ext_of(name: &[u8]) -> String {
    let Some(dot) = name.iter().rposition(|b| *b == b'.') else {
        return String::new();
    };
    if dot == 0 || dot + 1 >= name.len() || name.len() - dot - 1 > 16 {
        return String::new();
    }
    let ext = String::from_utf8_lossy(&name[dot + 1..]);
    if ext.contains(' ') {
        return String::new();
    }
    ext.to_lowercase()
}

impl Tree {
    /// An empty tree whose root is the folder at `path`.
    pub fn new(path: &Path) -> Tree {
        let mut t = Tree {
            nodes: Vec::new(),
            root: 0,
            exts: vec!["".into()],
            ext_index: HashMap::from([("".into(), 0)]),
            mode: SizeMode::default(),
        };
        t.nodes.push(Node {
            name: path.as_os_str().as_bytes().into(),
            parent: NO_NODE,
            children: Vec::new(),
            kind: NodeKind::Dir,
            flags: 0,
            ext: 0,
            logical: 0,
            allocated: 0,
            cloud: 0,
            items: 0,
            denied: 0,
            mtime_ns: 0,
        });
        t
    }

    /// The root node's id.
    pub fn root(&self) -> NodeId {
        self.root
    }
    /// A node. Panics on an id that was never handed out.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }
    /// A node, if the id is valid and not removed.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes
            .get(id as usize)
            .filter(|n| !n.has(flags::REMOVED))
    }
    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }
    /// How many nodes the arena holds (removed ones included).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    /// True when only the root exists.
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }
    /// The children of `id`, largest first.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id as usize].children
    }
    /// The size mode children are sorted by.
    pub fn mode(&self) -> SizeMode {
        self.mode
    }
    /// The type name for an extension index ("jpg", or "" for none).
    pub fn ext_name(&self, ext: u16) -> &str {
        self.exts.get(ext as usize).map_or("", |s| s)
    }
    /// How many distinct types the tree knows.
    pub fn ext_count(&self) -> usize {
        self.exts.len()
    }

    /// The index for a type key, adding it when new.
    pub(crate) fn intern_ext(&mut self, ext: &str) -> u16 {
        if let Some(i) = self.ext_index.get(ext) {
            return *i;
        }
        if self.exts.len() >= u16::MAX as usize {
            return 0;
        }
        let i = self.exts.len() as u16;
        self.exts.push(ext.into());
        self.ext_index.insert(ext.into(), i);
        i
    }

    /// Adds a node under `parent` (totals are the caller's job).
    pub(crate) fn push(&mut self, mut node: Node, parent: NodeId) -> NodeId {
        let id = self.nodes.len() as NodeId;
        node.parent = parent;
        self.nodes.push(node);
        if parent != NO_NODE {
            self.nodes[parent as usize].children.push(id);
        }
        id
    }

    /// The full path of a node.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = id;
        while cur != NO_NODE {
            let n = &self.nodes[cur as usize];
            parts.push(n.name_os());
            cur = n.parent;
        }
        let mut p = PathBuf::new();
        for part in parts.into_iter().rev() {
            p.push(part);
        }
        p
    }

    /// The chain from the root down to `id` (both included).
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = Vec::new();
        let mut cur = id;
        while cur != NO_NODE {
            v.push(cur);
            cur = self.nodes[cur as usize].parent;
        }
        v.reverse();
        v
    }

    /// The depth of a node (the root is 0).
    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).len() - 1
    }

    /// True when `id` is `anc` or below it.
    pub fn is_within(&self, id: NodeId, anc: NodeId) -> bool {
        let mut cur = id;
        while cur != NO_NODE {
            if cur == anc {
                return true;
            }
            cur = self.nodes[cur as usize].parent;
        }
        false
    }

    /// Finds the node for a path at or below the root, by exact name. Returns
    /// the deepest node found and whether it is the whole path.
    pub fn find_nearest(&self, path: &Path) -> Option<(NodeId, bool)> {
        let root_path = Path::new(OsStr::from_bytes(&self.nodes[self.root as usize].name));
        let rest = path.strip_prefix(root_path).ok()?;
        let mut cur = self.root;
        for comp in rest.components() {
            let name = comp.as_os_str().as_bytes();
            let next = self.nodes[cur as usize]
                .children
                .iter()
                .copied()
                .find(|c| &*self.nodes[*c as usize].name == name);
            match next {
                Some(n) => cur = n,
                None => return Some((cur, false)),
            }
        }
        Some((cur, true))
    }

    /// The node for an exact path, if scanned.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        match self.find_nearest(path) {
            Some((id, true)) => Some(id),
            _ => None,
        }
    }

    /// Sorts every folder's children, largest first in `mode` (ties by name).
    pub fn sort(&mut self, mode: SizeMode) {
        self.mode = mode;
        for i in 0..self.nodes.len() {
            if self.nodes[i].children.len() < 2 {
                continue;
            }
            let mut kids = std::mem::take(&mut self.nodes[i].children);
            self.sort_ids(&mut kids);
            self.nodes[i].children = kids;
        }
    }

    fn sort_ids(&self, kids: &mut [NodeId]) {
        let mode = self.mode;
        kids.sort_by(|a, b| {
            let (na, nb) = (&self.nodes[*a as usize], &self.nodes[*b as usize]);
            nb.size(mode)
                .cmp(&na.size(mode))
                .then_with(|| na.name.cmp(&nb.name))
        });
    }

    /// Re-sorts one folder's children (after a change below it).
    pub(crate) fn resort(&mut self, id: NodeId) {
        let mut kids = std::mem::take(&mut self.nodes[id as usize].children);
        self.sort_ids(&mut kids);
        self.nodes[id as usize].children = kids;
    }

    /// Recomputes a folder's totals from its children.
    pub(crate) fn recompute(&mut self, id: NodeId) {
        let kids = std::mem::take(&mut self.nodes[id as usize].children);
        let (mut l, mut a, mut c, mut it, mut d) = (0u64, 0u64, 0u64, 0u64, 0u32);
        let mut m = 0i64;
        for k in &kids {
            let n = &self.nodes[*k as usize];
            l += n.logical;
            a += n.allocated;
            c += n.cloud;
            it += n.weight();
            d += n.denied;
            m = m.max(n.mtime_ns);
        }
        let own_denied = u32::from(self.nodes[id as usize].has(flags::DENIED));
        let node = &mut self.nodes[id as usize];
        node.children = kids;
        node.logical = l;
        node.allocated = a;
        node.cloud = c;
        node.items = it;
        node.denied = d + own_denied;
        // The folder's own time (set when scanned) or its newest child.
        node.mtime_ns = node.mtime_ns.max(m);
    }

    /// Recomputes and re-sorts `id` and every folder above it.
    pub(crate) fn bubble_up(&mut self, id: NodeId) {
        let mut cur = id;
        while cur != NO_NODE {
            self.recompute(cur);
            self.resort(cur);
            cur = self.nodes[cur as usize].parent;
        }
    }

    /// Takes a node (and so everything below it) out of the tree and fixes
    /// the totals above it. The root can't be removed.
    pub fn remove(&mut self, id: NodeId) -> bool {
        if id == self.root || self.get(id).is_none() {
            return false;
        }
        let parent = self.nodes[id as usize].parent;
        self.mark_removed(id);
        if parent != NO_NODE {
            self.nodes[parent as usize].children.retain(|c| *c != id);
            self.bubble_up(parent);
        }
        true
    }

    fn mark_removed(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = &mut self.nodes[n as usize];
            node.flags |= flags::REMOVED;
            stack.extend(node.children.iter().copied());
        }
    }

    /// Replaces the subtree at `id` with `other`'s whole tree (a rescan of
    /// that folder), keeping `id`'s name and place, and fixes totals above.
    pub fn graft(&mut self, id: NodeId, other: Tree) {
        let old_kids = std::mem::take(&mut self.nodes[id as usize].children);
        for k in old_kids {
            self.mark_removed(k);
        }
        // Copy other's nodes below its root.
        let mut map: HashMap<NodeId, NodeId> = HashMap::new();
        map.insert(other.root, id);
        let mut order = vec![other.root];
        let mut i = 0;
        while i < order.len() {
            let src = order[i];
            i += 1;
            for c in other.children(src) {
                let n = other.node(*c);
                let ext_name = other.ext_name(n.ext).to_string();
                let mut copy = n.clone();
                copy.children = Vec::new();
                copy.ext = self.intern_ext(&ext_name);
                let new_parent = map[&src];
                let nid = self.push(copy, new_parent);
                map.insert(*c, nid);
                order.push(*c);
            }
        }
        let src_root = other.node(other.root);
        let dst = &mut self.nodes[id as usize];
        dst.flags = (dst.flags & !(flags::DENIED | flags::INCOMPLETE | flags::ERROR))
            | (src_root.flags & (flags::DENIED | flags::INCOMPLETE | flags::ERROR));
        dst.mtime_ns = src_root.mtime_ns;
        // Sort the new nodes, then fix totals from `id` up.
        for nid in map.values().copied().collect::<Vec<_>>() {
            if self.nodes[nid as usize].children.len() > 1 {
                self.resort(nid);
            }
        }
        self.bubble_up(id);
    }
}
