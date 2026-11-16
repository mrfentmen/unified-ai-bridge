use super::{invalid, unsupported, MAX_INPUT};
use crate::failure::ToolFailure;
use serde_json::{json, Value};

#[derive(Debug)]
pub(super) struct Wave<'a> {
    pub(super) channels: u16,
    pub(super) rate: u32,
    pub(super) bits: u16,
    align: u16,
    data: &'a [u8],
    other: Vec<String>,
}
fn u16_at(s: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(s[i..i + 2].try_into().unwrap())
}
fn u32_at(s: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(s[i..i + 4].try_into().unwrap())
}
pub(super) fn parse(bytes: &[u8]) -> Result<Wave<'_>, ToolFailure> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(unsupported("media tools currently support little-endian PCM RIFF/WAVE only; MP3, MP4, compressed audio, RF64 and video are unavailable"));
    }
    if bytes.len() as u64 > MAX_INPUT || u64::from(u32_at(bytes, 4)) + 8 != bytes.len() as u64 {
        return Err(invalid(
            "WAV length is truncated, oversized or contains trailing bytes",
        ));
    }
    let mut at = 12;
    let mut format = None;
    let mut data = None;
    let mut other = vec![];
    let mut chunks = 0;
    while at < bytes.len() {
        chunks += 1;
        if chunks > 128 || bytes.len() - at < 8 {
            return Err(invalid(
                "WAV chunk count exceeds 128 or header is truncated",
            ));
        }
        let id = &bytes[at..at + 4];
        let length = u32_at(bytes, at + 4) as usize;
        at += 8;
        let end = at
            .checked_add(length)
            .ok_or_else(|| invalid("WAV chunk length overflow"))?;
        let padded = end
            .checked_add(length % 2)
            .ok_or_else(|| invalid("WAV padding overflow"))?;
        if padded > bytes.len() {
            return Err(invalid("WAV chunk extends outside container"));
        }
        let chunk = &bytes[at..end];
        match id {
            b"fmt " => {
                if format.is_some() || !matches!(chunk.len(), 16 | 18) {
                    return Err(unsupported(
                        "WAV needs one PCM fmt chunk with no extended format data",
                    ));
                }
                if u16_at(chunk, 0) != 1 || (chunk.len() == 18 && u16_at(chunk, 16) != 0) {
                    return Err(unsupported("only integer PCM WAV is supported; float, compressed and extensible formats are unavailable"));
                }
                let channels = u16_at(chunk, 2);
                let rate = u32_at(chunk, 4);
                let byte_rate = u32_at(chunk, 8);
                let align = u16_at(chunk, 12);
                let bits = u16_at(chunk, 14);
                if !(1..=2).contains(&channels)
                    || !(8000..=192000).contains(&rate)
                    || !matches!(bits, 8 | 16 | 24 | 32)
                {
                    return Err(unsupported(
                        "WAV supports mono/stereo, 8–192 kHz and 8/16/24/32-bit integer PCM",
                    ));
                }
                if align != channels * (bits / 8) || byte_rate != rate * u32::from(align) {
                    return Err(invalid("WAV block alignment or byte rate is inconsistent"));
                }
                format = Some((channels, rate, bits, align));
            }
            b"data" => {
                if data.is_some() || format.is_none() {
                    return Err(invalid("WAV needs one data chunk after its format"));
                }
                data = Some(chunk);
            }
            b"JUNK" | b"PAD " => {}
            _ => {
                // Return only hex identifiers, never unbounded tags or metadata.
                other.push(hex::encode(id));
            }
        }
        at = padded;
    }
    let (channels, rate, bits, align) =
        format.ok_or_else(|| invalid("WAV format chunk missing"))?;
    let data = data.ok_or_else(|| invalid("WAV audio data missing"))?;
    if data.is_empty() || data.len() % usize::from(align) != 0 {
        return Err(invalid(
            "WAV has no frames or an incomplete interleaved frame",
        ));
    }
    if data.len() as u64 / u64::from(align) > u64::from(rate) * 1800 {
        return Err(invalid("WAV exceeds 30-minute duration limit"));
    }
    Ok(Wave {
        channels,
        rate,
        bits,
        align,
        data,
        other,
    })
}
impl Wave<'_> {
    fn frames(&self) -> u64 {
        self.data.len() as u64 / u64::from(self.align)
    }
    pub(super) fn metadata(&self) -> Value {
        json!({"format":"wav","codec":"integer_pcm","channels":self.channels,"sample_rate":self.rate,"bits_per_sample":self.bits,"block_align":self.align,"frames":self.frames(),"duration_seconds":self.frames() as f64/f64::from(self.rate),"audio_bytes":self.data.len(),"extra_chunk_ids_hex":self.other,"trim_supported":self.other.is_empty(),"inspection":"validated container/frame layout only; no playback, waveform analysis, transcription or audio upload"})
    }
    pub(super) fn trim(&self, start: u64, end: u64) -> Result<Vec<u8>, ToolFailure> {
        if !self.other.is_empty() {
            return Err(unsupported("WAV has metadata, markers or unsupported chunks; refusing to discard them during trim"));
        }
        if start >= end || end > self.frames() {
            return Err(invalid(
                "trim requires 0 <= start_frame < end_frame <= frames; end is exclusive",
            ));
        }
        let data = &self.data
            [(start * u64::from(self.align)) as usize..(end * u64::from(self.align)) as usize];
        let padding = data.len() % 2;
        let mut out = Vec::with_capacity(44 + data.len() + padding);
        out.extend(b"RIFF");
        out.extend(((36 + data.len() + padding) as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(self.channels.to_le_bytes());
        out.extend(self.rate.to_le_bytes());
        out.extend((self.rate * u32::from(self.align)).to_le_bytes());
        out.extend(self.align.to_le_bytes());
        out.extend(self.bits.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        if padding != 0 {
            out.push(0);
        }
        if out.len() as u64 > MAX_INPUT {
            return Err(invalid("trim output exceeds 32 MiB"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn sample(bits: u16, channels: u16) -> Vec<u8> {
        // Real generated PCM fixture for parser tests, not a provider/media claim.
        let bytes_per = usize::from(channels * (bits / 8));
        let bytes = (0..bytes_per * 17)
            .map(|i| (i % 256) as u8)
            .collect::<Vec<_>>();
        let w = Wave {
            channels,
            rate: 44100,
            bits,
            align: channels * (bits / 8),
            data: &bytes,
            other: vec![],
        };
        w.trim(0, 17).unwrap()
    }
    #[test]
    fn trims_preserve_every_sample_width_and_odd_byte_padding_with_independent_decoder() {
        for bits in [8, 16, 24, 32] {
            for channels in [1, 2] {
                let bytes = sample(bits, channels);
                let w = parse(&bytes).unwrap();
                let out = w.trim(2, 11).unwrap();
                let result = parse(&out).unwrap();
                assert_eq!(result.frames(), 9);
                assert_eq!(
                    result.data,
                    &w.data[usize::from(w.align) * 2..usize::from(w.align) * 11]
                );
                let mut original = hound::WavReader::new(Cursor::new(&bytes)).unwrap();
                let mut reader = hound::WavReader::new(Cursor::new(&out)).unwrap();
                assert_eq!(reader.duration(), 9);
                assert_eq!(reader.spec(), original.spec());
                let expected = original
                    .samples::<i32>()
                    .skip(2 * usize::from(channels))
                    .take(9 * usize::from(channels))
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(
                    reader
                        .samples::<i32>()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap(),
                    expected
                );
            }
        }
    }
    #[test]
    fn malformed_lengths_formats_frame_alignment_and_extra_metadata_fail_closed() {
        let base = sample(16, 2);
        for cut in 0..base.len() {
            assert!(parse(&base[..cut]).is_err());
        }
        for (offset, bytes) in [
            (20, 3u16.to_le_bytes().to_vec()),
            (22, 9u16.to_le_bytes().to_vec()),
            (24, 0u32.to_le_bytes().to_vec()),
            (28, 1u32.to_le_bytes().to_vec()),
            (32, 1u16.to_le_bytes().to_vec()),
            (34, 12u16.to_le_bytes().to_vec()),
            (40, u32::MAX.to_le_bytes().to_vec()),
        ] {
            let mut b = base.clone();
            b[offset..offset + bytes.len()].copy_from_slice(&bytes);
            assert!(parse(&b).is_err());
        }
        let mut b = base.clone();
        b.extend(b"LIST\x04\0\0\0INFO");
        let len = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&len.to_le_bytes());
        let w = parse(&b).unwrap();
        assert_eq!(w.metadata()["trim_supported"], false);
        assert!(w.trim(0, 1).is_err());
        let w = parse(&base).unwrap();
        for (start, end) in [(0, 0), (1, 0), (0, 18), (u64::MAX, u64::MAX)] {
            assert!(w.trim(start, end).is_err());
        }
        let mut duplicate = base.clone();
        duplicate.extend(b"data\x04\0\0\0\0\0\0\0");
        let len = (duplicate.len() - 8) as u32;
        duplicate[4..8].copy_from_slice(&len.to_le_bytes());
        assert!(parse(&duplicate).is_err());
    }

    #[test]
    fn chunk_and_duration_budgets_fail_before_transformation() {
        let mut bytes = sample(8, 1);
        for _ in 0..127 {
            bytes.extend(b"JUNK\0\0\0\0");
        }
        let len = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&len.to_le_bytes());
        assert!(parse(&bytes).unwrap_err().to_string().contains("128"));
        let data = vec![0; 8000 * 1800 + 1];
        let w = Wave {
            channels: 1,
            rate: 8000,
            bits: 8,
            align: 1,
            data: &data,
            other: vec![],
        };
        let bytes = w.trim(0, data.len() as u64).unwrap();
        assert!(parse(&bytes).unwrap_err().to_string().contains("30-minute"));
    }
}
