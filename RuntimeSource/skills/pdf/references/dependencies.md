# PDF dependency map

Install only what the chosen workflow needs.

Core Python packages:

```sh
uv pip install pypdf pdfplumber reportlab pillow pypdfium2
```

True redaction:

```sh
uv pip install pymupdf
```

If `uv` is unavailable, use the active Python environment's package installer.

Preferred system tools on macOS:

```sh
brew install poppler ocrmypdf
brew install --cask libreoffice
```

On Ubuntu/Debian:

```sh
sudo apt-get install -y poppler-utils ocrmypdf libreoffice
```

Prefer `pdftoppm` for rendering and fall back to `pypdfium2`. Prefer `pdfplumber` for layout-aware text/table extraction, `pypdf` for structure/editing, and PyMuPDF for true redaction.
