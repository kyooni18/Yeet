#!/usr/bin/env python3
"""Convert an office document to PDF using LibreOffice headless mode."""
from __future__ import annotations
import argparse, shutil, subprocess, sys, tempfile
from pathlib import Path

def find_soffice():
    for name in ('soffice','libreoffice'):
        found=shutil.which(name)
        if found: return found
    mac=Path('/Applications/LibreOffice.app/Contents/MacOS/soffice')
    return str(mac) if mac.exists() else None

def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('input',type=Path); p.add_argument('-o','--output',type=Path,required=True); a=p.parse_args()
    exe=find_soffice()
    if not exe: print('error: LibreOffice not found; see references/dependencies.md',file=sys.stderr); return 2
    source=a.input.resolve()
    if not source.exists(): print(f'error: input not found: {source}',file=sys.stderr); return 2
    a.output.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='pdf-convert-') as temp:
        d=Path(temp)
        try: subprocess.run([exe,'--headless','--convert-to','pdf','--outdir',str(d),str(source)],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        except subprocess.CalledProcessError as e: print(e.stdout,file=sys.stderr); print(e.stderr,file=sys.stderr); return e.returncode or 2
        made=d/f'{source.stem}.pdf'
        if not made.exists(): print('error: LibreOffice did not produce the expected PDF',file=sys.stderr); return 2
        shutil.copy2(made,a.output)
    print(a.output); return 0
if __name__=='__main__': raise SystemExit(main())
