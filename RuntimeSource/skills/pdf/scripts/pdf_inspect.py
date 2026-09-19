#!/usr/bin/env python3
"""Inspect PDF metadata, geometry, annotations, and AcroForm fields."""
from __future__ import annotations
import argparse, json, sys
from pathlib import Path

def j(v): return v if v is None or isinstance(v,(str,int,float,bool)) else str(v)
def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('input',type=Path); p.add_argument('--password'); p.add_argument('-o','--output',type=Path); a=p.parse_args()
    try:
        from pypdf import PdfReader
        r=PdfReader(str(a.input)); enc=r.is_encrypted
        if enc:
            if not a.password: raise RuntimeError('PDF is encrypted; provide --password')
            if not r.decrypt(a.password): raise RuntimeError('incorrect password')
        pages=[]
        for i,page in enumerate(r.pages,1):
            b=page.mediabox; pages.append({'page':i,'width_pt':float(b.width),'height_pt':float(b.height),'rotation':int(page.get('/Rotate',0) or 0),'annotations':len(page.get('/Annots') or [])})
        fields=[]
        for name,f in (r.get_fields() or {}).items(): fields.append({'name':name,'type':j(f.get('/FT')),'value':j(f.get('/V')),'flags':j(f.get('/Ff')),'options':j(f.get('/Opt'))})
        result={'file':str(a.input),'encrypted':enc,'pages':pages,'metadata':{str(k):j(v) for k,v in (r.metadata or {}).items()},'form_fields':fields}
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
    text=json.dumps(result,indent=2,ensure_ascii=False)
    if a.output: a.output.write_text(text,encoding='utf-8'); print(a.output)
    else: print(text)
    return 0
if __name__=='__main__': raise SystemExit(main())
