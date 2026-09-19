#!/usr/bin/env python3
"""Structural PDF edits: merge, select/split, rotate, watermark, encrypt, decrypt."""
from __future__ import annotations
import argparse, sys
from pathlib import Path

def parse_pages(spec,count):
    out=set()
    for token in spec.split(','):
        token=token.strip()
        if not token: continue
        if '-' in token:
            a,b=map(int,token.split('-',1)); a,b=(a,b) if a<=b else (b,a); out.update(range(a,b+1))
        else: out.add(int(token))
    bad=[p for p in out if p<1 or p>count]
    if bad: raise ValueError(f'page(s) outside 1..{count}: {bad}')
    return sorted(out)

def write(writer,path):
    path.parent.mkdir(parents=True,exist_ok=True)
    with path.open('wb') as f: writer.write(f)

def main():
    p=argparse.ArgumentParser(description=__doc__); s=p.add_subparsers(dest='cmd',required=True)
    q=s.add_parser('merge'); q.add_argument('inputs',nargs='+',type=Path); q.add_argument('-o','--output',type=Path,required=True)
    q=s.add_parser('select'); q.add_argument('input',type=Path); q.add_argument('-o','--output',type=Path,required=True); q.add_argument('--pages',required=True)
    q=s.add_parser('split'); q.add_argument('input',type=Path); q.add_argument('--out-dir',type=Path,required=True); q.add_argument('--chunk-size',type=int,default=1)
    q=s.add_parser('rotate'); q.add_argument('input',type=Path); q.add_argument('-o','--output',type=Path,required=True); q.add_argument('--pages',required=True); q.add_argument('--degrees',type=int,choices=[90,180,270],required=True)
    q=s.add_parser('watermark'); q.add_argument('input',type=Path); q.add_argument('watermark',type=Path); q.add_argument('-o','--output',type=Path,required=True)
    q=s.add_parser('encrypt'); q.add_argument('input',type=Path); q.add_argument('-o','--output',type=Path,required=True); q.add_argument('--user-password',required=True); q.add_argument('--owner-password')
    q=s.add_parser('decrypt'); q.add_argument('input',type=Path); q.add_argument('-o','--output',type=Path,required=True); q.add_argument('--password',required=True)
    a=p.parse_args()
    try:
        from pypdf import PdfReader, PdfWriter
        if a.cmd=='merge':
            w=PdfWriter()
            for f in a.inputs: w.append(str(f))
            write(w,a.output)
        elif a.cmd=='select':
            r=PdfReader(str(a.input)); w=PdfWriter()
            for n in parse_pages(a.pages,len(r.pages)): w.add_page(r.pages[n-1])
            write(w,a.output)
        elif a.cmd=='split':
            r=PdfReader(str(a.input))
            if a.chunk_size<1: raise ValueError('--chunk-size must be >= 1')
            a.out_dir.mkdir(parents=True,exist_ok=True)
            for start in range(0,len(r.pages),a.chunk_size):
                w=PdfWriter(); end=min(start+a.chunk_size,len(r.pages))
                for page in r.pages[start:end]: w.add_page(page)
                out=a.out_dir/f'pages-{start+1:04d}-{end:04d}.pdf'; write(w,out); print(out)
            return 0
        elif a.cmd=='rotate':
            r=PdfReader(str(a.input)); chosen=set(parse_pages(a.pages,len(r.pages))); w=PdfWriter()
            for i,page in enumerate(r.pages,1):
                if i in chosen: page.rotate(a.degrees)
                w.add_page(page)
            write(w,a.output)
        elif a.cmd=='watermark':
            r=PdfReader(str(a.input)); wm=PdfReader(str(a.watermark)); w=PdfWriter()
            if not wm.pages: raise ValueError('watermark PDF has no pages')
            for i,page in enumerate(r.pages): page.merge_page(wm.pages[min(i,len(wm.pages)-1)]); w.add_page(page)
            write(w,a.output)
        elif a.cmd=='encrypt':
            r=PdfReader(str(a.input)); w=PdfWriter(); w.append_pages_from_reader(r); w.encrypt(a.user_password,owner_password=a.owner_password or a.user_password); write(w,a.output)
        elif a.cmd=='decrypt':
            r=PdfReader(str(a.input))
            if r.is_encrypted and not r.decrypt(a.password): raise ValueError('incorrect password')
            w=PdfWriter(); w.append_pages_from_reader(r); write(w,a.output)
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
    print(a.output); return 0
if __name__=='__main__': raise SystemExit(main())
