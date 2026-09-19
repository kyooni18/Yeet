# PDF workflows

## Read or inspect

Run `pdf_inspect.py`, render relevant pages, then use `pdf_extract.py` for text/tables. Treat rendered pages as authoritative for visual interpretation.

## Create

Choose ReportLab, DOCX->PDF, PPTX->PDF, or an existing HTML pipeline based on document type. Read `quality.md`, generate/export, render every page, and iterate until visually clean.

## Structural edit

```sh
python scripts/pdf_edit.py merge -o merged.pdf a.pdf b.pdf
python scripts/pdf_edit.py select input.pdf -o excerpt.pdf --pages 1-3,8
python scripts/pdf_edit.py rotate input.pdf -o rotated.pdf --pages 2,4 --degrees 90
python scripts/pdf_edit.py watermark input.pdf stamp.pdf -o marked.pdf
```

Render the output and use `compare_pdf.py` when visual preservation matters.

## Forms

```sh
python scripts/pdf_forms.py list form.pdf
python scripts/pdf_forms.py fill form.pdf values.json -o filled.pdf
```

Map exact field names to values in JSON, then render filled pages. Use `--flatten` only when a fixed non-editable result is desired.

## OCR

```sh
python scripts/ocr_pdf.py scan.pdf -o searchable.pdf --deskew --rotate-pages
```

Verify extracted text is searchable and compare rendered appearance with the original.

## Redaction

Use a JSON array with 1-based page numbers and either a text search or rectangle:

```json
[
  {"page": 1, "text": "Secret Account Number"},
  {"page": 2, "rect": [72, 120, 320, 152]}
]
```

```sh
python scripts/pdf_redact.py input.pdf redactions.json -o redacted.pdf
```

Re-extract text to prove sensitive content is gone and render affected pages.

## Convert office documents

```sh
python scripts/convert_to_pdf.py report.docx -o report.pdf
```

Render every converted page because pagination/fonts/charts may change.

## Visual comparison

```sh
python scripts/compare_pdf.py before.pdf after.pdf --out-dir tmp/pdfs/diff
```

Review the JSON summary and diff PNGs; differences are acceptable only when intentional.
