#!/usr/bin/env python3
"""Apply true PDF redactions from text searches or explicit rectangles using PyMuPDF."""
from __future__ import annotations
import argparse, json, sys
from pathlib import Path

def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('input',type=Path); p.add_argument('spec',type=Path); p.add_argument('-o','--output',type=Path,required=True); p.add_argument('--replacement',default=''); a=p.parse_args()
    try:
        import fitz
        specs=json.loads(a.spec.read_text(encoding='utf-8'))
        if not isinstance(specs,list): raise ValueError('redaction spec must be a JSON array')
        doc=fitz.open(str(a.input)); affected=set()
        for item in specs:
            if not isinstance(item,dict) or 'page' not in item: raise ValueError("each redaction needs a 1-based 'page'")
            n=int(item['page'])
            if n<1 or n>len(doc): raise ValueError(f'page {n} outside 1..{len(doc)}')
            page=doc[n-1]; rects=[]
            if 'text' in item:
                rects.extend(page.search_for(str(item['text'])))
                if not rects: raise ValueError(f"text not found on page {n}: {item['text']!r}")
            if 'rect' in item:
                r=item['rect']
                if not isinstance(r,list) or len(r)!=4: raise ValueError('rect must be [x0,y0,x1,y1] in PDF points')
                rects.append(fitz.Rect(*map(float,r)))
            if not rects: raise ValueError("each redaction needs 'text' or 'rect'")
            for r in rects: page.add_redact_annot(r,text=a.replacement,fill=(0,0,0),text_color=(1,1,1))
            affected.add(n)
        for n in sorted(affected): doc[n-1].apply_redactions()
        a.output.parent.mkdir(parents=True,exist_ok=True); doc.save(str(a.output),garbage=4,deflate=True,clean=True); doc.close(); print(a.output); return 0
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
if __name__=='__main__': raise SystemExit(main())
