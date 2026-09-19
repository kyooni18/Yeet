#!/usr/bin/env python3
"""Render two PDFs, create page diffs, and emit a JSON comparison summary."""
from __future__ import annotations
import argparse, json, sys
from pathlib import Path
from PIL import Image, ImageChops, ImageEnhance, ImageStat
from render_pdf import render

def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('before',type=Path); p.add_argument('after',type=Path); p.add_argument('--out-dir',type=Path,required=True); p.add_argument('--dpi',type=int,default=200); p.add_argument('--pages'); p.add_argument('--backend',choices=['auto','poppler','pdfium'],default='auto'); a=p.parse_args()
    bd=a.out_dir/'before'; ad=a.out_dir/'after'; dd=a.out_dir/'diff'; dd.mkdir(parents=True,exist_ok=True)
    try: before=render(a.before,bd,a.dpi,a.pages,a.backend); after=render(a.after,ad,a.dpi,a.pages,a.backend)
    except Exception as e: print(f'error: {e}',file=sys.stderr); return 2
    rows=[]
    for i in range(max(len(before),len(after))):
        row={'page_index':i+1}
        if i>=len(before) or i>=len(after): row['status']='missing-page'; rows.append(row); continue
        x=Image.open(before[i]).convert('RGB'); y=Image.open(after[i]).convert('RGB')
        if x.size!=y.size: row.update({'status':'size-changed','before_size':x.size,'after_size':y.size}); rows.append(row); continue
        diff=ImageChops.difference(x,y); bbox=diff.getbbox(); stat=ImageStat.Stat(diff); row['status']='identical' if bbox is None else 'changed'; row['mean_absolute_difference']=round(sum(stat.mean)/len(stat.mean),6); row['changed_bbox']=bbox
        if bbox is not None:
            out=dd/f'page-{i+1:04d}-diff.png'; ImageEnhance.Contrast(diff).enhance(4.0).save(out); row['diff_image']=str(out)
        rows.append(row)
    summary={'before':str(a.before),'after':str(a.after),'dpi':a.dpi,'page_count_before':len(before),'page_count_after':len(after),'pages':rows}; out=a.out_dir/'summary.json'; out.write_text(json.dumps(summary,indent=2),encoding='utf-8'); print(out); return 0
if __name__=='__main__': raise SystemExit(main())
