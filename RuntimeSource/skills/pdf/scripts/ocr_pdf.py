#!/usr/bin/env python3
"""OCR a scanned PDF with OCRmyPDF while preserving page appearance."""
from __future__ import annotations
import argparse, shutil, subprocess, sys
from pathlib import Path

def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('input',type=Path); p.add_argument('-o','--output',type=Path,required=True); p.add_argument('--language',default='eng'); p.add_argument('--deskew',action='store_true'); p.add_argument('--rotate-pages',action='store_true'); p.add_argument('--force-ocr',action='store_true'); p.add_argument('--optimize',type=int,choices=[0,1,2,3],default=1); a=p.parse_args()
    exe=shutil.which('ocrmypdf')
    if not exe: print('error: ocrmypdf not found; see references/dependencies.md',file=sys.stderr); return 2
    a.output.parent.mkdir(parents=True,exist_ok=True); cmd=[exe,'--language',a.language,'--optimize',str(a.optimize)]
    if a.deskew: cmd.append('--deskew')
    if a.rotate_pages: cmd.append('--rotate-pages')
    if a.force_ocr: cmd.append('--force-ocr')
    cmd += [str(a.input),str(a.output)]
    try: subprocess.run(cmd,check=True)
    except subprocess.CalledProcessError as e: print(f'error: OCRmyPDF exited with {e.returncode}',file=sys.stderr); return e.returncode or 2
    print(a.output); return 0
if __name__=='__main__': raise SystemExit(main())
