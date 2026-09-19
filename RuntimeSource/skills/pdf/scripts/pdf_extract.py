#!/usr/bin/env python3
"""Extract PDF text and/or tables with pdfplumber."""
from __future__ import annotations
import argparse, json, sys
from pathlib import Path

def pages(spec,count):
    if not spec: return list(range(1,count+1))
    out=set()
    for token in spec.split(','):
        token=token.strip()
        if not token: continue
        if '-' in token:
            a,b=map(int,token.split('-',1)); a,b=(a,b) if a<=b else (b,a); out.update(range(a,b+1))
        else: out.add(int(token))
    if any(p<1 or p>count for p in out): raise ValueError(f'page selection outside 1..{count}')
    return sorted(out)

def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('input',type=Path); p.add_argument('--mode',choices=['text','tables','both'],default='both'); p.add_argument('--pages'); p.add_argument('--format',choices=['json','text'],default='json'); p.add_argument('-o','--output',type=Path); a=p.parse_args()
    try:
        import pdfplumber
        rows=[]
        with pdfplumber.open(str(a.input)) as pdf:
            for n in pages(a.pages,len(pdf.pages)):
                page=pdf.pages[n-1]; row={'page':n}
                if a.mode in ('text','both'): row['text']=page.extract_text() or ''
                if a.mode in ('tables','both'): row['tables']=page.extract_tables() or []
                rows.append(row)
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
    if a.format=='text':
        if a.mode=='tables': print('error: text format requires text extraction',file=sys.stderr); return 2
        text='\n\n'.join(f"--- page {r['page']} ---\n{r.get('text','')}" for r in rows)
    else: text=json.dumps({'file':str(a.input),'pages':rows},indent=2,ensure_ascii=False)
    if a.output: a.output.write_text(text,encoding='utf-8'); print(a.output)
    else: print(text)
    return 0
if __name__=='__main__': raise SystemExit(main())
