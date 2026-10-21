#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""x86_64明示SIMDの実験。本体を変更せずtarget内のコピーで比較する。"""
import argparse
import csv
import json
import math
from pathlib import Path
import platform
import random
import shutil
import struct
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'target/simd-experiment'
DATA = ROOT / 'benchmarks/simd'
parser = argparse.ArgumentParser()
parser.add_argument('--cpu', type=int)
parser.add_argument('--verify-only', action='store_true')
args = parser.parse_args()
assert platform.machine() in ('x86_64', 'AMD64'), 'x86_64 only'
prefix = [] if args.cpu is None else ['taskset', '-c', str(args.cpu)]
OUT.mkdir(parents=True, exist_ok=True)
flags = ['--edition=2024', '-D', 'warnings', '-C', 'opt-level=3', '-C', 'panic=abort', '-C', 'codegen-units=16']
probe = OUT / 'probe.rs'
probe.write_text('fn main() { println!("{}", std::is_x86_feature_detected!("avx2")); }')
subprocess.run(['rustc', str(probe), '-o', str(OUT / 'probe')], check=True)
avx2 = subprocess.check_output([*prefix, str(OUT / 'probe')], text=True).strip() == 'true'
source = (ROOT / 'benchmarks/lookups.rs').read_text().split('fn main() {')[0].replace('../src/', 'src/') + (DATA / 'experiment.rs').read_text()
variants = {}
for isa in ['sse2', *(['avx2'] if avx2 else [])]:
    for candidate in ['scalar', 'unison', 'voice', 'both', 'batch-scalar']:
        variants[f'{isa}-{candidate}'] = (isa, candidate)
if avx2:
    variants['avx2-gather'] = ('avx2', 'gather')


def replace(directory, filename, old, new):
    path = directory / 'src' / filename
    text = path.read_text()
    assert text.count(old) == 1, f'source changed: {filename}: {old}'
    path.write_text(text.replace(old, new))


def kernel(isa, gather):
    width = 2 if isa == 'sse2' else 4
    intr = '_mm_' if width == 2 else '_mm256_'
    if gather:
        pair = '''let i = _mm256_cvttpd_epi32(index);
        (_mm256_i32gather_pd::<8>(crate::tables::SINE.as_ptr(), i),
         _mm256_i32gather_pd::<8>(crate::tables::SINE.as_ptr(), _mm_add_epi32(i, _mm_set1_epi32(1))))'''
    else:
        values = lambda extra: ', '.join(f'crate::tables::SINE[indices[{i}] as usize{extra}]' for i in reversed(range(width)))
        pair = f'''let mut indices = [0_i32; 4];
        _mm_storeu_si128(indices.as_mut_ptr().cast(), {intr}cvttpd_epi32(index));
        ({intr}set_pd({values('')}), {intr}set_pd({values(' + 1')}))'''
    stores = '\n'.join(
        f'_mm_storel_epi64({channel}.as_mut_ptr().add(i).cast(), _mm_castps_si128(values));' if width == 2
        else f'_mm_storeu_ps({channel}.as_mut_ptr().add(i), values);' for channel in ['left', 'right'])
    return (DATA / 'kernels.rs').read_text().replace('WIDTH', str(width)).replace('VECTOR', '__m128d' if width == 2 else '__m256d').replace('V_', intr).replace('LT(', '_mm_cmplt_pd(' if width == 2 else '_mm256_cmp_pd::<_CMP_LT_OQ>(').replace('TABLE_PAIR', pair).replace('STORE_FLOATS', stores)


commands = {}
for name, (isa, candidate) in variants.items():
    print(f'build {name}', flush=True)
    directory = OUT / name
    shutil.copytree(ROOT / 'src', directory / 'src', dirs_exist_ok=True)
    (directory / 'experiment.rs').write_text(source)
    (directory / 'simd.rs').write_text(kernel(isa, candidate == 'gather'))
    replace(directory, 'oscillator.rs', '    fn unit_sample(', '    pub(crate) fn unit_sample(')
    if candidate in ('unison', 'both', 'gather'):
        replace(directory, 'oscillator.rs', '''let sum: f64 = self.state.phases[..count]
            .iter()
            .map(|&phase| input.waveform.unit_sample(phase))
            .sum();''', '// SAFETY: 初期位相0と非負のfract更新により、全位相は[0, 1)。\n        let sum = unsafe { crate::simd::generate_unison_sample(&self.state.phases[..count], input.waveform) };')
        replace(directory, 'oscillator.rs', '''for (phase, increment) in self.state.phases[..input.rendered_voices()]
                .iter_mut()
                .zip(&self.state.phase_increments)
            {
                *phase = (*phase + increment).fract();
            }''', 'crate::simd::update_phase_sample(&mut self.state.phases[..input.rendered_voices()], &self.state.phase_increments);')
    if candidate in ('voice', 'both', 'batch-scalar'):
        for filename in ['filter.rs', 'voice.rs']:
            with (directory / 'src' / filename).open('a') as out:
                out.write('\n' + (DATA / filename).read_text())
        replace(directory, 'synth.rs', '''self.state.voices[..self.state.polyphony]
            .iter_mut()
            .map(|voice| voice.generate_sample(self.state.sample_rate))
            .sum()''', '''let mut sum = 0.0;
        let mut chunks = self.state.voices[..self.state.polyphony].chunks_exact_mut(crate::simd::LANES);
        for voices in &mut chunks {
            for sample in Voice::generate_parallel_sample(voices, self.state.sample_rate) { sum += sample; }
        }
        for voice in chunks.into_remainder() { sum += voice.generate_sample(self.state.sample_rate); }
        sum''')
    extra = ['-C', 'target-feature=+avx2'] if isa == 'avx2' else []
    if candidate == 'batch-scalar':
        extra += ['--cfg', 'scalar_batch']
    command = ['rustc', *flags, *extra, str(directory / 'experiment.rs'), '-o', str(directory / 'bench')]
    commands[name] = [part.replace(str(ROOT), '.') for part in command]
    subprocess.run(command, check=True)
    with (directory / 'verification.txt').open('w') as out:
        subprocess.run([*prefix, str(directory / 'bench'), 'verify'], stdout=out, stderr=out, check=True)

# 同じISAの対照群に対し、音源全体を比較。f64内部精度と加算順を維持する。
references = {}
with (OUT / 'audio-errors.csv').open('w') as out:
    writer = csv.writer(out)
    writer.writerow(['variant', 'samples', 'max_absolute', 'rms_error', 'bit_exact'])
    for name, (isa, _) in variants.items():
        data = subprocess.check_output([*prefix, str(OUT / name / 'bench'), 'audio'])
        samples = struct.unpack(f'<{len(data)//8}d', data)
        assert len(samples) == 258048
        if isa not in references:
            references[isa] = (data, samples)
        raw, expected = references[isa]
        differences = [a-b for a, b in zip(samples, expected)]
        maximum = max(map(abs, differences))
        rms = math.sqrt(sum(d*d for d in differences)/len(samples))
        writer.writerow([name, len(samples), maximum, rms, data == raw])
        assert maximum <= 1e-12, f'{name}: {maximum}'

metadata = {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
            'platform': platform.platform(), 'cpu_affinity': args.cpu, 'avx2': avx2,
            'variants': variants, 'commands': commands}
(OUT / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
if not args.verify_only:
    orders = []
    for repeat in range(3):
        order = list(variants)
        random.Random(20261022 + repeat).shuffle(order)
        orders.append(order)
        for name in order:
            print(f'measure {repeat} {name}', flush=True)
            with (OUT / f'{name}-{repeat}.csv').open('w') as out:
                subprocess.run([*prefix, str(OUT / name / 'bench')], stdout=out, check=True)
    (OUT / 'orders.json').write_text(json.dumps(orders, indent=2) + '\n')
print(OUT)
