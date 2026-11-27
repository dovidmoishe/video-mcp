param([string]$Output = '.local/fixtures', [string]$FFmpeg = 'ffmpeg')
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $Output | Out-Null
function Run-FFmpeg([string[]]$Arguments) {
    & $FFmpeg -hide_banner -loglevel error -y @Arguments
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg fixture generation failed: $Arguments" }
}
Run-FFmpeg @('-f','lavfi','-i','testsrc2=size=320x240:rate=25:duration=3','-f','lavfi','-i','sine=frequency=440:sample_rate=48000:duration=3','-c:v','libx264','-bf','3','-pix_fmt','yuv420p','-c:a','aac','-ac','2',(Join-Path $Output 'av.mp4'))
Run-FFmpeg @('-i',(Join-Path $Output 'av.mp4'),'-an','-c:v','copy',(Join-Path $Output 'silent.mp4'))
Run-FFmpeg @('-f','lavfi','-i','sine=frequency=440:sample_rate=8000:duration=3','-c:a','pcm_s16le',(Join-Path $Output 'tone.wav'))
Run-FFmpeg @('-i',(Join-Path $Output 'av.mp4'),'-map','0','-c','copy','-output_ts_offset','5',(Join-Path $Output 'offset.mkv'))
Run-FFmpeg @('-i',(Join-Path $Output 'av.mp4'),'-vf',"select='if(lt(t,1),not(mod(n,2)),not(mod(n,5)))'",'-fps_mode','vfr','-an','-c:v','libx264',(Join-Path $Output 'vfr.mkv'))
[System.IO.File]::WriteAllText((Join-Path (Resolve-Path $Output) 'invalid.bin'),'not a media container')
Run-FFmpeg @('-i',(Join-Path $Output 'av.mp4'),'-an','-c:v','copy','-bsf:v','h264_mp4toannexb',(Join-Path $Output 'missing-pts.h264'))
Run-FFmpeg @('-f','lavfi','-i','sine=frequency=440:sample_rate=16000:duration=2','-af','asetpts=PTS+gte(T\,1)/TB','-c:a','pcm_s16le',(Join-Path $Output 'audio-gap.mkv'))
Run-FFmpeg @('-f','lavfi','-i','anullsrc=r=16000:cl=mono','-t','1','-c:a','pcm_s16le',(Join-Path $Output 'silence.wav'))
Run-FFmpeg @('-f','lavfi','-i','color=c=black:size=320x180:rate=30:duration=3','-vf','drawbox=color=white:t=fill:enable=between(t\,1.2\,1.3)','-c:v','libx264','-pix_fmt','yuv420p',(Join-Path $Output 'brief-flash.mp4'))
# Display matrices: one stored 640x360 picture (red/green over blue/yellow quadrants,
# a white 60x60 marker at 40,30 and a magenta 60x60 flash at 520,260 during 1.2-1.3s)
# remuxed with each matrix the ffmpeg CLI attaches. -display_* are input options.
# Each .rgb is the first frame as the ffmpeg CLI shows it (autorotate on): rgb24,
# 640x360 or 360x640.
$orientation = Join-Path $Output 'orientation-source.mp4'
Run-FFmpeg @('-f','lavfi','-i','color=c=0xdc2828:size=640x360:rate=30:duration=2','-vf','drawbox=x=320:y=0:w=320:h=180:color=0x28c828:t=fill,drawbox=x=0:y=180:w=320:h=180:color=0x283cdc:t=fill,drawbox=x=320:y=180:w=320:h=180:color=0xe6d228:t=fill,drawbox=x=40:y=30:w=60:h=60:color=white:t=fill,drawbox=x=520:y=260:w=60:h=60:color=0xe628e6:t=fill:enable=between(t\,1.2\,1.3)','-c:v','libx264','-pix_fmt','yuv420p',$orientation)
$variants = [ordered]@{
    'orientation-none' = @()
    'orientation-r90' = @('-display_rotation','90')
    'orientation-r-90' = @('-display_rotation','-90')
    'orientation-r180' = @('-display_rotation','180')
    'orientation-hflip' = @('-display_hflip')
    'orientation-vflip' = @('-display_vflip')
    'orientation-r90-hflip' = @('-display_rotation','90','-display_hflip')
    'orientation-r-90-hflip' = @('-display_rotation','-90','-display_hflip')
}
foreach ($name in $variants.Keys) {
    $file = Join-Path $Output "$name.mp4"
    Run-FFmpeg (@($variants[$name]) + @('-i',$orientation,'-c','copy',$file))
    Run-FFmpeg @('-i',$file,'-frames:v','1','-f','rawvideo','-pix_fmt','rgb24',(Join-Path $Output "$name.rgb"))
}
Write-Output "Fixtures generated in $((Resolve-Path $Output).Path)"
