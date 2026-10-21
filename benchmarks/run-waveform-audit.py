#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""同一コンパイラで最適化各段の波形を比較し、ケース別の差をCSVに保存する。"""
import array
import csv
import json
import math
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
USER = '--user-case' in sys.argv
AUTOMATION = '--automation' in sys.argv
OUT = ROOT / ('target/waveform-audit-automation' if AUTOMATION else 'target/waveform-audit-user' if USER else 'target/waveform-audit')
OUT.mkdir(parents=True, exist_ok=True)
versions = {'before': 'bc84079', 'ftz': 'c6caf3b', 'silence': '971e47b',
            'osc-cache': 'a5411d6', 'filter-cache': '7bdde83', 'tables': '94c4221',
            'sine-only': '94c4221', 'detune-only': '94c4221'}
if USER:
    versions = {k:v for k,v in versions.items() if k in ['before','filter-cache','tables','sine-only','detune-only']}
if AUTOMATION:
    versions = {k:v for k,v in versions.items() if k in ['before','filter-cache','tables','sine-only','detune-only']}
modules = ['abi.rs','envelope.rs','filter.rs','oscillator.rs','parameters.rs','synth.rs','voice.rs']
flags = ['--edition=2024','-D','warnings','-C','opt-level=3','-C','panic=abort','-C','codegen-units=16']
for name, rev in versions.items():
    print(f'render {name} {rev}', flush=True)
    directory = OUT / name
    (directory / 'src').mkdir(parents=True, exist_ok=True)
    for file in modules + (['oscillator_tables.rs'] if name in ['tables','sine-only','detune-only'] else []):
        (directory / 'src' / file).write_bytes(subprocess.check_output(['git','show',f'{rev}:src/{file}'], cwd=ROOT))
    path = directory / 'src/oscillator.rs'
    source = path.read_text()
    if name == 'sine-only':
        old='interpolate(cents + 100.0, &tables::DETUNE)'; assert source.count(old)==1
        source=source.replace(old,'2.0_f64.powf(cents / 1200.0)')
    if name == 'detune-only':
        old='Self::Sine => sine_sample(phase)'; assert source.count(old)==1
        source=source.replace(old,'Self::Sine => (std::f64::consts::TAU * phase).sin()')
    path.write_text(source)
    shutil.copyfile(ROOT / 'src/float_mode.rs', directory / 'float_mode.rs')
    shutil.copyfile(ROOT / 'benchmarks/waveform-audit.rs',directory / 'audit.rs')
    shutil.copyfile(ROOT / 'benchmarks/automation-audit.rs',directory / 'automation.rs')
    subprocess.run(['rustc',*flags,*([] if name=='before' else ['--cfg','flush']),str(directory/'audit.rs'),'-o',str(directory/'audit')],check=True)
    with (directory/'audio.f64le').open('wb') as out:
        subprocess.run([str(directory/'audit'),str(directory/'manifest.csv'), *(['automation'] if AUTOMATION else ['user'] if USER else [])],stdout=out,check=True)

manifest = list(csv.DictReader((OUT/'before/manifest.csv').open()))
for name in versions:
    assert (OUT/name/'manifest.csv').read_bytes()==(OUT/'before/manifest.csv').read_bytes()


def read_samples(file, count):
    result=array.array('d');result.fromfile(file,count)
    if sys.byteorder!='little': result.byteswap()
    return result


def compare(name, reference):
    print(f'compare {name} / {reference}',flush=True)
    with (OUT/name/'audio.f64le').open('rb') as actual, (OUT/reference/'audio.f64le').open('rb') as expected, (OUT/f'{name}-vs-{reference}.csv').open('w') as out:
        writer=csv.writer(out)
        writer.writerow(['case','samples','changed','max_absolute','error_rms','relative_rms','before_peak','after_peak','nonfinite'])
        for row in manifest:
            count=int(row['samples']);a=read_samples(actual,count);b=read_samples(expected,count)
            changed=sum(x!=y for x,y in zip(a,b));nonfinite=sum(not(math.isfinite(x) and math.isfinite(y)) for x,y in zip(a,b))
            ap=max(map(abs,a));bp=max(map(abs,b))
            if nonfinite: maximum=rms=relative=math.inf
            elif not changed: maximum=rms=relative=0.0
            else:
                diff=[x-y for x,y in zip(a,b)];maximum=max(map(abs,diff))
                # 不安定な急変で大振幅になっても、二乗和をオーバーフローさせない。
                rms=maximum*math.sqrt(math.fsum((x/maximum)**2 for x in diff)/count)
                brms=bp*math.sqrt(math.fsum((x/bp)**2 for x in b)/count) if bp else 0.0
                relative=rms/brms if brms else math.inf
            writer.writerow([row['case'],count,changed,maximum,rms,relative,bp,ap,nonfinite])

for name in list(versions)[1:]: compare(name,'before')
for name, reference in [('silence','ftz'),('osc-cache','silence'),('filter-cache','osc-cache'),('tables','filter-cache')]:
    if name in versions and reference in versions: compare(name,reference)
(OUT/'metadata.json').write_text(json.dumps({'revisions':{k:subprocess.check_output(['git','rev-parse',v],text=True).strip() for k,v in versions.items()},'rustc':subprocess.check_output(['rustc','-Vv'],text=True),'flags':flags,'cases':len(manifest),'samples':sum(int(r['samples']) for r in manifest)},indent=2)+'\n')
print(OUT)
