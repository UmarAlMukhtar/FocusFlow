use serde::{Deserialize, Serialize};
use crate::{project::{self, invalid, require, valid_id, Clip, ProjectDocument, ZoomElement}, CoreError, CoreResult, ErrorCode};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum Edit {
    SetCursorSettings { settings: project::CursorSettings },
    SetFrameStyle { style: project::FrameStyle },
    TrimClip { clip_id: String, in_us: u64, out_us: u64 },
    SplitClip { clip_id: String, at_us: u64, right_clip_id: String },
    RemoveClip { clip_id: String },
    PutZoom { #[serde(deserialize_with="project::deserialize_edit_zoom")] zoom: ZoomElement },
    RemoveZoom { zoom_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MappedClip {
    pub clip_id: String,
    pub source_start_us: u64,
    pub source_end_us: u64,
    pub output_start_us: u64,
}

/// Apply an ordered batch without changing the source document or its revision.
pub fn apply(doc: &ProjectDocument, edits: &[Edit]) -> CoreResult<ProjectDocument> {
    require((1..=256).contains(&edits.len()), "edits", "Expected between 1 and 256 edits")?;
    project::validate(doc)?;
    let mut candidate = doc.clone();
    candidate.schema_version = 4;
    for edit in edits {
        match edit {
            Edit::SetCursorSettings { settings } => {
                project::validate_cursor_settings(settings)?;
                candidate.cursor_settings = settings.clone();
                project::validate(&candidate)?;
            }
            Edit::SetFrameStyle { style } => {
                crate::frame::geometry(style, candidate.video.width, candidate.video.height)?;
                candidate.frame_style = style.clone();
            }
            Edit::TrimClip { clip_id, in_us, out_us } => {
                let index = clip_index(&candidate, clip_id)?;
                let clip = &mut candidate.clips[index];
                require(
                    in_us < out_us && *out_us <= candidate.video.duration_us,
                    "inUs",
                    "Trim must retain a nonempty interval inside the source video",
                )?;
                clip.in_us = *in_us;
                clip.out_us = *out_us;
                candidate.zooms.retain_mut(|zoom| {
                    if zoom.clip_id != *clip_id { return true; }
                    zoom.start_us = zoom.start_us.max(*in_us);
                    zoom.end_us = zoom.end_us.min(*out_us);
                    zoom.start_us < zoom.end_us
                });
            }
            Edit::SplitClip { clip_id, at_us, right_clip_id } => {
                let index = clip_index(&candidate, clip_id)?;
                let clip = &candidate.clips[index];
                require(clip.in_us < *at_us && *at_us < clip.out_us, "atUs", "Split must be strictly inside the clip")?;
                require(valid_id(right_clip_id), "rightClipId", "Invalid right clip ID")?;
                require(!candidate.clips.iter().any(|clip| clip.id == *right_clip_id), "rightClipId", "Right clip ID already exists")?;
                require(candidate.clips.len() < 4096, "clips", "At most 4096 clips are supported")?;
                let right = Clip { id: right_clip_id.clone(), in_us: *at_us, out_us: clip.out_us };
                let mut zooms = Vec::with_capacity(candidate.zooms.len());
                for zoom in &candidate.zooms {
                    if zoom.clip_id != *clip_id || zoom.end_us <= *at_us {
                        zooms.push(zoom.clone());
                    } else if zoom.start_us >= *at_us {
                        let mut right_zoom = zoom.clone();
                        right_zoom.clip_id = right_clip_id.clone();
                        zooms.push(right_zoom);
                    } else {
                        let right_id = format!("{}__{}", zoom.id, right_clip_id);
                        require(valid_id(&right_id), "rightClipId", "Derived zoom ID exceeds the 128-byte limit")?;
                        require(!candidate.zooms.iter().any(|other| other.id == right_id), "rightClipId", "Derived zoom ID already exists")?;
                        let mut left_zoom = zoom.clone();
                        left_zoom.end_us = *at_us;
                        zooms.push(left_zoom);
                        let mut right_zoom = zoom.clone();
                        right_zoom.id = right_id;
                        right_zoom.clip_id = right_clip_id.clone();
                        right_zoom.start_us = *at_us;
                        zooms.push(right_zoom);
                    }
                }
                require(zooms.len() <= 16384, "zooms", "At most 16384 zooms are supported")?;
                candidate.clips[index].out_us = *at_us;
                candidate.clips.insert(index + 1, right);
                candidate.zooms = zooms;
            }
            Edit::RemoveClip { clip_id } => {
                let index = clip_index(&candidate, clip_id)?;
                require(candidate.clips.len() > 1, "clips", "Cannot remove the last retained clip")?;
                candidate.clips.remove(index);
                candidate.zooms.retain(|zoom| zoom.clip_id != *clip_id);
            }
            Edit::PutZoom { zoom } => {
                project::validate_zoom(zoom, &candidate, "zoom")?;
                if let Some(existing) = candidate.zooms.iter_mut().find(|other| other.id == zoom.id) {
                    *existing = zoom.clone();
                } else {
                    require(candidate.zooms.len() < 16384, "zooms", "At most 16384 zooms are supported")?;
                    candidate.zooms.push(zoom.clone());
                }
            }
            Edit::RemoveZoom { zoom_id } => {
                let index = candidate.zooms.iter().position(|zoom| zoom.id == *zoom_id)
                    .ok_or_else(|| not_found("zoomId", "Zoom ID does not exist"))?;
                candidate.zooms.remove(index);
            }
        }
    }
    project::validate(&candidate)?;
    Ok(candidate)
}

/// Playback-ordered half-open source intervals; events at source_end_us are excluded.
pub fn map_clips(doc: &ProjectDocument) -> CoreResult<Vec<MappedClip>> {
    project::validate(doc)?;
    let mut output_start_us = 0_u64;
    doc.clips.iter().map(|clip| {
        let mapped = MappedClip {
            clip_id: clip.id.clone(), source_start_us: clip.in_us,
            source_end_us: clip.out_us, output_start_us,
        };
        output_start_us = output_start_us.checked_add(clip.out_us - clip.in_us)
            .ok_or_else(|| invalid("clips", "Output duration overflow"))?;
        Ok(mapped)
    }).collect()
}

fn clip_index(doc: &ProjectDocument, id: &str) -> CoreResult<usize> {
    doc.clips.iter().position(|clip| clip.id == id)
        .ok_or_else(|| not_found("clipId", "Clip ID does not exist"))
}
fn not_found(field: &str, message: &str) -> CoreError {
    CoreError { code: ErrorCode::NotFound, message: message.into(), field: Some(field.into()) }
}
