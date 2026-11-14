//! The command registry: every action the app can perform, with its default
//! bindings in each profile. This table is the single source for menus, the
//! palette, the overlay, hints and the Settings shortcut editor (F-006).
//!
//! Key strings use the key-cap text of [`crate::display`]; `|` separates
//! alternative bindings (primary first) and spaces separate the steps of a
//! sequence. A bad string panics when the registry is first built, and the
//! tests build it.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::binding::Binding;
use crate::command::{ArgKind, Category, CommandArg, CommandId, CommandInfo};
use crate::when::When;

struct Registry {
    list: Vec<CommandInfo>,
    index: HashMap<String, usize>,
}

fn registry_cell() -> &'static Registry {
    static CELL: OnceLock<Registry> = OnceLock::new();
    CELL.get_or_init(|| {
        let list = build();
        let index = list
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.as_str().to_string(), i))
            .collect();
        Registry { list, index }
    })
}

/// Every registered command, in display order.
pub fn all() -> &'static [CommandInfo] {
    &registry_cell().list
}

/// Looks a command up by id.
pub fn get(id: &str) -> Option<&'static CommandInfo> {
    let r = registry_cell();
    r.index.get(id).map(|i| &r.list[*i])
}

struct B(CommandInfo);

fn c(id: &str, title: &'static str, category: Category, description: &'static str) -> B {
    B(CommandInfo {
        id: CommandId::new(id),
        title,
        category,
        description,
        standard: vec![],
        vim: vec![],
        contexts: vec![When::ListFocused],
        arg: None,
        shows_in_menu: true,
        shows_in_palette: true,
        hint_eligible: true,
        repeatable: false,
    })
}

fn bindings(keys: &str, when: &When, arg: Option<CommandArg>) -> Vec<Binding> {
    keys.split('|')
        .map(|k| {
            let mut b = Binding::parse(k.trim(), when.clone())
                .unwrap_or_else(|| panic!("bad key string '{k}'"));
            b.arg = arg.clone();
            b
        })
        .collect()
}

impl B {
    fn first_ctx(&self) -> When {
        self.0.contexts[0].clone()
    }
    /// Applies everywhere (even in text fields).
    fn global(mut self) -> Self {
        self.0.contexts = vec![When::Always];
        self
    }
    fn ctx(mut self, w: When) -> Self {
        self.0.contexts = vec![w];
        self
    }
    fn also(mut self, w: When) -> Self {
        self.0.contexts.push(w);
        self
    }
    /// Standard bindings in the command's first context.
    fn s(mut self, keys: &str) -> Self {
        let w = self.first_ctx();
        self.0.standard.extend(bindings(keys, &w, None));
        self
    }
    /// Standard bindings in an explicit context.
    fn sw(mut self, when: When, keys: &str) -> Self {
        self.0.standard.extend(bindings(keys, &when, None));
        self
    }
    fn sa(mut self, when: When, keys: &str, arg: CommandArg) -> Self {
        self.0.standard.extend(bindings(keys, &when, Some(arg)));
        self
    }
    /// Vim normal-mode bindings.
    fn v(mut self, keys: &str) -> Self {
        self.0.vim.extend(bindings(keys, &When::vim_normal(), None));
        self
    }
    fn va(mut self, keys: &str, arg: CommandArg) -> Self {
        self.0
            .vim
            .extend(bindings(keys, &When::vim_normal(), Some(arg)));
        self
    }
    /// Vim visual-mode bindings.
    fn vv(mut self, keys: &str) -> Self {
        self.0.vim.extend(bindings(keys, &When::vim_visual(), None));
        self
    }
    fn vva(mut self, keys: &str, arg: CommandArg) -> Self {
        self.0
            .vim
            .extend(bindings(keys, &When::vim_visual(), Some(arg)));
        self
    }
    fn arg(mut self, k: ArgKind) -> Self {
        self.0.arg = Some(k);
        self
    }
    /// Repeats while its key is held (movement only; see `CommandInfo::repeatable`).
    fn repeats(mut self) -> Self {
        self.0.repeatable = true;
        self
    }
    fn nomenu(mut self) -> Self {
        self.0.shows_in_menu = false;
        self
    }
    fn nohint(mut self) -> Self {
        self.0.hint_eligible = false;
        self
    }
    /// Not for menus, the palette or hints: scripting and text-field keys.
    fn hidden(mut self) -> Self {
        self.0.shows_in_menu = false;
        self.0.shows_in_palette = false;
        self.0.hint_eligible = false;
        self
    }
}

fn both() -> When {
    When::All(vec![When::ListFocused, When::DualPane])
}

#[rustfmt::skip]
fn table() -> Vec<B> {
    use Category::*;
    let lf = When::ListFocused;
    let ab = When::AddressBarEditing;
    vec![
        // ── Navigation ──
        c("nav.open", "Open", Navigation, "Open the selected folder or file").s("⌘↓|⌘O").v("l|↩"),
        c("nav.open_other_pane", "Open in Other Pane", Navigation, "Open the selected folder in the other pane").ctx(both()),
        c("nav.open_new_tab", "Open in New Tab", Navigation, "Open the selected folder in a new tab"),
        c("nav.up", "Enclosing Folder", Navigation, "Go to the parent folder").s("⌘↑").v("h|-"),
        c("nav.back", "Back", Navigation, "Go back in this tab's history").s("⌘[").v("H"),
        c("nav.forward", "Forward", Navigation, "Go forward in this tab's history").s("⌘]").v("L"),
        c("nav.go_to", "Go to Folder…", Navigation, "Go to a given folder").global().arg(ArgKind::Path),
        c("nav.address_bar", "Go to Address Bar", Navigation, "Edit the path of the current folder")
            .global().sw(When::Always, "⌘L|⇧⌘G").sa(lf.clone(), "/", CommandArg::Char('/'))
            .sa(lf.clone(), "~", CommandArg::Char('~')),
        c("nav.other_address_bar", "Focus Other Pane's Address Bar", Navigation, "Edit the other pane's path")
            .ctx(When::DualPane).s("⇧⌘L"),
        c("nav.quick_open", "Quick Open…", Navigation, "Jump to a folder or recent item by name").global().s("⌘P"),
        c("nav.go_home", "Go to Home", Navigation, "Go to your home folder").global().s("⇧⌘H").v("g h"),
        c("nav.go_desktop", "Go to Desktop", Navigation, "Go to the Desktop folder").global().s("⇧⌘D").v("g d"),
        c("nav.go_documents", "Go to Documents", Navigation, "Go to the Documents folder").global().s("⇧⌘O"),
        c("nav.go_downloads", "Go to Downloads", Navigation, "Go to the Downloads folder").global().s("⌥⌘L"),
        c("nav.go_applications", "Go to Applications", Navigation, "Go to the Applications folder").global().s("⇧⌘A"),
        c("nav.go_icloud", "Go to iCloud Drive", Navigation, "Go to iCloud Drive").global().s("⇧⌘I"),
        c("nav.go_root", "Go to Root Folder", Navigation, "Go to the top of the startup disk").global(),
        c("nav.go_trash", "Go to Trash", Navigation, "Open the Trash folder").global(),
        c("nav.connect_server", "Connect to Server…", Navigation, "Connect to a network server").global().s("⌘K"),
        c("nav.show_package_contents", "Show Package Contents", Navigation, "Open a package such as an app as a folder"),
        c("nav.disk_usage", "Show Disk Usage", Navigation, "See what uses the space in a folder or volume, as a tree and a treemap").global().arg(ArgKind::Path),
        c("nav.mark_set", "Set Mark", Vim, "Remember this folder under a letter").arg(ArgKind::Char).nomenu().nohint().v("m {a-z}"),
        c("nav.mark_jump", "Jump to Mark", Vim, "Go to the folder remembered under a letter").arg(ArgKind::Char).nomenu().nohint().v("' {a-z}"),
        // ── Selection and cursor ──
        c("select.all", "Select All", Selection, "Select every item").s("⌘A"),
        c("select.none", "Select None", Selection, "Clear the selection"),
        c("select.invert", "Invert Selection", Selection, "Select what is not selected and the reverse").s("⌃⌘A"),
        c("select.by_pattern", "Select by Pattern…", Selection, "Select items whose names match a pattern").s("⌘="),
        c("select.deselect_pattern", "Deselect by Pattern…", Selection, "Deselect items whose names match a pattern").s("⌘-"),
        c("select.extend_up", "Extend Selection Up", Selection, "Add the item above to the selection").s("⇧↑").vv("k").nomenu().nohint().repeats(),
        c("select.extend_down", "Extend Selection Down", Selection, "Add the item below to the selection").s("⇧↓").vv("j").nomenu().nohint().repeats(),
        c("select.visual", "Visual Selection", Vim, "Start selecting items as the cursor moves").nomenu().nohint().v("v"),
        c("select.visual_line", "Visual Line Selection", Vim, "Start selecting whole rows as the cursor moves").nomenu().nohint().v("V"),
        c("select.exit_visual", "Leave Visual Mode", Vim, "Stop extending the selection").nomenu().nohint().vv("⎋"),
        c("cursor.up", "Move Up", Selection, "Move the cursor to the previous item").s("↑").v("k").nomenu().nohint().repeats(),
        c("cursor.down", "Move Down", Selection, "Move the cursor to the next item").s("↓").v("j").nomenu().nohint().repeats(),
        c("cursor.top", "Move to First Item", Selection, "Move the cursor to the first item").s("↖").v("g g").nomenu().nohint().repeats(),
        c("cursor.bottom", "Move to Last Item", Selection, "Move the cursor to the last item").s("↘").v("G").nomenu().nohint().repeats(),
        c("cursor.page_up", "Page Up", Selection, "Move the cursor up one page").s("⇞").v("⌃U").nomenu().nohint().repeats(),
        c("cursor.page_down", "Page Down", Selection, "Move the cursor down one page").s("⇟").v("⌃D").nomenu().nohint().repeats(),
        // ── File ──
        c("file.rename", "Rename", File, "Rename the selected item in place").s("↩").v("r"),
        c("file.rename_dialog", "Rename or Move As…", File, "Rename or move the selection through a dialog").s("⇧F6"),
        c("file.new_folder", "New Folder", File, "Create a folder here").s("⇧⌘N|F7"),
        c("file.new_file", "New File", File, "Create an empty file here").s("⌥⇧⌘N"),
        c("file.new_folder_with_selection", "New Folder with Selection", File, "Create a folder and move the selection into it").s("⌃⌘N"),
        c("file.trash", "Move to Trash", File, "Move the selection to the Trash").s("⌘⌫|F8")
            .va("x", CommandArg::Confirm).vva("x", CommandArg::Confirm),
        c("file.delete_immediately", "Delete Immediately…", File, "Delete the selection for good, after asking").s("⌥⌘⌫"),
        c("file.duplicate", "Duplicate", File, "Make a copy next to the original").s("⌘D"),
        c("file.copy_to_other_pane", "Copy to Other Pane", File, "Copy the selection into the other pane's folder").ctx(both()).s("F5"),
        c("file.move_to_other_pane", "Move to Other Pane", File, "Move the selection into the other pane's folder").ctx(both()).s("F6"),
        c("file.move_here", "Move Item Here", File, "Move the copied items into this folder").s("⌥⌘V"),
        c("file.make_alias", "Make Alias", File, "Create an alias to the selection"),
        c("file.compress", "Compress", File, "Compress the selection into an archive"),
        c("file.open_with", "Open With…", File, "Choose an app to open the selection"),
        c("file.folder_size", "Get Folder Size", File, "Work out how much space a folder uses"),
        c("file.batch_rename", "Batch Rename…", File, "Rename several items at once with a pattern").s("⌃⌘R"),
        c("tags.toggle", "Toggle Tag", File, "Add or remove a Finder tag on the selection").arg(ArgKind::Text).nomenu().nohint(),
        c("tags.edit", "Tags…", File, "Edit the Finder tags of the selection"),
        c("file.open_in_editor", "Open in Editor", File, "Open the selection in your editor").s("⌘E"),
        // ── Edit ──
        c("edit.copy", "Copy", Edit, "Copy the selection").s("⌘C").v("y y").vv("y"),
        c("edit.cut", "Cut (Mark for Move)", Edit, "Mark the selection to be moved when you paste; it is never deleted").s("⌘X").v("d d").vv("d"),
        c("edit.paste", "Paste", Edit, "Paste the copied items here").s("⌘V").v("p"),
        c("edit.undo", "Undo", Edit, "Undo the last file operation").s("⌘Z").v("u"),
        c("edit.redo", "Redo", Edit, "Redo the operation you undid").s("⇧⌘Z").v("⌃R"),
        // ── View ──
        c("view.set_mode", "Set View Mode", View, "Show the folder as icons, a list, columns or a gallery").hidden().arg(ArgKind::ViewMode),
        c("view.icons", "as Icons", View, "Show items as icons").s("⌘1"),
        c("view.list", "as List", View, "Show items as a list").s("⌘2"),
        c("view.columns", "as Columns", View, "Show items as columns").s("⌘3"),
        c("view.gallery", "as Gallery", View, "Show items as a gallery").s("⌘4"),
        c("view.show_hidden", "Show Hidden Files", View, "Show or hide files whose names start with a dot").s("⇧⌘.").v("z h"),
        c("view.refresh", "Refresh", View, "Reload the folder").s("⌘R"),
        c("view.toggle_sidebar", "Show or Hide Sidebar", View, "Show or hide the sidebar").global().s("⌃⌘S"),
        c("view.toggle_inspector", "Show or Hide Preview", View, "Show or hide the preview pane on the right: a picture and details of the selected item").global().s("⌥⌘P|⌥⌘I"),
        c("view.show_operations", "Show Operations", View, "Show the queue of file operations, with the reason for anything that failed").global(),
        c("transfer.resume", "Resume Transfer", View, "Resume an interrupted copy or move; on a disk in the sidebar, the ones to or from that disk").global().arg(ArgKind::Path),
        c("transfer.resume_all", "Resume All Transfers", View, "Resume every interrupted copy or move whose disks are connected").global(),
        // ── Sort ──
        c("sort.name", "Sort by Name", Sort, "Sort by name"),
        c("sort.kind", "Sort by Kind", Sort, "Sort by kind"),
        c("sort.size", "Sort by Size", Sort, "Sort by size"),
        c("sort.date_modified", "Sort by Date Modified", Sort, "Sort by date modified"),
        c("sort.date_created", "Sort by Date Created", Sort, "Sort by date created"),
        c("sort.reverse", "Reverse Sort Order", Sort, "Reverse the sort direction"),
        c("sort.folders_first", "Keep Folders First", Sort, "Keep folders above files"),
        // ── Tabs and windows ──
        c("tab.new", "New Tab", Tabs, "Open a new tab").global().s("⌘T"),
        c("tab.close", "Close Tab", Tabs, "Close this tab").global().s("⌘W"),
        c("tab.next", "Next Tab", Tabs, "Switch to the next tab").global().s("⌃⇥").v("g t"),
        c("tab.previous", "Previous Tab", Tabs, "Switch to the previous tab").global().s("⌃⇧⇥").v("g T"),
        c("tab.reopen_closed", "Reopen Closed Tab", Tabs, "Bring back the tab you closed last").global().s("⇧⌘T"),
        c("tab.duplicate", "Duplicate Tab", Tabs, "Open a copy of this tab").global(),
        c("tab.select", "Go to Tab", Tabs, "Switch to a tab by number").global().arg(ArgKind::Index).nohint(),
        c("tab.close_others", "Close Other Tabs", Tabs, "Close every tab but this one").global(),
        c("tab.move_to_other_pane", "Move Tab to Other Pane", Tabs, "Move this tab to the other pane").ctx(When::DualPane),
        c("window.new", "New Window", Tabs, "Open a new window").global().s("⌘N"),
        c("window.close", "Close Window", Tabs, "Close this window").global().s("⇧⌘W"),
        // ── Panes ──
        c("pane.toggle_dual", "Toggle Dual Pane", Panes, "Show or hide the second pane").global().s("⌘\\"),
        c("pane.switch", "Switch Pane", Panes, "Move focus to the other pane").ctx(both()).s("⇥|⌥⌘←|⌥⌘→"),
        c("pane.swap", "Swap Panes", Panes, "Swap the folders of the two panes").ctx(both()).s("⌥⌘S"),
        c("pane.same_folder", "Same Folder in Other Pane", Panes, "Show this folder in the other pane").ctx(both()).s("⌥⌘O"),
        c("pane.same_folder_reverse", "Same Folder as Other Pane", Panes, "Show the other pane's folder here").ctx(When::DualPane),
        c("pane.equalize", "Equalise Panes", Panes, "Give both panes the same width").ctx(When::DualPane),
        // ── Workspaces ──
        // `workspace.save` keeps its id so saved rebindings still apply.
        c("workspace.save", "New Workspace…", Workspaces, "Make a workspace with its own tabs and layout").global(),
        c("workspace.switch", "Switch to Workspace", Workspaces, "Switch to a workspace by its place in the switcher").global().arg(ArgKind::Index).nomenu().nohint()
            .sa(When::Always, "⌃⌘1", CommandArg::Index(0)).sa(When::Always, "⌃⌘2", CommandArg::Index(1))
            .sa(When::Always, "⌃⌘3", CommandArg::Index(2)).sa(When::Always, "⌃⌘4", CommandArg::Index(3))
            .sa(When::Always, "⌃⌘5", CommandArg::Index(4)).sa(When::Always, "⌃⌘6", CommandArg::Index(5))
            .sa(When::Always, "⌃⌘7", CommandArg::Index(6)).sa(When::Always, "⌃⌘8", CommandArg::Index(7))
            .sa(When::Always, "⌃⌘9", CommandArg::Index(8)),
        c("workspace.next", "Next Workspace", Workspaces, "Switch to the next workspace").global(),
        c("workspace.previous", "Previous Workspace", Workspaces, "Switch to the previous workspace").global(),
        c("workspace.duplicate", "Duplicate Workspace", Workspaces, "Copy this workspace's tabs and layout to a new workspace").global().arg(ArgKind::Index),
        c("workspace.delete", "Delete Workspace…", Workspaces, "Forget this workspace's tabs and layout (no files are touched)").global().arg(ArgKind::Index),
        c("workspace.manage", "Manage Workspaces…", Workspaces, "Rename, reorder, change icons or delete workspaces").global(),
        // ── Quick Look and Get Info ──
        c("preview.quick_look", "Quick Look", Preview, "Preview the selection").s("Space|⌘Y"),
        c("preview.get_info", "Get Info", Preview, "Show details about the selection").s("⌘I"),
        // ── Paths and open in ──
        c("path.copy_path", "Copy Path", Path, "Copy the path of the selection").s("⌥⌘C"),
        c("path.copy_name", "Copy Name", Path, "Copy the name of the selection").s("⇧⌘C"),
        c("path.copy_folder_path", "Copy Folder Path", Path, "Copy the path of this folder").s("⌥⇧⌘C"),
        c("path.copy_escaped", "Copy Path, Escaped", Path, "Copy the path with spaces escaped for Terminal"),
        c("path.copy_home_relative", "Copy Path Relative to Home", Path, "Copy the path starting with ~"),
        c("path.copy_file_url", "Copy as File URL", Path, "Copy the path as a file:// URL"),
        c("path.copy_quoted", "Copy Path, Quoted", Path, "Copy the path in single quotes"),
        c("terminal.open", "Open in Terminal", Path, "Open Terminal in this folder").s("⇧⌘R"),
        c("file.reveal_in_finder", "Reveal in Finder", Path, "Show the selection in Finder").s("⌥⌘R"),
        // Also a system-wide shortcut while Settings › General › Open and Save panels is on.
        c("panel.jump_here", "Send Open/Save Panel Here", Path, "Make the Open or Save panel in front go to this pane's folder").global().s("⌃⌥⌘G").nomenu(),
        c("favourites.add", "Add to Favourites", Navigation, "Pin the selected folders, or this folder, to the sidebar").s("⌃⌘T").arg(ArgKind::Path),
        c("favourites.remove", "Remove from Favourites", Navigation, "Unpin the selected folders, or this folder, from the sidebar").arg(ArgKind::Path),
        c("sidebar.remove", "Remove from Favourites", Navigation, "Remove a favourite from the sidebar").arg(ArgKind::Path).hidden(),
        c("sidebar.rename", "Rename Favourite…", Navigation, "Give a sidebar favourite another name").arg(ArgKind::Path).nomenu().nohint(),
        c("drive.eject", "Eject", Navigation, "Eject a drive, card or disk image").arg(ArgKind::Path).nomenu().nohint(),
        c("drive.info", "Show Drive Info", Navigation, "Show details about a drive").arg(ArgKind::Path).nomenu().nohint(),
        // ── Palette, filter, search ──
        c("palette.open", "Command Palette…", Search, "Search every command").global().s("⇧⌘P").v(":"),
        c("filter.focus", "Filter List", Search, "Filter the list as you type").s("⌘F").v("/"),
        c("filter.clear", "Clear Filter", Search, "Clear the filter and keep the selection").also(When::FilterEditing).sw(lf.clone(), "⎋").sw(When::FilterEditing, "⎋").nomenu().nohint(),
        // M3: ⌘F stays the filter (F-018); ⌘F again in the filter field, or ⌥⌘F anywhere, searches below the folder.
        c("search.global", "Search…", Search, "Search for files below this folder or on this Mac").global().s("⌥⌘F").sw(When::FilterEditing, "⌘F"),
        // ── Settings ──
        c("app.settings", "Settings…", Settings, "Open Settings").global().s("⌘,"),
        c("app.keyboard_shortcuts", "Keyboard Shortcuts…", Settings, "Open Settings at the shortcut editor").global(),
        c("app.power_tour", "File Studio Power Tour", Settings, "A one-minute tour of the power-user tools and their keys").global(),
        // ── Text fields (never in menus) ──
        c("addressbar.commit", "Go", TextFields, "Go to the typed or highlighted path").ctx(ab.clone()).s("↩").hidden(),
        c("addressbar.commit_other_pane", "Go in Other Pane", TextFields, "Go to the path in the other pane").ctx(ab.clone()).s("⌘↩").hidden(),
        c("addressbar.cancel", "Cancel", TextFields, "Close the suggestions, then leave the address bar").ctx(ab.clone()).s("⎋").hidden(),
        c("addressbar.complete_next", "Complete / Next Suggestion", TextFields, "Extend to the common prefix, then cycle suggestions").ctx(ab.clone()).s("⇥").hidden().repeats(),
        c("addressbar.complete_prev", "Previous Suggestion", TextFields, "Cycle suggestions backwards").ctx(ab.clone()).s("⇧⇥").hidden().repeats(),
        c("addressbar.next", "Highlight Next Suggestion", TextFields, "Move down the suggestion list").ctx(ab.clone()).s("↓").hidden().repeats(),
        c("addressbar.prev", "Highlight Previous Suggestion", TextFields, "Move up the suggestion list").ctx(ab.clone()).s("↑").hidden().repeats(),
        c("addressbar.accept_ghost", "Accept Suggestion", TextFields, "Accept the grey completion text").ctx(ab.clone()).s("↘").hidden(),
        c("addressbar.delete_component", "Delete Path Component", TextFields, "Delete back to the previous /").ctx(ab.clone()).s("⌥⌫").hidden(),
        c("addressbar.clear_line", "Clear to Start", TextFields, "Delete back to the start of the path").ctx(ab.clone()).s("⌘⌫").hidden(),
        c("addressbar.add_favourite", "Add to Favourites", TextFields, "Add the highlighted or current folder to Favourites").ctx(ab).s("⌘D").hidden(),
        c("palette.run", "Run", TextFields, "Run the highlighted command").ctx(When::PaletteOpen).s("↩").hidden(),
        c("palette.next", "Next Result", TextFields, "Highlight the next result").ctx(When::PaletteOpen).s("↓").hidden().repeats(),
        c("palette.prev", "Previous Result", TextFields, "Highlight the previous result").ctx(When::PaletteOpen).s("↑").hidden().repeats(),
        c("palette.close", "Close Palette", TextFields, "Close the palette").ctx(When::PaletteOpen).s("⎋").hidden(),
        c("rename.commit", "Finish Rename", TextFields, "Apply the new name").ctx(When::RenameEditing).s("↩").hidden(),
        c("rename.cancel", "Cancel Rename", TextFields, "Keep the old name").ctx(When::RenameEditing).s("⎋").hidden(),
        c("filter.commit", "Finish Filtering", TextFields, "Move focus to the list, keeping the filter").ctx(When::FilterEditing).s("↩").hidden(),
    ]
}

fn build() -> Vec<CommandInfo> {
    table().into_iter().map(|b| b.0).collect()
}
