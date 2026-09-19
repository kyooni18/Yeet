---
name: pdf
description: Reliable PDF creation, inspection, extraction, editing, form filling, OCR, redaction, conversion, and visual regression checking. Use whenever a task directly involves .pdf files, asks to create or deliver a PDF, modify or combine PDFs, extract PDF text/tables, fill a PDF form, OCR a scan, permanently redact content, convert office documents to PDF, or verify PDF rendering/layout quality.
---

# PDF

Use a render-first workflow. Treat page images as the authority for layout and visual correctness.

## Core loop

1. Inspect the input and determine whether the task is read/extract, create, edit, form, OCR, redact, convert, or compare.
2. Render relevant pages before modifying an existing PDF.
3. Perform the smallest operation that satisfies the request.
4. Render the result again.
5. Inspect every changed page for clipping, overlap, broken glyphs, missing images, shifted form values, bad page breaks, and unreadable text.
6. For edits where layout preservation matters, run a before/after visual diff.
7. Deliver only the final artifact unless the user asks for intermediates.

Never call a PDF correct merely because text extraction or a library operation succeeded. Verify the rendered pages.

## Choose the authoring route

For a new PDF, choose the source format before writing:

- Use ReportLab for programmatic documents, forms, certificates, simple reports, generated tables, and layouts that are naturally code-driven.
- Prefer a DOCX source for long text-heavy reports, headings, TOCs, long flowing tables, and business-document typography when a DOCX workflow is available; convert the final DOCX to PDF.
- Prefer a PPTX source for slide-like fixed layouts, posters, visual briefs, and chart-heavy pages when a slide workflow is available; export the final deck to PDF.
- Prefer HTML/CSS only when the environment already has a reliable HTML-to-PDF renderer and the design benefits from web layout.

If a ReportLab implementation starts requiring repeated manual line-break or page-break tuning, switch to a better authoring source instead of fighting the PDF canvas.

## Bundled tools

Use the scripts in `scripts/` instead of rewriting fragile PDF plumbing:

- `render_pdf.py`: render pages to PNG with Poppler or pypdfium2.
- `compare_pdf.py`: render two PDFs and create per-page pixel diffs plus a JSON summary.
- `pdf_inspect.py`: inspect metadata, encryption, page geometry, annotations, and form fields.
- `pdf_extract.py`: extract text and tables to JSON or plain text.
- `pdf_edit.py`: merge, split/select, rotate, watermark, encrypt, and decrypt.
- `pdf_forms.py`: list and fill AcroForm fields.
- `pdf_redact.py`: apply true redactions with PyMuPDF, removing underlying content.
- `ocr_pdf.py`: make scanned PDFs searchable through OCRmyPDF.
- `convert_to_pdf.py`: convert DOCX/PPTX/XLSX/ODT/ODS/ODP and related office formats with LibreOffice.

Run a script with `--help` when its arguments are not obvious.

## Task routing

For detailed recipes, read `references/workflows.md` only for the relevant task.
For document creation or major redesign, read `references/quality.md` before authoring.
For missing packages or system binaries, read `references/dependencies.md` and install only what the chosen path needs.

## Safety and correctness rules

- For redaction, never draw an opaque rectangle and call it redacted. Use `pdf_redact.py` or another true content-removal path, then verify the removed text cannot be extracted or searched.
- For OCR, preserve the original page appearance. OCR should add a searchable text layer rather than re-typeset the document unless the user explicitly asks for reconstruction.
- For fillable forms, inspect field names first and render after filling.
- For encrypted PDFs, do not attempt password bypass. Ask for the password if required.
- Do not silently flatten forms unless the user asks for a final non-editable form or flattening is necessary for reliable appearance.
- Preserve page order, page size, rotation, crop boxes, links, annotations, and metadata unless the requested operation requires changing them.
- When editing only a few pages, verify those pages plus their immediate neighbors. For creation, conversion, OCR, or broad edits, inspect all pages.

## Workspace conventions

- Put intermediates under `tmp/pdfs/` or another disposable workspace-local directory.
- Keep final filenames stable and descriptive.
- Keep page renders and diff images out of the final delivery folder.
- Do not overwrite an original input unless the user explicitly asks for in-place replacement.

## Final acceptance

Before delivery, confirm the final PDF opens cleanly, page count/order are correct, all changed pages render without clipping or overlap, glyphs are readable, tables/forms do not overflow, redactions remove underlying content, OCR is searchable while preserving appearance, and all before/after differences are intentional.
