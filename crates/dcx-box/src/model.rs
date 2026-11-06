//! Panel state and domain actions.
//!
//! Kept free of Win32 and egui drawing: the view layer only reads this and
//! appends [`Cmd`]s, which the frame loop executes after painting. That keeps
//! the "what happened" logic in one place and makes every destructive action
//! go through a single, explicit path.

use dcx_organizer::{
    filter_entries, is_same_or_under, sort_entries, CategoryCount, Config, Entry, Organizer,
    Snapshot, SortKey, ViewMode, CATCH_ALL,
};
use egui::Pos2;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A tab. Two are virtual (not written to `organizer.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    /// Items still on the real desktop (not collected yet).
    Desktop,
    /// Every item in the collection.
    All,
    /// Modified in the last seven days.
    Recent,
    /// One configured category.
    Key(String),
}

impl Tab {
    pub fn key(&self) -> Option<&str> {
        match self {
            Tab::Key(k) => Some(k),
            _ => None,
        }
    }
}


/// Expanded folders larger than this only paint their first page; the rest
/// stay reachable through「打开文件夹」. Keeps the 1.2 s rescan cheap even
/// when an expanded folder contains tens of thousands of files.
const MAX_CHILDREN: usize = 500;
/// Deepest nesting shown inline. Deeper levels need Explorer.
const MAX_DEPTH: u8 = 6;

/// One painted row: a top-level desktop item, or a child of an expanded
/// folder. `Entry` stays the data-layer type; depth and parent are view
/// concerns and live here.
#[derive(Debug, Clone)]
pub struct RowItem {
    pub entry: Entry,
    pub depth: u8,
    /// Immediate containing folder for a nested row.
    pub parent: Option<PathBuf>,
    /// Folder is currently expanded (directories only).
    pub expanded: bool,
}

impl RowItem {
    pub fn path(&self) -> &PathBuf {
        &self.entry.path
    }
}

/// Something the view wants the frame loop to do.
#[derive(Debug, Clone)]
pub enum Cmd {
    Open(PathBuf),
    Reveal(PathBuf),
    Properties(PathBuf),
    CopyText(String),
    OpenDesktop,
    Refresh,
    ClosePanel,
    FocusSearch,
    AskDelete(Vec<PathBuf>),
    DeleteNow(Vec<PathBuf>),
    ReleaseToDesktop(Vec<PathBuf>, Option<Pos2>),
    Assign(Vec<PathBuf>, String),
    ClearOverrides(Vec<PathBuf>),
    ReclassifyAll,
    RenameDialog(PathBuf),
    RenameApply(PathBuf, String),
    NewCategoryDialog,
    NewCategory(String, String),
    AboutDialog,
    SetSort(SortKey),
    ToggleView,
    ToggleHidden,
    ToggleSingleClick,
    /// Display switches in the header dropdown.
    ToggleTime,
    ToggleSize,
    ToggleFolders,
    ToggleExt,
    /// Collapse behaviour switches in the 「更多」 dropdown.
    ToggleCloseAfterOpen,
    ToggleCloseOnOutside,
    /// Taskbar-style acrylic backdrop behind the panel.
    ToggleAcrylic,
    ClearSelection,
    SelectAll,
    Absorb(Vec<PathBuf>, String),
    /// Move items into the collection; `None` lets the rules classify them.
    Collect(Vec<PathBuf>, Option<String>),
    /// Ask before moving every visible desktop item into the collection.
    AskCollectDesktop,
    CollectDesktop(Vec<PathBuf>),
    /// Open the collection root in Explorer.
    OpenLibrary,
    /// Copy the collection root path to the clipboard.
    CopyLibraryPath,
    /// Expand / collapse one folder row.
    ToggleExpand(PathBuf),
    /// Physically move items into a folder.
    MoveToFolder(Vec<PathBuf>, PathBuf),
    /// Pick any folder on disk as the destination.
    ChooseMoveFolder(Vec<PathBuf>),
    /// Physically move nested rows back to the desktop root.
    MoveToDesktop(Vec<PathBuf>),
    /// `None` = create on the desktop, `Some(dir)` = create inside a folder.
    NewFolderDialog(Option<PathBuf>),
    NewFolderApply(PathBuf, String),
    OpenSettings,
    SavePrefs,
    /// Right-click on the taskbar tile.
    TileMenu,
}

/// One row of a context / toolbar popup.
#[derive(Debug, Clone)]
pub struct PopupItem {
    pub label: String,
    pub cmd: Option<Cmd>,
    pub danger: bool,
    pub checked: bool,
    pub disabled: bool,
    pub separator: bool,
    pub icon: MenuIcon,
    /// `Some(0..1)` renders the row as a slider instead of a button.
    pub slider: Option<f32>,
}

/// Glyph drawn before a menu label. Kept as data here so `view` owns the
/// painting and every menu in the panel uses the same vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIcon {
    None,
    Outside,
    Acrylic,
    Refresh,
    Folder,
    Reclassify,
    List,
    Grid,
    Eye,
    EyeOff,
    Cursor,
    Plus,
    Gear,
    Info,
    Open,
    Copy,
    Rename,
    Tag,
    TextName,
    Clock,
    SortSize,
    FileType,
    ArrowUpDown,
    Release,
    Trash,
    Check,
    Expand,
    Collapse,
    MoveIn,
}

impl PopupItem {
    pub fn action(label: impl Into<String>, cmd: Cmd) -> Self {
        PopupItem {
            label: label.into(),
            cmd: Some(cmd),
            danger: false,
            checked: false,
            disabled: false,
            separator: false,
            icon: MenuIcon::None,
            slider: None,
        }
    }
    pub fn danger(label: impl Into<String>, cmd: Cmd) -> Self {
        let mut i = Self::action(label, cmd);
        i.danger = true;
        i
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn icon(mut self, icon: MenuIcon) -> Self {
        self.icon = icon;
        self
    }
    pub fn disabled(mut self, on: bool) -> Self {
        self.disabled = on;
        self
    }
    pub fn sep() -> Self {
        PopupItem {
            label: String::new(),
            cmd: None,
            danger: false,
            checked: false,
            disabled: false,
            separator: true,
            icon: MenuIcon::None,
            slider: None,
        }
    }
    /// A slider row: `value` is the knob position, 0..1.
    pub fn slider(label: impl Into<String>, value: f32, icon: MenuIcon) -> Self {
        let mut i = PopupItem::action(label, Cmd::SavePrefs);
        i.cmd = None;
        i.slider = Some(value.clamp(0.0, 1.0));
        i.icon = icon;
        i
    }
}

#[derive(Debug, Clone)]
pub struct PopupState {
    /// Top-left in window-local points.
    pub at: Pos2,
    pub title: Option<String>,
    pub items: Vec<PopupItem>,
    pub width: f32,
    pub scroll: f32,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    ConfirmDelete { paths: Vec<PathBuf> },
    Rename { path: PathBuf, text: String },
    NewFolder { parent: PathBuf, text: String },
    NewCategory { label: String, exts: String },
    ConfirmCollectDesktop { paths: Vec<PathBuf> },
    About,
}

pub struct Model {
    pub cfg: Config,
    pub org: Organizer,
    pub snap: Snapshot,
    /// Top-level items still on the real desktop. The「桌面」tab is a view over
    /// these; the category tabs and「全部」are the physical collection.
    pub desk_items: Vec<Entry>,
    pub active: Tab,
    pub query: String,
    pub selection: BTreeSet<PathBuf>,
    /// Last clicked row, for shift-range selection.
    pub anchor: Option<PathBuf>,
    pub popup: Option<PopupState>,
    pub dialog: Option<Dialog>,
    pub toast: Option<(String, Instant)>,
    pub cmds: Vec<Cmd>,
    pub tab_scroll: f32,
    /// Where `tab_scroll` is easing towards. The wheel and the arrows write
    /// here; the render position follows, which keeps the strip smooth even
    /// when a wheel event delivers a large delta in one frame.
    pub tab_scroll_target: f32,
    /// Which tab the strip last scrolled into view. Auto-reveal must only run
    /// when the selection *changes* — running it every frame meant that any
    /// attempt to scroll the active chip off-screen was immediately undone,
    /// which read as "the strip is stuck".
    pub tab_synced: String,
    /// Category chip being dragged to a new position, if any.
    pub tab_drag: Option<TabDrag>,
    /// Chip currently held down but not yet a drag (see [`TabPress`]).
    pub tab_press: Option<TabPress>,
    pub list_scroll: f32,
    /// Where inside the scrollbar thumb the pointer grabbed it (points). Kept
    /// between frames so dragging follows the pointer 1:1 instead of snapping
    /// the thumb centre under it.
    pub scroll_grab: Option<f32>,
    pub last_refresh: Instant,
    /// When the opacity slider last moved, so the write is debounced.
    pub alpha_dirty: Option<Instant>,
    pub cfg_stamp: Option<(u64, u128)>,
    pub search_focus: bool,
    pub drag_in: bool,
    /// Folders currently expanded inline (session state, not persisted).
    pub expanded: BTreeSet<PathBuf>,
    /// Rows currently visible (after tab/query/sort/tree). Rebuilt on refresh.
    pub rows: Vec<RowItem>,
}

/// A category chip being dragged to a new position.
///
/// Only real categories move: the two virtual tabs are fixed, and the catch-all
/// is pinned last by `Config::repair` — letting it move would only make it snap
/// back on the next launch, which reads as "my change was not saved".
#[derive(Debug, Clone)]
pub struct TabDrag {
    /// Category key under the pointer.
    pub key: String,
    /// Where it would be inserted, as an index into the chip row (virtual tabs
    /// included). Resolved to a category index on release.
    pub target: usize,
}

/// A chip was pressed but has not yet decided between a click and a drag.
/// egui's `drag_started()` is unusable here: this panel runs on a winit loop
/// whose `CursorMoved` events never reach egui (the pointer's `interact_pos`
/// stays frozen at the press point), so a drag is never detected. The press is
/// therefore tracked by hand from the live global cursor, exactly like the
/// panel-move / resize gestures in the frame loop.
#[derive(Debug, Clone)]
pub struct TabPress {
    /// Widget id of the pressed chip ("k:image").
    pub key: String,
    /// Window-local cursor at press time.
    pub origin: Pos2,
    /// Whether this chip may move at all. The two virtual tabs and the
    /// catch-all may not, and saying so once beats silently ignoring the drag.
    pub movable: bool,
    /// The refusal was already explained this gesture.
    pub spoken: bool,
}

impl Model {
    pub fn new() -> Self {
        let cfg = Config::load();
        let mut org = Organizer::new(cfg.clone());
        let snap = org.snapshot();
        let desk_items = org.desktop_items();
        // First run of the library model (or an empty collection): show the
        // desktop, where the files still are, instead of an empty「全部」tab.
        let active = if snap.items.is_empty() && !desk_items.is_empty() {
            Tab::Desktop
        } else if cfg.remember_tab {
            match cfg.last_tab.as_deref() {
                Some(k) if cfg.category(k).is_some() => Tab::Key(k.to_string()),
                _ => Tab::All,
            }
        } else {
            Tab::All
        };
        let mut m = Model {
            cfg,
            org,
            snap,
            desk_items,
            active,
            query: String::new(),
            selection: BTreeSet::new(),
            anchor: None,
            popup: None,
            dialog: None,
            toast: None,
            cmds: Vec::new(),
            tab_scroll: 0.0,
            tab_scroll_target: 0.0,
            tab_synced: String::new(),
            tab_drag: None,
            tab_press: None,
            list_scroll: 0.0,
            scroll_grab: None,
            last_refresh: Instant::now(),
            alpha_dirty: None,
            cfg_stamp: config_stamp(),
            search_focus: false,
            drag_in: false,
            expanded: BTreeSet::new(),
            rows: Vec::new(),
        };
        m.rebuild_rows();
        m
    }

    pub fn push(&mut self, cmd: Cmd) {
        self.cmds.push(cmd);
    }

    pub fn take_cmds(&mut self) -> Vec<Cmd> {
        std::mem::take(&mut self.cmds)
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    pub fn toast_alive(&mut self) -> bool {
        match &self.toast {
            Some((_, at)) if at.elapsed().as_millis() < 2600 => true,
            Some(_) => {
                self.toast = None;
                false
            }
            None => false,
        }
    }

    // -- configuration ------------------------------------------------------

    /// Reload when the settings centre (or another box instance) wrote the
    /// document. UI-only state (query, selection, menu) survives; only the
    /// configuration-derived parts are rebuilt.
    pub fn reload(&mut self) {
        let cfg = Config::load();
        self.cfg = cfg.clone();
        // Rebuild rather than assign `org.cfg`: the settings centre can also
        // change the library root, which is captured in `Organizer::root`.
        self.org = Organizer::new(cfg);
        if let Tab::Key(k) = &self.active {
            if self.cfg.category(k).is_none() {
                self.active = Tab::All;
            }
        }
        self.cfg_stamp = config_stamp();
        self.refresh();
    }

    pub fn check_config(&mut self) {
        if config_stamp() != self.cfg_stamp {
            self.reload();
        }
    }

    /// Write UI preferences back **without** clobbering fields owned by the
    /// settings centre (categories, enabled, hidden, overrides …). Read-modify-
    /// write against the current on-disk document.
    pub fn persist_prefs(&mut self) {
        let c = self.cfg.clone();
        let _ = Config::patch(|d| {
            d.tile_mode = c.tile_mode;
            d.tile_offset = c.tile_offset;
            d.tile_fixed_x = c.tile_fixed_x;
            d.sort_by = c.sort_by;
            d.sort_desc = c.sort_desc;
            d.view_mode = c.view_mode;
            d.panel_width = c.panel_width;
            d.panel_height = c.panel_height;
            d.show_hidden = c.show_hidden;
            d.single_click_open = c.single_click_open;
            d.remember_tab = c.remember_tab;
            d.last_tab = c.last_tab.clone();
            d.confirm_delete = c.confirm_delete;
            d.show_time = c.show_time;
            d.show_size = c.show_size;
            d.show_folders = c.show_folders;
            d.hide_ext = c.hide_ext;
            d.close_after_open = c.close_after_open;
            d.close_on_outside = c.close_on_outside;
            d.acrylic = c.acrylic;
            d.acrylic_alpha = c.acrylic_alpha;
        });
        // `org` keeps its own `Config`. Anything the organiser itself reads
        // (sort for expanded children, hidden-item policy, ignore rules) has
        // to follow the panel immediately; otherwise children kept the old
        // order/filter until the next external config reload.
        self.org.cfg.sort_by = c.sort_by;
        self.org.cfg.sort_desc = c.sort_desc;
        self.org.cfg.show_hidden = c.show_hidden;
        self.org.cfg.ignore = c.ignore.clone();
        self.cfg_stamp = config_stamp();
    }

    pub fn set_active(&mut self, tab: Tab) {
        self.active = tab.clone();
        self.selection.clear();
        self.anchor = None;
        self.popup = None;
        self.query.clear();
        if self.cfg.remember_tab {
            self.cfg.last_tab = tab.key().map(|s| s.to_string());
            self.persist_prefs();
        }
        self.rebuild_rows();
    }

    /// Apply a drag-reorder of the category chips and write it to
    /// `organizer.toml`, so the new order survives a restart.
    ///
    /// `order` holds category keys in their new sequence; anything missing
    /// keeps its relative position at the end, and the catch-all is pinned
    /// last exactly like `Config::repair` would.
    pub fn reorder_categories(&mut self, order: &[String]) {
        let mut pool = self.cfg.categories.clone();
        let mut next = Vec::with_capacity(pool.len());
        for key in order {
            if let Some(i) = pool.iter().position(|c| &c.key == key) {
                next.push(pool.remove(i));
            }
        }
        next.extend(pool);
        if let Some(i) = next.iter().position(|c| c.key == CATCH_ALL) {
            let c = next.remove(i);
            next.push(c);
        }
        if next
            .iter()
            .map(|c| &c.key)
            .eq(self.cfg.categories.iter().map(|c| &c.key))
        {
            return;
        }
        self.cfg.categories = next;
        // The organiser keeps its **own** copy of the document and writes it
        // back whenever desktop visibility changes. Leaving the pre-drag order
        // in that copy is what made a reorder snap back: the next refresh
        // re-saved the stale document and `check_config` reloaded it.
        self.org.cfg.categories = self.cfg.categories.clone();
        // Read-modify-write: the settings centre owns the same document.
        let cats = self.cfg.categories.clone();
        let _ = Config::patch(|c| c.categories = cats);
        self.cfg_stamp = config_stamp();
        // The tab strip is built from `snap.counts`, not from the config, and
        // that snapshot is only rebuilt by a full desktop rescan on the 1.2 s
        // timer. Without re-ordering it here the chip went back to its old slot
        // on release and only jumped to the new one a second later.
        let mut counts: std::collections::HashMap<String, CategoryCount> = self
            .snap
            .counts
            .drain(..)
            .map(|c| (c.key.clone(), c))
            .collect();
        self.snap.counts = self
            .cfg
            .categories
            .iter()
            .map(|c| {
                counts.remove(&c.key).unwrap_or(CategoryCount {
                    key: c.key.clone(),
                    label: c.label.clone(),
                    count: 0,
                    bytes: 0,
                })
            })
            .collect();
        self.rebuild_rows();
    }

    pub fn set_sort(&mut self, by: SortKey) {
        if self.cfg.sort_by == by {
            self.cfg.sort_desc = !self.cfg.sort_desc;
        } else {
            self.cfg.sort_by = by;
            self.cfg.sort_desc = false;
        }
        self.persist_prefs();
        self.rebuild_rows();
    }

    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.cfg.view_mode = mode;
        self.persist_prefs();
        // Grid has no inline tree; the row list must drop (or restore) the
        // expanded children when the mode changes.
        self.rebuild_rows();
    }

    // -- data ---------------------------------------------------------------

    pub fn refresh(&mut self) {
        self.org.prune();
        self.snap = self.org.snapshot();
        self.desk_items = self.org.desktop_items();
        // A folder that was renamed, moved or deleted can no longer be a tree
        // node. Cheap enough to check once per scan.
        self.expanded.retain(|p| p.is_dir());
        self.last_refresh = Instant::now();
        self.rebuild_rows();
        // Selected rows that vanished (deleted, moved, filtered out) must not
        // stay in the bag: Delete would then try to recycle a ghost path.
        self.selection.retain(|p| self.rows.iter().any(|r| r.path() == p));
    }

    fn recent_cutoff() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().saturating_sub(7 * 24 * 3600))
            .unwrap_or(0)
    }

    /// "Recently on the desktop" is creation **or** modification: a file
    /// copied/moved in with an old mtime is still new to the user.
    fn recent_stamp(e: &Entry) -> u64 {
        e.created.max(e.modified)
    }

    fn is_recent(e: &Entry) -> bool {
        Self::recent_stamp(e) >= Self::recent_cutoff()
    }

    /// Called once when the panel opens: diff against the last snapshot and
    /// return files that appeared while it was closed. Ignored items and files
    /// hidden by other tools never enter the snapshot, so they are not
    /// announced.
    pub fn refresh_collect_new(&mut self) -> Vec<PathBuf> {
        let before: BTreeSet<PathBuf> = self.snap.items.iter().map(|e| e.path.clone()).collect();
        self.refresh();
        self.snap
            .items
            .iter()
            .filter(|e| !e.is_dir && !before.contains(&e.path))
            .map(|e| e.path.clone())
            .collect()
    }

    /// Land on the recent view after new files were absorbed, without
    /// overwriting the remembered category (`last_tab`).
    pub fn show_recent(&mut self, new_count: usize) {
        self.active = Tab::Recent;
        self.selection.clear();
        self.anchor = None;
        self.popup = None;
        self.query.clear();
        self.list_scroll = 0.0;
        // Let the tab strip scroll this chip into view again.
        self.tab_synced.clear();
        self.rebuild_rows();
        self.toast(format!("新增 {new_count} 个文件，已放到「最近」"));
    }

    pub fn rebuild_rows(&mut self) {
        // A query searches the active surface (collection or the desktop tab),
        // which is what makes the box useful once it holds a few hundred items.
        let base: Vec<Entry> = if !self.query.trim().is_empty() {
            if self.active == Tab::Desktop {
                self.desk_items.clone()
            } else {
                self.snap.items.clone()
            }
        } else {
            match &self.active {
                Tab::Desktop => self.desk_items.clone(),
                Tab::All => self.snap.items.clone(),
                Tab::Recent => self
                    .snap
                    .items
                    .iter()
                    .filter(|e| Self::is_recent(e))
                    .cloned()
                    .collect(),
                Tab::Key(k) => self
                    .snap
                    .items
                    .iter()
                    .filter(|e| &e.category == k)
                    .cloned()
                    .collect(),
            }
        };
        let mut entries = filter_entries(&base, &self.query);
        if !self.cfg.show_folders {
            entries.retain(|e| !e.is_dir);
        }
        let recent_view = matches!(self.active, Tab::Recent) && self.query.trim().is_empty();
        if recent_view {
            // "Recently" is only useful with the newest first; the global sort
            // would bury a just-saved file in the middle of an alphabetical
            // list. All other tabs keep the user's sort.
            entries.sort_by(|a, b| {
                Self::recent_stamp(b)
                    .cmp(&Self::recent_stamp(a))
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        } else {
            sort_entries(&mut entries, self.cfg.sort_by, self.cfg.sort_desc);
        }

        // Searching is a flat operation over the whole desktop; a tree under a
        // filtered result would show children that do not match the query.
        // Grid cells have no room for indentation, so it stays flat there too.
        let tree = self.query.trim().is_empty()
            && self.cfg.view_mode == ViewMode::List
            && self.active != Tab::Desktop;
        let mut rows = Vec::with_capacity(entries.len());
        for e in entries {
            let expanded = tree && e.is_dir && self.expanded.contains(&e.path);
            let path = e.path.clone();
            rows.push(RowItem {
                entry: e,
                depth: 0,
                parent: None,
                expanded,
            });
            if expanded {
                self.push_children(&mut rows, &path, 1);
            }
        }
        self.rows = rows;
    }

    fn push_children(&self, rows: &mut Vec<RowItem>, dir: &Path, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        let (mut kids, _total) = self.org.folder_children(dir, MAX_CHILDREN);
        for k in &mut kids {
            k.category = self.org.classify(k);
            k.manual = self.org.is_manual(k);
        }
        for k in kids {
            let expanded = depth < MAX_DEPTH && k.is_dir && self.expanded.contains(&k.path);
            let path = k.path.clone();
            rows.push(RowItem {
                entry: k,
                depth,
                parent: Some(dir.to_path_buf()),
                expanded,
            });
            if expanded {
                self.push_children(rows, &path, depth + 1);
            }
        }
    }

    /// Expand or collapse a folder row. Expanding from grid switches to the
    /// list because a tree has nowhere to go inside a card grid.
    pub fn toggle_expand(&mut self, path: &Path) {
        if self.expanded.remove(path) {
            self.rebuild_rows();
            return;
        }
        self.expanded.insert(path.to_path_buf());
        if self.cfg.view_mode == ViewMode::Grid {
            self.cfg.view_mode = ViewMode::List;
            self.persist_prefs();
        }
        let (_, total) = self.org.folder_children(path, MAX_CHILDREN);
        self.rebuild_rows();
        if total > MAX_CHILDREN {
            self.toast(format!("文件夹较大，仅展开前 {MAX_CHILDREN} 项"));
        } else if total == 0 {
            self.toast("这个文件夹是空的");
        }
    }

    pub fn on_query_changed(&mut self) {
        self.selection.clear();
        self.rebuild_rows();
    }

    pub fn tab_infos(&self) -> Vec<(Tab, String, usize)> {
        let mut out = vec![
            (
                Tab::Desktop,
                "桌面".to_string(),
                self.desk_items.iter().filter(|e| !e.is_dir || self.cfg.show_folders).count(),
            ),
            (Tab::All, "全部".to_string(), self.snap.items.len()),
            (
                Tab::Recent,
                "最近".to_string(),
                self.snap.items.iter().filter(|e| Self::is_recent(e)).count(),
            ),
        ];
        for c in &self.snap.counts {
            out.push((Tab::Key(c.key.clone()), c.label.clone(), c.count));
        }
        out
    }

    pub fn active_label(&self) -> String {
        if !self.query.trim().is_empty() {
            return "搜索结果".to_string();
        }
        match &self.active {
            Tab::Desktop => "桌面".to_string(),
            Tab::All => "全部".to_string(),
            Tab::Recent => "最近 7 天".to_string(),
            Tab::Key(k) => self
                .cfg
                .category(k)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| k.clone()),
        }
    }

    /// Footer counts mirror the tab counts: expanded children are visible
    /// rows but not items of the active tab.
    pub fn top_count(&self) -> usize {
        self.rows.iter().filter(|r| r.depth == 0).count()
    }

    pub fn total_bytes(&self) -> u64 {
        self.rows
            .iter()
            .filter(|r| r.depth == 0 && !r.entry.is_dir)
            .map(|r| r.entry.size)
            .sum()
    }

    // -- selection ----------------------------------------------------------

    pub fn clicked_row(&mut self, path: &PathBuf, index: usize, ctrl: bool, shift: bool) {
        if shift {
            if let Some(a) = &self.anchor {
                if let (Some(ai), Some(bi)) = (
                    self.rows.iter().position(|r| r.path() == a),
                    self.rows.iter().position(|r| r.path() == path),
                ) {
                    let (lo, hi) = (ai.min(bi), ai.max(bi));
                    for r in &self.rows[lo..=hi] {
                        self.selection.insert(r.entry.path.clone());
                    }
                }
            } else {
                self.selection.insert(path.clone());
                self.anchor = Some(path.clone());
            }
        } else if ctrl {
            if !self.selection.insert(path.clone()) {
                self.selection.remove(path);
            }
            self.anchor = Some(path.clone());
        } else if self.selection.contains(path) {
            // Plain click on something already selected = deselect. Without
            // this a multi-selection could only be cleared by clicking empty
            // space, which a full list does not have.
            self.selection.remove(path);
            self.anchor = Some(path.clone());
        } else {
            self.selection.clear();
            self.selection.insert(path.clone());
            self.anchor = Some(path.clone());
        }
        let _ = index;
    }

    /// Switches for the header's 「显示」 dropdown. `checked` is drawn by the
    /// shared popup renderer, so these read as toggles rather than commands.
    pub fn display_items(&self) -> Vec<PopupItem> {
        vec![
            PopupItem::action("显示时间", Cmd::ToggleTime)
                .checked(self.cfg.show_time)
                .icon(MenuIcon::Clock),
            PopupItem::action("显示大小", Cmd::ToggleSize)
                .checked(self.cfg.show_size)
                .icon(MenuIcon::SortSize),
            PopupItem::action("显示文件夹", Cmd::ToggleFolders)
                .checked(self.cfg.show_folders)
                .icon(MenuIcon::Folder),
            PopupItem::action("隐藏扩展名", Cmd::ToggleExt)
                .checked(self.cfg.hide_ext)
                .icon(MenuIcon::EyeOff),
        ]
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection.iter().cloned().collect()
    }

    /// Paths a menu should act on: the selection, or just this row.
    pub fn targets_for(&self, path: &PathBuf) -> Vec<PathBuf> {
        if self.selection.contains(path) && self.selection.len() > 1 {
            self.selected_paths()
        } else {
            vec![path.clone()]
        }
    }

    /// Paths a drag should carry: the whole selection when the pressed row is
    /// part of one, otherwise just that row.
    pub fn drag_targets(&self, path: &PathBuf) -> Vec<PathBuf> {
        self.targets_for(path)
    }

    /// A drop is valid when the destination is neither one of the dragged
    /// items nor inside one of them.
    pub fn drop_allowed(&self, paths: &[PathBuf], dir: &Path) -> bool {
        !paths
            .iter()
            .any(|p| is_same_or_under(dir, p))
    }

    pub fn select_all(&mut self) {
        self.selection = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        self.anchor = self.rows.first().map(|r| r.entry.path.clone());
    }

    pub fn row_for(&self, path: &Path) -> Option<&RowItem> {
        self.rows.iter().find(|r| r.path() == path)
    }

    // -- domain operations (all metadata, except explicit delete/rename) ----

    pub fn assign(&mut self, paths: &[PathBuf], key: &str) {
        let n = self.org.set_overrides(paths, key);
        let _ = self.org.save();
        self.refresh();
        let label = self
            .cfg
            .category(key)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| key.to_string());
        self.toast(format!("已将 {n} 项归入「{label}」"));
    }

    pub fn clear_overrides(&mut self, paths: &[PathBuf]) {
        let n = self.org.clear_overrides_of(paths);
        let _ = self.org.save();
        self.refresh();
        self.toast(format!("已恢复 {n} 项的自动分类"));
    }

    pub fn reclassify_all(&mut self) {
        let n = self.org.clear_overrides();
        let _ = self.org.save();
        self.refresh();
        self.toast(format!("已按规则重新分类（清除 {n} 项手动记录）"));
    }

    /// Move desktop/external items into the collection, filing them under a
    /// named category.
    pub fn absorb(&mut self, paths: &[PathBuf], key: &str) {
        let (tagged, moved) = self.org.absorb(paths, key);
        let _ = self.org.save();
        self.refresh();
        self.leave_source_tab(Some(key));
        let label = self
            .cfg
            .category(key)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| key.to_string());
        self.toast(if moved > 0 {
            format!("已收纳 {tagged} 项到收纳库「{label}」")
        } else {
            format!("已将 {tagged} 项归入「{label}」")
        });
    }

    /// Drop on the list body: move everything into the collection and let the
    /// rules decide the category.
    pub fn absorb_auto(&mut self, paths: &[PathBuf]) {
        let (adopted, _moved) = self.org.absorb_auto(paths);
        let _ = self.org.save();
        self.refresh();
        self.leave_source_tab(None);
        if adopted == 0 {
            self.toast("没有可收纳的项目");
        } else {
            self.toast(format!("已收纳 {adopted} 项到收纳库并自动分类"));
        }
    }

    /// Collect desktop/external items, optionally into a named category.
    pub fn collect(&mut self, paths: &[PathBuf], key: Option<&str>) {
        match key {
            Some(key) => self.absorb(paths, key),
            None => self.absorb_auto(paths),
        }
    }

    /// Bulk migration: move a confirmed list of desktop items into the library.
    pub fn collect_desktop(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let (adopted, _) = self.org.absorb_auto(paths);
        let _ = self.org.save();
        self.refresh();
        self.leave_source_tab(None);
        self.toast(format!("已收纳 {adopted} 项到收纳库"));
    }

    pub fn desktop_paths(&self) -> Vec<PathBuf> {
        self.desk_items.iter().map(|e| e.path.clone()).collect()
    }

    pub fn library_folder(&self) -> PathBuf {
        self.org.root().to_path_buf()
    }

    /// The desktop tab is a fixed list; a drop that files something in the
    /// library must move the view, or it looks like nothing happened.
    fn leave_source_tab(&mut self, key: Option<&str>) {
        if self.active == Tab::Desktop {
            self.active = key
                .map(|k| Tab::Key(k.to_string()))
                .unwrap_or(Tab::All);
            self.tab_synced.clear();
            self.rebuild_rows();
        }
    }

    /// A drag or menu choice asking for a *physical* move into `dir`.
    pub fn move_to_folder(&mut self, paths: &[PathBuf], dir: &Path) {
        if !self.drop_allowed(paths, dir) {
            self.toast("不能把文件夹移进它自己");
            return;
        }
        let (moved, failed) = self.org.move_to_folder(paths, dir);
        let _ = self.org.save();
        self.refresh();
        let label = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string());
        if failed > 0 {
            self.toast(format!("已移入 {moved} 项，{failed} 项失败"));
        } else if moved == 0 {
            self.toast("这些项目已经在目标文件夹中");
        } else {
            self.toast(format!("已移入 {moved} 项到「{label}」"));
        }
    }

    /// Collection items dragged out of the panel go back to the real desktop.
    /// Returns the new paths so the frame loop can place their icons.
    pub fn move_to_desktop(&mut self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let out = self.org.move_to_desktop(paths);
        if !out.is_empty() {
            let _ = self.org.save();
            self.refresh();
            self.toast(format!("已放回桌面 {} 项", out.len()));
        } else if !paths.is_empty() {
            // A locked file fails inside the organiser; without this the
            // action looked like it had done nothing at all.
            self.toast("未能放回桌面：文件可能正在使用中");
        }
        out
    }

    pub fn create_folder(&mut self, parent: &Path, name: &str) {
        match self.org.create_folder(parent, name) {
            Ok(p) => {
                // Show the new folder where the user just acted, even if its
                // parent was collapsed.
                if !dcx_organizer::same_path(parent, self.org.root())
                    && !dcx_organizer::same_path(parent, self.org.desktop())
                {
                    self.expanded.insert(parent.to_path_buf());
                }
                self.refresh();
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.toast(format!("已新建文件夹「{name}」"));
            }
            Err(e) => self.toast(format!("新建文件夹失败：{e}")),
        }
    }

    pub fn forget(&mut self, paths: &[PathBuf]) {
        self.org.forget_many(paths);
        for p in paths {
            self.selection.remove(p);
        }
        let _ = self.org.save();
        self.refresh();
    }

    pub fn rename(&mut self, path: &PathBuf, new_name: &str) {
        match self.org.rename(path, new_name) {
            Ok(_) => {
                self.refresh();
                self.toast("已重命名");
            }
            Err(e) => self.toast(format!("重命名失败：{e}")),
        }
    }

    pub fn new_category(&mut self, label: &str, exts: &str) {
        let label = label.trim();
        if label.is_empty() {
            self.toast("分类名称不能为空");
            return;
        }
        let mut disk = Config::load();
        let key = format!("cat{}", dcx_organizer::now_ms() % 100_000);
        disk.categories.push(dcx_organizer::Category {
            key: key.clone(),
            label: label.to_string(),
            exts: exts
                .split(|c: char| c.is_whitespace() || c == ',' || c == '，')
                .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        });
        disk.repair();
        let _ = disk.save();
        self.reload();
        self.set_active(Tab::Key(key));
        self.toast(format!("已创建分类「{label}」"));
    }

    // -- popups -------------------------------------------------------------

    pub fn open_popup(
        &mut self,
        at: Pos2,
        title: Option<String>,
        items: Vec<PopupItem>,
        width: f32,
    ) {
        self.popup = Some(PopupState {
            at,
            title,
            items,
            width,
            scroll: 0.0,
        });
    }

    pub fn close_popup(&mut self) {
        self.popup = None;
    }

    /// Menu shown by right-clicking the taskbar tile.
    ///
    /// There is deliberately **one** position rule now (right after the last
    /// program icon, in the band the engine reserves). The old trio
    /// 「托盘左侧 / 跟随图标 / 锁定」 all resolved to the same measured number
    /// and only made the button look unstable, so the menu keeps the two
    /// actions a user actually needs.
    pub fn tile_menu_items(&self) -> Vec<PopupItem> {
        vec![
            PopupItem::action("打开设置…", Cmd::OpenSettings).icon(MenuIcon::Gear),
            PopupItem::action("关于收纳盒", Cmd::AboutDialog).icon(MenuIcon::Info),
        ]
    }

    /// Context menu for one row, or for the whole selection when the row is
    /// part of a multi-selection.
    pub fn context_items(&self, path: &PathBuf) -> Vec<PopupItem> {
        let paths = self.targets_for(path);
        let multi = paths.len() > 1;
        let row = self.row_for(path);
        let any_nested = paths
            .iter()
            .any(|p| self.row_for(p).map(|r| r.depth > 0).unwrap_or(false));
        let is_dir = row
            .map(|r| r.entry.is_dir)
            .or_else(|| {
                self.snap
                    .items
                    .iter()
                    .find(|e| &e.path == path)
                    .map(|e| e.is_dir)
            })
            .unwrap_or(false);
        let in_library = self.org.contains(path);
        let mut v = Vec::new();
        if !multi {
            v.push(
                PopupItem::action(
                    if is_dir { "打开文件夹" } else { "打开" },
                    Cmd::Open(path.clone()),
                )
                .icon(MenuIcon::Open),
            );
            if !in_library {
                v.push(
                    PopupItem::action("收纳到盒子", Cmd::Collect(paths.clone(), None))
                        .icon(MenuIcon::MoveIn),
                );
            }
            if is_dir && in_library {
                if self.query.trim().is_empty() {
                    let expanded = self.expanded.contains(path);
                    v.push(
                        PopupItem::action(
                            if expanded { "折叠" } else { "展开" },
                            Cmd::ToggleExpand(path.clone()),
                        )
                        .icon(if expanded {
                            MenuIcon::Collapse
                        } else {
                            MenuIcon::Expand
                        }),
                    );
                }
                v.push(
                    PopupItem::action(
                        "新建子文件夹…",
                        Cmd::NewFolderDialog(Some(path.clone())),
                    )
                    .icon(MenuIcon::Plus),
                );
            }
            v.push(
                PopupItem::action("在资源管理器中显示", Cmd::Reveal(path.clone()))
                    .icon(MenuIcon::Folder),
            );
            v.push(PopupItem::action("属性", Cmd::Properties(path.clone())).icon(MenuIcon::Info));
            v.push(PopupItem::sep());
            v.push(
                PopupItem::action(
                    "复制完整路径",
                    Cmd::CopyText(path.to_string_lossy().to_string()),
                )
                .icon(MenuIcon::Copy),
            );
            v.push(
                PopupItem::action(
                    "复制文件名",
                    Cmd::CopyText(
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    ),
                )
                .icon(MenuIcon::Copy),
            );
            v.push(
                PopupItem::action("重命名…", Cmd::RenameDialog(path.clone()))
                    .icon(MenuIcon::Rename),
            );
        } else {
            // An information header, not an action: clicking it used to run a
            // refresh, which looked like the menu did something arbitrary.
            v.push(
                PopupItem::action(format!("已选中 {} 项", paths.len()), Cmd::Refresh)
                    .icon(MenuIcon::Check)
                    .disabled(true),
            );
            v.push(
                PopupItem::action(
                    "复制全部路径",
                    Cmd::CopyText(
                        paths
                            .iter()
                            .map(|p| p.to_string_lossy().to_string())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                )
                .icon(MenuIcon::Copy),
            );
        }
        v.push(PopupItem::sep());

        // Folders on the desktop are the drag-free way to move something in.
        // The selection itself and the row's current parent are not targets.
        let parent = row.and_then(|r| r.parent.clone());
        let mut targets = Vec::new();
        for e in &self.snap.items {
            if !e.is_dir || paths.iter().any(|p| p == &e.path) {
                continue;
            }
            if parent.as_ref().map(|p| p == &e.path).unwrap_or(false) {
                continue;
            }
            targets.push(
                PopupItem::action(
                    format!("移入「{}」", e.name),
                    Cmd::MoveToFolder(paths.clone(), e.path.clone()),
                )
                .icon(MenuIcon::MoveIn),
            );
        }
        v.push(PopupItem {
            label: "移入文件夹".to_string(),
            cmd: None,
            danger: false,
            checked: false,
            disabled: true,
            separator: false,
            icon: MenuIcon::Folder,
            slider: None,
        });
        v.extend(targets);
        v.push(
            PopupItem::action("选择文件夹…", Cmd::ChooseMoveFolder(paths.clone()))
                .icon(MenuIcon::Folder),
        );
        v.push(PopupItem::sep());

        // Classification is metadata for collection items only. A desktop item
        // is first *collected*, and only then filed under a category.
        let entry = if any_nested {
            None
        } else {
            self.snap
                .items
                .iter()
                .find(|e| &e.path == path)
                .or_else(|| self.desk_items.iter().find(|e| &e.path == path))
        };
        if let Some(e) = entry {
            let mut assign = Vec::new();
            for c in &self.cfg.categories {
                if c.key != e.category {
                    let cmd = if in_library {
                        Cmd::Assign(paths.clone(), c.key.clone())
                    } else {
                        Cmd::Collect(paths.clone(), Some(c.key.clone()))
                    };
                    assign.push(
                        PopupItem::action(format!("归入「{}」", c.label), cmd)
                            .icon(MenuIcon::Tag),
                    );
                }
            }
            if !assign.is_empty() {
                v.push(PopupItem {
                    label: if in_library {
                        "移动到分类".to_string()
                    } else {
                        "收纳到分类".to_string()
                    },
                    cmd: None,
                    danger: false,
                    checked: false,
                    disabled: true,
                    separator: false,
                    icon: MenuIcon::Tag,
                    slider: None,
                });
                v.extend(assign);
            }
            if in_library && e.manual {
                v.push(
                    PopupItem::action("清除手动分类", Cmd::ClearOverrides(paths.clone()))
                        .icon(MenuIcon::Reclassify),
                );
            }
            v.push(PopupItem::sep());
        }

        if in_library {
            v.push(
                PopupItem::action("放回桌面", Cmd::ReleaseToDesktop(paths.clone(), None))
                    .icon(MenuIcon::Release),
            );
        }
        v.push(
            PopupItem::danger(
                if multi {
                    format!("删除选中 {} 项到回收站", paths.len())
                } else {
                    "删除到回收站".to_string()
                },
                Cmd::AskDelete(paths.clone()),
            )
            .icon(MenuIcon::Trash),
        );
        v.push(PopupItem::sep());
        v.push(PopupItem::action("取消选择", Cmd::ClearSelection).icon(MenuIcon::None));
        v.push(PopupItem::action("打开桌面目录", Cmd::OpenDesktop).icon(MenuIcon::Folder));
        v
    }

    /// Right-click on empty list space. Previously it did nothing, which felt
    /// broken once the list could be dense.
    pub fn blank_items(&self) -> Vec<PopupItem> {
        vec![
            PopupItem::action("刷新", Cmd::Refresh).icon(MenuIcon::Refresh),
            PopupItem::action("新建文件夹…", Cmd::NewFolderDialog(None)).icon(MenuIcon::Plus),
            PopupItem::action("自动收纳桌面", Cmd::AskCollectDesktop).icon(MenuIcon::MoveIn),
            PopupItem::sep(),
            PopupItem::action("全选", Cmd::SelectAll).icon(MenuIcon::Check),
            PopupItem::action("打开收纳库", Cmd::OpenLibrary).icon(MenuIcon::Folder),
            PopupItem::action("打开桌面目录", Cmd::OpenDesktop).icon(MenuIcon::Folder),
        ]
    }

    pub fn sort_items(&self) -> Vec<PopupItem> {
        [
            SortKey::Name,
            SortKey::Modified,
            SortKey::Size,
            SortKey::Type,
        ]
        .iter()
        .map(|k| {
            let icon = match k {
                SortKey::Name => MenuIcon::TextName,
                SortKey::Modified => MenuIcon::Clock,
                SortKey::Size => MenuIcon::SortSize,
                SortKey::Type => MenuIcon::FileType,
            };
            PopupItem::action(k.label().to_string(), Cmd::SetSort(*k))
                .checked(self.cfg.sort_by == *k)
                .icon(icon)
        })
        .chain(std::iter::once(
            PopupItem::action(
                if self.cfg.sort_desc {
                    "当前：降序 · 点此改为升序"
                } else {
                    "当前：升序 · 点此改为降序"
                },
                Cmd::SetSort(self.cfg.sort_by),
            )
            .icon(MenuIcon::ArrowUpDown),
        ))
        .collect()
    }

    pub fn more_items(&self) -> Vec<PopupItem> {
        let mut items = vec![
            PopupItem::action("刷新", Cmd::Refresh).icon(MenuIcon::Refresh),
            PopupItem::action("打开收纳库", Cmd::OpenLibrary).icon(MenuIcon::Folder),
            PopupItem::action("自动收纳桌面", Cmd::AskCollectDesktop).icon(MenuIcon::MoveIn),
            PopupItem::action(
                if self.cfg.show_hidden {
                    "隐藏系统隐藏项目"
                } else {
                    "显示隐藏项目"
                },
                Cmd::ToggleHidden,
            )
            .icon(if self.cfg.show_hidden {
                MenuIcon::EyeOff
            } else {
                MenuIcon::Eye
            }),
            PopupItem::action(
                if self.cfg.single_click_open {
                    "改为双击打开"
                } else {
                    "改为单击打开"
                },
                Cmd::ToggleSingleClick,
            )
            .icon(MenuIcon::Cursor),
            PopupItem::sep(),
            PopupItem::action("打开后自动收起", Cmd::ToggleCloseAfterOpen)
                .checked(self.cfg.close_after_open)
                .icon(MenuIcon::Open),
            PopupItem::action("点击外部后收起", Cmd::ToggleCloseOnOutside)
                .checked(self.cfg.close_on_outside)
                .icon(MenuIcon::Outside),
            PopupItem::action("压克力背景", Cmd::ToggleAcrylic)
                .checked(self.cfg.acrylic)
                .icon(MenuIcon::Acrylic),
        ];
        if self.cfg.acrylic {
            // 120..255 maps onto the whole track; the tint never goes fully
            // clear or the list text stops being readable.
            let v = ((self.cfg.acrylic_alpha.clamp(120, 255) - 120) as f32) / 135.0;
            items.push(PopupItem::slider("透明度", v, MenuIcon::None));
        }
        items.push(PopupItem::sep());
        items.push(PopupItem::action("新建分类", Cmd::NewCategoryDialog).icon(MenuIcon::Plus));
        items.push(PopupItem::action("平台设置", Cmd::OpenSettings).icon(MenuIcon::Gear));
        items.push(PopupItem::action("关于收纳盒", Cmd::AboutDialog).icon(MenuIcon::Info));
        items
    }
}

/// Cheap change token for `organizer.toml`.
pub fn config_stamp() -> Option<(u64, u128)> {
    let m = std::fs::metadata(Config::path()).ok()?;
    let t = m.modified().ok()?;
    Some((m.len(), t.duration_since(UNIX_EPOCH).ok()?.as_millis()))
}
