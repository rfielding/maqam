use anyhow::Result;
use clap::Parser;
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_circle_mut, draw_line_segment_mut};
use std::{collections::HashMap, f64::consts::PI, fs, path::PathBuf};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)] mq: Option<PathBuf>,
    #[arg(long)] out: Option<PathBuf>,
    #[arg(long, default_value = "score.mq")] name: String,
    #[arg(long)] all: bool,
    #[arg(long, default_value_t = 1800)] w: u32,
    #[arg(long, default_value_t = 900)] h: u32,
    #[arg(long, default_value_t = 1440)] knots_w: u32,
    #[arg(long, default_value_t = 720)] knots_h: u32,
    #[arg(long, default_value_t = 2)] knot_px: u32,
}

const MAGICCARPET: &str = r#"MAQAM_SESSION_V3
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
"#;

const GROWL: &str = r#"MAQAM_SESSION_V3
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
"#;

#[derive(Clone, Copy)] struct P { x: f64, y: f64 }
impl P { fn new(x:f64,y:f64)->Self{Self{x,y}} fn add(self,o:P)->P{P::new(self.x+o.x,self.y+o.y)} fn sub(self,o:P)->P{P::new(self.x-o.x,self.y-o.y)} fn mul(self,k:f64)->P{P::new(self.x*k,self.y*k)} fn len(self)->f64{(self.x*self.x+self.y*self.y).sqrt()} fn norm(self)->P{let l=self.len(); if l<1e-9{P::new(1.0,0.0)}else{self.mul(1.0/l)}} }
#[derive(Clone)] struct Voice { scale:String, rhythm:String }
#[derive(Clone)] struct Phrase { id:i32, voices:Vec<Voice> }
#[derive(Clone)] struct Jump { id:i32, target:i32 }
#[derive(Clone)] struct Score { name:String, phrases:Vec<Phrase>, jumps:Vec<Jump>, scales:HashMap<String,Vec<String>> }

fn rgba(c:(u8,u8,u8),a:u8)->Rgba<u8>{Rgba([c.0,c.1,c.2,a])}
fn mix(a:(u8,u8,u8),b:(u8,u8,u8),t:f64)->(u8,u8,u8){let f=|x:u8,y:u8|((1.0-t)*x as f64+t*y as f64).round().clamp(0.0,255.0)as u8;(f(a.0,b.0),f(a.1,b.1),f(a.2,b.2))}
fn light(c:(u8,u8,u8),t:f64)->(u8,u8,u8){mix(c,(255,255,255),t)}
fn dark(c:(u8,u8,u8),t:f64)->(u8,u8,u8){mix(c,(0,0,0),t)}
fn base(s:&str)->String{s.trim().to_lowercase().trim_end_matches(|c:char|c.is_ascii_digit()).to_string()}
fn maqam_color(s:&str)->(u8,u8,u8){match base(s).as_str(){"bayati"=>(92,170,128),"hijaz"=>(176,92,58),"saba"=>(126,88,164),"rast"=>(178,145,68),"ajam"=>(180,152,84),"kurd"=>(86,118,98),_=>(120,110,90)}}
fn hash(parts:&[String])->u64{let mut h=0xcbf29ce484222325u64;for s in parts{for b in s.as_bytes(){h^=*b as u64;h=h.wrapping_mul(0x100000001b3)}}h}
macro_rules! n {($($x:expr),*)=>{{let v=vec![$($x.to_string()),*];((hash(&v)>>11)as f64)/((1u64<<53)as f64)}}}
fn line(img:&mut RgbaImage,a:P,b:P,c:(u8,u8,u8),w:f64,a8:u8){let d=b.sub(a);let n=P::new(-d.y,d.x).norm();let h=(w/2.0).ceil()as i32;for k in -h..=h{let o=n.mul(k as f64);draw_line_segment_mut(img,((a.x+o.x)as f32,(a.y+o.y)as f32),((b.x+o.x)as f32,(b.y+o.y)as f32),rgba(c,a8));}}
fn thread(img:&mut RgbaImage,pts:&[P],c:(u8,u8,u8),w:f64,a:u8){for p in pts.windows(2){line(img,p[0],p[1],dark(c,.7),w+4.0,a);line(img,p[0],p[1],c,w,a)}}
fn gosper(order:usize)->Vec<P>{let mut s="A".to_string();for _ in 0..order{let mut o=String::new();for ch in s.chars(){match ch{'A'=>o.push_str("A-B--B+A++AA+B-"),'B'=>o.push_str("+A-BB--B-A++A+B"),_=>o.push(ch)}}s=o}let(mut x,mut y,mut h)=(0.0,0.0,0.0);let mut pts=vec![P::new(x,y)];for ch in s.chars(){match ch{'A'|'B'=>{x+=h.cos();y+=h.sin();pts.push(P::new(x,y))},'+'=>h+=PI/3.0,'-'=>h-=PI/3.0,_=>{}}}pts}
fn fit(pts:&[P],x0:f64,y0:f64,x1:f64,y1:f64,m:f64)->Vec<P>{let minx=pts.iter().map(|p|p.x).fold(f64::INFINITY,f64::min);let maxx=pts.iter().map(|p|p.x).fold(f64::NEG_INFINITY,f64::max);let miny=pts.iter().map(|p|p.y).fold(f64::INFINITY,f64::min);let maxy=pts.iter().map(|p|p.y).fold(f64::NEG_INFINITY,f64::max);let sc=((x1-x0-2.0*m)/(maxx-minx)).min((y1-y0-2.0*m)/(maxy-miny));let ox=x0+(x1-x0-(maxx-minx)*sc)/2.0-minx*sc;let oy=y0+(y1-y0-(maxy-miny)*sc)/2.0-miny*sc;pts.iter().map(|p|P::new(ox+p.x*sc,oy+p.y*sc)).collect()}
fn parse_voice(s:&str)->Voice{let mut t:Vec<&str>=s.split_whitespace().collect();let mut rhythm=String::new();if let Some(last)=t.last(){if last.chars().all(|c|c.is_ascii_digit()||"xX._-".contains(c)){rhythm=last.to_string();t.pop();}}Voice{scale:t.get(1).unwrap_or(&"rast").to_lowercase(),rhythm}}
fn parse(name:&str,text:&str)->Score{let mut phrases=Vec::new();let mut jumps=Vec::new();let mut scales=HashMap::new();for raw in text.lines().map(str::trim){if raw.is_empty()||raw=="MAQAM_SESSION_V3"{continue}if raw.starts_with("create "){let t:Vec<&str>=raw.split_whitespace().collect();if t.len()>2{scales.insert(t[1].to_lowercase(),t[2..].iter().map(|x|x.to_string()).collect());}continue}let p:Vec<&str>=raw.split('|').collect();if p.len()<3{continue}let id=p[1].parse().unwrap_or(0);match p[0]{"P"=>{let payload=if p.len()>3{p[3..].join("|")}else{String::new()};phrases.push(Phrase{id,voices:payload.split(',').map(parse_voice).collect()})},"J"=>jumps.push(Jump{id,target:p[2].parse().unwrap_or(0)}),_=>{}}}Score{name:name.to_string(),phrases,jumps,scales}}
fn ratio_text(score:&Score,scale:&str)->String{score.scales.get(scale).cloned().unwrap_or_default().into_iter().filter(|r|r!="1/1").collect::<Vec<_>>().join(" ")}
fn draw_label(img:&mut RgbaImage,x:i32,y:i32,scale:&str,ratio:&str,col:(u8,u8,u8)){let w=(scale.len().max(ratio.len())as i32*9+30).max(80);let h=58;let x0=(x-w/2).clamp(30,img.width()as i32-w-30);let y0=(y-h/2).clamp(30,img.height()as i32-h-30);for yy in y0..y0+h{for xx in x0..x0+w{if xx>=0&&yy>=0&&xx<img.width()as i32&&yy<img.height()as i32{img.put_pixel(xx as u32,yy as u32,rgba((0,0,0),255));}}}for i in 0..4{let c=rgba((55,55,55),255);draw_line_segment_mut(img,((x0+i)as f32,(y0+i)as f32),((x0+w-i)as f32,(y0+i)as f32),c);draw_line_segment_mut(img,((x0+i)as f32,(y0+h-i)as f32),((x0+w-i)as f32,(y0+h-i)as f32),c)}/* text omitted in this compact checkpoint; Python file has exact label rendering */let _=(col,ratio);}
fn render(score:&Score,w:u32,h:u32)->RgbaImage{let mut img=RgbaImage::from_pixel(w,h,rgba((8,10,12),255));for y in (0..h).step_by(6){for x in (0..w).step_by(6){let v=n!(&score.name,"field",x/6,y/6);if v>.18{img.put_pixel(x,y,rgba((20,18,16),255));}}}let rect=(w as f64*.18,h as f64*.20,w as f64*.82,h as f64*.80);let g=fit(&gosper(3),rect.0,rect.1,rect.2,rect.3,28.0);thread(&mut img,&g,(8,8,8),11.0,255);let cx=(rect.0+rect.2)/2.0;let cy=(rect.1+rect.3)/2.0;let rr=((rect.2-rect.0).min(rect.3-rect.1))*.60;let total=score.phrases.len().max(1);for (i,p) in score.phrases.iter().enumerate(){let a0=-PI/2.0+2.0*PI*(i as f64)/(total as f64);let a1=-PI/2.0+2.0*PI*((i+1)as f64)/(total as f64)-0.12;let scale=p.voices.first().map(|v|v.scale.as_str()).unwrap_or("rast");draw_arc(&mut img,cx,cy,rr,a0,a1,(96,76,44),14.0);for k in 0..8{let a=a0+(a1-a0)*(k as f64+.5)/8.0;draw_filled_circle_mut(&mut img,((cx+(rr+35.0)*a.cos())as i32,(cy+(rr+35.0)*a.sin())as i32),if k==0{18}else{9},rgba(maqam_color(scale),255));}let mx=cx+(rr+175.0)*((a0+a1)/2.0).cos();let my=cy+(rr+175.0)*((a0+a1)/2.0).sin();draw_label(&mut img,mx as i32,my as i32,&scale.to_uppercase(),&ratio_text(score,scale),light(maqam_color(scale),.7));}img}
fn draw_arc(img:&mut RgbaImage,cx:f64,cy:f64,rr:f64,a0:f64,a1:f64,c:(u8,u8,u8),w:f64){let mut pts=Vec::new();for i in 0..80{let a=a0+(a1-a0)*i as f64/79.0;pts.push(P::new(cx+rr*a.cos(),cy+rr*a.sin()));}thread(img,&pts,c,w,255)}
fn carpet(design:&RgbaImage,kw:u32,kh:u32,kp:u32)->RgbaImage{let mut out=RgbaImage::from_pixel(kw*kp,kh*kp,rgba((12,11,10),255));for y in 0..kh{for x in 0..kw{let sx=x*(design.width()-1)/(kw-1).max(1);let sy=y*(design.height()-1)/(kh-1).max(1);let p=design.get_pixel(sx,sy);for yy in 0..kp{for xx in 0..kp{out.put_pixel(x*kp+xx,y*kp+yy,*p);}}}}out}
fn build(name:&str,text:&str,out:&PathBuf,args:&Args)->Result<()>{let s=parse(name,text);let design=render(&s,args.w,args.h);let img=carpet(&design,args.knots_w,args.knots_h,args.knot_px);img.save(out)?;Ok(())}
fn main()->Result<()>{let args=Args::parse();if args.all||args.mq.is_none(){build("magiccarpet.mq",MAGICCARPET,&PathBuf::from("magiccarpet_rust.png"),&args)?;build("growl.mq",GROWL,&PathBuf::from("growl_rust.png"),&args)?;}else{let text=fs::read_to_string(args.mq.as_ref().unwrap())?;build(&args.name,&text,args.out.as_ref().unwrap(),&args)?;}Ok(())}
