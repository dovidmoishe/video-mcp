"""Deterministic regression media and a synthetic visual diagnostic, using ordinary FFmpeg."""
import argparse,json,pathlib,subprocess
from PIL import Image,ImageDraw,ImageFont
p=argparse.ArgumentParser();p.add_argument('--output',default='.local/fixtures');p.add_argument('--ffmpeg',default='.local/ffmpeg-8.0-full_build-shared/bin/ffmpeg.exe');a=p.parse_args()
out=pathlib.Path(a.output);out.mkdir(parents=True,exist_ok=True)
font_path='C:/Windows/Fonts/arial.ttf';font=ImageFont.truetype(font_path,28)
ffmpeg=str(pathlib.Path(a.ffmpeg).resolve())
# turn/display: store each frame turned by a PIL transpose and attach the ffmpeg
# -display_rotation that turns it back, like phone footage. Players and the server
# should show exactly the unturned drawing.
def video(name,w,h,count,draw,turn=None,display=None):
 sw,sh=(h,w) if turn in (Image.Transpose.ROTATE_90,Image.Transpose.ROTATE_270) else (w,h)
 stored=out/name if display is None else out/('stored-'+name)
 process=subprocess.Popen([ffmpeg,'-v','error','-y','-f','rawvideo','-pixel_format','rgb24','-video_size',f'{sw}x{sh}','-framerate','30','-i','-','-an','-c:v','libx264','-crf','12','-pix_fmt','yuv420p',str(stored)],stdin=subprocess.PIPE)
 try:
  for i in range(count):
   im=Image.new('RGB',(w,h),'black');draw(im,i)
   if turn is not None:im=im.transpose(turn)
   process.stdin.write(im.tobytes())
 finally:process.stdin.close()
 if process.wait()!=0:raise RuntimeError(name)
 if display is not None:
  subprocess.run([ffmpeg,'-v','error','-y','-display_rotation',str(display),'-i',str(stored),'-c','copy',str(out/name)],check=True);stored.unlink()
def portrait(im,n):
 d=ImageDraw.Draw(im);d.rectangle((0,0,179,639),fill='#1450a0');d.rectangle((180,0,359,639),fill='#e07010');d.text((20,100),'PORTRAIT 248',font=font,fill='white')
video('portrait-regression.mp4',360,640,90,portrait)
# Stored 640x360 turned counterclockwise with an iPhone-style matrix (ffprobe rotation -90).
video('portrait-regression-rotated.mp4',360,640,90,portrait,Image.Transpose.ROTATE_90,-90)
def patch(im,n):
 if 34<=n<36:ImageDraw.Draw(im).rectangle((590,280,613,303),fill='white')
video('local-event.mp4',640,360,90,patch)
video('local-event-rotated.mp4',640,360,90,patch,Image.Transpose.ROTATE_180,180)
video('slow-change.mp4',320,180,90,lambda im,n:ImageDraw.Draw(im).rectangle((0,0,319,179),fill=(n*2,)*3))
def ocr(im,n):
 d=ImageDraw.Draw(im);d.rectangle((0,0,1199,699),fill='white');d.text((100,100),'BALANCE '+('248.00' if n<30 else '249.00'),font=ImageFont.truetype(font_path,56),fill='black');d.text((100+n,280),'MOVING',font=ImageFont.truetype(font_path,48),fill='black')
video('ocr-regression.mp4',1200,700,90,ocr)
# Stored 700x1200 with sideways text, turned clockwise; shown counterclockwise (rotation 90).
video('ocr-regression-rotated.mp4',1200,700,90,ocr,Image.Transpose.ROTATE_270,90)
def motion(im,n,reverse=False):
 if reverse:n=179-n
 d=ImageDraw.Draw(im);d.rectangle((0,0,1279,719),fill='#121c2c')
 d.text((45,38),'HELIOS',font=ImageFont.truetype(font_path,38),fill='white')
 d.text((45,96),'BALANCE '+('64.25' if n<75 else '64.75'),font=font,fill='white')
 x=180+round(850*min(1,max(0,(n-30)/30)))
 d.ellipse((x-38,322,x+38,398),fill='#26cbea')
 if n>=90:d.rectangle((200,490,255,545),fill='#d29830')
 if 66<=n<69:
  d.rectangle((1030,550,1179,591),fill='#ac1820');d.text((1035,552),'SYNC 9',font=font,fill='white')
 if n>=150:d.text((520,620),'READY',font=ImageFont.truetype(font_path,42),fill='white')
video('visual-pilot.mp4',1280,720,180,motion)
video('visual-pilot-reversed.mp4',1280,720,180,lambda im,n:motion(im,n,True))
video('visual-pilot-rotated.mp4',1280,720,180,motion,Image.Transpose.ROTATE_90,-90)
refs={'method':'Generation specification, locked before agent trials; no speech or audio streams.', 'forward':{'heading':'HELIOS','balance_before':'64.25','balance_after':'64.75','circle':'cyan/blue, left to right, 1.0 to 2.0 seconds','badge':'SYNC 9','badge_window':[2.2,2.3],'square_first':3.0,'order':'badge before square','ending':'READY','audio':False},'reverse':{'circle':'right to left, approximately 3.97 to 4.97 seconds','order':'square is already present, then badge appears around 3.7 seconds','ending':'no READY at end'}}
(out/'visual-references.json').write_text(json.dumps(refs,indent=2),encoding='utf-8');print('Generated ten visual fixtures (four stored turned with display matrices) and reference specification')
