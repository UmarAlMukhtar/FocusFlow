//! One bounded writer for both streams; files are never input authority until the final clock join.
use super::*;
use focusflow_core::{
    cursor::{CursorShape, MAX_NATIVE_BYTES, MAX_SHAPES},
    cursor_observation::*,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::recorder::windows_capture) struct ResourceSpool {
    pub schema_version: u32,
    pub shapes: Vec<CursorShape>,
}
pub(in crate::recorder::windows_capture) fn decode_resources(
    bytes: &[u8],
) -> Result<Vec<CursorShape>, String> {
    if bytes.len() > focusflow_core::cursor::MAX_INDEX_BYTES as usize {
        return Err("Resource spool exceeds cap".into());
    }
    let shape: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if !shape.is_object()
        || !shape
            .get("shapes")
            .and_then(|s| s.as_array())
            .is_some_and(|s| s.len() <= MAX_SHAPES && s.iter().all(|s| s.is_object()))
    {
        return Err("Resource spool requires bounded named objects".into());
    }
    let spool: ResourceSpool = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if spool.schema_version != 1 {
        return Err("Unknown resource spool schema".into());
    }
    Ok(spool.shapes)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::recorder::windows_capture) struct StreamReceipt {
    pub bytes: u64,
    pub records: u32,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::recorder::windows_capture) struct CursorObservationCompletion {
    pub profile: ModelProfile,
    pub pose: StreamReceipt,
    pub input: StreamReceipt,
    pub combined_bytes: u64,
    pub combined_records: u32,
    pub resources_bytes: u64,
    pub resources_sha256: String,
    pub resources_count: u32,
    pub terminal: ObservationTerminal,
    pub pose_joined: bool,
    pub input_joined: bool,
    pub files_usable: bool,
    pub degraded: bool,
    pub pose_stats: Cadence,
    pub input_stats: Cadence,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::recorder::windows_capture) struct Cadence {
    pub samples: u32,
    pub max_gap_qpc: i64,
    pub max_bracket_qpc: i64,
    #[serde(deserialize_with = "required")]
    pub first_qpc: Option<i64>,
    #[serde(deserialize_with = "required")]
    pub last_qpc: Option<i64>,
    pub overflow: bool,
}
impl Cadence {
    pub fn observe(&mut self, q: [i64; 2]) {
        if let Some(last) = self.last_qpc {
            self.max_gap_qpc = self.max_gap_qpc.max(q[0] - last);
        }
        self.first_qpc.get_or_insert(q[0]);
        self.last_qpc = Some(q[1]);
        self.max_bracket_qpc = self.max_bracket_qpc.max(q[1] - q[0]);
        self.samples = self.samples.saturating_add(1);
    }
}
impl CursorObservationCompletion {
    pub fn validate(&self, profile: ModelProfile, cutoff: i64) -> Result<(), String> {
        if !self.pose_joined
            || !self.input_joined
            || self.profile != profile
            || self.terminal.source_cutoff_qpc != cutoff
            || self.pose.bytes.checked_add(self.input.bytes) != Some(self.combined_bytes)
            || self.pose.records.checked_add(self.input.records) != Some(self.combined_records)
            || self.combined_bytes > focusflow_core::cursor::MAX_SAMPLES_BYTES
            || self.combined_records as usize > focusflow_core::cursor::MAX_SAMPLES
            || self.resources_bytes > focusflow_core::cursor::MAX_INDEX_BYTES
            || self.resources_count as usize > MAX_SHAPES
            || (!self.files_usable && !self.degraded)
        {
            return Err("Cursor terminal ownership, mode or accounting mismatch".into());
        }
        for (stats, records) in [
            (&self.pose_stats, self.pose.records),
            (&self.input_stats, self.input.records),
        ] {
            if stats.samples < records
                || stats.max_gap_qpc < 0
                || stats.max_bracket_qpc < 0
                || match (stats.first_qpc, stats.last_qpc) {
                    (None, None) => stats.samples != 0,
                    (Some(a), Some(b)) => stats.samples == 0 || a < 0 || b < a,
                    _ => true,
                }
            {
                return Err("Invalid sampler cadence accounting".into());
            }
        }
        for hash in [
            &self.pose.sha256,
            &self.input.sha256,
            &self.resources_sha256,
        ] {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("Invalid cursor SHA-256 receipt".into());
            }
        }
        for b in [&self.terminal.pose_barrier, &self.terminal.input_barrier]
            .into_iter()
            .flatten()
        {
            let a = qpc(&b.q0).map_err(|e| e.to_string())?;
            let z = qpc(&b.q1).map_err(|e| e.to_string())?;
            if a < 0 || z < a || b.reason.is_empty() || b.reason.len() > 256 {
                return Err("Invalid cursor terminal bracket".into());
            }
        }
        if self.terminal.stop_kind == StopKind::SamplerFailure && !self.degraded {
            return Err("Sampler failure cannot claim undegraded telemetry".into());
        }
        Ok(())
    }
}
struct Stream {
    file: File,
    receipt: StreamReceipt,
    hash: Sha256,
}
impl Stream {
    fn create(path: &Path) -> Result<Self, String> {
        Ok(Self {
            file: OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|e| e.to_string())?,
            receipt: StreamReceipt {
                bytes: 0,
                records: 0,
                sha256: format!("{:x}", Sha256::new().finalize()),
            },
            hash: Sha256::new(),
        })
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let result = self.file.write_all(bytes);
        if let Err(e) = result {
            let _ = self.file.set_len(self.receipt.bytes);
            let _ = self.file.seek(SeekFrom::Start(self.receipt.bytes));
            return Err(e.to_string());
        }
        self.hash.update(bytes);
        self.receipt.sha256 = format!("{:x}", self.hash.clone().finalize());
        self.receipt.bytes += bytes.len() as u64;
        self.receipt.records += 1;
        Ok(())
    }
}
pub(in crate::recorder::windows_capture) struct Streams {
    dir: PathBuf,
    pose: Stream,
    input: Stream,
    budget: ObservationBudget,
    shapes: Vec<image::Shape>,
    metadata: Vec<CursorShape>,
    decoded: usize,
}
impl Streams {
    pub fn create(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir(dir.join("cursor")).map_err(|e| e.to_string())?;
        std::fs::create_dir(dir.join("cursor/shapes")).map_err(|e| e.to_string())?;
        Ok(Self {
            dir: dir.to_owned(),
            pose: Stream::create(&dir.join(OBSERVATIONS_PATH))?,
            input: Stream::create(&dir.join(INPUT_OBSERVATIONS_PATH))?,
            budget: ObservationBudget::default(),
            shapes: vec![],
            metadata: vec![],
            decoded: 0,
        })
    }
    pub fn pose(&mut self, row: &WindowsCursorRowV2) -> Result<u32, String> {
        let index = self.pose.receipt.records;
        self.write(row, true)?;
        Ok(index)
    }
    pub fn input(&mut self, row: &WindowsInputRowV1) -> Result<(), String> {
        self.write(row, false)
    }
    fn write(&mut self, row: &impl Serialize, pose: bool) -> Result<(), String> {
        let mut b = serde_json::to_vec(row).map_err(|e| e.to_string())?;
        b.push(b'\n');
        let cap = if pose {
            MAX_POSE_RECORD_BYTES
        } else {
            MAX_INPUT_RECORD_BYTES
        };
        let retained = if pose {
            std::mem::size_of::<WindowsCursorRowV2>()
        } else {
            std::mem::size_of::<WindowsInputRowV1>()
        } + 2 * cap;
        if b.len() > cap || !self.budget.admit(b.len(), retained) {
            return Err("Cursor telemetry budget exhausted".into());
        }
        if pose {
            self.pose.write(&b)
        } else {
            self.input.write(&b)
        }
    }
    pub fn shape(&mut self, shape: image::Shape) -> Result<String, String> {
        if let Some(i) = self.shapes.iter().position(|s| s == &shape) {
            return Ok(self.metadata[i].id.clone());
        }
        if shape.width == 0
            || shape.height == 0
            || shape.width > 512
            || shape.height > 512
            || shape.hotspot[0] >= shape.width
            || shape.hotspot[1] >= shape.height
            || shape.rgba.len() != shape.width as usize * shape.height as usize * 4
            || self.shapes.len() >= MAX_SHAPES
            || shape.rgba.len() > MAX_NATIVE_BYTES.saturating_sub(self.decoded)
        {
            return Err("Cursor image budget or geometry invalid".into());
        }
        let id = format!("shape-{}", self.shapes.len());
        let path = format!("cursor/shapes/{id}.pam");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.dir.join(&path))
            .map_err(|e| e.to_string())?;
        write!(
            file,
            "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
            shape.width, shape.height
        )
        .and_then(|_| file.write_all(&shape.rgba))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
        self.metadata.push(CursorShape {
            id: id.clone(),
            path,
            width: shape.width,
            height: shape.height,
            hotspot_x: shape.hotspot[0],
            hotspot_y: shape.hotspot[1],
        });
        self.decoded += shape.rgba.len();
        self.shapes.push(shape);
        Ok(id)
    }
    pub fn finish(
        self,
        profile: ModelProfile,
        mut terminal: ObservationTerminal,
        pose_stats: Cadence,
        input_stats: Cadence,
        mut degraded: bool,
    ) -> Result<CursorObservationCompletion, String> {
        let spool = ResourceSpool {
            schema_version: 1,
            shapes: self.metadata,
        };
        let bytes = serde_json::to_vec(&spool).map_err(|e| e.to_string())?;
        let saved = (|| -> Result<(), String> {
            if bytes.len() > focusflow_core::cursor::MAX_INDEX_BYTES as usize {
                return Err("Cursor resource metadata exceeds cap".into());
            }
            let mut resources = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.dir.join("cursor/resources.json"))
                .map_err(|e| e.to_string())?;
            resources
                .write_all(&bytes)
                .and_then(|_| resources.sync_all())
                .map_err(|e| e.to_string())?;
            self.pose
                .file
                .sync_all()
                .and_then(|_| self.input.file.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if saved.is_err() {
            degraded = true;
            terminal.stop_kind = StopKind::SamplerFailure;
        }
        Ok(CursorObservationCompletion {
            profile,
            combined_bytes: self.pose.receipt.bytes + self.input.receipt.bytes,
            combined_records: self.pose.receipt.records + self.input.receipt.records,
            pose: self.pose.receipt,
            input: self.input.receipt,
            resources_bytes: bytes.len() as u64,
            resources_sha256: format!("{:x}", Sha256::digest(&bytes)),
            resources_count: spool.shapes.len() as u32,
            terminal,
            pose_joined: false,
            input_joined: false,
            files_usable: saved.is_ok(),
            degraded,
            pose_stats,
            input_stats,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn receipt() -> CursorObservationCompletion {
        CursorObservationCompletion {
            profile: ModelProfile::Popup,
            pose: StreamReceipt {
                bytes: 7,
                records: 1,
                sha256: "0".repeat(64),
            },
            input: StreamReceipt {
                bytes: 8,
                records: 1,
                sha256: "0".repeat(64),
            },
            combined_bytes: 15,
            combined_records: 2,
            resources_bytes: 2,
            resources_sha256: "0".repeat(64),
            resources_count: 0,
            terminal: ObservationTerminal {
                pose_barrier: None,
                input_barrier: None,
                source_cutoff_qpc: 20,
                stop_kind: StopKind::Stop,
            },
            pose_joined: true,
            input_joined: true,
            files_usable: true,
            degraded: false,
            pose_stats: Cadence {
                samples: 1,
                first_qpc: Some(10),
                last_qpc: Some(11),
                ..Default::default()
            },
            input_stats: Cadence {
                samples: 1,
                first_qpc: Some(12),
                last_qpc: Some(13),
                ..Default::default()
            },
        }
    }
    #[test]
    fn terminal_requires_two_joined_owners_exact_accounting_mode_and_cutoff() {
        let good = receipt();
        assert!(good.validate(ModelProfile::Popup, 20).is_ok());
        for i in 0..7 {
            let mut r = good.clone();
            match i {
                0 => r.pose_joined = false,
                1 => r.input_joined = false,
                2 => r.combined_bytes += 1,
                3 => r.combined_records += 1,
                4 => r.profile = ModelProfile::Decorated,
                5 => r.terminal.source_cutoff_qpc += 1,
                _ => r.resources_count = 257,
            };
            assert!(r.validate(ModelProfile::Popup, 20).is_err(), "case {i}");
        }
    }
    #[test]
    fn resource_identity_uses_fresh_bytes_and_hotspot_and_publishes_owned_pam() {
        let dir =
            std::env::temp_dir().join(format!("focusflow-cursor-storage-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut streams = Streams::create(&dir).unwrap();
        let s = image::Shape {
            width: 2,
            height: 1,
            hotspot: [0, 0],
            rgba: vec![255, 0, 0, 255, 0, 0, 0, 0],
        };
        let a = streams.shape(s.clone()).unwrap();
        assert_eq!(a, streams.shape(s.clone()).unwrap());
        let mut b = s.clone();
        b.hotspot = [1, 0];
        assert_ne!(a, streams.shape(b).unwrap());
        let mut c = s;
        c.rgba[0] = 0;
        assert_ne!(a, streams.shape(c).unwrap());
        assert_eq!(streams.metadata.len(), 3);
        let bytes = std::fs::read(dir.join("cursor/shapes/shape-0.pam")).unwrap();
        assert!(bytes.ends_with(&[255, 0, 0, 255, 0, 0, 0, 0]));
        drop(streams);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_resource_spool_save_is_explicit_absence_after_telemetry_stop() {
        let dir = std::env::temp_dir().join(format!(
            "focusflow-cursor-spool-fail-{}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let streams = Streams::create(&dir).unwrap();
        std::fs::create_dir(dir.join("cursor/resources.json")).unwrap();
        let done = streams
            .finish(
                ModelProfile::Popup,
                ObservationTerminal {
                    pose_barrier: None,
                    input_barrier: None,
                    source_cutoff_qpc: 20,
                    stop_kind: StopKind::Stop,
                },
                Cadence::default(),
                Cadence::default(),
                false,
            )
            .unwrap();
        assert!(!done.files_usable);
        assert!(done.degraded);
        assert_eq!(done.terminal.stop_kind, StopKind::SamplerFailure);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn combined_cap_closes_both_data_lanes_and_reserves_terminal_accounting() {
        let dir = std::env::temp_dir().join(format!("focusflow-cursor-cap-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut s = Streams::create(&dir).unwrap();
        s.budget = ObservationBudget::with_limits(5000, 3, 20 * 1024 * 1024);
        let row = WindowsCursorRowV2::Barrier {
            q0: "10".into(),
            q1: "11".into(),
            reason: "query failed".into(),
        };
        assert_eq!(s.pose(&row).unwrap(), 0);
        assert!(s
            .input(&WindowsInputRowV1::Barrier {
                q0: "12".into(),
                q1: "13".into(),
                reason: "query failed".into()
            })
            .is_err());
        assert!(s.pose(&row).is_err());
        assert_eq!(s.pose.receipt.records, 1);
        assert_eq!(s.input.receipt.records, 0);
        assert!(s.budget.admit_terminal(100));
        assert!(s.budget.admit_terminal(100));
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
