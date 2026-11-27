"""Exercise actual MCP regional events, crops, display rotation, OCR, tracking and zero-work replay."""
import argparse,base64,json,os,pathlib,subprocess,time
from mcp_client import Client
p=argparse.ArgumentParser();p.add_argument('--server',default='target/release/video-mcp.exe');p.add_argument('--output',default='.local/visual-smoke',help='new directory for evidence; must not exist');a=p.parse_args()
out=pathlib.Path(a.output);out.mkdir(parents=True,exist_ok=False)
registration=json.loads(subprocess.check_output(['codex','mcp','get','video-local','--json'],text=True));environment=os.environ.copy();environment.update(registration['transport']['env'])
def fixture(n):return str((pathlib.Path('.local/fixtures')/n).resolve())
def call(client,name,request,label):
 start=time.perf_counter();response=client.call(name,request);seconds=time.perf_counter()-start
 page=json.loads(response['content'][0]['text']);(out/f'{label}.json').write_text(json.dumps(page,indent=2),encoding='utf-8')
 images=[c for c in response['content'] if c['type']=='image']
 for i,c in enumerate(images):
  assert c['mimeType']=='image/jpeg';(out/f'{label}-{i}.jpg').write_bytes(base64.b64decode(c['data']))
 return page,images,seconds
with (out/'server.log').open('w',encoding='utf-8') as log:
 c=Client(pathlib.Path(a.server).resolve(),environment,log)
 try:
  tools=c.request('tools/list',{})['tools'];assert len(tools)==6
  watch,_,_=call(c,'watch_video',{'source':fixture('portrait-regression.mp4'),'seconds':3,'event_budget':0},'portrait')
  assert (watch['frames'][0]['width'],watch['frames'][0]['height'])==(320,569)
  detail_request={'video_id':watch['video_id'],'timestamps':[0.2,0.3],'crop':{'x':0.5,'y':0,'width':0.5,'height':1},'max_width':1080,'max_height':1920}
  detail,images,cold=call(c,'inspect_video_frames',detail_request,'crop');assert len(images)==2 and detail['frames'][0]['source_pixels']==[180,0,180,640]
  replay,replayed,warm=call(c,'inspect_video_frames',detail_request,'crop-replay');assert replay['cache_hit'] and replay['decoded_frames']==0;assert images==replayed
  c.call('close_video',{'video_id':watch['video_id']})
  # Same drawing stored 640x360 with an iPhone-style display matrix: identical upright views and coordinates.
  rotated,_,_=call(c,'watch_video',{'source':fixture('portrait-regression-rotated.mp4'),'seconds':3,'event_budget':0},'portrait-rotated')
  video=[s for s in rotated['metadata']['streams'] if s['kind']=='video'][0]
  assert (video['width'],video['height'],video['rotation'],video['display_transform'],video['display_width'],video['display_height'])==(640,360,-90,'rotate_90_clockwise',360,640),video
  assert (rotated['frames'][0]['width'],rotated['frames'][0]['height'])==(320,569)
  turned,_,_=call(c,'inspect_video_frames',dict(detail_request,video_id=rotated['video_id']),'portrait-rotated-crop');assert turned['frames'][0]['source_pixels']==[180,0,180,640]
  c.call('close_video',{'video_id':rotated['video_id']})
  local,images,_=call(c,'watch_video',{'source':fixture('local-event.mp4'),'seconds':3,'event_budget':1},'local-event')
  assert all(f['near_black'] for f in local['frames']);assert len(images)==2 and local['event_frames'] and local['change_candidates']
  c.call('close_video',{'video_id':local['video_id']})
  ocr,_,_=call(c,'inspect_video_frames',{'source':fixture('ocr-regression.mp4'),'timestamps':[0,1],'include_ocr':True},'ocr')
  assert all(f['ocr']['status']=='recognized_unverified' for f in ocr['frames']);assert any('248' in x['before'] and '249' in x['after'] for x in ocr['text_changes'])
  motion,_,_=call(c,'inspect_video_frames',{'source':fixture('visual-pilot.mp4'),'timestamps':[1,1.1,1.2,1.3,1.4,1.5],'track_region':{'x':0.1,'y':0.43,'width':0.08,'height':0.14}},'motion')
  valid=[m for m in motion['motion'] if m['region'] is not None];assert len(valid)>=4,motion['motion'];assert valid[-1]['region']['x']>valid[0]['region']['x']
  result={'status':'passed','tools':len(tools),'portrait_sample':[320,569],'rotated_portrait_sample':[rotated['frames'][0]['width'],rotated['frames'][0]['height']],'event_evidence_images':len(images),'detail_cold_seconds':cold,'detail_cached_seconds':warm,'cached_decoded_frames':replay['decoded_frames'],'ocr_changes':len(ocr['text_changes']),'matched_motion_observations':len(valid)}
  (out/'result.json').write_text(json.dumps(result,indent=2),encoding='utf-8');print(json.dumps(result,indent=2))
 finally:c.close()
