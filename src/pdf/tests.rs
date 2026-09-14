use super::*;

fn sample() -> DocumentSession {
    DocumentSession::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/hello.pdf")).unwrap()
}

#[test]
fn edit_history_save_and_reopen() {
    let mut s = sample();
    let count = s.page_count().unwrap();
    s.organize_pages(&[0], 2).unwrap();
    assert_eq!(s.page_count().unwrap(), count + 1);
    assert!(s.dirty);
    s.undo().unwrap();
    assert_eq!(s.page_count().unwrap(), count);
    assert!(!s.dirty);
    s.redo().unwrap();
    assert_eq!(s.page_count().unwrap(), count + 1);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("edited.pdf");
    s.save_as(&out, CompressPreset::Balanced.write_options())
        .unwrap();
    assert!(!s.dirty);
    s.undo().unwrap();
    assert!(s.dirty);
    s.redo().unwrap();
    assert!(!s.dirty);
    assert_eq!(
        DocumentSession::open(out).unwrap().page_count().unwrap(),
        count + 1
    );
}

#[test]
fn invalid_edit_and_failed_save_preserve_document() {
    let mut s = sample();
    let text = s.extract_text(0).unwrap();
    assert!(s.organize_pages(&[0], 3).is_err());
    assert!(s.set_crop(0, Rect::new(0., 0., 0., 0.)).is_err());
    assert_eq!(s.extract_text(0).unwrap(), text);
    assert!(!s.dirty);
    s.organize_pages(&[0], 0).unwrap();
    let old_path = s.path.clone();
    let dir = tempfile::tempdir().unwrap();
    assert!(s
        .save_as(
            dir.path().join("missing/out.pdf"),
            CompressPreset::Balanced.write_options()
        )
        .is_err());
    assert!(s.dirty);
    assert_eq!(s.path, old_path);
}

#[test]
fn atomic_replacement_failure_keeps_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("folder");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep"), b"original").unwrap();
    assert!(atomic_write(&target, b"new").is_err());
    assert_eq!(std::fs::read(target.join("keep")).unwrap(), b"original");
}

#[test]
fn crop_roundtrip_respects_rotation_and_reset() {
    for rotate in [false, true] {
        let mut s = sample();
        if rotate {
            s.organize_pages(&[0], 0).unwrap();
        }
        let (w, h) = s.page_size(0).unwrap();
        s.set_crop(0, Rect::new(20., 30., w - 40., h - 50.))
            .unwrap();
        let (cw, ch) = s.page_size(0).unwrap();
        assert!((cw - (w - 60.)).abs() < 1.0, "{cw} vs {w}");
        assert!((ch - (h - 80.)).abs() < 1.0);
        s.organize_pages(&[0], 6).unwrap();
        assert_eq!(s.page_size(0).unwrap(), (w, h));
    }
}

#[test]
fn annotations_persist_and_undo() {
    let mut s = sample();
    s.annotate(0, Rect::new(20., 20., 180., 70.), "A saved note", 1)
        .unwrap();
    s.annotate(0, Rect::new(20., 100., 250., 140.), "Visible text", 2)
        .unwrap();
    s.draw_ink(0, &[Point::new(20., 90.), Point::new(180., 95.)])
        .unwrap();
    let bytes = s
        .write_bytes(CompressPreset::Balanced.write_options())
        .unwrap();
    let reopened = DocumentSession::from_bytes(&bytes).unwrap();
    assert_eq!(
        reopened.doc.load_pdf_page(0).unwrap().annotations().count(),
        3
    );
    s.undo().unwrap();
    assert_eq!(s.doc.load_pdf_page(0).unwrap().annotations().count(), 2);
}

#[test]
fn search_selection_and_redaction_remove_content() {
    let mut s = sample();
    // Add known content with a separately preserved text line.
    {
        let mut p = s.doc.load_pdf_page(0).unwrap();
        let mut shape = mupdf::shape::Shape::new(&mut p).unwrap();
        shape
            .insert_text(Point::new(30., 30.), "SECRET_TOKEN", &Default::default())
            .unwrap();
        shape
            .insert_text(Point::new(30., 125.), "PUBLIC_TOKEN", &Default::default())
            .unwrap();
        shape.commit(&mut s.doc, true).unwrap();
    }
    let hits = s.search(0, "SECRET_TOKEN").unwrap();
    assert_eq!(hits.len(), 1);
    let r = hits[0];
    let area = Rect::new(r.x0 - 2., r.y0 - 2., r.x1 + 2., r.y1 + 2.);
    assert!(s.selected_text(0, area).unwrap().0.contains("SECRET_TOKEN"));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("redacted.pdf");
    s.redacted_copy(0, area, &out).unwrap();
    let copy = DocumentSession::open(&out).unwrap();
    assert!(!copy.extract_text(0).unwrap().contains("SECRET_TOKEN"));
    assert!(copy.extract_text(0).unwrap().contains("PUBLIC_TOKEN"));
    assert!(s.extract_text(0).unwrap().contains("SECRET_TOKEN"));
    let rendered = copy.render_page(0, 1.0).unwrap();
    let x = ((area.x0 + area.x1) / 2.) as usize;
    let y = ((area.y0 + area.y1) / 2.) as usize;
    let offset = (y * rendered.width as usize + x) * 4;
    assert!(rendered.rgba[offset..offset + 3].iter().all(|v| *v < 30));
}

#[test]
fn paragraph_replacement_rolls_back_overflow_and_preserves_other_text() {
    let mut s = sample();
    let original = s.extract_text(0).unwrap();
    assert!(s
        .replace_paragraph(0, Rect::new(10., 10., 20., 20.), "Too much text", 24.)
        .is_err());
    assert_eq!(s.extract_text(0).unwrap(), original);
    assert!(!s.dirty);
    s.replace_paragraph(
        0,
        Rect::new(10., 90., 290., 140.),
        "Replacement paragraph wraps safely into this box.",
        12.,
    )
    .unwrap();
    let text = s.extract_text(0).unwrap();
    assert!(text.contains("Replacement paragraph"));
    assert!(text.contains(original.trim()));
    s.undo().unwrap();
    assert_eq!(s.extract_text(0).unwrap(), original);
}

#[test]
fn form_value_survives_save_and_undo() {
    use lopdf::{dictionary, Object};
    let original = sample().write_bytes(PdfWriteOptions::default()).unwrap();
    let mut doc = lopdf::Document::load_mem(&original).unwrap();
    let page = *doc.get_pages().values().next().unwrap();
    let field = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
        "T" => Object::string_literal("Name"), "V" => Object::string_literal("Before"),
        "Rect" => vec![20.into(),20.into(),200.into(),45.into()],
        "DA" => Object::string_literal("/F1 12 Tf 0 g"), "P" => page,
    });
    doc.get_object_mut(page)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Annots", vec![Object::Reference(field)]);
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let form = doc.add_object(
        dictionary! { "Fields" => vec![Object::Reference(field)], "NeedAppearances" => true },
    );
    doc.get_object_mut(root)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("AcroForm", form);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let mut s = DocumentSession::from_bytes(&bytes).unwrap();
    let fields = s.fields(0).unwrap();
    assert_eq!(fields.len(), 1);
    s.set_field(0, fields[0].xref, "After").unwrap();
    let reopened = DocumentSession::from_bytes(
        &s.write_bytes(CompressPreset::Balanced.write_options())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reopened.fields(0).unwrap()[0].value, "After");
    s.undo().unwrap();
    assert_eq!(s.fields(0).unwrap()[0].value, "Before");
}
