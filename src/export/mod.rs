//! Export PDF page text to Markdown or Word (.docx).
//!
//! Default Markdown uses MuPDF per-page text with `## Page N` headings.
//! With `--features anydoc-export`, Markdown can use Firecrawl [anydoc](https://github.com/firecrawl/anydoc)
//! for structured GFM (tables, headings, lists). Docx always uses the MuPDF path.

use std::path::Path;
#[cfg(feature = "anydoc-export")]
use std::path::PathBuf;

#[cfg(feature = "anydoc-export")]
use crate::error::AppError;
use crate::error::Result;
use crate::pdf::DocumentSession;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageText {
    /// 1-based page number for display.
    pub page_1based: usize,
    pub text: String,
}

/// Whether this build uses anydoc for Markdown export.
pub const ANYDOC_MARKDOWN: bool = cfg!(feature = "anydoc-export");

/// Collect embedded text for 0-based page indices via MuPDF.
pub fn collect_page_texts(session: &DocumentSession, pages: &[usize]) -> Result<Vec<PageText>> {
    let mut out = Vec::with_capacity(pages.len());
    for &page in pages {
        let text = session.extract_text(page)?;
        out.push(PageText {
            page_1based: page + 1,
            text,
        });
    }
    Ok(out)
}

/// Format page texts as agent-friendly Markdown with page headings.
pub fn format_markdown(title: &str, pages: &[PageText]) -> String {
    let mut s = String::new();
    s.push_str("# ");
    s.push_str(title);
    s.push_str("\n\n");
    for (i, page) in pages.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&format!("## Page {}\n\n", page.page_1based));
        let body = page.text.trim_end();
        if !body.is_empty() {
            s.push_str(body);
            s.push('\n');
        }
    }
    s
}

pub fn write_markdown(title: &str, pages: &[PageText], path: &Path) -> Result<()> {
    let content = format_markdown(title, pages);
    crate::pdf::atomic_write(path, content.as_bytes())?;
    Ok(())
}

/// Convert a PDF file to Markdown via anydoc (structured GFM).
///
/// Caller should pass a PDF that already contains only the desired pages
/// (use [`crate::pdf::export_pages_to_path`] for ranges).
#[cfg(feature = "anydoc-export")]
pub fn markdown_via_anydoc(pdf_path: &Path) -> Result<String> {
    anydoc::to_markdown(pdf_path).map_err(|e| AppError::msg(format!("anydoc: {e}")))
}

/// Write Markdown for `pdf_path` using anydoc. Prepends `# {title}` when the
/// body does not already start with that heading.
///
/// On anydoc failure, falls back to MuPDF page text so export still works on
/// PDFs that pdf-inspector rejects.
#[cfg(feature = "anydoc-export")]
pub fn write_markdown_anydoc(title: &str, pdf_path: &Path, out: &Path) -> Result<()> {
    match markdown_via_anydoc(pdf_path) {
        Ok(body) => {
            let content = ensure_title_heading(title, &body);
            crate::pdf::atomic_write(out, content.as_bytes())?;
            Ok(())
        }
        Err(anydoc_err) => {
            let session = DocumentSession::open(pdf_path)?;
            let indices: Vec<usize> = (0..session.page_count()?).collect();
            let page_texts = collect_page_texts(&session, &indices)?;
            write_markdown(title, &page_texts, out).map_err(|e| {
                AppError::msg(format!(
                    "anydoc failed ({anydoc_err}); MuPDF fallback also failed: {e}"
                ))
            })
        }
    }
}

/// Ensure `# title` is the first heading (anydoc may omit a document title).
#[cfg(feature = "anydoc-export")]
pub fn ensure_title_heading(title: &str, body: &str) -> String {
    let trimmed = body.trim_start();
    let expected = format!("# {title}");
    if trimmed.starts_with(&expected) {
        return body.to_string();
    }
    // If anydoc already starts with some H1, still prefix our source filename title.
    let mut s = String::with_capacity(expected.len() + 2 + body.len());
    s.push_str(&expected);
    s.push_str("\n\n");
    s.push_str(trimmed);
    if !trimmed.is_empty() && !trimmed.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Build a temp PDF of `pages` from `src` when the selection is a subset;
/// otherwise returns `src` unchanged (no copy).
#[cfg(feature = "anydoc-export")]
pub fn pdf_for_page_selection(
    src: &Path,
    pages: &[usize],
    page_count: usize,
) -> Result<(PathBuf, bool)> {
    let all_selected = pages.len() == page_count && pages.iter().enumerate().all(|(i, &p)| p == i);
    if all_selected {
        return Ok((src.to_path_buf(), false));
    }
    let tmp = std::env::temp_dir().join(format!(
        "pdf-opener-anydoc-pages-{}-{}.pdf",
        std::process::id(),
        pages.len()
    ));
    crate::pdf::export_pages_to_path(src, pages, &tmp)?;
    Ok((tmp, true))
}

pub fn write_docx(title: &str, pages: &[PageText], path: &Path) -> Result<()> {
    use docx_rs::*;

    let mut children: Vec<Paragraph> = Vec::new();
    children.push(Paragraph::new().add_run(Run::new().add_text(title).bold().size(32)));

    for (i, page) in pages.iter().enumerate() {
        if i > 0 {
            children.push(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));
        }
        children.push(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(format!("Page {}", page.page_1based))
                    .bold()
                    .size(28),
            ),
        );
        let body = page.text.trim_end();
        if body.is_empty() {
            children.push(Paragraph::new());
        } else {
            for line in body.split('\n') {
                children.push(Paragraph::new().add_run(Run::new().add_text(line)));
            }
        }
    }

    let mut doc = Docx::new();
    for p in children {
        doc = doc.add_paragraph(p);
    }
    let mut output = std::io::Cursor::new(Vec::new());
    doc.build()
        .pack(&mut output)
        .map_err(|e| crate::error::AppError::msg(format!("failed to write docx: {e}")))?;
    crate::pdf::atomic_write(path, &output.into_inner())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn format_markdown_includes_title_and_page_headings() {
        let pages = vec![
            PageText {
                page_1based: 1,
                text: "Hello\n".into(),
            },
            PageText {
                page_1based: 3,
                text: String::new(),
            },
        ];
        let md = format_markdown("invoice.pdf", &pages);
        assert!(md.starts_with("# invoice.pdf\n\n"));
        assert!(md.contains("## Page 1\n\nHello\n"));
        assert!(md.contains("## Page 3\n\n"));
        // Empty page still has heading; no body after heading before next section or EOF
        let page3 = md.find("## Page 3\n\n").unwrap();
        assert_eq!(&md[page3..], "## Page 3\n\n");
    }

    #[test]
    fn write_docx_produces_valid_zip_with_document_xml() {
        let dir =
            std::env::temp_dir().join(format!("pdf-opener-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.docx");
        let pages = vec![
            PageText {
                page_1based: 1,
                text: "Line one".into(),
            },
            PageText {
                page_1based: 2,
                text: String::new(),
            },
        ];
        write_docx("demo.pdf", &pages, &path).unwrap();
        assert!(path.metadata().unwrap().len() > 0);

        let file = std::fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut document = archive.by_name("word/document.xml").unwrap();
        let mut xml = String::new();
        document.read_to_string(&mut xml).unwrap();
        assert!(xml.contains("demo.pdf"));
        assert!(xml.contains("Page 1"));
        assert!(xml.contains("Line one"));
        assert!(xml.contains("Page 2"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_and_write_markdown_from_hello_pdf() {
        let pdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/hello.pdf");
        if !pdf.exists() {
            return;
        }
        let session = DocumentSession::open(&pdf).unwrap();
        let pages = collect_page_texts(&session, &[0]).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].page_1based, 1);
        let dir = std::env::temp_dir().join(format!("pdf-opener-md-smoke-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("hello.md");
        write_markdown("hello.pdf", &pages, &out).unwrap();
        let md = std::fs::read_to_string(&out).unwrap();
        assert!(md.starts_with("# hello.pdf\n\n## Page 1\n\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "anydoc-export")]
    #[test]
    fn ensure_title_heading_prefixes_when_missing() {
        let out = ensure_title_heading("hello.pdf", "Hello world\n");
        assert!(out.starts_with("# hello.pdf\n\nHello world\n"));
    }

    #[cfg(feature = "anydoc-export")]
    #[test]
    fn ensure_title_heading_keeps_matching_h1() {
        let body = "# hello.pdf\n\nHello\n";
        assert_eq!(ensure_title_heading("hello.pdf", body), body);
    }

    #[cfg(feature = "anydoc-export")]
    #[test]
    fn anydoc_converts_hello_pdf() {
        let pdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/hello.pdf");
        if !pdf.exists() {
            return;
        }
        let md = markdown_via_anydoc(&pdf).unwrap();
        assert!(!md.trim().is_empty(), "anydoc returned empty markdown");
        // Spike artifact for manual compare (also printed by cargo test -nocapture).
        eprintln!("--- anydoc hello.pdf ---\n{md}\n--- end ---");
        let session = DocumentSession::open(&pdf).unwrap();
        let pages = collect_page_texts(&session, &[0]).unwrap();
        let mupdf_md = format_markdown("hello.pdf", &pages);
        eprintln!("--- mupdf hello.pdf ---\n{mupdf_md}\n--- end ---");
        assert!(md.len() > 0);
        assert!(mupdf_md.contains("## Page 1"));
    }
}
