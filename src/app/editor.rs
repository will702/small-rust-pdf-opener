use super::*;

impl PdfApp {
    pub(super) fn prepare_edit(&mut self, page: usize, area: PdfRect) {
        if self.mode == ToolMode::EditText {
            if let Some(s) = &self.session {
                match s.paragraph_at(page, area) {
                    Ok((rect, text)) => {
                        self.edit_target = Some((page, rect));
                        self.selection = Some((page, rect));
                        self.annotation_text = text;
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        } else {
            self.edit_target = Some((page, area));
            self.selection = Some((page, area));
        }
    }

    pub(super) fn editor_panel(&mut self, ui: &mut egui::Ui) {
        if !matches!(
            self.mode,
            ToolMode::Redact | ToolMode::EditText | ToolMode::Forms
        ) || self.session.is_none()
        {
            return;
        }
        egui::Panel::right("editor_properties").default_size(280.0).show(ui, |ui| {
            ui.add_enabled_ui(!self.busy, |ui| {
                match self.mode {
                    ToolMode::Forms => {
                        ui.heading("Form fields");
                        if self.form_page != Some(self.page) {
                            match self.session.as_ref().unwrap().fields(self.page) {
                                Ok(fields) => { self.form_fields = fields; self.form_page = Some(self.page); }
                                Err(e) => self.error = Some(e.to_string()),
                            }
                        }
                        if self.form_fields.is_empty() { ui.label("This page has no interactive form fields."); }
                        let mut update = None;
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for field in &mut self.form_fields {
                                ui.push_id(field.xref, |ui| {
                                    ui.label(&field.name); ui.weak(&field.kind);
                                    let supported = field.kind == "Text" || !field.choices.is_empty();
                                    ui.add_enabled_ui(!field.readonly && supported, |ui| {
                                        if field.choices.is_empty() { ui.text_edit_singleline(&mut field.value); }
                                        else { egui::ComboBox::from_id_salt("field_value").selected_text(&field.value).show_ui(ui, |ui| {
                                            for value in &field.choices { ui.selectable_value(&mut field.value,value.clone(),value); }
                                        }); }
                                        if ui.button("Apply value").clicked() { update = Some((field.xref,field.value.clone())); }
                                    });
                                    if !supported { ui.weak("This field type is not editable in this build."); }
                                    ui.separator();
                                });
                            }
                        });
                        if let Some((xref,value)) = update {
                            match self.session.as_mut().unwrap().set_field(self.page,xref,&value) {
                                Ok(()) => self.content_changed("Form field updated"), Err(e) => self.error = Some(e.to_string()),
                            }
                        }
                    }
                    ToolMode::EditText => {
                        ui.heading("Edit paragraph");
                        ui.label("Drag inside a text paragraph to select it.");
                        ui.label("Replacement uses Helvetica and reflows inside the selected box. ASCII text only.");
                        if let Some((page,area)) = self.edit_target {
                            ui.add(egui::TextEdit::multiline(&mut self.annotation_text).desired_rows(10).desired_width(f32::INFINITY));
                            ui.add(egui::DragValue::new(&mut self.edit_size).range(6.0..=96.0).suffix(" pt"));
                            if ui.button("Replace paragraph").clicked() {
                                match self.session.as_mut().unwrap().replace_paragraph(page,area,&self.annotation_text,self.edit_size) {
                                    Ok(()) => self.content_changed("Paragraph replaced — Undo is available"), Err(e) => self.error = Some(e.to_string()),
                                }
                            }
                        }
                    }
                    ToolMode::Redact => {
                        ui.heading("Redact content");
                        ui.label("Drag a rectangle over content to remove.");
                        ui.label("Creates a new PDF. Text and graphics touching the selection are removed; a touched image is removed entirely. Document metadata and attachments are also removed.");
                        if let Some((page,area)) = self.edit_target {
                            ui.label(format!("Selected area on page {}",page+1));
                            if ui.button("Save redacted copy…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name("redacted.pdf").save_file() {
                                    let source = self.session.as_ref().and_then(|s| s.path.as_ref());
                                    if source.is_some_and(|p| p == &path || p.canonicalize().ok().zip(path.canonicalize().ok()).is_some_and(|(a,b)| a==b)) {
                                        self.error = Some("Choose a different file to preserve your original document.".into());
                                    } else {
                                        match self.session.as_ref().unwrap().redacted_copy(page,area,&path) {
                                            Ok(()) => self.status = format!("Redacted copy saved: {}",path.display()), Err(e) => self.error = Some(e.to_string()),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if ui.button("Done").clicked() { self.mode = ToolMode::View; self.selection = None; self.edit_target = None; }
            });
        });
    }
}
