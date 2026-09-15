//! Main egui application.
mod editor;
mod workspace;
use workspace::PendingAction;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use eframe::egui;
use mupdf::Rect as PdfRect;

use crate::error::AppError;
use crate::export;
use crate::ocr;
use crate::page_range::{self, pages_filename_suffix};
use crate::pdf::{self, CompressPreset, DocumentSession};
use crate::sign::cert::{self, CertIdentity};
use crate::sign::visual::SignaturePad;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Markdown,
    Docx,
}

const PAGE_GAP: f32 = 16.0;
const TEXTURE_CACHE_RADIUS: isize = 2;

enum PageInteraction {
    SelectEnd {
        page: usize,
        start: egui::Pos2,
        end: egui::Pos2,
        rect: egui::Rect,
    },
    CropEnd {
        page: usize,
        start: egui::Pos2,
        end: egui::Pos2,
        rect: egui::Rect,
    },
    SignClick {
        page: usize,
        pos: egui::Pos2,
        rect: egui::Rect,
    },
    CertEnd {
        page: usize,
        start: egui::Pos2,
        end: egui::Pos2,
        rect: egui::Rect,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolMode {
    View,
    Draw,
    Redact,
    EditText,
    Forms,
    Highlight,
    Note,
    TextBox,
    Crop,
    VisualSign,
    CertSign,
}

#[derive(Debug, Clone)]
struct OcrOverlayLine {
    text: String,
    /// Top-left origin, PDF points (y is baseline / bottom of box, matching OcrLine).
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

enum BackgroundMsg {
    SearchDone(Result<Vec<(usize, PdfRect)>, String>),
    FileDone(Result<String, String>),
    OcrProgress(String),
    OcrPageDone {
        page: usize,
        lines: Result<Vec<ocr::OcrLine>, String>,
    },
    OcrJobFinished {
        ok: usize,
        failed: usize,
        cancelled: bool,
    },
    OcrModelsDone(Result<(), String>),
    Error(String),
}

pub struct PdfApp {
    session: Option<DocumentSession>,
    page: usize,
    zoom: f32,
    fit_width: bool,
    mode: ToolMode,
    search_query: String,
    search_hits: Vec<PdfRect>,
    document_hits: Vec<(usize, PdfRect)>,
    hit_index: usize,
    show_search: bool,
    show_pages: bool,
    show_outline: bool,
    outline: Vec<(String, usize, usize)>,
    fit_page: bool,
    thumbnails: HashMap<usize, egui::TextureHandle>,
    selected_pages: std::collections::BTreeSet<usize>,
    recent: Vec<PathBuf>,
    pending: Option<PendingAction>,
    allow_close: bool,
    annotation_text: String,
    edit_target: Option<(usize, PdfRect)>,
    edit_size: f32,
    form_fields: Vec<pdf::FormField>,
    form_page: Option<usize>,
    ink_points: Vec<mupdf::Point>,
    selection: Option<(usize, PdfRect)>,

    status: String,
    error: Option<String>,
    show_compress: bool,
    compress_preset: CompressPreset,
    show_ocr: bool,
    ocr_range_text: String,
    ocr_overlays: HashMap<usize, Vec<OcrOverlayLine>>,
    show_ocr_overlays: bool,
    ocr_cancel: Option<Arc<AtomicBool>>,
    show_merge: bool,
    merge_paths: Vec<PathBuf>,
    show_split: bool,
    split_range_text: String,
    show_extract: bool,
    extract_range_text: String,
    extract_open_after: bool,
    show_export: bool,
    export_range_text: String,
    export_format: ExportFormat,
    /// Cached page textures keyed by page index; invalidated when zoom changes.
    page_textures: HashMap<usize, egui::TextureHandle>,
    texture_zoom: f32,
    /// When set, viewer scrolls this page into view once.
    scroll_to_page: Option<usize>,
    // Crop
    crop_start: Option<egui::Pos2>,
    crop_end: Option<egui::Pos2>,
    crop_page: Option<usize>,
    // Visual sign
    sig_pad: SignaturePad,
    placing_sig: bool,
    imported_sig: Option<(u32, u32, Vec<u8>)>,
    staged_sig: Option<(usize, PdfRect)>,
    sig_texture: Option<egui::TextureHandle>,
    // Cert sign
    cert_identity: Option<CertIdentity>,
    cert_password: String,
    cert_path: Option<PathBuf>,
    cert_reason: String,
    placing_cert: bool,
    cert_place: Option<(egui::Pos2, egui::Pos2)>,
    cert_page: Option<usize>,
    // Branding
    logo: Option<egui::TextureHandle>,
    // Background work
    bg_tx: Sender<BackgroundMsg>,
    bg_rx: Receiver<BackgroundMsg>,
    busy: bool,
}

impl Default for PdfApp {
    fn default() -> Self {
        let (bg_tx, bg_rx) = mpsc::channel();
        Self {
            session: None,
            page: 0,
            zoom: 1.25,
            fit_width: true,
            mode: ToolMode::View,
            search_query: String::new(),
            search_hits: Vec::new(),
            document_hits: Vec::new(),
            hit_index: 0,
            show_search: false,
            show_pages: true,
            show_outline: false,
            outline: Vec::new(),
            fit_page: false,
            thumbnails: HashMap::new(),
            selected_pages: Default::default(),
            recent: Vec::new(),
            pending: None,
            allow_close: false,
            annotation_text: String::new(),
            selection: None,
            edit_target: None,
            edit_size: 12.0,
            form_fields: Vec::new(),
            form_page: None,
            ink_points: Vec::new(),

            status: "Open a PDF to begin".into(),
            error: None,
            show_compress: false,
            compress_preset: CompressPreset::Balanced,
            show_ocr: false,
            ocr_range_text: String::new(),
            ocr_overlays: HashMap::new(),
            show_ocr_overlays: true,
            ocr_cancel: None,
            show_merge: false,
            merge_paths: Vec::new(),
            show_split: false,
            split_range_text: String::new(),
            show_extract: false,
            extract_range_text: String::new(),
            extract_open_after: true,
            show_export: false,
            export_range_text: String::new(),
            export_format: ExportFormat::Markdown,
            page_textures: HashMap::new(),
            texture_zoom: 0.0,
            scroll_to_page: None,
            crop_start: None,
            crop_end: None,
            crop_page: None,
            sig_pad: SignaturePad::new(400, 150),
            placing_sig: false,
            imported_sig: None,
            staged_sig: None,
            sig_texture: None,
            cert_identity: None,
            cert_password: String::new(),
            cert_path: None,
            cert_reason: "Signed with PDF Opener".into(),
            placing_cert: false,
            cert_place: None,
            cert_page: None,
            logo: None,
            bg_tx,
            bg_rx,
            busy: false,
        }
    }
}

impl PdfApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        crate::theme::apply(&cc.egui_ctx);
        let mut app = Self::default();
        app.logo = load_logo_texture(&cc.egui_ctx);
        app.recent = workspace::load_recent();
        if let Some(path) = initial {
            app.open_path(path);
        }
        app
    }

    fn open_path(&mut self, path: PathBuf) {
        self.request_action(PendingAction::Open(path));
    }

    fn load_path(&mut self, path: PathBuf) {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
        {
            self.error = Some("DOCX editing is unavailable: the macOS office engine failed its document-loading test. Your file has not been changed.".into());
            return;
        }
        match DocumentSession::open(&path) {
            Ok(session) => {
                self.cancel_ocr();
                let (tx, rx) = mpsc::channel();
                self.bg_tx = tx;
                self.bg_rx = rx;
                self.busy = false;
                self.ocr_cancel = None;
                self.document_hits.clear();
                self.selection = None;
                self.selected_pages.clear();
                self.thumbnails.clear();
                self.mode = ToolMode::View;
                self.crop_start = None;
                self.crop_end = None;
                self.crop_page = None;
                self.placing_sig = false;
                self.placing_cert = false;
                self.staged_sig = None;
                self.sig_texture = None;
                self.recent.retain(|p| p != &path);
                self.recent.insert(0, path.clone());
                self.recent.truncate(8);
                workspace::save_recent(&self.recent);
                self.status = format!("Opened {}", path.display());
                self.page = 0;
                self.outline = session.outlines().unwrap_or_default();
                self.session = Some(session);
                self.ocr_overlays.clear();
                self.invalidate_textures();
                self.scroll_to_page = Some(0);
                self.search_hits.clear();
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
        {
            self.open_path(path);
        }
    }

    fn save(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.save() {
            Ok(()) => self.status = "Saved".into(),
            Err(e) => {
                // Fall back to Save As if no path / incremental issues
                self.error = Some(e.to_string());
                self.save_as();
            }
        }
    }

    fn save_as(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name("document.pdf")
            .save_file()
        else {
            return;
        };
        if let Some(session) = self.session.as_mut() {
            match session.save_as(&path, CompressPreset::Balanced.write_options()) {
                Ok(()) => self.status = format!("Saved {}", path.display()),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    fn invalidate_textures(&mut self) {
        self.page_textures.clear();
        self.thumbnails.clear();
        self.texture_zoom = 0.0;
    }

    fn update_zoom(&mut self, available_width: f32) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if self.fit_width {
            // Use first page (or current) width for fit-width zoom.
            let page = self
                .page
                .min(session.page_count().unwrap_or(1).saturating_sub(1));
            if let Ok((pw, _)) = session.page_size(page) {
                if pw > 0.0 {
                    let zoom = (available_width / pw).clamp(0.25, 4.0);
                    if (zoom - self.zoom).abs() > 0.001 {
                        self.zoom = zoom;
                        self.invalidate_textures();
                    }
                }
            }
        }
    }

    fn ensure_page_texture(&mut self, ctx: &egui::Context, page: usize) {
        let zoom = self.zoom;
        if (self.texture_zoom - zoom).abs() > 0.001 {
            self.page_textures.clear();
            self.texture_zoom = zoom;
        }
        if self.page_textures.contains_key(&page) {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        match session.render_page(page, zoom) {
            Ok(rendered) => {
                let img = rendered.to_color_image();
                let tex = ctx.load_texture(
                    format!("page-{page}-{zoom:.3}"),
                    img,
                    egui::TextureOptions::LINEAR,
                );
                self.page_textures.insert(page, tex);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn prune_texture_cache(&mut self) {
        let current = self.page as isize;
        self.page_textures.retain(|&p, _| {
            let d = (p as isize - current).abs();
            d <= TEXTURE_CACHE_RADIUS + 1
        });
    }

    fn page_display_size(&self, page: usize) -> egui::Vec2 {
        let zoom = self.zoom;
        if let Some(tex) = self.page_textures.get(&page) {
            return tex.size_vec2();
        }
        if let Some(session) = self.session.as_ref() {
            if let Ok((w, h)) = session.page_size(page) {
                return egui::vec2(w * zoom, h * zoom);
            }
        }
        egui::vec2(600.0, 800.0)
    }

    fn go_to_page(&mut self, page: usize) {
        let count = self.page_count();
        if count == 0 {
            return;
        }
        let page = page.min(count - 1);
        self.page = page;
        self.scroll_to_page = Some(page);
        self.search_hits = self
            .document_hits
            .iter()
            .filter(|(p, _)| *p == page)
            .map(|(_, r)| *r)
            .collect();
    }

    fn page_count(&self) -> usize {
        self.session
            .as_ref()
            .and_then(|s| s.page_count().ok())
            .unwrap_or(0)
    }

    fn poll_background(&mut self) {
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BackgroundMsg::FileDone(result) => {
                    self.busy = false;
                    match result {
                        Ok(message) => self.status = message,
                        Err(e) => self.error = Some(e),
                    }
                }
                BackgroundMsg::SearchDone(result) => {
                    self.busy = false;
                    match result {
                        Ok(hits) => {
                            self.document_hits = hits;
                            self.hit_index = 0;
                            self.focus_hit();
                        }
                        Err(e) => self.error = Some(e),
                    }
                }

                BackgroundMsg::OcrProgress(s) => self.status = s,
                BackgroundMsg::OcrModelsDone(res) => {
                    self.busy = false;
                    match res {
                        Ok(()) => self.status = "OCR models downloaded".into(),
                        Err(e) => self.error = Some(e),
                    }
                }
                BackgroundMsg::OcrPageDone { page, lines } => match lines {
                    Ok(lines) => self.apply_ocr_lines(page, lines),
                    Err(e) => {
                        self.status = format!("OCR failed on page {}: {e}", page + 1);
                    }
                },
                BackgroundMsg::OcrJobFinished {
                    ok,
                    failed,
                    cancelled,
                } => {
                    self.busy = false;
                    self.ocr_cancel = None;
                    self.show_ocr_overlays = true;
                    if cancelled {
                        self.status = format!("OCR cancelled — {ok} page(s) done, {failed} failed");
                    } else if failed > 0 {
                        self.status = format!("OCR finished — {ok} ok, {failed} failed");
                    } else {
                        self.status = format!("OCR finished — {ok} page(s)");
                    }
                }
                BackgroundMsg::Error(e) => {
                    self.busy = false;
                    self.ocr_cancel = None;
                    self.error = Some(e);
                }
            }
        }
    }

    fn apply_ocr_lines(&mut self, page: usize, lines: Vec<ocr::OcrLine>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Ok((_, _page_h)) = session.page_size(page) else {
            return;
        };
        let overlays: Vec<OcrOverlayLine> = lines
            .iter()
            .map(|l| OcrOverlayLine {
                text: l.text.clone(),
                x: l.x,
                y: l.y,
                width: l.width,
                height: l.height,
            })
            .collect();
        let mapped: Vec<(String, f32, f32, f32)> = lines
            .into_iter()
            .map(|l| {
                let pdf_x = l.x;
                let pdf_y = l.y;
                let fontsize = l.height.clamp(6.0, 36.0);
                (l.text, pdf_x, pdf_y, fontsize)
            })
            .collect();
        match session.add_ocr_text(page, &mapped) {
            Ok(()) => {
                self.document_hits.clear();
                self.search_hits.clear();
                self.ocr_overlays.insert(page, overlays);
                self.status = format!("OCR added {} text lines on page {}", mapped.len(), page + 1);
                self.invalidate_textures();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn start_ocr_pages(&mut self, pages: Vec<usize>) {
        if pages.is_empty() {
            return;
        }
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        if !ocr::models_installed() {
            self.error = Some("Download OCR models first (OCR panel)".into());
            self.show_ocr = true;
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let bytes = match session.write_bytes(CompressPreset::Balanced.write_options()) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.ocr_cancel = Some(cancel.clone());
        self.busy = true;
        self.status = format!("OCR page 1 / {}…", pages.len());
        let tx = self.bg_tx.clone();
        let total = pages.len();
        std::thread::spawn(move || {
            let zoom = 2.0f32;
            let session = match DocumentSession::from_bytes(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(BackgroundMsg::Error(e.to_string()));
                    return;
                }
            };
            let mut ok = 0usize;
            let mut failed = 0usize;
            let mut cancelled = false;
            for (i, page) in pages.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                let _ = tx.send(BackgroundMsg::OcrProgress(format!(
                    "OCR page {} / {}…",
                    i + 1,
                    total
                )));
                let result = (|| {
                    let rendered = session.render_page(page, zoom)?;
                    let lines =
                        ocr::recognize_rgba(rendered.width, rendered.height, &rendered.rgba)?;
                    Ok::<_, AppError>(
                        lines
                            .into_iter()
                            .map(|mut l| {
                                l.x /= zoom;
                                l.y /= zoom;
                                l.width /= zoom;
                                l.height /= zoom;
                                l
                            })
                            .collect::<Vec<_>>(),
                    )
                })()
                .map_err(|e| e.to_string());
                match &result {
                    Ok(_) => ok += 1,
                    Err(_) => failed += 1,
                }
                let _ = tx.send(BackgroundMsg::OcrPageDone {
                    page,
                    lines: result,
                });
            }
            let _ = tx.send(BackgroundMsg::OcrJobFinished {
                ok,
                failed,
                cancelled,
            });
        });
    }

    fn start_ocr_page(&mut self) {
        self.start_ocr_pages(vec![self.page]);
    }

    fn start_ocr_all_pages(&mut self) {
        let count = self.page_count();
        if count == 0 {
            return;
        }
        self.start_ocr_pages((0..count).collect());
    }

    fn start_ocr_range(&mut self) {
        let count = self.page_count();
        match page_range::parse_page_ranges(&self.ocr_range_text, count) {
            Ok(pages) => self.start_ocr_pages(pages),
            Err(e) => self.error = Some(e),
        }
    }

    fn cancel_ocr(&mut self) {
        if let Some(c) = &self.ocr_cancel {
            c.store(true, Ordering::Relaxed);
            self.status = "Cancelling OCR…".into();
        }
    }

    fn download_ocr_models(&mut self) {
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        self.busy = true;
        self.status = "Downloading OCR models…".into();
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let res = ocr::download_models(&mut |s| {
                let _ = tx.send(BackgroundMsg::OcrProgress(s.to_string()));
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(BackgroundMsg::OcrModelsDone(res));
        });
    }

    fn side_panels(&mut self, ui: &mut egui::Ui) {
        if self.busy {
            return;
        }
        if self.mode == ToolMode::VisualSign {
            egui::Panel::right("sign_panel")
                .default_size(320.0)
                .frame(crate::theme::chrome_frame(ui).inner_margin(egui::Margin::symmetric(12, 12)))
                .show(ui, |ui| {
                    crate::theme::hairline_left(ui);
                    ui.heading("Visual signature");
                    ui.label("Draw or import, place a preview, then apply.");
                    let (resp, painter) = ui.allocate_painter(
                        egui::vec2(ui.available_width().min(400.0), 150.0),
                        egui::Sense::click_and_drag(),
                    );
                    painter.rect_filled(resp.rect, 6.0, egui::Color32::WHITE);
                    painter.rect_stroke(
                        resp.rect,
                        6.0,
                        crate::theme::hairline(ui),
                        egui::StrokeKind::Outside,
                    );
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let local = (pos - resp.rect.min)
                            * egui::vec2(
                                self.sig_pad.pad_w as f32 / resp.rect.width(),
                                self.sig_pad.pad_h as f32 / resp.rect.height(),
                            );
                        if resp.drag_started() {
                            self.staged_sig = None;
                            self.sig_texture = None;
                            self.sig_pad.begin(local.x, local.y);
                        } else if resp.dragged() {
                            self.sig_pad.drag(local.x, local.y);
                        } else if resp.drag_stopped() {
                            self.sig_pad.end();
                        }
                    }
                    for stroke in &self.sig_pad.strokes {
                        for pair in stroke.windows(2) {
                            painter.line_segment(
                                [
                                    resp.rect.min
                                        + egui::vec2(
                                            pair[0].0 * resp.rect.width()
                                                / self.sig_pad.pad_w as f32,
                                            pair[0].1 * resp.rect.height()
                                                / self.sig_pad.pad_h as f32,
                                        ),
                                    resp.rect.min
                                        + egui::vec2(
                                            pair[1].0 * resp.rect.width()
                                                / self.sig_pad.pad_w as f32,
                                            pair[1].1 * resp.rect.height()
                                                / self.sig_pad.pad_h as f32,
                                        ),
                                ],
                                egui::Stroke::new(2.0, egui::Color32::from_rgb(20, 20, 40)),
                            );
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Clear").clicked() {
                            self.sig_pad.clear();
                            self.imported_sig = None;
                            self.sig_texture = None;
                            self.staged_sig = None;
                        }
                        if ui.button("Import PNG…").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Image", &["png", "jpg", "jpeg"])
                                .pick_file()
                            {
                                match image::open(&path) {
                                    Ok(img) => {
                                        let rgba = img.to_rgba8();
                                        self.imported_sig =
                                            Some((rgba.width(), rgba.height(), rgba.into_raw()));
                                        self.status = "Signature image loaded".into();
                                    }
                                    Err(e) => self.error = Some(e.to_string()),
                                }
                            }
                        }
                        if ui
                            .add_enabled(
                                !self.sig_pad.is_empty() || self.imported_sig.is_some(),
                                crate::theme::primary_button(ui, "Place on page"),
                            )
                            .clicked()
                        {
                            let (w, h, rgba) = self
                                .imported_sig
                                .clone()
                                .unwrap_or_else(|| self.sig_pad.to_rgba());
                            self.sig_texture = Some(ui.ctx().load_texture(
                                "signature-preview",
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [w as usize, h as usize],
                                    &rgba,
                                ),
                                egui::TextureOptions::LINEAR,
                            ));
                            self.placing_sig = true;
                            self.status = "Click on the page to place signature".into();
                        }
                    });
                    if let Some((page, mut area)) = self.staged_sig {
                        ui.separator();
                        ui.label("Signature preview (points)");
                        let mut w = area.width();
                        let mut h = area.height();
                        ui.horizontal(|ui| {
                            ui.label("X");
                            ui.add(egui::DragValue::new(&mut area.x0));
                            ui.label("Y");
                            ui.add(egui::DragValue::new(&mut area.y0));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Width");
                            ui.add(egui::DragValue::new(&mut w).range(2.0..=1000.0));
                            ui.label("Height");
                            ui.add(egui::DragValue::new(&mut h).range(2.0..=1000.0));
                        });
                        area.x1 = area.x0 + w;
                        area.y1 = area.y0 + h;
                        self.staged_sig = Some((page, area));
                        ui.label("Click elsewhere on the page to move the preview.");
                        if ui
                            .add(crate::theme::primary_button(ui, "Apply signature"))
                            .clicked()
                        {
                            let (w, h, rgba) = self
                                .imported_sig
                                .clone()
                                .unwrap_or_else(|| self.sig_pad.to_rgba());
                            if let Some(s) = self.session.as_mut() {
                                match s.stamp_image(page, &rgba, w, h, area) {
                                    Ok(()) => {
                                        self.content_changed("Signature applied");
                                        self.staged_sig = None;
                                        self.placing_sig = false;
                                    }
                                    Err(e) => self.error = Some(e.to_string()),
                                }
                            }
                        }
                        if ui.button("Cancel placement").clicked() {
                            self.staged_sig = None;
                            self.placing_sig = false;
                        }
                    }
                });
        }

        if self.mode == ToolMode::CertSign {
            egui::Panel::right("cert_panel")
                .default_size(320.0)
                .frame(crate::theme::chrome_frame(ui).inner_margin(egui::Margin::symmetric(12, 12)))
                .show(ui, |ui| {
                    crate::theme::hairline_left(ui);
                    ui.heading("Certificate signature");
                    ui.label("Import a .p12 / .pfx file.");
                    if ui.button("Choose PKCS#12…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("PKCS#12", &["p12", "pfx"])
                            .pick_file()
                        {
                            self.cert_path = Some(path);
                        }
                    }
                    if let Some(p) = &self.cert_path {
                        ui.label(p.display().to_string());
                    }
                    ui.horizontal(|ui| {
                        ui.label("Password");
                        ui.add(egui::TextEdit::singleline(&mut self.cert_password).password(true));
                    });
                    if ui.button("Load certificate").clicked() {
                        if let Some(path) = self.cert_path.clone() {
                            match CertIdentity::from_pkcs12(&path, &self.cert_password) {
                                Ok(id) => {
                                    self.status = format!("Loaded: {}", id.subject);
                                    self.cert_identity = Some(id);
                                    self.cert_password.clear();
                                }
                                Err(e) => self.error = Some(e.to_string()),
                            }
                        }
                    }
                    if let Some(id) = &self.cert_identity {
                        ui.label(format!("Subject: {}", id.subject));
                    }
                    ui.label("Reason");
                    ui.text_edit_singleline(&mut self.cert_reason);
                    if ui
                        .add_enabled(
                            self.cert_identity.is_some() && self.session.is_some(),
                            crate::theme::primary_button(ui, "Place & sign"),
                        )
                        .clicked()
                    {
                        self.placing_cert = true;
                        self.cert_place = None;
                        self.status = "Drag a rectangle on the page for the signature".into();
                    }
                });
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.show_compress {
            egui::Window::new("Compress PDF")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Choose a compression preset and save a new file.");
                    for preset in [
                        CompressPreset::Fast,
                        CompressPreset::Balanced,
                        CompressPreset::Small,
                    ] {
                        ui.radio_value(&mut self.compress_preset, preset, preset.label());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.session.is_some() && !self.busy,
                                crate::theme::primary_button(ui, "Compress & Save"),
                            )
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("PDF", &["pdf"])
                                .set_file_name("compressed.pdf")
                                .save_file()
                            {
                                if let Some(s) = self.session.as_ref() {
                                    match s.write_bytes(CompressPreset::Fast.write_options()) {
                                        Ok(bytes) => {
                                            let preset = self.compress_preset;
                                            self.show_compress = false;
                                            self.run_file_job("Compressing…",move || {
                                                let before = bytes.len();
                                                let mut s = DocumentSession::from_bytes(&bytes)?;
                                                s.compress_save(&path,preset)?;
                                                let after = std::fs::metadata(&path)?.len();
                                                Ok(format!("Compression: {:.1} KB → {:.1} KB ({:+.1}%) · {}",before as f64/1024.0,after as f64/1024.0,(after as f64/before as f64-1.0)*100.0,path.display()))
                                            });
                                        }
                                        Err(e) => self.error = Some(e.to_string()),
                                    }
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_compress = false;
                        }
                    });
                });
        }

        if self.show_ocr {
            egui::Window::new("OCR")
                .collapsible(false)
                .default_width(420.0)
                .show(ctx, |ui| {
                    ui.label("Local OCR uses downloadable neural models (Latin script).");
                    ui.label(
                        "Results embed an invisible searchable layer and show boxes + text on screen.",
                    );
                    if ocr::models_installed() {
                        ui.colored_label(crate::theme::system_green(ui), "Models installed");
                    } else {
                        ui.colored_label(crate::theme::system_red(ui), "Models not installed");
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Download models"))
                            .clicked()
                        {
                            self.download_ocr_models();
                        }
                        if ui
                            .add_enabled(
                                !self.busy && self.session.is_some() && ocr::models_installed(),
                                egui::Button::new("OCR this page"),
                            )
                            .clicked()
                        {
                            self.start_ocr_page();
                        }
                        if ui
                            .add_enabled(
                                !self.busy && self.session.is_some() && ocr::models_installed(),
                                egui::Button::new("OCR all pages"),
                            )
                            .clicked()
                            {
                                self.start_ocr_all_pages();
                            }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Range");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.ocr_range_text)
                                .hint_text("1-3,5")
                                .desired_width(120.0),
                        );
                        if ui
                            .add_enabled(
                                !self.busy && self.session.is_some() && ocr::models_installed(),
                                egui::Button::new("OCR range"),
                            )
                            .clicked()
                        {
                            self.start_ocr_range();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.busy && self.ocr_cancel.is_some(), egui::Button::new("Cancel"))
                            .clicked()
                        {
                            self.cancel_ocr();
                        }
                        ui.checkbox(&mut self.show_ocr_overlays, "Show OCR overlays");
                        if ui.button("Clear overlays").clicked() {
                            self.ocr_overlays.clear();
                        }
                        if ui.button("Copy page text").clicked() {
                            if let Some(lines) = self.ocr_overlays.get(&self.page) {
                                let text = lines
                                    .iter()
                                    .map(|l| l.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ui.ctx().copy_text(text);
                                self.status = "Copied OCR text for this page".into();
                            } else {
                                self.status = "No OCR text on this page".into();
                            }
                        }
                        if ui.button("Close").clicked() {
                            self.show_ocr = false;
                        }
                    });
                    if self.busy {
                        ui.spinner();
                        ui.label(&self.status);
                    }
                    if let Some(lines) = self.ocr_overlays.get(&self.page) {
                        ui.separator();
                        ui.label(format!(
                            "Recognized on page {} ({} lines):",
                            self.page + 1,
                            lines.len()
                        ));
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                for line in lines {
                                    ui.label(&line.text);
                                }
                            });
                    }
                });
        }

        if self.show_merge {
            egui::Window::new("Merge PDFs")
                .collapsible(false)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.label("Pick two or more PDFs. Order is merge order.");
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Add files…"))
                            .clicked()
                        {
                            if let Some(paths) = rfd::FileDialog::new()
                                .add_filter("PDF", &["pdf"])
                                .pick_files()
                            {
                                self.merge_paths.extend(paths);
                            }
                        }
                        if ui.button("Clear list").clicked() {
                            self.merge_paths.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            let mut remove = None;
                            for (i, path) in self.merge_paths.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}. {}", i + 1, path.display()));
                                    if ui.small_button("✕").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            }
                            if let Some(i) = remove {
                                self.merge_paths.remove(i);
                            }
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.busy && self.merge_paths.len() >= 2,
                                crate::theme::primary_button(ui, "Save merged…"),
                            )
                            .clicked()
                        {
                            self.run_merge();
                        }
                        if ui.button("Close").clicked() {
                            self.show_merge = false;
                        }
                    });
                });
        }

        if self.show_split {
            egui::Window::new("Split PDF")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Each comma-separated range becomes its own file (e.g. 1-2,4).");
                    ui.horizontal(|ui| {
                        ui.label("Ranges");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.split_range_text)
                                .hint_text("1-3,5")
                                .desired_width(160.0),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.busy && self.session.is_some(),
                                crate::theme::primary_button(ui, "Split to files…"),
                            )
                            .clicked()
                        {
                            self.run_split();
                        }
                        if ui.button("Close").clicked() {
                            self.show_split = false;
                        }
                    });
                });
        }

        if self.show_extract {
            egui::Window::new("Extract pages")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Save selected pages as one new PDF.");
                    ui.horizontal(|ui| {
                        ui.label("Pages");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.extract_range_text)
                                .hint_text("1-3,5")
                                .desired_width(160.0),
                        );
                    });
                    ui.checkbox(
                        &mut self.extract_open_after,
                        "Open extracted file in this window",
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.busy && self.session.is_some(),
                                crate::theme::primary_button(ui, "Extract…"),
                            )
                            .clicked()
                        {
                            self.run_extract();
                        }
                        if ui.button("Close").clicked() {
                            self.show_extract = false;
                        }
                    });
                });
        }

        if self.show_export {
            egui::Window::new("Export text")
                .collapsible(false)
                .show(ctx, |ui| {
                    if export::ANYDOC_MARKDOWN {
                        ui.label("Markdown via anydoc (structured GFM). Word still uses MuPDF text.");
                    } else {
                        ui.label("Export embedded PDF text to Markdown or Word.");
                    }
                    ui.label("Text export does not preserve the original layout, images, or tables. For scanned pages, run OCR first.");
                    ui.horizontal(|ui| {
                        ui.label("Format");
                        ui.selectable_value(
                            &mut self.export_format,
                            ExportFormat::Markdown,
                            if export::ANYDOC_MARKDOWN {
                                "Markdown (.md, anydoc)"
                            } else {
                                "Markdown (.md)"
                            },
                        );
                        ui.selectable_value(
                            &mut self.export_format,
                            ExportFormat::Docx,
                            "Word (.docx)",
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Pages");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.export_range_text)
                                .hint_text("1-3,5")
                                .desired_width(160.0),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.busy && self.session.is_some(),
                                crate::theme::primary_button(ui, "Export…"),
                            )
                            .clicked()
                        {
                            self.run_export();
                        }
                        if ui.button("Close").clicked() {
                            self.show_export = false;
                        }
                    });
                });
        }

        if let Some(err) = self.error.clone() {
            egui::Window::new("Error")
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(&err);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(crate::theme::primary_button(ui, "OK")).clicked() {
                            self.error = None;
                        }
                    });
                });
        }
    }

    fn viewer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.session.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.5 - 190.0).max(24.0));
                if let Some(logo) = &self.logo {
                    ui.add(
                        egui::Image::new((logo.id(), logo.size_vec2()))
                            .fit_to_exact_size(egui::vec2(112.0, 112.0))
                            .corner_radius(25.0),
                    );
                    ui.add_space(20.0);
                }
                ui.heading("Your documents. Your workspace.");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Read, organize, annotate, and sign — all on your device.")
                        .weak()
                        .size(14.0),
                );
                ui.add_space(18.0);
                let open = egui::Button::new(
                    egui::RichText::new("Open PDF…")
                        .color(egui::Color32::WHITE)
                        .size(14.0),
                )
                .fill(crate::theme::accent(ui))
                .stroke(egui::Stroke::NONE)
                .corner_radius(7.0)
                .min_size(egui::vec2(96.0, 32.0));
                if ui.add(open).clicked() {
                    self.open_dialog();
                }
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("or drop a PDF into this window")
                        .weak()
                        .size(12.0),
                );
                if !self.recent.is_empty() {
                    ui.add_space(28.0);
                    ui.label(egui::RichText::new("Recent").weak().size(11.0));
                    ui.add_space(4.0);
                    for path in self.recent.clone() {
                        if ui
                            .button(path.file_name().unwrap_or_default().to_string_lossy())
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            self.open_path(path);
                        }
                    }
                }
            });
            return;
        }

        let avail_w = ui.available_width() - 24.0;
        self.update_zoom(avail_w.max(100.0));
        if self.fit_page {
            if let Some(Ok((w, h))) = self.session.as_ref().map(|s| s.page_size(self.page)) {
                self.zoom = (avail_w / w)
                    .min((ui.available_height() - 24.0) / h)
                    .clamp(0.1, 4.0);
            }
        }

        let count = self.page_count();
        let zoom = self.zoom;
        let mode = self.mode;
        let viewport = ui.clip_rect();

        // Warm textures around the current page (updates as you scroll).
        let near_start = self.page.saturating_sub(TEXTURE_CACHE_RADIUS as usize);
        let near_end = (self.page + TEXTURE_CACHE_RADIUS as usize + 1).min(count);
        for page_idx in near_start..near_end {
            self.ensure_page_texture(ctx, page_idx);
        }
        self.prune_texture_cache();

        let scroll_target = self.scroll_to_page.take();
        let mut best_page = self.page;
        let mut best_overlap = 0.0f32;
        let mut interaction: Option<PageInteraction> = None;

        egui::ScrollArea::both()
            .id_salt("pdf_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                for page_idx in 0..count {
                    let size = self.page_display_size(page_idx);

                    ui.horizontal(|ui| {
                        let pad = ((ui.available_width() - size.x) * 0.5).max(0.0);
                        ui.add_space(pad);

                        let sense = if self.busy {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click_and_drag()
                        };

                        let (response, painter) = ui.allocate_painter(size, sense);
                        let rect = response.rect;

                        if scroll_target == Some(page_idx) {
                            response.scroll_to_me(Some(egui::Align::TOP));
                        }

                        let overlap = rect.intersect(viewport).height();
                        if overlap > best_overlap {
                            best_overlap = overlap;
                            best_page = page_idx;
                        }

                        // Soft shadow makes the page float over the viewer surface.
                        painter.add(crate::theme::page_shadow(ui, rect));
                        if let Some(tex) = self.page_textures.get(&page_idx) {
                            painter.image(
                                tex.id(),
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                            painter.rect_stroke(
                                rect,
                                0.0,
                                crate::theme::hairline(ui),
                                egui::StrokeKind::Outside,
                            );
                        } else {
                            painter.rect_filled(rect, 8.0, ui.visuals().faint_bg_color);
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("Page {}", page_idx + 1),
                                egui::FontId::proportional(13.0),
                                crate::theme::secondary_text(ui),
                            );
                            painter.rect_stroke(
                                rect,
                                8.0,
                                crate::theme::hairline(ui),
                                egui::StrokeKind::Outside,
                            );
                        }

                        if self.show_search {
                            for (_, hit) in
                                self.document_hits.iter().filter(|(p, _)| *p == page_idx)
                            {
                                let r = pdf_rect_to_screen(hit, zoom, rect);
                                painter.rect_filled(
                                    r,
                                    0.0,
                                    egui::Color32::from_rgba_unmultiplied(255, 214, 10, 76),
                                );
                            }
                        }

                        if let (Some((p, area)), Some(tex)) = (self.staged_sig, &self.sig_texture) {
                            if p == page_idx {
                                painter.image(
                                    tex.id(),
                                    pdf_rect_to_screen(&area, zoom, rect),
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                                painter.rect_stroke(
                                    pdf_rect_to_screen(&area, zoom, rect),
                                    0.0,
                                    egui::Stroke::new(1.0, crate::theme::accent(ui)),
                                    egui::StrokeKind::Outside,
                                );
                            }
                        }
                        if self.show_ocr_overlays {
                            if let Some(lines) = self.ocr_overlays.get(&page_idx) {
                                for line in lines {
                                    // OcrLine.y is bottom of box in top-left PDF points.
                                    let top = line.y - line.height;
                                    let box_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            rect.min.x + line.x * zoom,
                                            rect.min.y + top * zoom,
                                        ),
                                        egui::vec2(line.width * zoom, line.height * zoom),
                                    );
                                    painter.rect_stroke(
                                        box_rect,
                                        2.0,
                                        egui::Stroke::new(1.25, crate::theme::accent(ui)),
                                        egui::StrokeKind::Outside,
                                    );
                                    painter.rect_filled(
                                        box_rect,
                                        2.0,
                                        egui::Color32::from_rgba_unmultiplied(10, 132, 255, 26),
                                    );
                                    let font_size = (line.height * zoom * 0.85).clamp(9.0, 22.0);
                                    painter.text(
                                        egui::pos2(box_rect.min.x + 2.0, box_rect.min.y + 1.0),
                                        egui::Align2::LEFT_TOP,
                                        &line.text,
                                        egui::FontId::proportional(font_size),
                                        egui::Color32::from_rgb(10, 40, 90),
                                    );
                                }
                            }
                        }

                        if let Some((p, area)) = self.selection {
                            if p == page_idx {
                                painter.rect_filled(
                                    pdf_rect_to_screen(&area, zoom, rect),
                                    0.0,
                                    egui::Color32::from_rgba_unmultiplied(0, 122, 255, 44),
                                );
                            }
                        }
                        // Live drag tracking + deferred commit events.
                        match mode {
                            ToolMode::Draw => {
                                if response.drag_started() {
                                    self.ink_points.clear();
                                    self.crop_page = Some(page_idx);
                                }
                                if self.crop_page == Some(page_idx) {
                                    if response.dragged() {
                                        if let Some(pos) = response.interact_pointer_pos() {
                                            let local =
                                                (pos.clamp(rect.min, rect.max) - rect.min) / zoom;
                                            self.ink_points
                                                .push(mupdf::Point::new(local.x, local.y));
                                        }
                                    }
                                    for pair in self.ink_points.windows(2) {
                                        painter.line_segment(
                                            [
                                                rect.min + egui::vec2(pair[0].x, pair[0].y) * zoom,
                                                rect.min + egui::vec2(pair[1].x, pair[1].y) * zoom,
                                            ],
                                            egui::Stroke::new(
                                                2.0,
                                                egui::Color32::from_rgb(30, 90, 180),
                                            ),
                                        );
                                    }
                                    if response.drag_stopped() {
                                        if let Some(s) = self.session.as_mut() {
                                            match s.draw_ink(page_idx, &self.ink_points) {
                                                Ok(()) => self.content_changed("Drawing added"),
                                                Err(e) => self.error = Some(e.to_string()),
                                            }
                                        }
                                        self.ink_points.clear();
                                        self.crop_page = None;
                                    }
                                }
                            }

                            ToolMode::Crop
                            | ToolMode::View
                            | ToolMode::Highlight
                            | ToolMode::Note
                            | ToolMode::TextBox
                            | ToolMode::Redact
                            | ToolMode::EditText => {
                                if response.drag_started() {
                                    if let Some(pos) = response.interact_pointer_pos() {
                                        self.crop_start = Some(pos);
                                        self.crop_end = Some(pos);
                                        self.crop_page = Some(page_idx);
                                    }
                                }
                                if self.crop_page == Some(page_idx) {
                                    if response.dragged() {
                                        self.crop_end = response.interact_pointer_pos();
                                    }
                                    if response.drag_stopped() {
                                        if let (Some(a), Some(b)) = (self.crop_start, self.crop_end)
                                        {
                                            interaction = Some(if mode == ToolMode::Crop {
                                                PageInteraction::CropEnd {
                                                    page: page_idx,
                                                    start: a,
                                                    end: b,
                                                    rect,
                                                }
                                            } else {
                                                PageInteraction::SelectEnd {
                                                    page: page_idx,
                                                    start: a,
                                                    end: b,
                                                    rect,
                                                }
                                            });
                                        }
                                    }
                                    if let (Some(a), Some(b)) = (self.crop_start, self.crop_end) {
                                        painter.rect_stroke(
                                            egui::Rect::from_two_pos(a, b),
                                            0.0,
                                            egui::Stroke::new(1.25, crate::theme::accent(ui)),
                                            egui::StrokeKind::Outside,
                                        );
                                    }
                                }
                            }
                            ToolMode::VisualSign if self.placing_sig => {
                                if response.clicked() {
                                    if let Some(pos) = response.interact_pointer_pos() {
                                        interaction = Some(PageInteraction::SignClick {
                                            page: page_idx,
                                            pos,
                                            rect,
                                        });
                                    }
                                }
                            }
                            ToolMode::CertSign if self.placing_cert => {
                                if response.drag_started() {
                                    if let Some(pos) = response.interact_pointer_pos() {
                                        self.cert_place = Some((pos, pos));
                                        self.cert_page = Some(page_idx);
                                    }
                                }
                                if self.cert_page == Some(page_idx) {
                                    if response.dragged() {
                                        if let Some((start, _)) = self.cert_place {
                                            if let Some(p) = response.interact_pointer_pos() {
                                                self.cert_place = Some((start, p));
                                            }
                                        }
                                    }
                                    if response.drag_stopped() {
                                        if let Some((a, b)) = self.cert_place {
                                            interaction = Some(PageInteraction::CertEnd {
                                                page: page_idx,
                                                start: a,
                                                end: b,
                                                rect,
                                            });
                                        }
                                    }
                                    if let Some((a, b)) = self.cert_place {
                                        painter.rect_stroke(
                                            egui::Rect::from_two_pos(a, b),
                                            0.0,
                                            egui::Stroke::new(1.25, crate::theme::system_green(ui)),
                                            egui::StrokeKind::Outside,
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    });

                    ui.add_space(PAGE_GAP);
                }
            });

        if best_overlap > 0.0 && best_page != self.page {
            self.page = best_page;
        }

        if let Some(ev) = interaction {
            self.apply_page_interaction(ev, zoom);
        }
    }

    fn apply_page_interaction(&mut self, ev: PageInteraction, zoom: f32) {
        match ev {
            PageInteraction::SelectEnd {
                page,
                start,
                end,
                rect,
            } => {
                let area = screen_rect_to_pdf(
                    egui::Rect::from_two_pos(start, end).intersect(rect),
                    zoom,
                    rect,
                    0.0,
                );
                if matches!(self.mode, ToolMode::Redact | ToolMode::EditText) {
                    self.prepare_edit(page, area);
                    self.crop_start = None;
                    self.crop_end = None;
                    self.crop_page = None;
                    return;
                }
                if let Some(s) = self.session.as_mut() {
                    let result = match self.mode {
                        ToolMode::View => s.selected_text(page, area).map(|(text, _)| {
                            self.status = format!(
                                "Selected {} characters — copy with ⌘/Ctrl+C",
                                text.chars().count()
                            );
                            self.selection = Some((page, area));
                        }),
                        mode => s.annotate(
                            page,
                            area,
                            &self.annotation_text,
                            match mode {
                                ToolMode::Highlight => 0,
                                ToolMode::Note => 1,
                                _ => 2,
                            },
                        ),
                    };
                    match result {
                        Ok(()) => {
                            if self.mode != ToolMode::View {
                                self.content_changed("Annotation added");
                            }
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                self.crop_start = None;
                self.crop_end = None;
                self.crop_page = None;
            }
            PageInteraction::CropEnd {
                page,
                start,
                end,
                rect,
            } => {
                if let Some(session) = self.session.as_mut() {
                    if let Ok((_, page_h)) = session.page_size(page) {
                        let screen = egui::Rect::from_two_pos(start, end).intersect(rect);
                        let crop = screen_rect_to_pdf(screen, zoom, rect, page_h);
                        if let Err(e) = session.set_crop(page, crop) {
                            self.error = Some(e.to_string());
                        } else {
                            self.page = page;
                            self.status = format!("Crop applied on page {}", page + 1);
                            self.content_changed("Crop applied");
                        }
                    }
                }
                self.crop_start = None;
                self.crop_end = None;
                self.crop_page = None;
            }
            PageInteraction::SignClick { page, pos, rect } => {
                self.page = page;
                let (w, h, _) = self
                    .imported_sig
                    .clone()
                    .unwrap_or_else(|| self.sig_pad.to_rgba());
                let a = (pos - rect.min) / zoom;
                let (width, height) = self
                    .staged_sig
                    .map(|(_, r)| (r.width(), r.height()))
                    .unwrap_or((150.0, 150.0 * h as f32 / w as f32));
                self.staged_sig = Some((page, PdfRect::new(a.x, a.y, a.x + width, a.y + height)));
            }
            PageInteraction::CertEnd {
                page,
                start,
                end,
                rect,
            } => {
                self.page = page;
                self.apply_cert_signature(start, end, rect, zoom);
                self.placing_cert = false;
                self.cert_place = None;
                self.cert_page = None;
            }
        }
    }

    fn apply_cert_signature(
        &mut self,
        a: egui::Pos2,
        b: egui::Pos2,
        page_rect: egui::Rect,
        zoom: f32,
    ) {
        let Some(identity) = self.cert_identity.as_ref() else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Ok((_, page_h)) = session.page_size(self.page) else {
            return;
        };
        let screen = egui::Rect::from_two_pos(a, b);
        let crop = screen_rect_to_pdf(screen, zoom, page_rect, page_h);
        let crop = match session.raw_rect(self.page, crop) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let rect = [crop.x0, crop.y0, crop.x1, crop.y1];

        let bytes = match session.write_bytes(CompressPreset::Balanced.write_options()) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let reason = self.cert_reason.clone();
        let page = self.page;
        match cert::sign_pdf_bytes(&bytes, identity, page, rect, &reason) {
            Ok(signed) => {
                // Write to temp then reload, or save-as.
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .set_file_name("signed.pdf")
                    .save_file()
                {
                    if let Err(e) = pdf::atomic_write(&path, &signed) {
                        self.error = Some(e.to_string());
                        return;
                    }
                    match DocumentSession::open(&path) {
                        Ok(s) => {
                            drop(s);
                            self.load_path(path.clone());
                            self.status = format!("Signed → {}", path.display());
                        }
                        Err(e) => {
                            // File written even if reopen fails
                            self.status = format!("Signed file written ({e})");
                        }
                    }
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() || self.busy {
            return;
        }
        let next = ctx
            .input(|i| i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown));
        let prev =
            ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp));
        if !ctx.egui_wants_keyboard_input() && next && self.page + 1 < self.page_count() {
            self.go_to_page(self.page + 1);
        }
        if !ctx.egui_wants_keyboard_input() && prev && self.page > 0 {
            self.go_to_page(self.page.saturating_sub(1));
        }
        let open = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O));
        let save = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S));
        if open {
            self.open_dialog();
        }
        if save {
            if ctx.input(|i| i.modifiers.shift) {
                self.save_as();
            } else {
                self.save();
            }
        }
    }

    fn append_pdfs_dialog(&mut self) {
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_files()
        else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut appended = 0usize;
        for path in &paths {
            match session.append_pdf(path) {
                Ok(()) => appended += 1,
                Err(e) => {
                    self.error = Some(format!("Append failed ({}): {e}", path.display()));
                    break;
                }
            }
        }
        if appended > 0 {
            self.invalidate_textures();
            self.content_changed(&format!("Appended {appended} PDF(s)"));
        }
    }

    fn run_merge(&mut self) {
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        if self.merge_paths.len() < 2 {
            self.error = Some("Select at least two PDFs to merge".into());
            return;
        }
        let Some(out) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name("merged.pdf")
            .save_file()
        else {
            return;
        };
        let paths = self.merge_paths.clone();
        self.show_merge = false;
        self.run_file_job("Merging PDFs…", move || {
            pdf::merge_files_to_path(&paths, &out)?;
            Ok(format!("Merged → {}", out.display()))
        });
    }

    fn run_split(&mut self) {
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let count = match session.page_count() {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let groups = match page_range::parse_page_range_groups(&self.split_range_text, count) {
            Ok(g) => g,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let src = match self.materialize_session_path() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let stem = self
            .session
            .as_ref()
            .and_then(|s| s.path.as_ref())
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("split");
        let mut written = 0usize;
        for pages in &groups {
            let name = format!("{}-{}.pdf", stem, pages_filename_suffix(pages));
            let out = dir.join(name);
            match pdf::export_pages_to_path(&src, pages, &out) {
                Ok(()) => written += 1,
                Err(e) => {
                    self.error = Some(format!("Split failed ({}): {e}", out.display()));
                    return;
                }
            }
        }
        self.status = format!("Split into {written} file(s) in {}", dir.display());
        self.show_split = false;
    }

    fn run_extract(&mut self) {
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let count = match session.page_count() {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let pages = match page_range::parse_page_ranges(&self.extract_range_text, count) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let src = match self.materialize_session_path() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let default_name = format!(
            "{}-{}.pdf",
            self.session
                .as_ref()
                .and_then(|s| s.path.as_ref())
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("extract"),
            pages_filename_suffix(&pages)
        );
        let Some(out) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        match pdf::export_pages_to_path(&src, &pages, &out) {
            Ok(()) => {
                self.status = format!("Extracted → {}", out.display());
                self.show_extract = false;
                if self.extract_open_after {
                    self.open_path(out);
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn run_export(&mut self) {
        if self.busy {
            self.error = Some("Another job is already running".into());
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let count = match session.page_count() {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let pages = match page_range::parse_page_ranges(&self.export_range_text, count) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let stem = self
            .session
            .as_ref()
            .and_then(|s| s.path.as_ref())
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("export");
        let title = self
            .session
            .as_ref()
            .and_then(|s| s.path.as_ref())
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        let suffix = pages_filename_suffix(&pages);
        let (ext, filter_name, filter_exts) = match self.export_format {
            ExportFormat::Markdown => ("md", "Markdown", &["md"][..]),
            ExportFormat::Docx => ("docx", "Word", &["docx"][..]),
        };
        let default_name = format!("{stem}-{suffix}.{ext}");
        let Some(out) = rfd::FileDialog::new()
            .add_filter(filter_name, filter_exts)
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };

        let result = match self.export_format {
            #[cfg(feature = "anydoc-export")]
            ExportFormat::Markdown => self.export_markdown_anydoc(&title, &pages, count, &out),
            #[cfg(not(feature = "anydoc-export"))]
            ExportFormat::Markdown => {
                let page_texts = match export::collect_page_texts(session, &pages) {
                    Ok(t) => t,
                    Err(e) => {
                        self.error = Some(e.to_string());
                        return;
                    }
                };
                export::write_markdown(&title, &page_texts, &out)
            }
            ExportFormat::Docx => {
                let page_texts = match export::collect_page_texts(session, &pages) {
                    Ok(t) => t,
                    Err(e) => {
                        self.error = Some(e.to_string());
                        return;
                    }
                };
                export::write_docx(&title, &page_texts, &out)
            }
        };
        match result {
            Ok(()) => {
                let backend = if matches!(self.export_format, ExportFormat::Markdown)
                    && export::ANYDOC_MARKDOWN
                {
                    " (anydoc)"
                } else {
                    ""
                };
                self.status = format!("Exported{backend} → {}", out.display());
                self.show_export = false;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    #[cfg(feature = "anydoc-export")]
    fn export_markdown_anydoc(
        &self,
        title: &str,
        pages: &[usize],
        page_count: usize,
        out: &std::path::Path,
    ) -> crate::error::Result<()> {
        let src = self
            .materialize_session_path()
            .map_err(crate::error::AppError::msg)?;
        let (pdf, is_temp) = export::pdf_for_page_selection(&src, pages, page_count)?;
        let result = export::write_markdown_anydoc(title, &pdf, out);
        if is_temp {
            let _ = std::fs::remove_file(&pdf);
        }
        result
    }

    /// Path to current document bytes on disk (writes a temp file when dirty / unsaved).
    fn materialize_session_path(&self) -> Result<tempfile::TempPath, String> {
        use std::io::Write;
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "No document open".to_string())?;
        let bytes = session
            .write_bytes(CompressPreset::Balanced.write_options())
            .map_err(|e| e.to_string())?;
        let mut file = tempfile::Builder::new()
            .prefix("pdf-opener-export-")
            .suffix(".pdf")
            .tempfile()
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok(file.into_temp_path())
    }
}

impl eframe::App for PdfApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background();
        self.workspace_events(ctx);
        self.handle_keys(ctx);
        self.dialogs(ctx);
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("toolbar")
            .frame(crate::theme::chrome_frame(ui).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                self.toolbar(ui);
                crate::theme::hairline_bottom(ui);
            });
        egui::Panel::bottom("status")
            .frame(crate::theme::chrome_frame(ui).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| {
                crate::theme::hairline_top(ui);
                self.status_bar(ui);
            });
        self.page_sidebar(ui, &ctx);
        self.side_panels(ui);
        self.editor_panel(ui);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::viewer_bg(ui))
                    .inner_margin(16.0),
            )
            .show(ui, |ui| self.viewer(ui, &ctx));
    }
}

fn pdf_rect_to_screen(r: &PdfRect, zoom: f32, page_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        page_rect.min + egui::vec2(r.x0, r.y0) * zoom,
        page_rect.min + egui::vec2(r.x1, r.y1) * zoom,
    )
}

/// App logo as a texture, shared by the toolbar mark and welcome screen.
fn load_logo_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon-256.png")).ok()?;
    Some(ctx.load_texture(
        "app-logo",
        egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        ),
        egui::TextureOptions::default(),
    ))
}

fn screen_rect_to_pdf(
    screen: egui::Rect,
    zoom: f32,
    page_rect: egui::Rect,
    _page_h: f32,
) -> PdfRect {
    let a = (screen.min - page_rect.min) / zoom;
    let b = (screen.max - page_rect.min) / zoom;
    PdfRect::new(a.x, a.y, b.x, b.y)
}
