#!/usr/bin/env python3
"""List or fill AcroForm fields using pypdf."""
from __future__ import annotations
import argparse, json, sys, tempfile
from pathlib import Path

def simple(v):
    if v is None or isinstance(v,(str,int,float,bool)): return v
    if isinstance(v,list): return [simple(x) for x in v]
    return str(v)

def main():
    p=argparse.ArgumentParser(description=__doc__); s=p.add_subparsers(dest='cmd',required=True)
    q=s.add_parser('list'); q.add_argument('input',type=Path)
    q=s.add_parser('fill'); q.add_argument('input',type=Path); q.add_argument('values',type=Path); q.add_argument('-o','--output',type=Path,required=True); q.add_argument('--flatten',action='store_true')
    a=p.parse_args()
    try:
        from pypdf import PdfReader, PdfWriter
        if a.cmd=='list':
            r=PdfReader(str(a.input)); fields=r.get_fields() or {}; rows=[]
            for name,f in fields.items(): rows.append({'name':name,'type':simple(f.get('/FT')),'value':simple(f.get('/V')),'options':simple(f.get('/Opt')),'flags':simple(f.get('/Ff'))})
            print(json.dumps(rows,indent=2,ensure_ascii=False)); return 0
        values=json.loads(a.values.read_text(encoding='utf-8'))
        if not isinstance(values,dict): raise ValueError('values JSON must be an object mapping field names to values')
        r=PdfReader(str(a.input)); w=PdfWriter(); w.clone_document_from_reader(r)
        for page in w.pages: w.update_page_form_field_values(page,values,auto_regenerate=False)
        a.output.parent.mkdir(parents=True,exist_ok=True)
        if not a.flatten:
            with a.output.open('wb') as f: w.write(f)
        else:
            with tempfile.NamedTemporaryFile(suffix='.pdf',delete=False) as tmp: w.write(tmp); temp=Path(tmp.name)
            try:
                import fitz
                doc=fitz.open(str(temp))
                if not hasattr(doc,'bake'): raise RuntimeError('installed PyMuPDF lacks Document.bake(); omit --flatten')
                doc.bake(annots=True,widgets=True); doc.save(str(a.output),garbage=4,deflate=True); doc.close()
            finally: temp.unlink(missing_ok=True)
        print(a.output); return 0
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
if __name__=='__main__': raise SystemExit(main())
