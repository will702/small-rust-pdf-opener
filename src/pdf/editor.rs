use super::*;
use mupdf::pdf::{
    PdfRedactImageMethod, PdfRedactLineArtMethod, PdfRedactOptions, PdfRedactTextMethod,
};

#[derive(Clone)]
pub struct FormField {
    pub xref: i32,
    pub name: String,
    pub value: String,
    pub readonly: bool,
    pub kind: String,
    pub choices: Vec<String>,
}

impl DocumentSession {
    pub fn fields(&self, page: usize) -> Result<Vec<FormField>> {
        let p = self.doc.load_pdf_page(page as i32)?;
        p.widgets()
            .map(|w| {
                let kind = format!("{:?}", w.r#type()?);
                let object = w.annotation().object();
                let mut choices = Vec::new();
                if kind == "Checkbox" || kind == "RadioButton" {
                    if let Some(ap) = object.get_dict("AP")? {
                        if let Some(normal) = ap.get_dict("N")? {
                            if normal.is_dict()? {
                                for i in 0..normal.dict_len()? {
                                    if let Some(key) = normal.get_dict_key(i as i32)? {
                                        choices.push(
                                            String::from_utf8_lossy(&key.as_name()?).into_owned(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if !choices.iter().any(|c| c == "Off") {
                        choices.insert(0, "Off".into());
                    }
                }
                if kind == "Combobox" || kind == "Listbox" {
                    if let Some(options) = object.get_dict_inheritable("Opt")? {
                        for i in 0..options.len()? {
                            if let Some(option) = options.get_array(i as i32)? {
                                let value = if option.is_array()? {
                                    option.get_array(0)?.and_then(|v| v.as_string().ok())
                                } else {
                                    option.as_string().ok()
                                };
                                if let Some(value) = value {
                                    choices.push(value);
                                }
                            }
                        }
                    }
                }
                Ok(FormField {
                    xref: w.xref()?,
                    name: w.name()?.unwrap_or_default(),
                    value: w.value()?.unwrap_or_default(),
                    readonly: w.is_readonly()?,
                    kind,
                    choices,
                })
            })
            .collect()
    }

    pub fn set_field(&mut self, page: usize, xref: i32, value: &str) -> Result<()> {
        self.edit(|s| {
            let p = s.doc.load_pdf_page(page as i32)?;
            let mut w = p
                .load_widget(xref)?
                .ok_or_else(|| AppError::msg("Form field no longer exists"))?;
            if w.is_readonly()? {
                return Err(AppError::msg("This field is read-only"));
            }
            if !w.set_value(&mut s.doc, value, true)? {
                return Err(AppError::msg("The field rejected this value"));
            }
            w.update()?;
            Ok(())
        })
    }

    pub fn draw_ink(&mut self, page: usize, points: &[Point]) -> Result<()> {
        if points.len() < 2 {
            return Ok(());
        }
        self.edit(|s| {
            let mut p = s.doc.load_pdf_page(page as i32)?;
            p.add_ink_annotation([points.iter().copied()])?;
            p.update()?;
            Ok(())
        })
    }

    /// Redactions are written to a new full-rewrite document, never an incremental revision.
    pub fn redacted_copy(&self, page: usize, area: Rect, path: &Path) -> Result<()> {
        if area.width() < 2.0 || area.height() < 2.0 {
            return Err(AppError::msg("Select a larger redaction area"));
        }
        let mut copy = Self::from_bytes(&self.write_bytes(PdfWriteOptions::default())?)?;
        let mut p = copy.doc.load_pdf_page(page as i32)?;
        p.add_redact_annotation(area)?;
        p.apply_redactions_with_options(PdfRedactOptions {
            black_boxes: true,
            image_method: PdfRedactImageMethod::Remove,
            line_art: PdfRedactLineArtMethod::RemoveIfTouched,
            text: PdfRedactTextMethod::Remove,
        })?;
        drop(p);
        // Remove metadata and embedded-file/name trees from the sanitized copy.
        let mut trailer = copy.doc.trailer()?;
        trailer.dict_delete("Info")?;
        if let Some(mut root) = trailer.get_dict("Root")? {
            root.dict_delete("Metadata")?;
            root.dict_delete("Names")?;
            root.dict_delete("AF")?;
        }
        copy.save_as(path, CompressPreset::Small.write_options())
    }

    pub fn paragraph_at(&self, page: usize, area: Rect) -> Result<(Rect, String)> {
        let p = self.doc.load_pdf_page(page as i32)?;
        let text = p.to_text_page(mupdf::TextPageFlags::empty())?.structured();
        let cx = (area.x0 + area.x1) / 2.0;
        let cy = (area.y0 + area.y1) / 2.0;
        for block in text.blocks {
            let r = block.bounds;
            if cx >= r.x0 && cx <= r.x1 && cy >= r.y0 && cy <= r.y1 {
                if let mupdf::TextBlockContent::Text { lines } = block.content {
                    if lines
                        .iter()
                        .any(|l| l.wmode != mupdf::WriteMode::Horizontal)
                    {
                        break;
                    }
                    return Ok((
                        r,
                        lines
                            .iter()
                            .map(|l| l.text.as_str())
                            .collect::<Vec<_>>()
                            .join(" "),
                    ));
                }
            }
        }
        Err(AppError::msg(
            "No supported text paragraph here. Scanned pages need OCR first.",
        ))
    }

    pub fn replace_paragraph(
        &mut self,
        page: usize,
        area: Rect,
        text: &str,
        size: f32,
    ) -> Result<()> {
        // Base-14 Helvetica cannot faithfully encode arbitrary scripts; never silently lose glyphs.
        if !text.is_ascii() {
            return Err(AppError::msg(
                "Paragraph replacement currently supports ASCII text with Helvetica only",
            ));
        }
        if text.trim().is_empty() || !size.is_finite() || !(6.0..=96.0).contains(&size) {
            return Err(AppError::msg(
                "Enter text and a font size from 6 to 96 points",
            ));
        }
        self.edit(|s| {
            let options = mupdf::shape::TextboxOptions {
                fontsize: size,
                ..Default::default()
            };
            let mut p = s.doc.load_pdf_page(page as i32)?;
            {
                let mut shape = mupdf::shape::Shape::new(&mut p)?;
                if shape.insert_textbox(area, text, &options)? < 0.0 {
                    return Err(AppError::msg(
                        "Text overflows the paragraph. Shorten it or reduce the font size.",
                    ));
                }
            }
            p.add_redact_annotation(area)?;
            p.apply_redactions_with_options(PdfRedactOptions {
                black_boxes: false,
                image_method: PdfRedactImageMethod::None,
                line_art: PdfRedactLineArtMethod::None,
                text: PdfRedactTextMethod::Remove,
            })?;
            let mut shape = mupdf::shape::Shape::new(&mut p)?;
            shape.insert_textbox(area, text, &options)?;
            shape.commit(&mut s.doc, true)?;
            Ok(())
        })
    }
}
