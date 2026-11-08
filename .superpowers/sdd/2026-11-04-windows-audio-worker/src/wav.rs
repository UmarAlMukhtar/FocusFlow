use crate::{format::{Encoding, Format}, packet::Packet, protocol::WavInventory, Result};
use std::{fs::{self, File, OpenOptions}, io::{Seek, Write}, path::{Path, PathBuf}};

/// Only filesystem operations vary in failure tests; header/sample writing stays in hound.
pub trait FinalizeOps {
    /// Test seam gates the actual sink; production never delays the unchanged sample writer.
    fn before_append(&mut self) {}
    fn sync(&mut self, file: &File) -> Result<()> { file.sync_all().map_err(|e| e.to_string()) }
    fn full_validation(&mut self, path: &Path) -> bool { focusflow_core::audio::is_usable_wav(path) }
}
struct FileOps;
impl FinalizeOps for FileOps {}

pub struct PartialWav<W: Write + Seek = File> {
    path: PathBuf,
    writer: hound::WavWriter<W>,
    same_file: File,
    format: Format,
    frames: u64,
    poisoned: bool,
}

/// Reject links/reparse points in every existing path component before creating a new file.
pub(crate) fn reject_links(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() { path.to_path_buf() }
        else { std::env::current_dir().map_err(|e| e.to_string())?.join(path) };
    for part in absolute.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(m) => {
                #[cfg(windows)]
                let link = { use std::os::windows::fs::MetadataExt; m.file_attributes() & 0x400 != 0 };
                #[cfg(not(windows))]
                let link = m.file_type().is_symlink();
                if link { return Err("Audio output path contains a symlink/reparse point".into()); }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

pub(crate) fn create_new(path: &Path) -> Result<File> {
    reject_links(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    // Keep the exact new file pinned against replacement/writes while hound and validation read it.
    #[cfg(windows)]
    { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
    options.open(path).map_err(|e| e.to_string())
}

impl PartialWav<File> {
    pub fn create(path: &Path, format: Format) -> Result<Self> { Self::create_with(path, format, |f| f) }
    pub(crate) fn from_created_file(path: &Path, format: Format, file: File) -> Result<Self> {
        Self::from_created(path, format, file, |f|f)
    }
}
impl<W: Write + Seek> PartialWav<W> {
    pub fn create_with(path: &Path, format: Format, wrap: impl FnOnce(File) -> W) -> Result<Self> {
        format.validate()?;
        let file = create_new(path)?;
        Self::from_created(path, format, file, wrap)
    }
    fn from_created(path: &Path, format: Format, file: File, wrap: impl FnOnce(File) -> W) -> Result<Self> {
        format.validate()?;
        let same_file = file.try_clone().map_err(|e| e.to_string())?;
        let spec = hound::WavSpec {
            channels: format.channels, sample_rate: format.rate, bits_per_sample: format.bits,
            sample_format: match format.encoding { Encoding::Pcm => hound::SampleFormat::Int,
                Encoding::Float => hound::SampleFormat::Float },
        };
        let writer = hound::WavWriter::new(wrap(file), spec).map_err(|e| e.to_string())?;
        Ok(Self { path: path.into(), writer, same_file, format, frames: 0, poisoned: false })
    }

    pub fn written_complete_frames(&self) -> u64 { self.frames }

    pub fn append(&mut self, packet: &Packet) -> Result<()> {
        if self.poisoned { return Err("WAV writer poisoned; partial retained".into()); }
        // A rejected packet cannot leave a caller free to finalize an incomplete accepted prefix.
        if let Err(e) = packet.validate(self.format, self.frames) { self.poisoned = true; return Err(e); }
        for b in packet.bytes.chunks_exact(self.format.bits as usize / 8) {
            let write = match (self.format.encoding, self.format.bits) {
                (Encoding::Pcm, 16) => self.writer.write_sample(i16::from_le_bytes([b[0], b[1]])),
                (Encoding::Pcm, 24) => self.writer.write_sample((i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8) >> 8),
                (Encoding::Pcm, 32) => self.writer.write_sample(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                (Encoding::Float, 32) => self.writer.write_sample(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                _ => { self.poisoned = true; return Err("Unsupported WAV encoding".into()); }
            };
            if let Err(e) = write { self.poisoned = true; return Err(e.to_string()); }
        }
        self.frames += u64::from(packet.frames);
        Ok(())
    }

    pub fn finish_inventory(self) -> Result<WavInventory> { self.finish_inventory_with(&mut FileOps) }

    pub fn finish_inventory_with(self, ops: &mut impl FinalizeOps) -> Result<WavInventory> {
        if self.poisoned { return Err("WAV writer poisoned; no finalization/publication; partial retained".into()); }
        self.writer.finalize().map_err(|e| format!("WAV finalize failed; partial retained: {e}"))?;
        // This retained clone refers to the same open file, never a replacement selected by path.
        ops.sync(&self.same_file).map_err(|e| format!("WAV sync failed; partial retained: {e}"))?;
        Self::check_same_file(&self.path, &self.same_file)?;
        if !ops.full_validation(&self.path) {
            return Err("Finalized WAV failed complete sample validation; partial retained".into());
        }
        let r = hound::WavReader::open(&self.path).map_err(|e| e.to_string())?;
        let spec = r.spec();
        let encoding = match spec.sample_format { hound::SampleFormat::Int => Encoding::Pcm,
            hound::SampleFormat::Float => Encoding::Float };
        if u64::from(r.duration()) != self.frames || self.frames == 0
            || r.len() % u32::from(self.format.channels) != 0
            || spec.channels != self.format.channels || spec.sample_rate != self.format.rate
            || spec.bits_per_sample != self.format.bits || encoding != self.format.encoding {
            return Err("Finalized WAV inventory mismatch; partial retained".into());
        }
        Self::check_same_file(&self.path, &self.same_file)?;
        Ok(WavInventory { frames: self.frames, rate: spec.sample_rate, channels: spec.channels,
            bits: spec.bits_per_sample, encoding, file_bytes: self.same_file.metadata().map_err(|e| e.to_string())?.len() })
    }

    fn check_same_file(path: &Path, same_file: &File) -> Result<()> {
        reject_links(path)?;
        let held = same_file.metadata().map_err(|e| e.to_string())?;
        let current = fs::metadata(path).map_err(|e| e.to_string())?;
        if !held.is_file() || !current.is_file() || held.len() != current.len() {
            return Err("WAV path no longer matches owned file".into());
        }
        #[cfg(unix)]
        { use std::os::unix::fs::MetadataExt;
          if held.dev() != current.dev() || held.ino() != current.ino() {
              return Err("WAV path replaced; owned partial retained".into());
          }
        }
        Ok(())
    }

    /// Compatibility for the accepted copied proof's callers; all checks use the same path.
    pub fn finish(self) -> Result<PathBuf> {
        let path = self.path.clone(); self.finish_inventory()?; Ok(path)
    }
}
