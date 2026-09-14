//! PDF document session backed by MuPDF.
mod editor;
#[cfg(test)]
mod tests;
pub use editor::FormField;

use std::path::{Path, PathBuf};

use mupdf::pdf::{
    InsertImageOptions, InsertPdfOptions, InsertPosition, PageImageSource, PageSelection,
    PdfDocument, PdfWriteOptions,
};
use mupdf::{Colorspace, Image, Matrix, Point, Rect, TextExtractOptions};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressPreset {
    Fast,
    Balanced,
    Small,
}

impl CompressPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Balanced => "Balanced",
            Self::Small => "Small",
        }
    }

    pub fn write_options(self) -> PdfWriteOptions {
        let mut opts = PdfWriteOptions::default();
        opts.set_compress(true)
            .set_compress_images(true)
            .set_compress_fonts(true)
            .set_clean(true);
        match self {
            Self::Fast => {
                opts.set_garbage_level(1);
            }
            Self::Balanced => {
                opts.set_garbage_level(2);
            }
            Self::Small => {
                opts.set_garbage_level(4);
                opts.set_sanitize(true);
            }
        }
        opts
    }
}

struct Snapshot {
    bytes: Vec<u8>,
    revision: u64,
}

pub struct DocumentSession {
    pub path: Option<PathBuf>,
    doc: PdfDocument,
    pub dirty: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    revision: u64,
    saved_revision: u64,
    next_revision: u64,
}

impl DocumentSession {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let doc = PdfDocument::open(path)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            doc,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
            next_revision: 1,
        })
    }

    pub fn organize_pages(&mut self, pages: &[usize], action: u8) -> Result<()> {
        let count = self.page_count()?;
        if pages.is_empty() || pages.iter().any(|p| *p >= count) {
            return Err(AppError::msg("Invalid page selection"));
        }
        if action == 3 && pages.len() >= count {
            return Err(AppError::msg("Keep at least one page"));
        }
        if matches!(action, 4 | 5) && pages.len() != 1 {
            return Err(AppError::msg("Select one page to move"));
        }
        self.edit(|s| {
            for &page in pages.iter().rev() {
                match action {
                    0 | 1 => {
                        let mut p = s.doc.load_pdf_page(page as i32)?;
                        p.set_rotation((p.rotation()? + if action == 0 { 90 } else { 270 }) % 360)?;
                    }
                    2 => {
                        s.doc.copy_page(page, InsertPosition::After(page))?;
                    }
                    3 => s.doc.delete_pages(PageSelection::Pages(vec![page]))?,
                    4 if page > 0 => s.doc.move_page(page, page - 1)?,
                    5 if page + 1 < count => s.doc.move_page(page, page + 1)?,
                    6 => {
                        let p = s.doc.load_pdf_page(page as i32)?;
                        let mut object = p.object();
                        object.dict_delete("CropBox")?;
                    }
                    _ => return Err(AppError::msg("Cannot move beyond the document boundary")),
                }
            }
            Ok(())
        })
    }

    pub fn outlines(&self) -> Result<Vec<(String, usize, usize)>> {
        fn walk(nodes: Vec<mupdf::Outline>, depth: usize, out: &mut Vec<(String, usize, usize)>) {
            for node in nodes {
                if let Some(dest) = node.dest {
                    out.push((node.title, dest.loc.page_number as usize, depth));
                }
                walk(node.down, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(self.doc.outlines()?, 0, &mut out);
        Ok(out)
    }

    pub fn raw_rect(&self, page: usize, area: Rect) -> Result<Rect> {
        let p = self.doc.load_pdf_page(page as i32)?;
        let inverse = p
            .ctm()?
            .invert()
            .ok_or_else(|| AppError::msg("Invalid page transform"))?;
        Ok(area.transform(&inverse))
    }

    pub fn selected_text(&self, page: usize, area: Rect) -> Result<(String, Vec<mupdf::Quad>)> {
        let p = self.doc.load_pdf_page(page as i32)?;
        let words = p.words(TextExtractOptions::default())?;
        let mut text = String::new();
        let mut quads = Vec::new();
        let mut last_line = None;
        for word in words {
            let r = word.bounds;
            let cx = (r.x0 + r.x1) / 2.0;
            let cy = (r.y0 + r.y1) / 2.0;
            if cx >= area.x0 && cx <= area.x1 && cy >= area.y0 && cy <= area.y1 {
                if !text.is_empty() {
                    text.push(if last_line == Some((word.block, word.line)) {
                        ' '
                    } else {
                        '\n'
                    });
                }
                text.push_str(&word.text);
                last_line = Some((word.block, word.line));
                quads.push(r.into());
            }
        }
        Ok((text, quads))
    }

    pub fn annotate(&mut self, page: usize, area: Rect, text: &str, kind: u8) -> Result<()> {
        if area.width() < 2.0 || area.height() < 2.0 {
            return Err(AppError::msg("Drag a larger area"));
        }
        let quads = if kind == 0 {
            self.selected_text(page, area)?.1
        } else {
            Vec::new()
        };
        if kind == 0 && quads.is_empty() {
            return Err(AppError::msg(
                "No text selected. Run OCR first for scanned pages.",
            ));
        }
        if kind != 0 && text.trim().is_empty() {
            return Err(AppError::msg("Enter the annotation text first"));
        }
        self.edit(|s| {
            let mut p = s.doc.load_pdf_page(page as i32)?;
            match kind {
                0 => {
                    p.add_highlight_annotation(quads)?;
                }
                1 => {
                    p.add_text_annotation(area, text)?;
                }
                _ => {
                    p.add_free_text_annotation(area, text)?;
                }
            }
            p.update()?;
            Ok(())
        })
    }

    fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            bytes: self.write_bytes(PdfWriteOptions::default())?,
            revision: self.revision,
        })
    }

    fn edit(&mut self, apply: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let before = self.snapshot()?;
        if let Err(error) = apply(self) {
            self.doc = PdfDocument::from_bytes(&before.bytes)?;
            self.dirty = self.revision != self.saved_revision;
            return Err(error);
        }
        self.undo.push(before);
        // ponytail: whole-document snapshots; keep one large snapshot, use disk history if needed.
        while self.undo.len() > 1
            && (self.undo.len() > 20
                || self.undo.iter().map(|s| s.bytes.len()).sum::<usize>() > 64 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision = self.next_revision;
        self.next_revision += 1;
        self.dirty = true;
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> Result<()> {
        self.restore_history(false)
    }
    pub fn redo(&mut self) -> Result<()> {
        self.restore_history(true)
    }

    fn restore_history(&mut self, redo: bool) -> Result<()> {
        let source = if redo { &self.redo } else { &self.undo };
        let Some(previous) = source.last() else {
            return Ok(());
        };
        let doc = PdfDocument::from_bytes(&previous.bytes)?;
        let current = self.snapshot()?;
        let previous = if redo {
            self.redo.pop().unwrap()
        } else {
            self.undo.pop().unwrap()
        };
        if redo {
            self.undo.push(current);
        } else {
            self.redo.push(current);
        }
        self.doc = doc;
        self.revision = previous.revision;
        self.dirty = self.revision != self.saved_revision;
        Ok(())
    }

    /// Open a PDF from in-memory bytes (no path).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let doc = PdfDocument::from_bytes(bytes)?;
        Ok(Self {
            path: None,
            doc,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
            next_revision: 1,
        })
    }

    pub fn page_count(&self) -> Result<usize> {
        Ok(self.doc.page_count()?.max(0) as usize)
    }

    pub fn page_size(&self, page: usize) -> Result<(f32, f32)> {
        let p = self.doc.load_pdf_page(page as i32)?;
        let b = p.bounds()?;
        Ok((b.width(), b.height()))
    }

    /// Render a page to RGBA8 pixels at the given zoom (1.0 = 72 dpi).
    pub fn render_page(&self, page: usize, zoom: f32) -> Result<RenderedPage> {
        let p = self.doc.load_pdf_page(page as i32)?;
        let ctm = Matrix::new_scale(zoom, zoom);
        let cs = Colorspace::device_rgb();
        let pixmap = p.to_pixmap(&ctm, &cs, true, true)?;
        let width = pixmap.width();
        let height = pixmap.height();
        let samples = pixmap.samples().to_vec();
        // MuPDF may return RGB or RGBA depending on alpha flag; we requested alpha.
        let n = pixmap.n() as usize;
        let rgba = if n == 4 {
            samples
        } else if n == 3 {
            let mut out = Vec::with_capacity(width as usize * height as usize * 4);
            for chunk in samples.chunks_exact(3) {
                out.extend_from_slice(chunk);
                out.push(255);
            }
            out
        } else {
            return Err(AppError::pdf(format!("unexpected pixmap components: {n}")));
        };
        Ok(RenderedPage {
            width,
            height,
            rgba,
        })
    }

    pub fn extract_text(&self, page: usize) -> Result<String> {
        let p = self.doc.load_pdf_page(page as i32)?;
        Ok(p.text(TextExtractOptions::default())?)
    }

    pub fn search(&self, page: usize, needle: &str) -> Result<Vec<Rect>> {
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let p = self.doc.load_pdf_page(page as i32)?;
        let hits = p
            .to_text_page(mupdf::TextPageFlags::empty())?
            .search(needle)?;
        Ok(hits.into_iter().map(|q| q.into()).collect())
    }

    pub fn set_crop(&mut self, page: usize, crop: Rect) -> Result<()> {
        self.edit(|session| {
            if ![crop.x0, crop.y0, crop.x1, crop.y1]
                .iter()
                .all(|v| v.is_finite())
                || crop.width() < 2.0
                || crop.height() < 2.0
            {
                return Err(AppError::msg(
                    "Crop area must be at least 2 points wide and high",
                ));
            }
            let p = session.doc.load_pdf_page(page as i32)?;
            let inverse = p
                .ctm()?
                .invert()
                .ok_or_else(|| AppError::msg("Invalid page transform"))?;
            let raw = crop.transform(&inverse);
            let mut array = session.doc.new_array()?;
            for v in [raw.x0, raw.y0, raw.x1, raw.y1] {
                array.array_push(session.doc.new_real(v)?)?;
            }
            p.object().dict_put("CropBox", array)?;
            session.dirty = true;
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub fn crop_box(&self, page: usize) -> Result<Rect> {
        let p = self.doc.load_pdf_page(page as i32)?;
        Ok(p.crop_box()?)
    }

    pub fn stamp_image(
        &mut self,
        page: usize,
        rgba: &[u8],
        width: u32,
        height: u32,
        rect: Rect,
    ) -> Result<()> {
        let (pw, ph) = self.page_size(page)?;
        if ![rect.x0, rect.y0, rect.x1, rect.y1]
            .iter()
            .all(|v| v.is_finite())
            || rect.x0 < 0.0
            || rect.y0 < 0.0
            || rect.x1 > pw
            || rect.y1 > ph
            || rect.width() < 2.0
            || rect.height() < 2.0
        {
            return Err(AppError::msg("Keep the image entirely inside the page"));
        }
        self.edit(|session| {
            // Build an RGB pixmap-compatible Image via PNG encode round-trip for simplicity.
            let mut png_buf = Vec::new();
            {
                let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
                use image::ImageEncoder;
                encoder
                    .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
                    .map_err(|e| AppError::msg(e.to_string()))?;
            }
            let img = Image::from_bytes(&png_buf)?;
            let mut p = session.doc.load_pdf_page(page as i32)?;
            p.insert_image(
                &mut session.doc,
                rect,
                PageImageSource::Image(&img),
                InsertImageOptions {
                    overlay: true,
                    opacity: None,
                    optional_content: None,
                },
            )?;
            session.dirty = true;
            Ok(())
        })
    }

    /// Insert invisible OCR text (render mode 3) at page coordinates.
    pub fn add_ocr_text(&mut self, page: usize, lines: &[(String, f32, f32, f32)]) -> Result<()> {
        self.edit(|session| {
            use mupdf::shape::{Shape, TextOptions};
            let mut p = session.doc.load_pdf_page(page as i32)?;
            let mut shape = Shape::new(&mut p)?;
            for (text, x, y, fontsize) in lines {
                if text.trim().is_empty() {
                    continue;
                }
                let opts = TextOptions {
                    fontsize: *fontsize,
                    render_mode: 3, // invisible
                    ..Default::default()
                };
                shape.insert_text(Point::new(*x, *y), text, &opts)?;
            }
            shape.commit(&mut session.doc, true)?;
            session.dirty = true;
            Ok(())
        })
    }

    pub fn save(&mut self) -> Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| AppError::msg("No file path — use Save As"))?;
        self.save_as(&path, CompressPreset::Balanced.write_options())?;
        Ok(())
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>, options: PdfWriteOptions) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.write_bytes(options)?;
        atomic_write(path, &bytes)?;
        self.path = Some(path.to_path_buf());
        self.saved_revision = self.revision;
        self.dirty = false;
        Ok(())
    }

    pub fn compress_save(&mut self, path: impl AsRef<Path>, preset: CompressPreset) -> Result<()> {
        self.save_as(path, preset.write_options())
    }

    /// Write document bytes for certificate signing (full rewrite, no incremental).
    pub fn write_bytes(&self, options: PdfWriteOptions) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.doc.write_to_with_options(&mut buf, options)?;
        Ok(buf)
    }

    #[allow(dead_code)]
    pub fn reload_from_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let path = self.path.clone();
        let dirty = self.dirty;
        let doc = PdfDocument::from_bytes(bytes)?;
        self.doc = doc;
        self.path = path;
        self.dirty = dirty;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn duplicate_page_after(&mut self, page: usize) -> Result<()> {
        self.edit(|session| {
            session.doc.copy_page(page, InsertPosition::After(page))?;
            session.dirty = true;
            Ok(())
        })
    }

    /// Append all pages from another PDF onto the end of this document.
    pub fn append_pdf(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.edit(|session| {
            let src = PdfDocument::open(path.as_ref())?;
            session.doc.insert_pdf(
                &src,
                InsertPdfOptions {
                    source_pages: PageSelection::All,
                    target: InsertPosition::Append,
                    ..InsertPdfOptions::default()
                },
            )?;
            session.dirty = true;
            Ok(())
        })
    }
}

/// Merge multiple PDF files in order into a new file at `out`.
pub fn merge_files_to_path(paths: &[PathBuf], out: impl AsRef<Path>) -> Result<()> {
    if paths.len() < 2 {
        return Err(AppError::msg("Select at least two PDFs to merge"));
    }
    let mut session = DocumentSession::open(&paths[0])?;
    for path in &paths[1..] {
        session.append_pdf(path)?;
    }
    session.save_as(out.as_ref(), CompressPreset::Balanced.write_options())?;
    Ok(())
}

/// Export selected 0-based pages from `src` into a new PDF at `out`.
pub fn export_pages_to_path(
    src: impl AsRef<Path>,
    pages: &[usize],
    out: impl AsRef<Path>,
) -> Result<()> {
    if pages.is_empty() {
        return Err(AppError::msg("No pages selected to export"));
    }
    let mut session = DocumentSession::open(src.as_ref())?;
    session
        .doc
        .select_pages(PageSelection::Pages(pages.to_vec()))?;
    session.save_as(out.as_ref(), CompressPreset::Balanced.write_options())?;
    Ok(())
}

pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RenderedPage {
    pub fn to_color_image(&self) -> egui::ColorImage {
        egui::ColorImage::from_rgba_unmultiplied(
            [self.width as usize, self.height as usize],
            &self.rgba,
        )
    }
}

/// Never truncate the destination before a complete replacement is ready.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|e| AppError::msg(format!("Could not replace {}: {}", path.display(), e.error)))?;
    Ok(())
}
