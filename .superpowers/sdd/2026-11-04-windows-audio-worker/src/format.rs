use crate::Result;

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
pub enum Encoding { Pcm, Float }
impl<'de> serde::Deserialize<'de> for Encoding {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match <String as serde::Deserialize>::deserialize(deserializer)?.as_str() {
            "Pcm" => Ok(Self::Pcm),
            "Float" => Ok(Self::Float),
            other => Err(serde::de::Error::unknown_variant(other, &["Pcm", "Float"])),
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub valid_bits: u16,
    pub align: u16,
    pub encoding: Encoding,
}

impl Format {
    pub fn validate(self) -> Result<Self> {
        if !(8_000..=192_000).contains(&self.rate)
            || !(1..=2).contains(&self.channels)
            || !matches!((self.encoding, self.bits),
                (Encoding::Pcm, 16 | 24 | 32) | (Encoding::Float, 32))
            || self.valid_bits != self.bits
            || self.align != self.channels * (self.bits / 8) {
            return Err("Unsupported microphone/loopback mix format".into());
        }
        Ok(self)
    }

    pub fn validate_mask(self, mask: u32) -> Result<()> {
        self.validate()?;
        if mask != 0 && mask != if self.channels == 1 { 4 } else { 3 } {
            return Err("Unsupported speaker layout; no channel remapping is performed".into());
        }
        Ok(())
    }

    pub fn byte_len(self, frames: u32) -> Result<usize> {
        self.validate()?;
        let n = usize::try_from(frames).map_err(|_| "Packet frame count overflow")?
            .checked_mul(usize::from(self.align)).ok_or("Packet byte size overflow")?;
        if n > 4 * 1024 * 1024 {
            return Err("Packet exceeds 4 MiB budget".into());
        }
        Ok(n)
    }

    pub fn decode(self, bytes: &[u8]) -> Result<Vec<f32>> {
        self.validate()?;
        if bytes.len() % usize::from(self.align) != 0 {
            return Err("Packet is not frame aligned".into());
        }
        let mut out = Vec::with_capacity(bytes.len() / (self.bits as usize / 8));
        for b in bytes.chunks_exact(self.bits as usize / 8) {
            let v = match (self.encoding, self.bits) {
                (Encoding::Pcm, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
                (Encoding::Pcm, 24) => ((i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8) >> 8)
                    as f32 / 8388608.0,
                (Encoding::Pcm, 32) => i32::from_le_bytes([b[0], b[1], b[2], b[3]])
                    as f32 / 2147483648.0,
                (Encoding::Float, 32) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                _ => return Err("Unsupported encoding".into()),
            };
            if !v.is_finite() {
                return Err("Non-finite input sample".into());
            }
            out.push(v);
        }
        Ok(out)
    }
}
