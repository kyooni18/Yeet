#!/usr/bin/env python3
"""Render PDF pages to PNG using Poppler or pypdfium2."""
from __future__ import annotations
import argparse, shutil, subprocess, sys
from pathlib import Path

def parse_pages(spec, count):
    if not spec: return list(range(1, count + 1))
    pages=set()
    for part in spec.split(','):
        part=part.strip()
        if not part: continue
        if '-' in part:
            a,b=part.split('-',1); a,b=int(a),int(b)
            if a>b: a,b=b,a
            pages.update(range(a,b+1))
        else: pages.add(int(part))
    bad=[p for p in pages if p<1 or p>count]
    if bad: raise ValueError(f'page(s) out of range 1..{count}: {bad}')
    return sorted(pages)

def page_count(pdf):
    try:
        from pypdf import PdfReader
        return len(PdfReader(str(pdf)).pages)
    except Exception:
        exe=shutil.which('pdfinfo')
        if not exe: raise RuntimeError('Need pypdf or Poppler pdfinfo to determine page count')
        p=subprocess.run([exe,str(pdf)],check=True,capture_output=True,text=True)
        for line in p.stdout.splitlines():
            if line.startswith('Pages:'): return int(line.split(':',1)[1].strip())
        raise RuntimeError('Could not determine page count')

def render(pdf, out_dir, dpi=200, page_spec=None, backend='auto'):
    pdf=Path(pdf).resolve(); out_dir=Path(out_dir); out_dir.mkdir(parents=True,exist_ok=True)
    if not pdf.exists(): raise FileNotFoundError(pdf)
    pages=parse_pages(page_spec,page_count(pdf)); outputs=[]
    if backend in ('auto','poppler') and shutil.which('pdftoppm'):
        exe=shutil.which('pdftoppm')
        for page in pages:
            prefix=out_dir/f'page-{page:04d}'
            subprocess.run([exe,'-png','-singlefile','-r',str(dpi),'-f',str(page),'-l',str(page),str(pdf),str(prefix)],check=True)
            out=prefix.with_suffix('.png')
            if not out.exists(): raise RuntimeError(f'renderer did not create {out}')
            outputs.append(out)
        return outputs
    if backend=='poppler': raise RuntimeError('Poppler requested but pdftoppm is unavailable')
    try: import pypdfium2 as pdfium
    except ImportError as e: raise RuntimeError('pypdfium2 is not installed') from e
    doc=pdfium.PdfDocument(str(pdf)); scale=dpi/72.0
    for num in pages:
        page=doc[num-1]; image=page.render(scale=scale).to_pil(); out=out_dir/f'page-{num:04d}.png'; image.save(out); outputs.append(out); page.close()
    doc.close(); return outputs

def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('input',type=Path); p.add_argument('--out-dir',type=Path,required=True); p.add_argument('--dpi',type=int,default=200); p.add_argument('--pages'); p.add_argument('--backend',choices=['auto','poppler','pdfium'],default='auto'); a=p.parse_args()
    try: outputs=render(a.input,a.out_dir,a.dpi,a.pages,a.backend)
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
    for path in outputs: print(path)
    return 0
if __name__=='__main__': raise SystemExit(main())
