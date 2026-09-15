use super::*;

pub(super) enum PendingAction {
    Open(PathBuf),
    Close,
    Quit,
}

fn recent_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("pdf-opener/recent.txt"))
}
pub(super) fn load_recent() -> Vec<PathBuf> {
    recent_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .take(8)
        .collect()
}
pub(super) fn save_recent(paths: &[PathBuf]) {
    if let Some(path) = recent_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = paths
            .iter()
            .filter_map(|p| p.to_str())
            .filter(|p| !p.contains(['\n', '\r']))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = pdf::atomic_write(&path, body.as_bytes());
    }
}

impl PdfApp {
    pub(super) fn request_action(&mut self, action: PendingAction) {
        if self.session.as_ref().is_some_and(|s| s.dirty) {
            self.pending = Some(action);
        } else {
            self.perform_action(action);
        }
    }

    fn perform_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::Open(path) => self.load_path(path),
            PendingAction::Close | PendingAction::Quit => {
                self.cancel_ocr();
                let (tx, rx) = mpsc::channel();
                self.bg_tx = tx;
                self.bg_rx = rx;
                self.busy = false;
                self.session = None;
                self.ocr_cancel = None;
                self.invalidate_textures();
                self.document_hits.clear();
                self.ocr_overlays.clear();
                self.allow_close = matches!(action, PendingAction::Quit);
            }
        }
    }

    pub(super) fn workspace_events(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_action(PendingAction::Quit);
        }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            self.open_path(path);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            self.show_search = true;
        }
        if !ctx.egui_wants_keyboard_input() && !self.busy && self.pending.is_none() {
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
                self.history(ctx.input(|i| i.modifiers.shift));
            }
        }
        if !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::C))
        {
            if let (Some(s), Some((page, rect))) = (&self.session, self.selection) {
                match s.selected_text(page, rect) {
                    Ok((text, _)) => ctx.copy_text(text),
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        }
        if self.pending.is_some() {
            egui::Modal::new(egui::Id::new("unsaved_changes")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("Save your changes?");
                ui.label("This document has unsaved edits.");
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(crate::theme::primary_button(ui, "Save changes"))
                        .clicked()
                    {
                        self.save();
                        if self.session.as_ref().is_some_and(|s| !s.dirty) {
                            if let Some(action) = self.pending.take() {
                                self.perform_action(action);
                            }
                        }
                    }
                    if ui.button("Discard changes").clicked() {
                        if let Some(action) = self.pending.take() {
                            self.perform_action(action);
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending = None;
                    }
                });
            });
        }
    }

    fn history(&mut self, redo: bool) {
        if let Some(s) = self.session.as_mut() {
            let result = if redo { s.redo() } else { s.undo() };
            match result {
                Ok(()) => self.content_changed(if redo { "Redo" } else { "Undo" }),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    pub(super) fn content_changed(&mut self, message: &str) {
        self.invalidate_textures();
        self.document_hits.clear();
        self.search_hits.clear();
        self.ocr_overlays.clear();
        self.selection = None;
        self.form_page = None;
        self.edit_target = None;
        self.page = self.page.min(self.page_count().saturating_sub(1));
        let count = self.page_count();
        self.selected_pages.retain(|p| *p < count);
        self.outline = self
            .session
            .as_ref()
            .and_then(|s| s.outlines().ok())
            .unwrap_or_default();
        self.status = message.into();
    }

    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(
                    egui::Image::new((logo.id(), logo.size_vec2()))
                        .fit_to_exact_size(egui::vec2(20.0, 20.0))
                        .corner_radius(4.5),
                );
            }
            ui.label(
                egui::RichText::new("PDF Opener")
                    .family(egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()))
                    .size(13.0),
            );
            ui.separator();
            ui.menu_button("File", |ui| {
                if ui.button("Open…                 ⌘O").clicked() {
                    self.open_dialog();
                    ui.close();
                }
                if ui.button("Merge PDFs…").clicked() {
                    self.show_merge = true;
                    ui.close();
                }
                if !self.recent.is_empty() {
                    ui.menu_button("Recent files", |ui| {
                        for path in self.recent.clone() {
                            if ui
                                .button(path.file_name().unwrap_or_default().to_string_lossy())
                                .clicked()
                            {
                                self.open_path(path);
                                ui.close();
                            }
                        }
                    });
                }
                ui.add_enabled_ui(self.session.is_some() && !self.busy, |ui| {
                    if ui.button("Save                   ⌘S").clicked() {
                        self.save();
                        ui.close();
                    }
                    if ui.button("Save As…            ⇧⌘S").clicked() {
                        self.save_as();
                        ui.close();
                    }
                    if ui.button("Export text…").clicked() {
                        self.show_export = true;
                        ui.close();
                    }
                    if ui.button("Close document").clicked() {
                        self.request_action(PendingAction::Close);
                        ui.close();
                    }
                });
            });
            let title = self
                .session
                .as_ref()
                .and_then(|s| s.path.as_ref())
                .and_then(|p| p.file_name())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Welcome".into());
            ui.label(egui::RichText::new(title).strong());
            if self.session.as_ref().is_some_and(|s| s.dirty) {
                ui.weak("• Edited");
            }
        });
        if self.session.is_none() {
            return;
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut self.show_pages, "Pages");
            ui.toggle_value(&mut self.show_search, "Find");
            ui.separator();
            ui.add_enabled_ui(!self.busy, |ui| {
                if ui
                    .add_enabled(
                        self.session.as_ref().is_some_and(|s| s.can_undo()),
                        egui::Button::new("Undo"),
                    )
                    .clicked()
                {
                    self.history(false);
                }
                if ui
                    .add_enabled(
                        self.session.as_ref().is_some_and(|s| s.can_redo()),
                        egui::Button::new("Redo"),
                    )
                    .clicked()
                {
                    self.history(true);
                }
                ui.separator();
                ui.selectable_value(&mut self.mode, ToolMode::View, "Select");
                ui.menu_button("Annotate", |ui| {
                    for (mode, label) in [
                        (ToolMode::Draw, "Draw"),
                        (ToolMode::Highlight, "Highlight"),
                        (ToolMode::Note, "Sticky note"),
                        (ToolMode::TextBox, "Text box"),
                    ] {
                        if ui.selectable_value(&mut self.mode, mode, label).clicked() {
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Organize", |ui| {
                    for (label, action) in [
                        ("Rotate clockwise", 0),
                        ("Rotate counterclockwise", 1),
                        ("Duplicate", 2),
                        ("Delete", 3),
                        ("Move up", 4),
                        ("Move down", 5),
                        ("Reset crop", 6),
                    ] {
                        if ui.button(label).clicked() {
                            self.organize(action);
                            ui.close();
                        }
                    }
                    if ui
                        .selectable_value(&mut self.mode, ToolMode::Crop, "Crop page")
                        .clicked()
                    {
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Append PDF…").clicked() {
                        self.append_pdfs_dialog();
                        ui.close();
                    }
                    if ui.button("Split…").clicked() {
                        self.show_split = true;
                        ui.close();
                    }
                    if ui.button("Extract…").clicked() {
                        self.extract_range_text = self
                            .selected_pages
                            .iter()
                            .map(|p| (p + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        self.show_extract = true;
                        ui.close();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    for (mode, label) in [
                        (ToolMode::EditText, "Edit paragraph"),
                        (ToolMode::Forms, "Fill form fields"),
                        (ToolMode::Redact, "Redact to a new PDF"),
                    ] {
                        if ui.selectable_value(&mut self.mode, mode, label).clicked() {
                            self.edit_target = None;
                            self.form_page = None;
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Sign", |ui| {
                    if ui
                        .selectable_value(
                            &mut self.mode,
                            ToolMode::VisualSign,
                            "Draw or import signature",
                        )
                        .clicked()
                    {
                        ui.close();
                    }
                    if ui
                        .selectable_value(
                            &mut self.mode,
                            ToolMode::CertSign,
                            "Certificate signature",
                        )
                        .clicked()
                    {
                        ui.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Recognize text (OCR)…").clicked() {
                        self.show_ocr = true;
                        ui.close();
                    }
                    if ui.button("Compress…").clicked() {
                        self.show_compress = true;
                        ui.close();
                    }
                    if ui.button("Export text…").clicked() {
                        self.show_export = true;
                        ui.close();
                    }
                    if ui.button("Copy page text").clicked() {
                        if let Some(s) = &self.session {
                            match s.extract_text(self.page) {
                                Ok(t) => ui.ctx().copy_text(t),
                                Err(e) => self.error = Some(e.to_string()),
                            }
                        }
                        ui.close();
                    }
                });
                if ui.add(crate::theme::primary_button(ui, "Save")).clicked() {
                    self.save();
                }
            });
        });
        if matches!(self.mode, ToolMode::Note | ToolMode::TextBox) {
            ui.horizontal(|ui| {
                ui.label("Text");
                ui.text_edit_singleline(&mut self.annotation_text);
                ui.weak("Drag a rectangle on the page to place");
            });
        } else if self.mode == ToolMode::Highlight {
            ui.weak("Drag across text to highlight it. Undo removes the last highlight.");
        }
        if self.show_search {
            ui.horizontal_wrapped(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Find in document")
                        .desired_width(220.0),
                );
                if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui
                        .add_enabled(!self.busy, egui::Button::new("Search"))
                        .clicked()
                {
                    self.run_search();
                }
                if ui
                    .add_enabled(
                        !self.document_hits.is_empty(),
                        egui::Button::new("Previous"),
                    )
                    .clicked()
                {
                    self.hit_index =
                        (self.hit_index + self.document_hits.len() - 1) % self.document_hits.len();
                    self.focus_hit();
                }
                if ui
                    .add_enabled(!self.document_hits.is_empty(), egui::Button::new("Next"))
                    .clicked()
                {
                    self.hit_index = (self.hit_index + 1) % self.document_hits.len();
                    self.focus_hit();
                }
                ui.label(format!(
                    "{} / {}",
                    if self.document_hits.is_empty() {
                        0
                    } else {
                        self.hit_index + 1
                    },
                    self.document_hits.len()
                ));
                if ui.small_button("Close").clicked() {
                    self.show_search = false;
                    self.document_hits.clear();
                    self.search_hits.clear();
                }
            });
        }
    }

    fn organize(&mut self, action: u8) {
        let mut pages: Vec<usize> = self.selected_pages.iter().copied().collect();
        if pages.is_empty() {
            pages.push(self.page);
        }
        if let Some(s) = self.session.as_mut() {
            let result = s.organize_pages(&pages, action);
            match result {
                Ok(()) => {
                    self.selected_pages.clear();
                    self.content_changed("Pages updated");
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    pub(super) fn run_file_job(
        &mut self,
        status: &str,
        job: impl FnOnce() -> crate::error::Result<String> + Send + 'static,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = status.into();
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(BackgroundMsg::FileDone(job().map_err(|e| e.to_string())));
        });
    }

    pub(super) fn run_search(&mut self) {
        if self.busy {
            return;
        }
        self.document_hits.clear();
        self.search_hits.clear();
        if self.search_query.trim().is_empty() {
            return;
        }
        let Some(s) = &self.session else {
            return;
        };
        let bytes = match s.write_bytes(pdf::CompressPreset::Fast.write_options()) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let query = self.search_query.clone();
        let tx = self.bg_tx.clone();
        self.busy = true;
        self.status = "Searching document…".into();
        std::thread::spawn(move || {
            let result = (|| {
                let s = DocumentSession::from_bytes(&bytes)?;
                let mut hits = Vec::new();
                for p in 0..s.page_count()? {
                    for r in s.search(p, &query)? {
                        hits.push((p, r));
                    }
                }
                Ok::<_, AppError>(hits)
            })()
            .map_err(|e| e.to_string());
            let _ = tx.send(BackgroundMsg::SearchDone(result));
        });
    }

    pub(super) fn focus_hit(&mut self) {
        if let Some((page, _)) = self.document_hits.get(self.hit_index) {
            self.go_to_page(*page);
        }
        self.status = format!("{} matches in document", self.document_hits.len());
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let small = |ui: &mut egui::Ui, text: String| {
                ui.label(egui::RichText::new(text).weak().size(11.0));
            };
            if self.busy {
                ui.spinner();
            }
            if self.session.is_some() {
                let mut number = self.page + 1;
                small(ui, "Page".into());
                if ui
                    .add(egui::DragValue::new(&mut number).range(1..=self.page_count()))
                    .changed()
                {
                    self.go_to_page(number - 1);
                }
                small(ui, format!("of {}", self.page_count()));
                ui.separator();
                if ui.selectable_label(self.fit_width, "Fit width").clicked() {
                    self.fit_width = true;
                    self.fit_page = false;
                }
                if ui.selectable_label(self.fit_page, "Fit page").clicked() {
                    self.fit_width = false;
                    self.fit_page = true;
                }
                let mut percent = (self.zoom * 100.0).round();
                if ui
                    .add(
                        egui::DragValue::new(&mut percent)
                            .range(10.0..=400.0)
                            .suffix("%"),
                    )
                    .changed()
                {
                    self.fit_width = false;
                    self.fit_page = false;
                    self.zoom = percent / 100.0;
                }
                ui.separator();
            }
            small(ui, self.status.clone());
        });
    }

    pub(super) fn page_sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.show_pages || self.session.is_none() {
            return;
        }
        egui::Panel::left("pages")
            .default_size(164.0)
            .size_range(140.0..=260.0)
            .frame(crate::theme::chrome_frame(ui).inner_margin(egui::Margin::symmetric(10, 10)))
            .show(ui, |ui| {
                crate::theme::hairline_right(ui);
                if let Some(segment) = crate::theme::segmented_control(
                    ui,
                    self.show_outline as usize,
                    &["Pages", "Outline"],
                ) {
                    self.show_outline = segment == 1;
                }
                if self.show_outline {
                    if self.outline.is_empty() {
                        ui.weak("No document outline");
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (title, page, depth) in self.outline.clone() {
                            ui.horizontal(|ui| {
                                ui.add_space((depth.min(8) * 10) as f32);
                                if ui.selectable_label(self.page == page, title).clicked() {
                                    self.go_to_page(page);
                                }
                            });
                        }
                    });
                    return;
                }
                ui.label(
                    egui::RichText::new("⌘/Ctrl-click to select several")
                        .weak()
                        .size(11.0),
                );
                egui::ScrollArea::vertical().show_rows(
                    ui,
                    166.0,
                    self.page_count(),
                    |ui, range| {
                        self.thumbnails.retain(|p, _| range.contains(p));
                        for page in range {
                            if !self.thumbnails.contains_key(&page) {
                                if let Some(s) = &self.session {
                                    if let Ok((w, h)) = s.page_size(page) {
                                        if let Ok(render) =
                                            s.render_page(page, (112.0 / w).min(132.0 / h))
                                        {
                                            self.thumbnails.insert(
                                                page,
                                                ctx.load_texture(
                                                    format!("thumb-{page}"),
                                                    render.to_color_image(),
                                                    egui::TextureOptions::LINEAR,
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                            let selected = self.selected_pages.contains(&page) || self.page == page;
                            egui::Frame::new()
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(if selected {
                                    egui::Stroke::new(2.0, crate::theme::accent(ui))
                                } else {
                                    egui::Stroke::NONE
                                })
                                .corner_radius(8.0)
                                .inner_margin(6.0)
                                .show(ui, |ui| {
                                    ui.set_min_height(150.0);
                                    ui.set_min_width(116.0);
                                    ui.vertical_centered(|ui| {
                                        if let Some(tex) = self.thumbnails.get(&page) {
                                            let response = ui.add(
                                                egui::Image::new((tex.id(), tex.size_vec2()))
                                                    .corner_radius(4.0)
                                                    .sense(egui::Sense::click()),
                                            );
                                            if response.clicked() {
                                                if ui.input(|i| i.modifiers.command) {
                                                    if !self.selected_pages.insert(page) {
                                                        self.selected_pages.remove(&page);
                                                    }
                                                } else {
                                                    self.selected_pages.clear();
                                                    self.selected_pages.insert(page);
                                                }
                                                self.go_to_page(page);
                                            }
                                        }
                                        ui.label(
                                            egui::RichText::new(format!("{}", page + 1))
                                                .weak()
                                                .size(11.0),
                                        );
                                    });
                                });
                        }
                    },
                );
            });
    }
}
