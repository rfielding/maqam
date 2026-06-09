#!/usr/bin/env python3
"""
Compact Python checkpoint matching the last design direction before the Rust pass.

This is intentionally source-readable rather than a generated binary asset.
Design choices:
- text is drawn before the carpet filter, so it is woven into the rug
- 1/1 ratios are omitted
- text is horizontal, giving the carpet an orientation
- center Gosper is a slightly thicker black thread
"""
from PIL import Image, ImageDraw, ImageFilter, ImageFont
from pathlib import Path
import argparse, hashlib, math, re

MAGICCARPET_MQ='''MAQAM_SESSION_V3
create saba2 1/1 13/12 6/5 5/4
vol 1
B|0|180
S|1|2
P|2|1|d bayati, f hijaz 4444
J|3|0|3
P|4|1|a saba, c hijaz
P|5|1|a saba2, c hijaz
J|6|4|4
P|7|1|g rast 664664
J|8|7|4
J|9|0|4
'''
GROWL_MQ='''MAQAM_SESSION_V3
create bayati2 8/9 1/1 12/11 32/27 4/3 3/2
create hijaz2 8/9 1/1 15/14 6/5 4/3 3/2
create saba2 8/9 1/1 13/12 6/5 5/4 11/8 3/2
vol 1
B|0|180
S|1|1.2
P|2|1|g hijaz 4444
J|3|2|3
P|4|1|g bayati 332332
P|5|1|g saba 664
J|6|5|4
J|7|0|4
'''
COLORS={'bayati':(92,170,128),'hijaz':(176,92,58),'saba':(126,88,164),'rast':(178,145,68),'ajam':(180,152,84),'kurd':(86,118,98)}
DEFAULT={'bayati':['1/1','12/11','32/27','4/3','3/2'],'hijaz':['1/1','256/243','81/64','4/3','3/2'],'saba':['1/1','13/12','32/27','5/4'],'rast':['1/1','9/8','27/22','4/3','3/2'],'ajam':['1/1','9/8','5/4','4/3','3/2']}
PALETTE=[(9,12,18),(18,23,32),(28,35,28),(34,23,38),(42,30,20),(19,44,48),(55,64,30),(79,41,27),(96,72,34),(84,31,61),(39,92,38),(61,113,53),(123,94,59),(145,119,77),(173,145,106),(211,189,150)]

def noise(*xs):
    h=hashlib.sha256(','.join(map(str,xs)).encode()).digest()
    return int.from_bytes(h[:4],'big')/0xffffffff

def mix(a,b,t): return tuple(int(round((1-t)*x+t*y)) for x,y in zip(a,b))
def dark(c,t=.2): return mix(c,(0,0,0),t)
def light(c,t=.2): return mix(c,(255,255,255),t)
def base(s): return re.sub(r'\d+$','',s.lower().strip())
def mcol(s): return COLORS.get(base(s),(120,110,90))

def parse_voice(s):
    t=s.strip().split(); rhythm=''
    if t and re.fullmatch(r'[0-9xX._-]+',t[-1]): rhythm=t.pop()
    return {'root':t[0].lower() if t else 'c','scale':t[1].lower() if len(t)>1 else 'rast','rhythm':rhythm}

def parse(name,text):
    phrases=[]; jumps=[]; scales={k:list(v) for k,v in DEFAULT.items()}
    for raw in text.strip().splitlines():
        raw=raw.strip()
        if not raw or raw=='MAQAM_SESSION_V3': continue
        if raw.startswith('create '):
            t=raw.split(); scales[t[1].lower()]=t[2:]; continue
        p=raw.split('|')
        if len(p)<3: continue
        if p[0]=='P':
            payload='|'.join(p[3:]) if len(p)>3 else ''
            phrases.append({'id':int(p[1]),'voices':[parse_voice(x) for x in payload.split(',')]})
        elif p[0]=='J':
            jumps.append({'id':int(p[1]),'target':int(p[2]) if p[2].lstrip('-').isdigit() else 0})
    return {'name':name,'phrases':sorted(phrases,key=lambda x:x['id']),'jumps':sorted(jumps,key=lambda x:x['id']),'scales':scales}

def draw_thread(img,pts,c,w,a=255):
    d=ImageDraw.Draw(img)
    pts=[tuple(p) for p in pts]
    d.line(pts,fill=dark(c,.72)+(a,),width=int(w+4),joint='curve')
    d.line(pts,fill=c+(a,),width=int(w),joint='curve')

def gosper(order):
    s='A'
    for _ in range(order):
        s=''.join('A-B--B+A++AA+B-' if c=='A' else '+A-BB--B-A++A+B' if c=='B' else c for c in s)
    x=y=h=0.0; pts=[(x,y)]
    for c in s:
        if c in 'AB': x+=math.cos(h); y+=math.sin(h); pts.append((x,y))
        elif c=='+': h+=math.pi/3
        elif c=='-': h-=math.pi/3
    return pts

def fit(pts,rect,margin=28):
    x0,y0,x1,y1=rect; xs=[p[0] for p in pts]; ys=[p[1] for p in pts]
    sx=(x1-x0-2*margin)/(max(xs)-min(xs)); sy=(y1-y0-2*margin)/(max(ys)-min(ys)); s=min(sx,sy)
    ox=x0+(x1-x0-(max(xs)-min(xs))*s)/2-min(xs)*s; oy=y0+(y1-y0-(max(ys)-min(ys))*s)/2-min(ys)*s
    return [(ox+x*s,oy+y*s) for x,y in pts]

def ring_layout(rect,phrases):
    cx=(rect[0]+rect[2])/2; cy=(rect[1]+rect[3])/2; rr=min(rect[2]-rect[0],rect[3]-rect[1])*.60
    groups=[]; a=-math.pi/2; gap=math.radians(10)
    total=sum(sum(int(ch) for ch in ((p['voices'][-1].get('rhythm') or '4') if p['voices'] else '4') if ch.isdigit()) for p in phrases) or 1
    step=(2*math.pi-len(phrases)*gap)/total
    for p in phrases:
        events=[]; rhythm=(p['voices'][-1].get('rhythm') or '4') if p['voices'] else '4'
        for ch in rhythm:
            if ch.isdigit(): events += ['kick']+['tick']*(int(ch)-1)
        start=a; angles=[]
        for _ in events: angles.append(a); a+=step
        groups.append({'phrase':p['id'],'start':start,'end':angles[-1] if angles else start,'angles':angles,'events':events})
        a+=gap
    return cx,cy,rr,groups

def ratios(score,scale): return ' '.join(r for r in score['scales'].get(scale,DEFAULT.get(base(scale),[])) if r!='1/1')

def load_font(size):
    for f in ['/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf','/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf']:
        try: return ImageFont.truetype(f,size)
        except Exception: pass
    return ImageFont.load_default()

def draw_label(img,x,y,scale,ratio):
    d=ImageDraw.Draw(img); f1=load_font(26); f2=load_font(16); col=light(mcol(scale),.78)
    w=max(d.textlength(scale.upper(),font=f1),d.textlength(ratio,font=f2))+36; h=66
    x=max(30,min(img.width-w-30,x-w/2)); y=max(30,min(img.height-h-30,y-h/2))
    d.rectangle((x,y,x+w,y+h),fill=(0,0,0),outline=(55,55,55),width=4)
    d.text((x+18,y+10),scale.upper(),font=f1,fill=col)
    if ratio: d.text((x+18,y+40),ratio,font=f2,fill=(235,235,235))

def render(score,size=(1800,900)):
    W,H=size; img=Image.new('RGBA',size,(8,10,12,255)); d=ImageDraw.Draw(img)
    for y in range(0,H,6):
        for x in range(0,W,6):
            if noise(score['name'],'field',x//6,y//6)>.18: d.point((x,y),fill=(20,18,16))
    rect=(W*.18,H*.20,W*.82,H*.80); draw_thread(img,fit(gosper(3),rect),(8,8,8),11,255)
    cx,cy,rr,groups=ring_layout(rect,score['phrases']); pmap={p['id']:p for p in score['phrases']}
    for g in groups:
        ph=pmap[g['phrase']]; scale=ph['voices'][0]['scale'] if ph['voices'] else 'rast'; c=mcol(scale)
        for a in [g['start']+(g['end']-g['start'])*i/80 for i in range(81)]:
            d.ellipse((cx+rr*math.cos(a)-3,cy+rr*math.sin(a)-3,cx+rr*math.cos(a)+3,cy+rr*math.sin(a)+3),fill=(96,76,44))
        for ev,a in zip(g['events'],g['angles']):
            r=16 if ev=='kick' else 8; d.ellipse((cx+(rr+34)*math.cos(a)-r,cy+(rr+34)*math.sin(a)-r,cx+(rr+34)*math.cos(a)+r,cy+(rr+34)*math.sin(a)+r),fill=c)
        mid=(g['start']+g['end'])/2; draw_label(img,cx+(rr+150)*math.cos(mid),cy+(rr+150)*math.sin(mid),scale,ratios(score,scale))
    return img

def nearest(c): return min(PALETTE,key=lambda p:(c[0]-p[0])**2+1.15*(c[1]-p[1])**2+(c[2]-p[2])**2)
def carpet(img,kw=1440,kh=720,kp=2):
    small=img.resize((kw,kh),Image.Resampling.LANCZOS).convert('RGB'); out=Image.new('RGB',(kw*kp,kh*kp),(12,11,10)); d=ImageDraw.Draw(out)
    for y in range(kh):
        for x in range(kw):
            c=nearest(small.getpixel((x,y))); d.rectangle((x*kp,y*kp,x*kp+kp-1,y*kp+kp-1),fill=c)
    return out

def add_fringes(rug,top=40,side=80):
    out=Image.new('RGB',(rug.width+2*side,rug.height+2*top),(9,8,7)); out.paste(rug,(side,top)); return out

def build(name,text,out): add_fringes(carpet(render(parse(name,text)))).save(out)
def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--mq'); ap.add_argument('--out'); ap.add_argument('--name',default='score.mq'); ap.add_argument('--all',action='store_true'); a=ap.parse_args()
    if a.all or not a.mq: build('magiccarpet.mq',MAGICCARPET_MQ,'magiccarpet_surface_gosper_beats_v45.png'); build('growl.mq',GROWL_MQ,'growl_surface_gosper_beats_v45.png')
    else: build(a.name,Path(a.mq).read_text(),a.out)
if __name__=='__main__': main()
