use crate::model::{Collection, Group, Param, Tab, Vault, VaultItem};
use crate::portable::{self, ExportPayload};
use crate::storage;
use eframe::egui;
use egui::{
    Align, Color32, CursorIcon, FontId, Layout, Rect, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Vec2,
};
use std::path::PathBuf;
use uuid::Uuid;

const SAVE_DEBOUNCE_FRAMES: u32 = 30; // ~0.5s at 60fps after last edit
const UNDO_CAP: usize = 100;
const COLOR_DANGER: Color32 = Color32::from_rgb(220, 80, 80);

// Use only glyphs that egui's bundled fonts render reliably.
const ICON_DRAG: &str = "::";
const ICON_DELETE: &str = "x";
const ICON_CLOSE: &str = "x";
const LABEL_ADD: &str = "+ Add";
const LABEL_ADD_MENU: &str = "+ Add...";
const LABEL_ADD_TEXT: &str = "+ Text";
const LABEL_ADD_KV: &str = "+ Key/Value";
const LABEL_ADD_REPLACE: &str = "+ Replace Text";
const LABEL_ADD_NOTE: &str = "+ Note";
const LABEL_ADD_GROUP: &str = "+ Add Group";
const LABEL_ADD_TAB: &str = "+ Tab";
const LABEL_COPY: &str = "Copy";
const LABEL_EDIT: &str = "Edit";
const LABEL_UPDATE: &str = "Update";
const LABEL_DETECT: &str = "Detect params";

#[derive(Debug, Clone)]
enum ConfirmAction {
    DeleteCollection(Uuid),
    DeleteTab { collection: Uuid, tab: Uuid },
    DeleteGroup { collection: Uuid, tab: Uuid, group: Uuid },
    DeleteItem { collection: Uuid, tab: Uuid, group: Uuid, item: Uuid },
    Restore,
}

#[derive(Debug, Clone)]
enum PasswordPurpose {
    ExportCollection { id: Uuid, path: PathBuf },
    BackupAll { path: PathBuf },
    DecryptImport { path: PathBuf },
    DecryptRestore { path: PathBuf },
}

#[derive(Debug, Clone)]
enum Dialog {
    Confirm {
        title: String,
        message: String,
        action: ConfirmAction,
    },
    Password {
        title: String,
        message: String,
        buf: String,
        purpose: PasswordPurpose,
        confirming: bool, // request password twice for new files
        confirm_buf: String,
        focus: bool,
    },
    Info {
        title: String,
        message: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RenameTarget {
    Collection(Uuid),
    Tab(Uuid),
    Group(Uuid),
    ItemLabel(Uuid),
    ItemKeyLabel(Uuid),
    ItemValueLabel(Uuid),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    Collection,
    Tab,
    Group,
    Item,
}

#[derive(Debug, Clone)]
struct DragState {
    kind: DragKind,
    src_id: Uuid,
    group_id: Option<Uuid>, // for Item only — restricts drops to same group
}

pub struct App {
    pub vault: Vault,
    pub active_collection_id: Option<Uuid>,
    pub active_tab_id: Option<Uuid>,

    rename: Option<RenameTarget>,
    rename_buf: String,
    rename_focus: bool,

    drag: Option<DragState>,

    dialog: Option<Dialog>,
    pending_restore: Option<Vault>,

    undo_stack: Vec<Vault>,
    redo_stack: Vec<Vault>,

    md_cache: egui_commonmark::CommonMarkCache,

    dirty_frames: u32,
    error: Option<String>,
    toast: Option<(String, std::time::Instant)>,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut vault = storage::load();
        if vault.collections.is_empty() {
            vault.collections.push(Collection::new("Default"));
        }
        let active_collection_id = vault.collections.first().map(|c| c.id);
        let active_tab_id = vault
            .collections
            .first()
            .and_then(|c| c.tabs.first().map(|t| t.id));

        Self {
            vault,
            active_collection_id,
            active_tab_id,
            rename: None,
            rename_buf: String::new(),
            rename_focus: false,
            drag: None,
            dialog: None,
            pending_restore: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            md_cache: egui_commonmark::CommonMarkCache::default(),
            dirty_frames: 0,
            error: None,
            toast: None,
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty_frames = 1;
    }

    /// Push current vault state onto the undo stack. Call this BEFORE a structural change
    /// (add/delete/reorder/update-commit/hide-flag/color/param add-or-delete).
    fn snapshot(&mut self) {
        if self.undo_stack.len() >= UNDO_CAP {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.vault.clone());
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            let cur = std::mem::replace(&mut self.vault, prev);
            self.redo_stack.push(cur);
            self.repair_active_ids();
            self.set_toast("Undo");
            self.mark_dirty();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            let cur = std::mem::replace(&mut self.vault, next);
            self.undo_stack.push(cur);
            self.repair_active_ids();
            self.set_toast("Redo");
            self.mark_dirty();
        }
    }

    fn repair_active_ids(&mut self) {
        // Active collection/tab might no longer exist — fall back to the first available.
        let collection_ok = self
            .active_collection_id
            .is_some_and(|id| self.vault.collections.iter().any(|c| c.id == id));
        if !collection_ok {
            self.active_collection_id = self.vault.collections.first().map(|c| c.id);
        }
        let tab_ok = self.active_collection_id.is_some_and(|cid| {
            self.vault
                .collections
                .iter()
                .find(|c| c.id == cid)
                .is_some_and(|c| {
                    self.active_tab_id
                        .is_some_and(|tid| c.tabs.iter().any(|t| t.id == tid))
                })
        });
        if !tab_ok {
            self.active_tab_id = self
                .active_collection_id
                .and_then(|cid| self.vault.collections.iter().find(|c| c.id == cid))
                .and_then(|c| c.tabs.first().map(|t| t.id));
        }
    }

    fn maybe_save(&mut self) {
        if self.dirty_frames == 0 {
            return;
        }
        self.dirty_frames += 1;
        if self.dirty_frames > SAVE_DEBOUNCE_FRAMES {
            if let Err(e) = storage::save(&self.vault) {
                self.error = Some(format!("Save failed: {e}"));
            }
            self.dirty_frames = 0;
        }
    }

    fn flush_save(&mut self) {
        if self.dirty_frames > 0 {
            if let Err(e) = storage::save(&self.vault) {
                self.error = Some(format!("Save failed: {e}"));
            }
            self.dirty_frames = 0;
        }
    }

    fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
    }

    fn copy(&mut self, ctx: &egui::Context, text: &str, what: &str) {
        ctx.copy_text(text.to_owned());
        self.set_toast(format!("Copied {what}"));
    }

    fn active_collection_idx(&self) -> Option<usize> {
        let id = self.active_collection_id?;
        self.vault.collections.iter().position(|c| c.id == id)
    }

    fn active_tab_idx(&self) -> Option<(usize, usize)> {
        let ci = self.active_collection_idx()?;
        let tid = self.active_tab_id?;
        let ti = self.vault.collections[ci]
            .tabs
            .iter()
            .position(|t| t.id == tid)?;
        Some((ci, ti))
    }

    /// Drag handle widget. Allocates a fixed-size hit-area painted with "::" so drag input is
    /// reliably detected (Label::sense() can be flaky on small text-only widgets).
    fn drag_handle(
        &mut self,
        ui: &mut egui::Ui,
        kind: DragKind,
        row_id: Uuid,
        group_id: Option<Uuid>,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(18.0, 22.0), Sense::click_and_drag());
        let painter = ui.painter();
        let color = if resp.hovered() || resp.dragged() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            ICON_DRAG,
            FontId::monospace(15.0),
            color,
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            if self.drag.is_none() {
                self.drag = Some(DragState {
                    kind,
                    src_id: row_id,
                    group_id,
                });
            }
        }
        resp
    }

    /// Drop-target hit test for a given row.
    /// Purely geometric — does NOT install an interact, so inner buttons/labels still receive clicks.
    fn dnd_drop_check(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        kind: DragKind,
        row_id: Uuid,
        group_id: Option<Uuid>,
        idx: usize,
        swap: &mut Option<(usize, usize)>,
    ) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        if drag.kind != kind || drag.group_id != group_id {
            return;
        }

        // Highlight the row currently being dragged
        if drag.src_id == row_id {
            let painter = ui.painter();
            painter.rect_stroke(
                rect.expand(2.0),
                4.0,
                Stroke::new(1.5, ui.visuals().selection.stroke.color),
            );
            return;
        }

        // pointer_interact_pos works during an active drag; pointer_hover_pos returns None
        // once the press is held outside a hover-sensing widget on some platforms.
        let pointer = ui
            .ctx()
            .input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos()));
        let hovered = pointer.is_some_and(|p| rect.contains(p));
        if !hovered {
            return;
        }

        // Draw drop indicator across the top edge of the target row
        let painter = ui.painter();
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(2.5, ui.visuals().selection.stroke.color),
        );

        if ui.input(|i| i.pointer.any_released()) {
            if let Some(s) = self.resolve_index(drag.kind, drag.src_id, group_id) {
                self.snapshot();
                *swap = Some((s, idx));
            }
            self.drag = None;
        }
    }

    fn resolve_index(&self, kind: DragKind, id: Uuid, group_id: Option<Uuid>) -> Option<usize> {
        match kind {
            DragKind::Collection => self.vault.collections.iter().position(|c| c.id == id),
            DragKind::Tab => {
                let ci = self.active_collection_idx()?;
                self.vault.collections[ci]
                    .tabs
                    .iter()
                    .position(|t| t.id == id)
            }
            DragKind::Group => {
                let (ci, ti) = self.active_tab_idx()?;
                self.vault.collections[ci].tabs[ti]
                    .groups
                    .iter()
                    .position(|g| g.id == id)
            }
            DragKind::Item => {
                let (ci, ti) = self.active_tab_idx()?;
                let gid = group_id?;
                let gi = self.vault.collections[ci].tabs[ti]
                    .groups
                    .iter()
                    .position(|g| g.id == gid)?;
                self.vault.collections[ci].tabs[ti].groups[gi]
                    .items
                    .iter()
                    .position(|x| x.id() == id)
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some((_, t)) = &self.toast {
            if t.elapsed().as_millis() > 1500 {
                self.toast = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }

        // NOTE: drag cleanup is intentionally at the END of update() so dnd_drop_check
        // has a chance to observe pointer.any_released() while self.drag is still Some.

        // Global undo/redo shortcuts (Ctrl+Z, Ctrl+Y, Ctrl+Shift+Z).
        // Only fire when no TextEdit is focused, so it doesn't fight with text-edit's own undo.
        let text_edit_focused = ctx.memory(|m| m.focused()).is_some();
        if !text_edit_focused {
            let (undo, redo) = ctx.input(|i| {
                let cmd = i.modifiers.command || i.modifiers.ctrl;
                let z = i.key_pressed(egui::Key::Z);
                let y = i.key_pressed(egui::Key::Y);
                let shift = i.modifiers.shift;
                let undo = cmd && z && !shift;
                let redo = (cmd && y) || (cmd && z && shift);
                (undo, redo)
            });
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
        }

        self.draw_menubar(ctx);
        self.draw_sidebar(ctx);
        self.draw_main(ctx);
        self.draw_status(ctx);
        self.draw_dialog(ctx);

        // Safety net: if a drag ends without ever passing over a valid drop target
        // (e.g. user drags out of the window then releases), clear it here.
        if self.drag.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            self.drag = None;
        }

        self.maybe_save();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush_save();
    }
}

// ---------- Sidebar (collections) ----------
impl App {
    fn draw_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("collections")
            .resizable(true)
            .default_width(260.0)
            .min_width(220.0)
            .max_width(420.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading("Collections");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(LABEL_ADD).on_hover_text("Add collection").clicked() {
                            self.snapshot();
                            let c = Collection::new(format!(
                                "Collection {}",
                                self.vault.collections.len() + 1
                            ));
                            self.active_collection_id = Some(c.id);
                            self.active_tab_id = c.tabs.first().map(|t| t.id);
                            self.vault.collections.push(c);
                            self.mark_dirty();
                        }
                    });
                });
                ui.separator();

                let mut delete: Option<Uuid> = None;
                let mut select: Option<Uuid> = None;
                let mut swap: Option<(usize, usize)> = None;
                let mut export: Option<Uuid> = None;

                ScrollArea::vertical().show(ui, |ui| {
                    let n = self.vault.collections.len();
                    for i in 0..n {
                        let cid = self.vault.collections[i].id;
                        let active = self.active_collection_id == Some(cid);
                        let rect = self.draw_collection_row(
                            ui,
                            i,
                            active,
                            &mut delete,
                            &mut select,
                            &mut export,
                        );
                        self.dnd_drop_check(
                            ui,
                            rect,
                            DragKind::Collection,
                            cid,
                            None,
                            i,
                            &mut swap,
                        );
                    }
                });

                if let Some((from, to)) = swap {
                    if from < self.vault.collections.len()
                        && to < self.vault.collections.len()
                        && from != to
                    {
                        let item = self.vault.collections.remove(from);
                        self.vault.collections.insert(to, item);
                        self.mark_dirty();
                    }
                }
                if let Some(id) = select {
                    self.active_collection_id = Some(id);
                    let tab_id = self
                        .vault
                        .collections
                        .iter()
                        .find(|c| c.id == id)
                        .and_then(|c| c.tabs.first().map(|t| t.id));
                    self.active_tab_id = tab_id;
                }
                if let Some(id) = delete {
                    self.request_delete_collection(id);
                }
                if let Some(id) = export {
                    self.action_export_collection(id);
                }
            });
    }

    fn request_delete_collection(&mut self, id: Uuid) {
        let Some(c) = self.vault.collections.iter().find(|c| c.id == id) else { return };
        if !c.has_content() {
            self.run_confirmed_action(ConfirmAction::DeleteCollection(id));
            return;
        }
        self.dialog = Some(Dialog::Confirm {
            title: "Delete collection?".into(),
            message: format!(
                "Collection \"{}\" still contains data. Deleting will remove all of its tabs, \
                 groups, and items. Continue?",
                c.name
            ),
            action: ConfirmAction::DeleteCollection(id),
        });
    }

    fn request_delete_tab(&mut self, collection: Uuid, tab: Uuid) {
        let has_content = self
            .vault
            .collections
            .iter()
            .find(|c| c.id == collection)
            .and_then(|c| c.tabs.iter().find(|t| t.id == tab))
            .is_some_and(|t| t.has_content());
        let name = self
            .vault
            .collections
            .iter()
            .find(|c| c.id == collection)
            .and_then(|c| c.tabs.iter().find(|t| t.id == tab))
            .map(|t| t.name.clone())
            .unwrap_or_default();
        if !has_content {
            self.run_confirmed_action(ConfirmAction::DeleteTab { collection, tab });
            return;
        }
        self.dialog = Some(Dialog::Confirm {
            title: "Delete tab?".into(),
            message: format!(
                "Tab \"{name}\" still contains data. Deleting will remove all of its groups and \
                 items. Continue?"
            ),
            action: ConfirmAction::DeleteTab { collection, tab },
        });
    }

    fn request_delete_item(&mut self, collection: Uuid, tab: Uuid, group: Uuid, item: Uuid) {
        let entry = self
            .vault
            .collections
            .iter()
            .find(|c| c.id == collection)
            .and_then(|c| c.tabs.iter().find(|t| t.id == tab))
            .and_then(|t| t.groups.iter().find(|g| g.id == group))
            .and_then(|g| g.items.iter().find(|x| x.id() == item));
        let Some(it) = entry else { return };
        if !it.has_content() {
            self.run_confirmed_action(ConfirmAction::DeleteItem { collection, tab, group, item });
            return;
        }
        let label = match it {
            VaultItem::Text { label, .. }
            | VaultItem::KeyValue { label, .. }
            | VaultItem::ReplaceText { label, .. }
            | VaultItem::Note { label, .. } => label.clone(),
        };
        self.dialog = Some(Dialog::Confirm {
            title: "Delete item?".into(),
            message: format!("\"{label}\" still has content stored. Delete it anyway?"),
            action: ConfirmAction::DeleteItem { collection, tab, group, item },
        });
    }

    fn request_delete_group(&mut self, collection: Uuid, tab: Uuid, group: Uuid) {
        let g = self
            .vault
            .collections
            .iter()
            .find(|c| c.id == collection)
            .and_then(|c| c.tabs.iter().find(|t| t.id == tab))
            .and_then(|t| t.groups.iter().find(|g| g.id == group));
        let (has_content, name) = match g {
            Some(g) => (g.has_content(), g.name.clone()),
            None => return,
        };
        if !has_content {
            self.run_confirmed_action(ConfirmAction::DeleteGroup { collection, tab, group });
            return;
        }
        self.dialog = Some(Dialog::Confirm {
            title: "Delete group?".into(),
            message: format!(
                "Group \"{name}\" still contains items. Deleting will remove all of its items. \
                 Continue?"
            ),
            action: ConfirmAction::DeleteGroup { collection, tab, group },
        });
    }

    fn draw_collection_row(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        active: bool,
        delete: &mut Option<Uuid>,
        select: &mut Option<Uuid>,
        export: &mut Option<Uuid>,
    ) -> Rect {
        let cid = self.vault.collections[idx].id;
        let bg = if active {
            ui.visuals().selection.bg_fill
        } else {
            Color32::TRANSPARENT
        };

        let frame = egui::Frame::none()
            .fill(bg)
            .rounding(4.0)
            .inner_margin(egui::Margin::symmetric(6.0, 4.0));

        let resp = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let dh = self.drag_handle(ui, DragKind::Collection, cid, None);
                    if dh.clicked() {
                        *select = Some(cid);
                    }

                    // Anchor right-side buttons first (right-to-left), then put the label
                    // in the actual remaining space. This avoids any "reserve N pixels"
                    // guesswork that breaks when fonts/themes change button widths.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(RichText::new(ICON_DELETE).color(COLOR_DANGER))
                            .on_hover_text("Delete collection")
                            .clicked()
                        {
                            *delete = Some(cid);
                        }
                        ui.menu_button("...", |ui| {
                            if ui.button("Export collection...").clicked() {
                                *export = Some(cid);
                                ui.close_menu();
                            }
                        })
                        .response
                        .on_hover_text("More actions");

                        // Remaining horizontal room → label/edit fills it left-aligned.
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let renaming =
                                self.rename == Some(RenameTarget::Collection(cid));
                            if renaming {
                                let edit = ui.add(
                                    TextEdit::singleline(&mut self.rename_buf)
                                        .desired_width(ui.available_width()),
                                );
                                if self.rename_focus {
                                    edit.request_focus();
                                    self.rename_focus = false;
                                }
                                if edit.lost_focus()
                                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    self.vault.collections[idx].name =
                                        std::mem::take(&mut self.rename_buf);
                                    self.rename = None;
                                    self.mark_dirty();
                                }
                            } else {
                                let name = self.vault.collections[idx].name.clone();
                                let lbl = ui.add(
                                    egui::Label::new(RichText::new(name).strong())
                                        .truncate()
                                        .sense(Sense::click()),
                                );
                                if lbl.clicked() {
                                    *select = Some(cid);
                                }
                                if lbl.double_clicked() {
                                    self.rename = Some(RenameTarget::Collection(cid));
                                    self.rename_buf =
                                        self.vault.collections[idx].name.clone();
                                    self.rename_focus = true;
                                }

                                // Fill remaining width with an invisible click area so
                                // the empty part of the row also selects the collection.
                                let avail = ui.available_size_before_wrap();
                                if avail.x > 0.0 {
                                    let h = ui.spacing().interact_size.y.max(avail.y);
                                    let (_, fill_resp) = ui.allocate_exact_size(
                                        Vec2::new(avail.x, h),
                                        Sense::click(),
                                    );
                                    if fill_resp.clicked() {
                                        *select = Some(cid);
                                    }
                                }
                            }
                        });
                    });
                });
            })
            .response;

        resp.rect
    }
}

// ---------- Main panel (tabs + groups) ----------
impl App {
    fn draw_main(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_tabs(ui);
            ui.separator();
            self.draw_groups(ui);
        });
    }

    fn draw_tabs(&mut self, ui: &mut egui::Ui) {
        let Some(ci) = self.active_collection_idx() else {
            ui.label("No collection. Click + Add to create one.");
            return;
        };

        let mut delete: Option<Uuid> = None;
        let mut select: Option<Uuid> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mut add_tab = false;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let n = self.vault.collections[ci].tabs.len();
            for i in 0..n {
                let tid = self.vault.collections[ci].tabs[i].id;
                let active = self.active_tab_id == Some(tid);
                let rect =
                    self.draw_tab_chip(ui, ci, i, active, &mut delete, &mut select);
                self.dnd_drop_check(ui, rect, DragKind::Tab, tid, None, i, &mut swap);
            }
            if ui.button(LABEL_ADD_TAB).clicked() {
                add_tab = true;
            }
        });

        if let Some((from, to)) = swap {
            let tabs = &mut self.vault.collections[ci].tabs;
            if from < tabs.len() && to < tabs.len() && from != to {
                let t = tabs.remove(from);
                tabs.insert(to, t);
                self.mark_dirty();
            }
        }
        if let Some(id) = select {
            self.active_tab_id = Some(id);
        }
        if let Some(id) = delete {
            let collection_id = self.vault.collections[ci].id;
            self.request_delete_tab(collection_id, id);
        }
        if add_tab {
            self.snapshot();
            let t = Tab::new(format!("Tab {}", self.vault.collections[ci].tabs.len() + 1));
            self.active_tab_id = Some(t.id);
            self.vault.collections[ci].tabs.push(t);
            self.mark_dirty();
        }
    }

    fn draw_tab_chip(
        &mut self,
        ui: &mut egui::Ui,
        ci: usize,
        ti: usize,
        active: bool,
        delete: &mut Option<Uuid>,
        select: &mut Option<Uuid>,
    ) -> Rect {
        let tid = self.vault.collections[ci].tabs[ti].id;
        let bg = if active {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        let frame = egui::Frame::none()
            .fill(bg)
            .rounding(6.0)
            .stroke(Stroke::new(
                1.0,
                if active {
                    ui.visuals().selection.stroke.color
                } else {
                    ui.visuals().widgets.inactive.bg_stroke.color
                },
            ))
            .inner_margin(egui::Margin::symmetric(8.0, 4.0));

        let resp = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let dh = self.drag_handle(ui, DragKind::Tab, tid, None);
                    if dh.clicked() {
                        *select = Some(tid);
                    }

                    let renaming = self.rename == Some(RenameTarget::Tab(tid));
                    if renaming {
                        let edit = ui.add(
                            TextEdit::singleline(&mut self.rename_buf).desired_width(140.0),
                        );
                        if self.rename_focus {
                            edit.request_focus();
                            self.rename_focus = false;
                        }
                        if edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.vault.collections[ci].tabs[ti].name =
                                std::mem::take(&mut self.rename_buf);
                            self.rename = None;
                            self.mark_dirty();
                        }
                    } else {
                        let name = self.vault.collections[ci].tabs[ti].name.clone();
                        let lbl = ui.add(
                            egui::Label::new(RichText::new(name))
                                .truncate()
                                .sense(Sense::click()),
                        );
                        if lbl.clicked() {
                            *select = Some(tid);
                        }
                        if lbl.double_clicked() {
                            self.rename = Some(RenameTarget::Tab(tid));
                            self.rename_buf = self.vault.collections[ci].tabs[ti].name.clone();
                            self.rename_focus = true;
                        }
                    }
                    if ui
                        .small_button(RichText::new(ICON_CLOSE).color(COLOR_DANGER))
                        .on_hover_text("Close tab")
                        .clicked()
                    {
                        *delete = Some(tid);
                    }
                });
            })
            .response;

        resp.rect
    }

    fn draw_groups(&mut self, ui: &mut egui::Ui) {
        let Some((ci, ti)) = self.active_tab_idx() else {
            ui.label("No tab selected.");
            return;
        };

        let mut delete_group: Option<Uuid> = None;
        let mut swap_groups: Option<(usize, usize)> = None;
        let mut add_group = false;

        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let n = self.vault.collections[ci].tabs[ti].groups.len();
                for gi in 0..n {
                    let gid = self.vault.collections[ci].tabs[ti].groups[gi].id;
                    let rect = self.draw_group(ui, ci, ti, gi, &mut delete_group);
                    self.dnd_drop_check(
                        ui,
                        rect,
                        DragKind::Group,
                        gid,
                        None,
                        gi,
                        &mut swap_groups,
                    );
                    ui.add_space(8.0);
                }
                ui.add_space(4.0);
                if ui
                    .add(egui::Button::new(RichText::new(LABEL_ADD_GROUP).size(14.0)))
                    .clicked()
                {
                    add_group = true;
                }
                ui.add_space(40.0);
            });

        if let Some((from, to)) = swap_groups {
            let groups = &mut self.vault.collections[ci].tabs[ti].groups;
            if from < groups.len() && to < groups.len() && from != to {
                let g = groups.remove(from);
                groups.insert(to, g);
                self.mark_dirty();
            }
        }
        if let Some(id) = delete_group {
            let collection_id = self.vault.collections[ci].id;
            let tab_id = self.vault.collections[ci].tabs[ti].id;
            self.request_delete_group(collection_id, tab_id, id);
        }
        if add_group {
            self.snapshot();
            let g = Group::new(format!(
                "Group {}",
                self.vault.collections[ci].tabs[ti].groups.len() + 1
            ));
            self.vault.collections[ci].tabs[ti].groups.push(g);
            self.mark_dirty();
        }
    }

    fn draw_group(
        &mut self,
        ui: &mut egui::Ui,
        ci: usize,
        ti: usize,
        gi: usize,
        delete_group: &mut Option<Uuid>,
    ) -> Rect {
        let gid = self.vault.collections[ci].tabs[ti].groups[gi].id;

        let frame = egui::Frame::group(ui.style())
            .rounding(8.0)
            .inner_margin(egui::Margin::same(10.0))
            .stroke(Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ));

        let resp = frame
            .show(ui, |ui| {
                // Header row
                ui.horizontal(|ui| {
                    self.drag_handle(ui, DragKind::Group, gid, None);

                    // Group color tag — small filled square + click opens color picker
                    let mut color_arr = self.vault.collections[ci].tabs[ti].groups[gi].color;
                    let mut color32 =
                        Color32::from_rgb(color_arr[0], color_arr[1], color_arr[2]);
                    let color_resp = ui.color_edit_button_srgba(&mut color32);
                    if color_resp.changed() {
                        self.snapshot();
                        color_arr = [color32.r(), color32.g(), color32.b()];
                        self.vault.collections[ci].tabs[ti].groups[gi].color = color_arr;
                        self.mark_dirty();
                    }

                    let renaming = self.rename == Some(RenameTarget::Group(gid));
                    if renaming {
                        let edit = ui.add(
                            TextEdit::singleline(&mut self.rename_buf).desired_width(260.0),
                        );
                        if self.rename_focus {
                            edit.request_focus();
                            self.rename_focus = false;
                        }
                        if edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.vault.collections[ci].tabs[ti].groups[gi].name =
                                std::mem::take(&mut self.rename_buf);
                            self.rename = None;
                            self.mark_dirty();
                        }
                    } else {
                        let name = self.vault.collections[ci].tabs[ti].groups[gi].name.clone();
                        let lbl = ui.add(
                            egui::Label::new(RichText::new(name).heading())
                                .sense(Sense::click()),
                        );
                        if lbl.double_clicked() {
                            self.rename = Some(RenameTarget::Group(gid));
                            self.rename_buf =
                                self.vault.collections[ci].tabs[ti].groups[gi].name.clone();
                            self.rename_focus = true;
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(RichText::new(ICON_DELETE).color(COLOR_DANGER))
                            .on_hover_text("Delete group")
                            .clicked()
                        {
                            *delete_group = Some(gid);
                        }
                        // Single combined dropdown for all "add ..." options
                        let mut add_kind: Option<u8> = None;
                        ui.menu_button(LABEL_ADD_MENU, |ui| {
                            ui.set_min_width(200.0);
                            if ui.button(LABEL_ADD_TEXT).clicked() {
                                add_kind = Some(0);
                                ui.close_menu();
                            }
                            if ui.button(LABEL_ADD_KV).clicked() {
                                add_kind = Some(1);
                                ui.close_menu();
                            }
                            if ui.button(LABEL_ADD_REPLACE).clicked() {
                                add_kind = Some(2);
                                ui.close_menu();
                            }
                            if ui.button(LABEL_ADD_NOTE).clicked() {
                                add_kind = Some(3);
                                ui.close_menu();
                            }
                        });
                        if let Some(k) = add_kind {
                            self.snapshot();
                            let new_item = match k {
                                0 => VaultItem::new_text(),
                                1 => VaultItem::new_kv(),
                                2 => VaultItem::new_replace(),
                                _ => VaultItem::new_note(),
                            };
                            self.vault.collections[ci].tabs[ti].groups[gi]
                                .items
                                .push(new_item);
                            self.mark_dirty();
                        }
                    });
                });

                ui.add_space(6.0);

                // Items
                let mut delete_item: Option<Uuid> = None;
                let mut swap_items: Option<(usize, usize)> = None;
                let n = self.vault.collections[ci].tabs[ti].groups[gi].items.len();
                for ii in 0..n {
                    let iid = self.vault.collections[ci].tabs[ti].groups[gi].items[ii].id();
                    let rect = self.draw_item(ui, ci, ti, gi, ii, &mut delete_item);
                    self.dnd_drop_check(
                        ui,
                        rect,
                        DragKind::Item,
                        iid,
                        Some(gid),
                        ii,
                        &mut swap_items,
                    );
                    ui.add_space(4.0);
                }
                if let Some((from, to)) = swap_items {
                    let items = &mut self.vault.collections[ci].tabs[ti].groups[gi].items;
                    if from < items.len() && to < items.len() && from != to {
                        let it = items.remove(from);
                        items.insert(to, it);
                        self.mark_dirty();
                    }
                }
                if let Some(id) = delete_item {
                    let collection_id = self.vault.collections[ci].id;
                    let tab_id = self.vault.collections[ci].tabs[ti].id;
                    self.request_delete_item(collection_id, tab_id, gid, id);
                }
            })
            .response;

        resp.rect
    }

    fn draw_item(
        &mut self,
        ui: &mut egui::Ui,
        ci: usize,
        ti: usize,
        gi: usize,
        ii: usize,
        delete_item: &mut Option<Uuid>,
    ) -> Rect {
        let item_clone = self.vault.collections[ci].tabs[ti].groups[gi].items[ii].clone();
        let iid = item_clone.id();

        let frame = egui::Frame::none()
            .fill(ui.visuals().extreme_bg_color)
            .rounding(6.0)
            .inner_margin(egui::Margin::same(8.0))
            .stroke(Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ));

        let resp = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.drag_handle(ui, DragKind::Item, iid, Some(self.vault.collections[ci].tabs[ti].groups[gi].id));
                    self.draw_item_label(ui, ci, ti, gi, ii, RenameTarget::ItemLabel(iid));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(RichText::new(ICON_DELETE).color(COLOR_DANGER))
                            .on_hover_text("Delete item")
                            .clicked()
                        {
                            *delete_item = Some(iid);
                        }
                    });
                });
                ui.add_space(4.0);

                match item_clone {
                    VaultItem::Text { .. } => self.draw_text_body(ui, ci, ti, gi, ii),
                    VaultItem::KeyValue { .. } => self.draw_kv_body(ui, ci, ti, gi, ii),
                    VaultItem::ReplaceText { .. } => self.draw_replace_body(ui, ci, ti, gi, ii),
                    VaultItem::Note { .. } => self.draw_note_body(ui, ci, ti, gi, ii),
                }
            })
            .response;

        resp.rect
    }

    fn draw_item_label(
        &mut self,
        ui: &mut egui::Ui,
        ci: usize,
        ti: usize,
        gi: usize,
        ii: usize,
        target: RenameTarget,
    ) {
        let renaming = self.rename.as_ref() == Some(&target);
        let label_value = match &self.vault.collections[ci].tabs[ti].groups[gi].items[ii] {
            VaultItem::Text { label, .. }
            | VaultItem::KeyValue { label, .. }
            | VaultItem::ReplaceText { label, .. }
            | VaultItem::Note { label, .. } => label.clone(),
        };
        if renaming {
            let edit = ui.add(
                TextEdit::singleline(&mut self.rename_buf).desired_width(260.0),
            );
            if self.rename_focus {
                edit.request_focus();
                self.rename_focus = false;
            }
            if edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let new = std::mem::take(&mut self.rename_buf);
                match &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii] {
                    VaultItem::Text { label, .. }
                    | VaultItem::KeyValue { label, .. }
                    | VaultItem::ReplaceText { label, .. }
                    | VaultItem::Note { label, .. } => *label = new,
                }
                self.rename = None;
                self.mark_dirty();
            }
        } else {
            let lbl = ui.add(
                egui::Label::new(RichText::new(label_value).strong()).sense(Sense::click()),
            );
            if lbl.double_clicked() {
                self.rename = Some(target);
                self.rename_buf = match &self.vault.collections[ci].tabs[ti].groups[gi].items[ii] {
                    VaultItem::Text { label, .. }
                    | VaultItem::KeyValue { label, .. }
                    | VaultItem::ReplaceText { label, .. }
                    | VaultItem::Note { label, .. } => label.clone(),
                };
                self.rename_focus = true;
            }
        }
    }

    fn draw_text_body(&mut self, ui: &mut egui::Ui, ci: usize, ti: usize, gi: usize, ii: usize) {
        let (is_editing, value_snapshot, hidden_snapshot) = {
            let item = &self.vault.collections[ci].tabs[ti].groups[gi].items[ii];
            let VaultItem::Text { value, editing, hidden, .. } = item else { return; };
            (*editing, value.clone(), *hidden)
        };

        if is_editing {
            let mut buf = value_snapshot.clone();
            let resp = ui.add(
                TextEdit::multiline(&mut buf)
                    .desired_rows(3)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(13.0)),
            );
            if resp.changed() {
                if let VaultItem::Text { value, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *value = buf.clone();
                }
                self.mark_dirty();
            }
            let mut hide_now = hidden_snapshot;
            let mut update_clicked = false;
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut hide_now, "Hide text")
                    .on_hover_text("Mask the value with * after Update")
                    .changed()
                {
                    if let VaultItem::Text { hidden, .. } =
                        &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                    {
                        *hidden = hide_now;
                    }
                    self.mark_dirty();
                }
                if ui.button(LABEL_UPDATE).clicked() {
                    update_clicked = true;
                }
            });
            if update_clicked {
                if let VaultItem::Text { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = false;
                }
                self.mark_dirty();
            }
        } else {
            let display = if hidden_snapshot {
                mask(&value_snapshot)
            } else {
                value_snapshot.clone()
            };
            let mut copy_clicked = false;
            let mut edit_clicked = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_COPY).on_hover_text("Copy text").clicked() {
                    copy_clicked = true;
                }
                if ui.button(LABEL_EDIT).clicked() {
                    edit_clicked = true;
                }
                if hidden_snapshot {
                    ui.label(RichText::new("(hidden)").small().weak());
                }
            });
            ui.add(
                TextEdit::multiline(&mut display.as_str())
                    .desired_rows(display.lines().count().clamp(1, 6) as usize)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(13.0)),
            );
            if copy_clicked {
                self.copy(ui.ctx(), &value_snapshot, "text");
            }
            if edit_clicked {
                self.snapshot();
                if let VaultItem::Text { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = true;
                }
            }
        }
    }

    fn draw_kv_body(&mut self, ui: &mut egui::Ui, ci: usize, ti: usize, gi: usize, ii: usize) {
        let snapshot = self.vault.collections[ci].tabs[ti].groups[gi].items[ii].clone();
        let VaultItem::KeyValue {
            id: iid,
            key_label,
            value_label,
            key,
            value,
            editing,
            hide_key,
            hide_value,
            ..
        } = snapshot
        else {
            return;
        };

        if editing {
            ui.horizontal(|ui| {
                self.draw_inline_label(
                    ui,
                    RenameTarget::ItemKeyLabel(iid),
                    &key_label,
                    |slf, new| {
                        if let VaultItem::KeyValue { key_label, .. } =
                            &mut slf.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                        {
                            *key_label = new;
                        }
                    },
                );
            });
            let mut k = key.clone();
            let r1 = ui.add(
                TextEdit::singleline(&mut k)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(13.0)),
            );
            if r1.changed() {
                if let VaultItem::KeyValue { key, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *key = k.clone();
                }
                self.mark_dirty();
            }
            let mut hk_now = hide_key;
            if ui
                .checkbox(&mut hk_now, "Hide key")
                .on_hover_text("Mask the key with * after Update")
                .changed()
            {
                if let VaultItem::KeyValue { hide_key, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *hide_key = hk_now;
                }
                self.mark_dirty();
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                self.draw_inline_label(
                    ui,
                    RenameTarget::ItemValueLabel(iid),
                    &value_label,
                    |slf, new| {
                        if let VaultItem::KeyValue { value_label, .. } =
                            &mut slf.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                        {
                            *value_label = new;
                        }
                    },
                );
            });
            let mut v = value.clone();
            let r2 = ui.add(
                TextEdit::multiline(&mut v)
                    .desired_rows(2)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(13.0)),
            );
            if r2.changed() {
                if let VaultItem::KeyValue { value, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *value = v.clone();
                }
                self.mark_dirty();
            }
            let mut hv_now = hide_value;
            if ui
                .checkbox(&mut hv_now, "Hide value")
                .on_hover_text("Mask the value with * after Update")
                .changed()
            {
                if let VaultItem::KeyValue { hide_value, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *hide_value = hv_now;
                }
                self.mark_dirty();
            }

            let mut update_clicked = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_UPDATE).clicked() {
                    update_clicked = true;
                }
            });
            if update_clicked {
                if let VaultItem::KeyValue { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = false;
                }
                self.mark_dirty();
            }
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&key_label).small().weak());
                if hide_key {
                    ui.label(RichText::new("(hidden)").small().weak());
                }
            });
            let mut k_copy = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_COPY).on_hover_text("Copy key").clicked() {
                    k_copy = true;
                }
                let k_show = if hide_key { mask(&key) } else { key.clone() };
                ui.add(
                    TextEdit::singleline(&mut k_show.as_str())
                        .desired_width(ui.available_width())
                        .font(FontId::monospace(13.0)),
                );
            });
            if k_copy {
                self.copy(ui.ctx(), &key, &key_label);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&value_label).small().weak());
                if hide_value {
                    ui.label(RichText::new("(hidden)").small().weak());
                }
            });
            let mut v_copy = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_COPY).on_hover_text("Copy value").clicked() {
                    v_copy = true;
                }
                let v_show = if hide_value { mask(&value) } else { value.clone() };
                ui.add(
                    TextEdit::multiline(&mut v_show.as_str())
                        .desired_rows(v_show.lines().count().clamp(1, 4) as usize)
                        .desired_width(ui.available_width())
                        .font(FontId::monospace(13.0)),
                );
            });
            if v_copy {
                self.copy(ui.ctx(), &value, &value_label);
            }
            let mut edit_clicked = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_EDIT).clicked() {
                    edit_clicked = true;
                }
            });
            if edit_clicked {
                self.snapshot();
                if let VaultItem::KeyValue { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = true;
                }
            }
        }
    }

    fn draw_replace_body(
        &mut self,
        ui: &mut egui::Ui,
        ci: usize,
        ti: usize,
        gi: usize,
        ii: usize,
    ) {
        let snapshot = self.vault.collections[ci].tabs[ti].groups[gi].items[ii].clone();
        let VaultItem::ReplaceText {
            template,
            params,
            editing,
            hidden,
            ..
        } = snapshot
        else {
            return;
        };

        if editing {
            let mut t = template.clone();
            let r = ui.add(
                TextEdit::multiline(&mut t)
                    .desired_rows(2)
                    .desired_width(ui.available_width())
                    .hint_text("e.g.  http://{{base_url}}/api/{{path}}")
                    .font(FontId::monospace(13.0)),
            );
            if r.changed() {
                if let VaultItem::ReplaceText { template, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *template = t.clone();
                }
                self.mark_dirty();
            }

            ui.add_space(4.0);
            let mut detect = false;
            let mut add_param = false;
            ui.horizontal(|ui| {
                if ui
                    .small_button(LABEL_DETECT)
                    .on_hover_text("Add rows for {{name}} placeholders found in the template")
                    .clicked()
                {
                    detect = true;
                }
                if ui.small_button("+ Param").clicked() {
                    add_param = true;
                }
            });
            if detect {
                self.sync_replace_params(ci, ti, gi, ii);
            }
            if add_param {
                if let VaultItem::ReplaceText { params, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    params.push(Param {
                        name: String::new(),
                        value: String::new(),
                    });
                }
                self.mark_dirty();
            }

            let p_count = params.len();
            let mut delete_idx: Option<usize> = None;
            for pi in 0..p_count {
                ui.horizontal(|ui| {
                    let mut name = params[pi].name.clone();
                    let mut val = params[pi].value.clone();
                    ui.label("{{");
                    let r1 = ui.add(
                        TextEdit::singleline(&mut name)
                            .desired_width(120.0)
                            .hint_text("name")
                            .font(FontId::monospace(13.0)),
                    );
                    ui.label("}}  =");
                    let r2 = ui.add(
                        TextEdit::singleline(&mut val)
                            .desired_width(ui.available_width() - 60.0)
                            .hint_text("value")
                            .font(FontId::monospace(13.0)),
                    );
                    if r1.changed() || r2.changed() {
                        if let VaultItem::ReplaceText { params, .. } =
                            &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                        {
                            if pi < params.len() {
                                params[pi].name = name;
                                params[pi].value = val;
                            }
                        }
                        self.mark_dirty();
                    }
                    if ui.small_button(ICON_DELETE).clicked() {
                        delete_idx = Some(pi);
                    }
                });
            }
            if let Some(idx) = delete_idx {
                if let VaultItem::ReplaceText { params, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    if idx < params.len() {
                        params.remove(idx);
                    }
                }
                self.mark_dirty();
            }

            let mut update_clicked = false;
            let mut hide_now = hidden;
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut hide_now, "Hide formatted output")
                    .on_hover_text("Mask the rendered text with * after Update")
                    .changed()
                {
                    if let VaultItem::ReplaceText { hidden, .. } =
                        &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                    {
                        *hidden = hide_now;
                    }
                    self.mark_dirty();
                }
                if ui.button(LABEL_UPDATE).clicked() {
                    update_clicked = true;
                }
            });
            if update_clicked {
                self.sync_replace_params(ci, ti, gi, ii);
                if let VaultItem::ReplaceText { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = false;
                }
                self.mark_dirty();
            }
        } else {
            let formatted = format_template(&template, &params);
            let display = if hidden { mask(&formatted) } else { formatted.clone() };

            let mut copy_clicked = false;
            let mut edit_clicked = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_COPY).on_hover_text("Copy formatted text").clicked() {
                    copy_clicked = true;
                }
                if ui.button(LABEL_EDIT).clicked() {
                    edit_clicked = true;
                }
                if hidden {
                    ui.label(RichText::new("(hidden)").small().weak());
                }
            });
            ui.add(
                TextEdit::multiline(&mut display.as_str())
                    .desired_rows(display.lines().count().clamp(1, 4) as usize)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(13.0)),
            );
            ui.add_space(4.0);
            if !params.is_empty() {
                ui.label(RichText::new("Params").small().weak());
                let p_count = params.len();
                for pi in 0..p_count {
                    ui.horizontal(|ui| {
                        ui.label(format!("{{{{{}}}}}", params[pi].name));
                        ui.label("=");
                        let mut v = params[pi].value.clone();
                        let r = ui.add(
                            TextEdit::singleline(&mut v)
                                .desired_width(ui.available_width() - 60.0)
                                .font(FontId::monospace(13.0)),
                        );
                        if r.changed() {
                            if let VaultItem::ReplaceText { params, .. } =
                                &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                            {
                                if pi < params.len() {
                                    params[pi].value = v;
                                }
                            }
                            self.mark_dirty();
                        }
                    });
                }
            }
            if copy_clicked {
                self.copy(ui.ctx(), &formatted, "formatted text");
            }
            if edit_clicked {
                self.snapshot();
                if let VaultItem::ReplaceText { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = true;
                }
            }
        }
    }

    fn draw_note_body(&mut self, ui: &mut egui::Ui, ci: usize, ti: usize, gi: usize, ii: usize) {
        let (is_editing, body_snapshot, iid) = {
            let item = &self.vault.collections[ci].tabs[ti].groups[gi].items[ii];
            let VaultItem::Note { body, editing, id, .. } = item else { return; };
            (*editing, body.clone(), *id)
        };

        if is_editing {
            let mut buf = body_snapshot.clone();
            let resp = ui.add(
                TextEdit::multiline(&mut buf)
                    .desired_rows(6)
                    .desired_width(ui.available_width())
                    .hint_text("Markdown supported: # heading, **bold**, *italic*, `code`, lists, links")
                    .font(FontId::monospace(13.0)),
            );
            if resp.changed() {
                if let VaultItem::Note { body, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *body = buf.clone();
                }
                self.mark_dirty();
            }
            let mut update_clicked = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_UPDATE).clicked() {
                    update_clicked = true;
                }
                ui.label(RichText::new("(Markdown)").small().weak());
            });
            if update_clicked {
                if let VaultItem::Note { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = false;
                }
                self.mark_dirty();
            }
        } else {
            let mut copy_clicked = false;
            let mut edit_clicked = false;
            ui.horizontal(|ui| {
                if ui.button(LABEL_COPY).on_hover_text("Copy raw markdown").clicked() {
                    copy_clicked = true;
                }
                if ui.button(LABEL_EDIT).clicked() {
                    edit_clicked = true;
                }
            });
            // Render markdown preview
            egui::Frame::none()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::same(6.0))
                .rounding(4.0)
                .show(ui, |ui| {
                    if body_snapshot.trim().is_empty() {
                        ui.label(RichText::new("(empty note)").weak());
                    } else {
                        let _ = iid;
                        egui_commonmark::CommonMarkViewer::new()
                            .max_image_width(Some(512))
                            .show(ui, &mut self.md_cache, &body_snapshot);
                    }
                });
            if copy_clicked {
                self.copy(ui.ctx(), &body_snapshot, "note");
            }
            if edit_clicked {
                self.snapshot();
                if let VaultItem::Note { editing, .. } =
                    &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii]
                {
                    *editing = true;
                }
            }
        }
    }

    fn sync_replace_params(&mut self, ci: usize, ti: usize, gi: usize, ii: usize) {
        let item = &mut self.vault.collections[ci].tabs[ti].groups[gi].items[ii];
        let VaultItem::ReplaceText { template, params, .. } = item else { return; };
        let names = extract_param_names(template);
        for n in &names {
            if !params.iter().any(|p| &p.name == n) {
                params.push(Param {
                    name: n.clone(),
                    value: String::new(),
                });
            }
        }
    }

    fn draw_inline_label(
        &mut self,
        ui: &mut egui::Ui,
        target: RenameTarget,
        current: &str,
        mut apply: impl FnMut(&mut Self, String),
    ) {
        let renaming = self.rename.as_ref() == Some(&target);
        if renaming {
            let edit = ui.add(
                TextEdit::singleline(&mut self.rename_buf).desired_width(140.0),
            );
            if self.rename_focus {
                edit.request_focus();
                self.rename_focus = false;
            }
            if edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let new = std::mem::take(&mut self.rename_buf);
                apply(self, new);
                self.rename = None;
                self.mark_dirty();
            }
        } else {
            let lbl = ui.add(
                egui::Label::new(RichText::new(current).small().weak())
                    .sense(Sense::click()),
            );
            if lbl.double_clicked() {
                self.rename = Some(target);
                self.rename_buf = current.to_owned();
                self.rename_focus = true;
            }
        }
    }
}

// ---------- Status bar ----------
impl App {
    fn draw_status(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(e) = self.error.clone() {
                    ui.colored_label(Color32::LIGHT_RED, format!("! {e}"));
                    if ui.small_button("dismiss").clicked() {
                        self.error = None;
                    }
                } else if let Some((msg, _)) = &self.toast {
                    ui.colored_label(Color32::LIGHT_GREEN, format!("v {msg}"));
                } else {
                    ui.label(RichText::new(format!(
                        "{} collection(s) - double-click any name to rename, drag {} to reorder",
                        self.vault.collections.len(),
                        ICON_DRAG
                    ))
                    .weak());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let saving = self.dirty_frames > 0;
                    let label = if saving { "* unsaved" } else { "saved" };
                    let color = if saving { Color32::YELLOW } else { Color32::LIGHT_GREEN };
                    ui.colored_label(color, label);
                });
            });
        });
    }
}

// ---------- Menu bar / dialogs / file IO ----------
impl App {
    fn draw_menubar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Import collection...").clicked() {
                        ui.close_menu();
                        self.action_import();
                    }
                    ui.separator();
                    if ui.button("Backup all...").clicked() {
                        ui.close_menu();
                        self.action_backup();
                    }
                    if ui.button("Restore from backup...").clicked() {
                        ui.close_menu();
                        self.action_restore();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.close_menu();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        ui.close_menu();
                        self.dialog = Some(Dialog::Info {
                            title: "About KeyVault".into(),
                            message:
                                "KeyVault — local secrets/commands vault.\nData is stored as JSON \
                                 in your OS data dir. Exports/backups can be encrypted with a \
                                 password (Argon2id + AES-256-GCM)."
                                    .into(),
                            is_error: false,
                        });
                    }
                });
                ui.separator();
                let can_undo = !self.undo_stack.is_empty();
                let can_redo = !self.redo_stack.is_empty();
                if ui
                    .add_enabled(can_undo, egui::Button::new("Undo"))
                    .on_hover_text("Undo (Ctrl+Z)")
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new("Redo"))
                    .on_hover_text("Redo (Ctrl+Y)")
                    .clicked()
                {
                    self.redo();
                }
            });
        });
    }

    // ----- entry points wired to menu / collection menu -----

    fn action_export_collection(&mut self, id: Uuid) {
        let Some(c) = self.vault.collections.iter().find(|c| c.id == id) else {
            return;
        };
        let default_name = format!(
            "{}.kv",
            sanitize_filename(if c.name.is_empty() { "collection" } else { &c.name })
        );
        let path = rfd::FileDialog::new()
            .set_title("Export collection")
            .add_filter("KeyVault file", &["kv"])
            .add_filter("JSON", &["json"])
            .set_file_name(&default_name)
            .save_file();
        let Some(path) = path else { return };
        self.dialog = Some(Dialog::Password {
            title: "Encrypt export".into(),
            message:
                "Enter a password to encrypt this file, or leave both fields empty to save as \
                 plain JSON."
                    .into(),
            buf: String::new(),
            confirm_buf: String::new(),
            confirming: true,
            purpose: PasswordPurpose::ExportCollection { id, path },
            focus: true,
        });
    }

    fn action_import(&mut self) {
        let path = rfd::FileDialog::new()
            .set_title("Import collection")
            .add_filter("KeyVault file", &["kv", "json"])
            .add_filter("Any", &["*"])
            .pick_file();
        let Some(path) = path else { return };
        match portable::read_raw(&path) {
            Ok((bytes, encrypted)) => {
                if encrypted {
                    self.dialog = Some(Dialog::Password {
                        title: "Decrypt import".into(),
                        message: "This file is encrypted. Enter the password.".into(),
                        buf: String::new(),
                        confirm_buf: String::new(),
                        confirming: false,
                        purpose: PasswordPurpose::DecryptImport { path },
                        focus: true,
                    });
                } else {
                    self.apply_import(&bytes, "");
                }
            }
            Err(e) => self.show_error("Import failed", e.to_string()),
        }
    }

    fn action_backup(&mut self) {
        let path = rfd::FileDialog::new()
            .set_title("Backup all collections")
            .add_filter("KeyVault backup", &["kv"])
            .add_filter("JSON", &["json"])
            .set_file_name("keyvault-backup.kv")
            .save_file();
        let Some(path) = path else { return };
        self.dialog = Some(Dialog::Password {
            title: "Encrypt backup".into(),
            message:
                "Enter a password to encrypt the backup, or leave both fields empty to save as \
                 plain JSON."
                    .into(),
            buf: String::new(),
            confirm_buf: String::new(),
            confirming: true,
            purpose: PasswordPurpose::BackupAll { path },
            focus: true,
        });
    }

    fn action_restore(&mut self) {
        let path = rfd::FileDialog::new()
            .set_title("Restore from backup")
            .add_filter("KeyVault backup", &["kv", "json"])
            .add_filter("Any", &["*"])
            .pick_file();
        let Some(path) = path else { return };
        match portable::read_raw(&path) {
            Ok((bytes, encrypted)) => {
                if encrypted {
                    self.dialog = Some(Dialog::Password {
                        title: "Decrypt backup".into(),
                        message: "This backup is encrypted. Enter the password.".into(),
                        buf: String::new(),
                        confirm_buf: String::new(),
                        confirming: false,
                        purpose: PasswordPurpose::DecryptRestore { path },
                        focus: true,
                    });
                } else {
                    self.confirm_restore(&bytes, "");
                }
            }
            Err(e) => self.show_error("Restore failed", e.to_string()),
        }
    }

    // ----- finishers -----

    fn finish_export_collection(&mut self, id: Uuid, path: PathBuf, password: &str) {
        let Some(c) = self.vault.collections.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let payload = ExportPayload::collection(c);
        match portable::write(&path, &payload, password) {
            Ok(()) => self.show_info("Export complete", format!("Saved to {}", path.display())),
            Err(e) => self.show_error("Export failed", e.to_string()),
        }
    }

    fn finish_backup(&mut self, path: PathBuf, password: &str) {
        let payload = ExportPayload::backup(self.vault.clone());
        match portable::write(&path, &payload, password) {
            Ok(()) => self.show_info("Backup complete", format!("Saved to {}", path.display())),
            Err(e) => self.show_error("Backup failed", e.to_string()),
        }
    }

    fn apply_import(&mut self, bytes: &[u8], password: &str) {
        match portable::decode(bytes, password) {
            Ok(ExportPayload::Collection { mut collection, .. }) => {
                portable::rekey_collection(&mut collection);
                let new_id = collection.id;
                let first_tab_id = collection.tabs.first().map(|t| t.id);
                self.vault.collections.push(collection);
                self.active_collection_id = Some(new_id);
                self.active_tab_id = first_tab_id;
                self.mark_dirty();
                self.set_toast("Collection imported");
            }
            Ok(ExportPayload::Backup { .. }) => self.show_error(
                "Wrong file type",
                "That looks like a full backup. Use File > Restore from backup.".to_string(),
            ),
            Err(e) => self.show_error("Import failed", e.to_string()),
        }
    }

    fn confirm_restore(&mut self, bytes: &[u8], password: &str) {
        // Decode now so user gets immediate feedback if the file is wrong, then
        // store the decoded payload via a temp file path → simpler: we re-decode after confirm.
        match portable::decode(bytes, password) {
            Ok(ExportPayload::Backup { vault, .. }) => {
                self.dialog = Some(Dialog::Confirm {
                    title: "Replace all data?".into(),
                    message: format!(
                        "Restoring will REPLACE all current data with the backup contents \
                         ({} collection(s)). This cannot be undone. Continue?",
                        vault.collections.len()
                    ),
                    // We reuse Restore variant carrying a sentinel path; because
                    // we already have the decoded vault, stash it on self.
                    action: ConfirmAction::Restore,
                });
                self.pending_restore = Some(vault);
            }
            Ok(ExportPayload::Collection { .. }) => self.show_error(
                "Wrong file type",
                "That file is a single-collection export. Use File > Import collection."
                    .to_string(),
            ),
            Err(e) => self.show_error("Restore failed", e.to_string()),
        }
    }

    fn apply_restore(&mut self) {
        if let Some(v) = self.pending_restore.take() {
            self.vault = v;
            self.active_collection_id = self.vault.collections.first().map(|c| c.id);
            self.active_tab_id = self
                .vault
                .collections
                .first()
                .and_then(|c| c.tabs.first().map(|t| t.id));
            self.mark_dirty();
            self.set_toast("Backup restored");
        }
    }

    fn show_info(&mut self, title: impl Into<String>, msg: impl Into<String>) {
        self.dialog = Some(Dialog::Info {
            title: title.into(),
            message: msg.into(),
            is_error: false,
        });
    }
    fn show_error(&mut self, title: impl Into<String>, msg: impl Into<String>) {
        self.dialog = Some(Dialog::Info {
            title: title.into(),
            message: msg.into(),
            is_error: true,
        });
    }

    fn draw_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };

        let mut keep: Option<Dialog> = None;

        match dialog {
            Dialog::Confirm { title, message, action } => {
                let mut yes = false;
                let mut no = false;
                egui::Window::new(title.clone())
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.set_min_width(360.0);
                        ui.label(&message);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(
                                    RichText::new("Yes, proceed").color(COLOR_DANGER),
                                ))
                                .clicked()
                            {
                                yes = true;
                            }
                            if ui.button("Cancel").clicked() {
                                no = true;
                            }
                        });
                    });
                if yes {
                    self.run_confirmed_action(action);
                } else if no {
                    self.pending_restore = None;
                } else {
                    keep = Some(Dialog::Confirm { title, message, action });
                }
            }
            Dialog::Password {
                title,
                message,
                mut buf,
                mut confirm_buf,
                confirming,
                purpose,
                mut focus,
            } => {
                let mut ok = false;
                let mut cancel = false;
                let mut mismatch = false;
                egui::Window::new(title.clone())
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.set_min_width(380.0);
                        ui.label(&message);
                        ui.add_space(6.0);
                        let r1 = ui.add(
                            TextEdit::singleline(&mut buf)
                                .password(true)
                                .desired_width(f32::INFINITY)
                                .hint_text("Password"),
                        );
                        if focus {
                            r1.request_focus();
                            focus = false;
                        }
                        if confirming {
                            ui.add(
                                TextEdit::singleline(&mut confirm_buf)
                                    .password(true)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("Confirm password"),
                            );
                            if !confirm_buf.is_empty() && buf != confirm_buf {
                                ui.colored_label(COLOR_DANGER, "Passwords do not match");
                            }
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("OK").clicked()
                                || ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                if confirming && buf != confirm_buf {
                                    mismatch = true;
                                } else {
                                    ok = true;
                                }
                            }
                            if ui.button("Cancel").clicked()
                                || ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                cancel = true;
                            }
                        });
                    });

                if ok {
                    let pwd = buf.clone();
                    match purpose.clone() {
                        PasswordPurpose::ExportCollection { id, path } => {
                            self.finish_export_collection(id, path, &pwd);
                        }
                        PasswordPurpose::BackupAll { path } => {
                            self.finish_backup(path, &pwd);
                        }
                        PasswordPurpose::DecryptImport { path } => match portable::read_raw(&path)
                        {
                            Ok((bytes, _)) => self.apply_import(&bytes, &pwd),
                            Err(e) => self.show_error("Import failed", e.to_string()),
                        },
                        PasswordPurpose::DecryptRestore { path } => match portable::read_raw(&path)
                        {
                            Ok((bytes, _)) => self.confirm_restore(&bytes, &pwd),
                            Err(e) => self.show_error("Restore failed", e.to_string()),
                        },
                    }
                } else if cancel {
                    // dropped
                } else if mismatch {
                    keep = Some(Dialog::Password {
                        title,
                        message,
                        buf,
                        confirm_buf,
                        confirming,
                        purpose,
                        focus,
                    });
                } else {
                    keep = Some(Dialog::Password {
                        title,
                        message,
                        buf,
                        confirm_buf,
                        confirming,
                        purpose,
                        focus,
                    });
                }
            }
            Dialog::Info { title, message, is_error } => {
                let mut close = false;
                egui::Window::new(title.clone())
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.set_min_width(340.0);
                        if is_error {
                            ui.colored_label(COLOR_DANGER, &message);
                        } else {
                            ui.label(&message);
                        }
                        ui.add_space(6.0);
                        if ui.button("OK").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape))
                        {
                            close = true;
                        }
                    });
                if !close {
                    keep = Some(Dialog::Info { title, message, is_error });
                }
            }
        }
        self.dialog = keep;
    }

    fn run_confirmed_action(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteCollection(id) => {
                self.snapshot();
                self.vault.collections.retain(|c| c.id != id);
                if self.active_collection_id == Some(id) {
                    self.active_collection_id = self.vault.collections.first().map(|c| c.id);
                    self.active_tab_id = self
                        .vault
                        .collections
                        .first()
                        .and_then(|c| c.tabs.first().map(|t| t.id));
                }
                self.mark_dirty();
            }
            ConfirmAction::DeleteTab { collection, tab } => {
                self.snapshot();
                if let Some(c) = self.vault.collections.iter_mut().find(|c| c.id == collection) {
                    c.tabs.retain(|t| t.id != tab);
                    if self.active_tab_id == Some(tab) {
                        self.active_tab_id = c.tabs.first().map(|t| t.id);
                    }
                }
                self.mark_dirty();
            }
            ConfirmAction::DeleteGroup { collection, tab, group } => {
                self.snapshot();
                if let Some(c) = self.vault.collections.iter_mut().find(|c| c.id == collection) {
                    if let Some(t) = c.tabs.iter_mut().find(|t| t.id == tab) {
                        t.groups.retain(|g| g.id != group);
                    }
                }
                self.mark_dirty();
            }
            ConfirmAction::DeleteItem { collection, tab, group, item } => {
                self.snapshot();
                if let Some(c) = self.vault.collections.iter_mut().find(|c| c.id == collection) {
                    if let Some(t) = c.tabs.iter_mut().find(|t| t.id == tab) {
                        if let Some(g) = t.groups.iter_mut().find(|g| g.id == group) {
                            g.items.retain(|x| x.id() != item);
                        }
                    }
                }
                self.mark_dirty();
            }
            ConfirmAction::Restore => {
                self.snapshot();
                self.apply_restore();
            }
        }
    }
}

fn mask(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '\n' || c == '\r' { c } else { '*' })
        .collect()
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

// ---------- helpers ----------
fn extract_param_names(template: &str) -> Vec<String> {
    static_re()
        .captures_iter(template)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_owned()))
        .fold(Vec::new(), |mut acc, n| {
            if !n.is_empty() && !acc.contains(&n) {
                acc.push(n);
            }
            acc
        })
}

fn format_template(template: &str, params: &[Param]) -> String {
    let re = static_re();
    re.replace_all(template, |caps: &regex::Captures| {
        let name = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        params
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.clone())
            .unwrap_or_else(|| caps.get(0).map(|m| m.as_str().to_owned()).unwrap_or_default())
    })
    .into_owned()
}

fn static_re() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").unwrap())
}
