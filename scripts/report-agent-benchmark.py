"""Render saved ordinary-tools/MCP benchmark evidence; never run a model."""
import argparse
import html
import json
import math
import pathlib
import statistics

parser = argparse.ArgumentParser()
parser.add_argument('--data', default='.local/ordinary-agent-benchmark')
parser.add_argument('--canvas', required=True)
args = parser.parse_args()
directory = pathlib.Path(args.data)
runs = json.loads((directory / 'agent-results.json').read_text(encoding='utf-8'))
method = json.loads((directory / 'methodology.json').read_text(encoding='utf-8'))
review_path = directory / 'review.json'
review = json.loads(review_path.read_text(encoding='utf-8')) if review_path.exists() else None
labels = {'baseline': 'Ordinary tools', 'mcp': 'Video MCP'}
audio_setup = ('The identical upstream ggml speech model was staged for both arms. Ordinary FFmpeg includes its own Whisper filter; baseline agents chose whether/how to use it. No project speech implementation was exposed.' if method.get('speech_model_sha256') else 'No local speech model was exposed to the baseline. Its FFmpeg includes a Whisper filter but needs a model; MCP had its configured model. This setup difference limits coverage comparisons.')
assert [r['arm'] for r in runs] == method['order'][:len(runs)], 'run order does not match methodology'
summary = {'arms': {}, 'all_runs_completed': len(runs) == len(method['order']) and all(r['status'] == 'completed' for r in runs)}
for arm in labels:
    rows = [r for r in runs if r['arm'] == arm and r['status'] == 'completed']
    if rows:
        summary['arms'][arm] = {
            'completed': len(rows),
            'median_seconds': statistics.median(r['wall_seconds'] for r in rows),
            'median_cumulative_input_tokens': statistics.median(r['usage']['input_tokens'] for r in rows),
            'median_uncached_input_tokens': statistics.median(r['usage']['input_tokens'] - r['usage']['cached_input_tokens'] for r in rows),
            'median_output_tokens': statistics.median(r['usage']['output_tokens'] for r in rows),
        }
if len(summary['arms']) == 2:
    baseline = summary['arms']['baseline']['median_seconds']
    mcp = summary['arms']['mcp']['median_seconds']
    summary['mcp_wall_reduction_percent'] = 100 * (1 - mcp / baseline)
(directory / 'summary.json').write_text(json.dumps(summary, indent=2), encoding='utf-8')

maximum = math.ceil(max(r['wall_seconds'] for r in runs) / 20) * 20
bars = []
for r in runs:
    label = f"{r['run']}. {labels[r['arm']]}"
    color = 'var(--chart-1)' if r['arm'] == 'mcp' else 'var(--muted-foreground)'
    bars.append(f'<div class="row"><span>{label}</span><div class="track"><div class="bar" style="width:{100*r["wall_seconds"]/maximum:.3f}%;background:{color}"></div></div><span>{r["wall_seconds"]:.2f} s</span></div>')
page = '''<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>
body{margin:0;padding:0;color:var(--foreground);font:15px/1.5 var(--font-sans)}h2{font-size:20px;font-weight:500;margin:22px 0 12px}h3{font-size:17px;font-weight:500}.muted{color:var(--muted-foreground);overflow-wrap:anywhere}.chart{height:255px}.row{display:grid;grid-template-columns:112px minmax(0,1fr) 72px;gap:8px;align-items:center;min-height:44px;font-size:14px}.track{height:25px;border-left:1px solid var(--border)}.bar{height:25px}.axis{display:flex;justify-content:space-between;margin:8px 80px 0 120px;font-size:12px;color:var(--muted-foreground)}.axislabel{text-align:center;font-size:13px;margin:6px}section{border-top:1px solid var(--border);margin:24px 0 0;padding:8px 0 0}li{margin:6px 0}details{border-top:1px solid var(--border);margin-top:12px;padding-top:12px}summary{cursor:pointer}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:14px/1.5 var(--font-sans)}code{font-family:var(--font-mono)}
</style></head><body><h2>Ordinary Codex tools versus video MCP</h2>'''
page += f'<p class="muted">22 November 2026 · 15-second 1080p clip · {html.escape(method["model"])} / {html.escape(method["reasoning_effort"])} · {len(runs)} fresh contexts</p>'
page += '<p>Ordinary tools: standalone FFmpeg/ffprobe, Python/Pillow and image viewing. No project CLI, library, server or skill. MCP: the release server and local model. Both agents chose their own inspection steps for the same task.</p>'
page += '<h3>End-to-end wall time, including startup and model latency</h3><div class="chart" role="img" aria-label="Individual Codex trial wall times in seconds, zero baseline">' + ''.join(bars) + f'<div class="axis"><span>0</span><span>{maximum/2:g}</span><span>{maximum:g}</span></div><p class="axislabel">Wall time (seconds); rows follow execution order</p></div>'
if len(summary['arms']) == 2:
    delta = summary['mcp_wall_reduction_percent']
    difference = f'{abs(delta):.1f}% ' + ('lower' if delta >= 0 else 'higher')
    page += f'<p><strong>Median: ordinary tools {baseline:.1f} s; MCP {mcp:.1f} s.</strong> MCP wall time was {difference} in this case study. Answer coverage must be considered separately.</p>'
    b = summary['arms']['baseline']
    m = summary['arms']['mcp']
    page += f'<p>Median cumulative input: ordinary tools {b["median_cumulative_input_tokens"]:,.0f} tokens; MCP {m["median_cumulative_input_tokens"]:,.0f}. Median uncached input: {b["median_uncached_input_tokens"]:,.0f} versus {m["median_uncached_input_tokens"]:,.0f}. These sum across model requests, vary with cache behavior and are not a price comparison.</p>'
page += '<section><h3>Method and limits</h3><ul><li>Identical outcome-based prompt; no fixed frame count, resolution or required pipeline.</li><li>Sequential baseline → MCP → MCP → baseline. Same configured model, reasoning and permissions. Neutral clip names and separate temporary workspaces.</li><li>FFmpeg was staged on PATH before both arms; provisioning excluded. No downloads, package installs or external media services.</li><li>'+html.escape(audio_setup)+'</li><li>One short clip and two runs per arm. Caches and provider/network timing vary. No Gemini test; no general accuracy or speed claim.</li><li>Prompt isolation plus tool-log audit, not an OS sandbox. Raw logs, workspace outputs and timed failures are retained. Built-in image-view invocations are not all exposed by JSONL; shell/MCP counts are not total tool counts.</li></ul></section>'
if review:
    page += '<section><h3>Manual answer and tool audit</h3>'
    for text in review['findings']:
        page += '<p>' + html.escape(text) + '</p>'
    page += '</section>'
page += '<section><h3>Actual agent answers</h3>'
for r in runs:
    page += f'<details><summary>Run {r["run"]}: {labels[r["arm"]]} — {r["wall_seconds"]:.2f} s · {html.escape(r["status"])}</summary><pre>{html.escape(r["response"] or "No final response")}</pre></details>'
page += '</section><p class="muted">Source: methodology.json, agent-results.json, raw JSONL/tool logs and review.json in '+html.escape(str(directory))+'. Token counts in summary.json are cumulative across model requests, not unique source tokens or a price estimate.</p></body></html>'
(directory / 'report.html').write_text(page, encoding='utf-8')

canvas = '''import {BarChart,useHostTheme} from "cursor/canvas";
const runs=RUNS;
type ArmMetrics={completed:number;median_seconds:number;median_cumulative_input_tokens:number;median_uncached_input_tokens:number;median_output_tokens:number};
const totals:{arms:Partial<Record<"baseline"|"mcp",ArmMetrics>>;all_runs_completed:boolean;mcp_wall_reduction_percent?:number}=TOTALS;
const findings:FINDING_TYPE=FINDINGS;
export default function OrdinaryVideoBenchmark(){const theme=useHostTheme();
const categories=Array.from({length:Math.max(runs.filter(r=>r.arm==="baseline").length,runs.filter(r=>r.arm==="mcp").length)},(_,i)=>`Trial ${i+1}`);
return <main style={{padding:24,color:theme.text.primary,fontFamily:"system-ui",maxWidth:960}}>
<h1 style={{fontSize:24,fontWeight:500}}>Ordinary Codex tools versus video MCP</h1>
<p>Same 15-second 1080p clip, gpt-6.1-sol / xhigh, outcome-based task and permissions. Separate fresh contexts; baseline → MCP → MCP → baseline. Each agent chose its own processing and sampling.</p>
<p style={{color:theme.text.secondary}}>Baseline: standalone FFmpeg/ffprobe, Python/Pillow, image viewing. No project CLI/core/server/skill. MCP: release server. AUDIO_SETUP Preinstalled tool provisioning excluded; package installs and external media services disabled in both arms.</p>
<h2 style={{fontSize:20,fontWeight:500}}>End-to-end answer time</h2><p>Horizontal axis: trial within each arm. Vertical axis: wall seconds including startup, model and network time. Source: four local Codex exec trials, 22 November 2026.</p>
<BarChart categories={categories} series={[{name:"Ordinary tools",data:runs.filter(r=>r.arm==="baseline").map(r=>r.wall_seconds),tone:"neutral"},{name:"Video MCP",data:runs.filter(r=>r.arm==="mcp").map(r=>r.wall_seconds),tone:"info"}]} height={260} valueSuffix=" s" showValues/>
<p>Every observation: {runs.map(r=>`${r.run}. ${r.arm}: ${r.wall_seconds}s (${r.status})`).join("; ")}</p>
{totals.arms.baseline&&totals.arms.mcp&&<section><h2 style={{fontSize:20,fontWeight:500}}>Cumulative model input</h2><p>Horizontal axis: integration; vertical axis: median input tokens, summed across model requests. Cache behavior varies; these are not unique video tokens or a bill. Source: Codex turn.completed usage, this study.</p><BarChart categories={["Ordinary tools","Video MCP"]} series={[{name:"Total input",data:[totals.arms.baseline.median_cumulative_input_tokens,totals.arms.mcp.median_cumulative_input_tokens],tone:"neutral"},{name:"Uncached input",data:[totals.arms.baseline.median_uncached_input_tokens,totals.arms.mcp.median_uncached_input_tokens],tone:"info"}]} height={240} showValues/></section>}
<h2 style={{fontSize:20,fontWeight:500}}>Answer quality and independence</h2>{findings.map((f,i)=><p key={i}>{f}</p>)}
<h2 style={{fontSize:20,fontWeight:500}}>Actual answers</h2>{runs.map(r=><details key={r.run} style={{marginTop:12}}><summary>{r.run}. {r.arm}: {r.wall_seconds}s</summary><p style={{whiteSpace:"pre-wrap"}}>{r.response}</p></details>)}
<p style={{color:theme.text.secondary}}>Limits: one short clip, two trials per arm, variable provider/cache latency, no OS containment and no Gemini test. Results do not establish general speed or accuracy. Source: DATA_SOURCE raw JSONL, prompts, outputs, review and methodology. Built-in image-view invocations are not all exposed by the event stream.</p>
</main>}
'''.replace('RUNS', json.dumps(runs)).replace('TOTALS', json.dumps(summary)).replace('FINDING_TYPE', 'string[]').replace('FINDINGS', json.dumps(review['findings'] if review else [])).replace('AUDIO_SETUP', audio_setup).replace('DATA_SOURCE',str(directory).replace('\\','/'))
pathlib.Path(args.canvas).write_text(canvas, encoding='utf-8')
print(json.dumps(summary, indent=2))
