#!/usr/bin/env python3
"""Compare two float WAV renders: level of the difference relative to the reference,
overall and per octave band. Used to measure what single precision costs."""
import struct, sys
import numpy as np

def read(path):
    b = open(path, 'rb').read()
    i = b.index(b'data')
    n = struct.unpack('<I', b[i + 4:i + 8])[0]
    ch = struct.unpack('<H', b[22:24])[0]
    fs = struct.unpack('<I', b[24:28])[0]
    return np.frombuffer(b[i + 8:i + 8 + n], dtype='<f4').reshape(-1, ch).astype(np.float64), fs

ref, fs = read(sys.argv[1])
tst, _ = read(sys.argv[2])
n = min(len(ref), len(tst))
ref, tst = ref[:n, 0], tst[:n, 0]
diff = tst - ref
db = lambda x: 10 * np.log10(np.mean(x ** 2) + 1e-300)
print(f"reference level {db(ref):7.1f} dBFS, difference {db(diff):7.1f} dBFS, ratio {db(diff) - db(ref):7.1f} dB")
seg = fs // 2
print("per half second (difference - reference, dB):", " ".join(f"{db(diff[i:i+seg]) - db(ref[i:i+seg]):.0f}" for i in range(0, n - seg + 1, seg)))
R, D = np.abs(np.fft.rfft(ref * np.hanning(n))) ** 2, np.abs(np.fft.rfft(diff * np.hanning(n))) ** 2
f = np.fft.rfftfreq(n, 1 / fs)
print("band (Hz)      reference dB   difference dB")
lo = 31.25
while lo < fs / 2:
    m = (f >= lo) & (f < 2 * lo)
    print(f"{lo:7.0f}-{min(2*lo, fs/2):6.0f}   {10*np.log10(R[m].sum()+1e-300):8.1f}       {10*np.log10(D[m].sum()+1e-300):8.1f}")
    lo *= 2
