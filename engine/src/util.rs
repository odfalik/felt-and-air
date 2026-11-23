//! Small self-contained helpers: WAV output and a radix-2 FFT for analysis tools and tests.

use std::f64::consts::PI;
use std::io::Write;

pub fn write_wav_float(path: &str, interleaved: &[f32], channels: u16, sample_rate: u32) -> std::io::Result<()> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write_wav_float_to(&mut f, interleaved, channels, sample_rate)?;
    f.flush()
}

/// Same WAV encoder for callers that require exclusive file creation.
pub fn write_wav_float_to(mut f: impl Write, interleaved: &[f32], channels: u16, sample_rate: u32) -> std::io::Result<()> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid or oversized float WAV");
    if channels == 0 || sample_rate == 0 || interleaved.len() % channels as usize != 0 {
        return Err(invalid());
    }
    let data_bytes = interleaved.len().checked_mul(4).and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n <= u32::MAX - 36).ok_or_else(invalid)?;
    let byte_rate = sample_rate.checked_mul(channels as u32).and_then(|n| n.checked_mul(4)).ok_or_else(invalid)?;
    let alignment = channels.checked_mul(4).ok_or_else(invalid)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_bytes).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?;
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&alignment.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_bytes.to_le_bytes())?;
    for x in interleaved {
        f.write_all(&x.to_le_bytes())?;
    }
    Ok(())
}

/// In-place complex FFT (re, im); the length must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for h in 0..len / 2 {
                let (a, b) = (i + h, i + h + len / 2);
                let (vr, vi) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - vr;
                im[b] = im[a] - vi;
                re[a] += vr;
                im[a] += vi;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// Hann-windowed magnitude spectrum of a real signal (zero-padded to nfft).
pub fn mag_spectrum(x: &[f64], nfft: usize) -> Vec<f64> {
    let n = x.len().min(nfft);
    let mut re = vec![0.0; nfft];
    let mut im = vec![0.0; nfft];
    for i in 0..n {
        re[i] = x[i] * (0.5 - 0.5 * (2.0 * PI * i as f64 / (x.len() - 1) as f64).cos());
    }
    fft(&mut re, &mut im);
    (0..nfft / 2).map(|i| re[i].hypot(im[i])).collect()
}

/// Refine a spectral peak near bin b by parabolic interpolation on log magnitude.
pub fn peak_bin(m: &[f64], b: usize) -> f64 {
    if b == 0 || b + 1 >= m.len() {
        return b as f64;
    }
    let (a, c, d) = ((m[b - 1] + 1e-300).ln(), (m[b] + 1e-300).ln(), (m[b + 1] + 1e-300).ln());
    let den = a - 2.0 * c + d;
    b as f64 + if den != 0.0 { 0.5 * (a - d) / den } else { 0.0 }
}

/// Frequency of the strongest spectral peak inside [lo, hi] Hz.
pub fn peak_frequency(x: &[f64], fs: f64, lo: f64, hi: f64) -> f64 {
    let nfft = (x.len() * 4).next_power_of_two();
    let m = mag_spectrum(x, nfft);
    let b0 = (lo / fs * nfft as f64) as usize;
    let b1 = (m.len() - 2).min((hi / fs * nfft as f64) as usize);
    let mut best = b0;
    for b in b0..=b1 {
        if m[b] > m[best] {
            best = b;
        }
    }
    peak_bin(&m, best) * fs / nfft as f64
}
