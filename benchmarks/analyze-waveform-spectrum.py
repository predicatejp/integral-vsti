#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""保存済みの前後波形を位相非依存のパワースペクトルで比較する。解析時のみnumpy/matplotlibが必要。"""
import csv
import json
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parent.parent / 'target'
OUT = ROOT / 'waveform-spectrum'
OUT.mkdir(parents=True, exist_ok=True)
RATE = 48000
BANDS = [0, 200, 1000, 5000, 10000, 20000, 24000]


def load(suite, variant, case):
    path = ROOT / suite / variant
    offset = 0
    for row in csv.DictReader((path / 'manifest.csv').open()):
        count = int(row['samples'])
        if row['case'] == case:
            return np.memmap(path / 'audio.f64le', dtype='<f8', mode='r', offset=offset * 8, shape=(count,))
        offset += count
    raise ValueError(case)


def psd(samples, size, hop):
    """Welch: Hann、50%重複（呼び出しで指定）、各窓のDCを消さず片側PSDにする。"""
    window = np.hanning(size)
    frames = np.lib.stride_tricks.sliding_window_view(samples, size)[::hop]
    power = abs(np.fft.rfft(frames * window, axis=1)) ** 2 / (RATE * np.sum(window ** 2))
    power[:, 1:-1] *= 2
    return np.fft.rfftfreq(size, 1 / RATE), power


cases = []
for kind in range(7):
    cases += [
        ('waveform-audit-user', f'user/long/r48000/f{kind}/q10/active32', 48000, 32768),
        ('waveform-audit-user', f'user/fixed/r48000/f{kind}/c1200/q0.7071067811865476/active32', 4800, 8192),
        ('waveform-audit-user', f'user/cut-step/r48000/f{kind}/q10/active32', 0, 8192),
    ]
for parameter in [10, 11]:
    for mode in ['ramp', 'head', 'jump']:
        cases.append(('waveform-audit-automation', f'queue/{mode}/p{parameter}/stride256', 0, 8192))
# 同じ高音条件で正弦波とSawを比較し、振幅誤差とエッジ位置の影響を分ける。
for wave in [0, 3]:
    for kind in [0, 2, 4, 6]:
        cases.append(('waveform-audit', f'synth/long/r48000/f{kind}/w{wave}/n108/q10/u7', 48000, 32768))

spectra = {}
with (OUT / 'summary.csv').open('w') as out, (OUT / 'bands.csv').open('w') as bands:
    writer = csv.writer(out)
    writer.writerow(['case', 'skip_samples', 'fft_size', 'bin_hz', 'windows', 'rms_wave_difference_relative',
                     'rms_magnitude_spectrum_difference_relative', 'total_power_change_db', 'max_bin_change_db_above_minus80db'])
    bw = csv.writer(bands)
    bw.writerow(['case', 'low_hz', 'high_hz', 'before_power', 'after_power', 'change_db'])
    for suite, case, skip, size in cases:
        before, after = [load(suite, v, case)[skip:] for v in ['before', 'tables']]
        frequency, b = psd(before, size, size // 2)
        _, a = psd(after, size, size // 2)
        b, a = b.mean(axis=0), a.mean(axis=0)
        magnitude_error = np.linalg.norm(np.sqrt(a) - np.sqrt(b)) / np.linalg.norm(np.sqrt(b))
        wave_error = np.linalg.norm(after - before) / np.linalg.norm(before)
        mask = (b > b.max() * 1e-8) & (a > 0)
        change = 10 * np.log10(np.maximum(a, 1e-300) / np.maximum(b, 1e-300))
        writer.writerow([case, skip, size, RATE / size, (len(before)-size)//(size//2)+1,
                         wave_error, magnitude_error, 10*np.log10(a.sum()/b.sum()), abs(change[mask]).max()])
        for lo, hi in zip(BANDS, BANDS[1:]):
            selected = (frequency >= lo) & (frequency < hi)
            bp, ap = b[selected].sum() * RATE / size, a[selected].sum() * RATE / size
            bw.writerow([case, lo, hi, bp, ap, 10*np.log10(ap/bp)])
        spectra[case] = frequency, b, a

plt.rcParams.update({'font.size': 10, 'figure.dpi': 140})
selected = [
    ('user/long/r48000/f4/q10/active32', 'Stationary: HighPass2, Q=10 (1-10 s)'),
    ('user/fixed/r48000/f4/c1200/q0.7071067811865476/active32', 'Stationary: HighPass2, Q=1/sqrt(2) (0.1-1 s)'),
    ('user/cut-step/r48000/f6/q10/active32', 'Fast cutoff: BandPass2, Q=10 (0-1 s)'),
    ('queue/jump/p10/stride256', 'Sparse Cutoff jumps: LowPass2, Q=10 (0-1.024 s)'),
]
fig, axes = plt.subplots(4, 2, figsize=(13, 12), constrained_layout=True)
for row, (case, title) in enumerate(selected):
    f, b, a = spectra[case]
    reference = max(b.max(), a.max())
    for values, label, style in [(b, 'Before', '-'), (a, 'After', '--')]:
        axes[row, 0].plot(f/1000, 10*np.log10(np.maximum(values/reference, 1e-14)), style, label=label, lw=.8)
    axes[row, 0].set(title=title, ylabel='PSD (dB, shared reference)', ylim=(-100, 3), xlim=(0, 24))
    axes[row, 0].legend()
    mask = (b > b.max()*1e-8) & (a > 0)
    delta = np.where(mask, 10*np.log10(np.maximum(a, 1e-300)/np.maximum(b, 1e-300)), np.nan)
    axes[row, 1].plot(f/1000, delta, color='tab:red', lw=.7)
    axes[row, 1].set(ylabel='PSD change (dB)', xlim=(0, 24), title='Bins below -80 dB in baseline omitted')
for ax in axes[-1]: ax.set_xlabel('Frequency (kHz)')
fig.suptitle('Saw / Unison 7 / Detune 25 cent / 32 active notes; identical scaling', fontsize=14)
fig.savefig(OUT / 'spectra.png'); plt.close(fig)

case = 'user/cut-step/r48000/f6/q10/active32'
before, after = [load('waveform-audit-user', v, case) for v in ['before', 'tables']]
f, b = psd(before, 512, 32); _, a = psd(after, 512, 32)
time = (np.arange(len(b))*32 + 256)/RATE
reference = max(b.max(), a.max())
fig, axes = plt.subplots(3, 1, figsize=(11, 8), constrained_layout=True, sharex=True)
for ax, power, title in zip(axes, [b, a], ['Before', 'After']):
    mesh = ax.pcolormesh(time, f/1000, 10*np.log10(np.maximum(power.T/reference, 1e-12)), shading='auto', vmin=-80, vmax=0)
    ax.set(title=title, ylabel='kHz');fig.colorbar(mesh, ax=ax, label='dB (shared reference)')
mask = (b > reference*1e-8) & (a > reference*1e-8)
delta = np.where(mask, 10*np.log10(np.maximum(a,1e-300)/np.maximum(b,1e-300)), np.nan)
mesh = axes[2].pcolormesh(time, f/1000, delta.T, shading='auto', cmap='RdBu_r', vmin=-3, vmax=3)
axes[2].set(title='Local power change (values outside +/-3 dB saturate)', xlabel='Time (s)', ylabel='kHz')
fig.colorbar(mesh, ax=axes[2], label='dB change')
fig.suptitle('Fast cutoff / BandPass2 / Q=10; Hann 512, hop 32 (375 Hz modulation)', fontsize=13)
fig.savefig(OUT / 'spectrogram.png');plt.close(fig)
(OUT / 'metadata.json').write_text(json.dumps({'numpy':np.__version__, 'matplotlib':matplotlib.__version__,
    'sample_rate':RATE,'window':'numpy.hanning (symmetric Hann)','detrend':False,'psd_overlap':0.5,
    'phase_alignment':False,'individual_gain_normalization':False,'spectrogram_size':512,'spectrogram_hop':32}, indent=2)+'\n')
print(OUT)
