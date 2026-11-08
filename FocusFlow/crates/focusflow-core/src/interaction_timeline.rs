//! Legacy recorder timeline policy, shared without changing its compatibility format.
use serde::{Deserialize, Serialize};
use std::time::Duration;
pub const DRAG_POINT_INTERVAL: Duration = Duration::from_millis(50);
pub const DRAG_MIN_DISTANCE_PX: i32 = 4;
pub const DRAG_POINT_MIN_DISTANCE_PX: i32 = 2;
pub const DRAG_FORCE_SAMPLE_DISTANCE_PX: i32 = 12;
const PRE_CLICK_ZOOM_SECONDS: f64 = 0.3;
const IDLE_BEFORE_ZOOM_OUT_SECONDS: f64 = 1.0;
const ZOOM_OUT_SECONDS: f64 = 0.35;
const MIN_TIMELINE_SEGMENT_SECONDS: f64 = 0.001;
/// Two adjacent camera sequences are merged into one continuous sequence when
/// the gap between them is shorter than this value.  This avoids a zoom-out
/// followed immediately by a zoom-in when the user clicks or drags in quick
/// succession.
const TIMELINE_MERGE_WINDOW_SECONDS: f64 = 0.3;
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ClickEvent {
    pub timestamp: f64,
    pub x: i32,
    pub y: i32,
    pub button: ClickButton,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DragInteraction {
    pub start: f64,
    pub end: f64,
    pub button: ClickButton,
    pub points: Vec<DragPoint>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DragPoint {
    pub timestamp: f64,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClickButton {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TimelineSegment {
    pub start: f64,
    pub end: f64,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct InteractionEvents {
    pub clicks: Vec<ClickEvent>,
    pub drags: Vec<DragInteraction>,
}
pub fn build_timeline_segments(
    clicks: Vec<ClickEvent>,
    drags: Vec<DragInteraction>,
) -> Vec<TimelineSegment> {
    let mut timeline = build_click_timeline_segments(clicks);
    timeline.extend(build_drag_timeline_segments(drags));
    cleanup_timeline_segments(timeline)
}

/// Converts a raw, potentially overlapping list of [`TimelineSegment`]s
/// (produced by independently merging click and drag segments) into a clean,
/// non-overlapping, sorted list suitable for the export pipeline.
///
/// Steps (all in a single pass after sorting):
///
/// 1. **Sort** by `start` ascending.
/// 2. **Deduplicate**: when two segments share the same `start` time, keep
///    the one with the later `end` (wider coverage).
/// 3. **Merge nearby sequences**: if the gap between the end of the current
///    accumulator segment and the start of the next segment is within
///    [`TIMELINE_MERGE_WINDOW_SECONDS`], extend the accumulator instead of
///    starting a new zoom-out/zoom-in cycle.  The new segment is appended to
///    the accumulator's chain so the camera pans smoothly.
/// 4. **Clip overlaps**: if a following segment still starts before the
///    previous segment ends, trim the previous segment's `end` to the
///    following segment's `start`.
/// 5. **Drop invalid segments**: remove any segment where `end <= start`.
fn cleanup_timeline_segments(mut raw: Vec<TimelineSegment>) -> Vec<TimelineSegment> {
    if raw.is_empty() {
        return raw;
    }

    // Step 1 — sort by start time, break ties by preferring later end (wider).
    raw.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.end
                    .partial_cmp(&a.end)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Step 2 — deduplicate same-start segments (keep the one with the later end).
    raw.dedup_by(|later, earlier| {
        if (later.start - earlier.start).abs() <= MIN_TIMELINE_SEGMENT_SECONDS {
            // `dedup_by` removes `later` when returning true.  We want to keep
            // the one with the later `end`, so copy its fields into `earlier`
            // (which survives) if `later` is wider.
            if later.end > earlier.end {
                earlier.end = later.end;
                earlier.x = later.x;
                earlier.y = later.y;
            }
            true
        } else {
            false
        }
    });

    // Steps 3 & 4 — single forward pass: merge nearby sequences, clip overlaps.
    let mut out: Vec<TimelineSegment> = Vec::with_capacity(raw.len());

    for seg in raw {
        if let Some(prev) = out.last_mut() {
            let gap = seg.start - prev.end;

            if gap <= TIMELINE_MERGE_WINDOW_SECONDS {
                // The two sequences are close enough to merge: extend `prev`
                // to cover `seg` without zooming out in between.  We update
                // the camera target to `seg`'s position and widen the window.
                // (The export pipeline's `timeline_sequences` groups contiguous
                // segments into one sequence, so no artificial boundary is
                // needed here.)
                if seg.end > prev.end {
                    // Push a new segment starting where prev ends (or at
                    // seg.start if that is later) so the export pipeline sees
                    // a smooth pan rather than a jump.
                    let pan_start = prev.end.max(seg.start);
                    if pan_start < seg.end - MIN_TIMELINE_SEGMENT_SECONDS {
                        out.push(TimelineSegment {
                            start: pan_start,
                            end: seg.end,
                            x: seg.x,
                            y: seg.y,
                            scale: seg.scale,
                        });
                    } else {
                        // The window is too narrow to add a segment; just
                        // extend the previous one.
                        prev.end = seg.end;
                        prev.x = seg.x;
                        prev.y = seg.y;
                    }
                }
                // If seg is entirely contained within prev, discard it.
                continue;
            }

            // Gap is wide enough for a separate sequence.  Before appending,
            // clip any leftover overlap (shouldn't happen after dedup but
            // defends against floating-point edge cases).
            if seg.start < prev.end {
                prev.end = seg.start;
            }
        }

        // Only push segments that still have positive duration.
        if seg.end > seg.start + MIN_TIMELINE_SEGMENT_SECONDS {
            out.push(seg);
        }
    }

    // Step 5 — final pass: drop any zero-width or inverted segments that
    // slipped through (defensive).
    out.retain(|seg| seg.end > seg.start + MIN_TIMELINE_SEGMENT_SECONDS);

    out
}

fn build_click_timeline_segments(mut clicks: Vec<ClickEvent>) -> Vec<TimelineSegment> {
    clicks.sort_by(|left, right| {
        left.timestamp
            .partial_cmp(&right.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut timeline = Vec::new();
    let mut sequence_complete_at = 0.0;

    for click in clicks
        .into_iter()
        .filter(|click| click.button == ClickButton::Left)
    {
        let segment_end = click.timestamp + IDLE_BEFORE_ZOOM_OUT_SECONDS;

        if timeline.is_empty() {
            timeline.push(TimelineSegment {
                start: (click.timestamp - PRE_CLICK_ZOOM_SECONDS).max(0.0),
                end: segment_end,
                x: click.x,
                y: click.y,
                scale: 2.0,
            });
            sequence_complete_at = segment_end + ZOOM_OUT_SECONDS;
            continue;
        }

        if click.timestamp <= sequence_complete_at {
            if let Some(last_segment) = timeline.last_mut() {
                if click.timestamp > last_segment.start + MIN_TIMELINE_SEGMENT_SECONDS {
                    last_segment.end = click.timestamp;
                    timeline.push(TimelineSegment {
                        start: click.timestamp,
                        end: segment_end,
                        x: click.x,
                        y: click.y,
                        scale: 2.0,
                    });
                } else {
                    last_segment.end = segment_end;
                    last_segment.x = click.x;
                    last_segment.y = click.y;
                }
            }
        } else {
            let segment_start = (click.timestamp - PRE_CLICK_ZOOM_SECONDS)
                .max(0.0)
                .max(sequence_complete_at + MIN_TIMELINE_SEGMENT_SECONDS)
                .min(click.timestamp);

            timeline.push(TimelineSegment {
                start: segment_start,
                end: segment_end,
                x: click.x,
                y: click.y,
                scale: 2.0,
            });
        }

        sequence_complete_at = segment_end + ZOOM_OUT_SECONDS;
    }

    timeline
}

fn build_drag_timeline_segments(mut drags: Vec<DragInteraction>) -> Vec<TimelineSegment> {
    drags.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut timeline = Vec::new();

    for drag in drags
        .into_iter()
        .filter(|drag| drag.button == ClickButton::Left)
    {
        let points = normalized_drag_points(drag.points);

        if points.len() < 2 {
            continue;
        }

        let drag_start = drag.start.min(points[0].timestamp);
        let drag_end = drag.end.max(
            points
                .last()
                .map(|point| point.timestamp)
                .unwrap_or(drag.end),
        );
        let sequence_start = (drag_start - PRE_CLICK_ZOOM_SECONDS).max(0.0);
        let sequence_end = drag_end + IDLE_BEFORE_ZOOM_OUT_SECONDS;
        let mut previous_segment_start = sequence_start;

        for index in 0..points.len() {
            let point = &points[index];
            let segment_start = if index == 0 {
                sequence_start
            } else {
                point
                    .timestamp
                    .max(previous_segment_start + MIN_TIMELINE_SEGMENT_SECONDS)
            };
            let segment_end = points
                .get(index + 1)
                .map(|next_point| next_point.timestamp)
                .unwrap_or(sequence_end)
                .max(segment_start + MIN_TIMELINE_SEGMENT_SECONDS);

            timeline.push(TimelineSegment {
                start: segment_start,
                end: segment_end,
                x: point.x,
                y: point.y,
                scale: 2.0,
            });
            previous_segment_start = segment_start;
        }
    }

    timeline
}

fn normalized_drag_points(mut points: Vec<DragPoint>) -> Vec<DragPoint> {
    points.retain(|point| point.timestamp.is_finite() && point.timestamp >= 0.0);
    points.sort_by(|left, right| {
        left.timestamp
            .partial_cmp(&right.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    points.dedup_by(|right, left| {
        right.timestamp <= left.timestamp + f64::EPSILON && right.x == left.x && right.y == left.y
    });

    points
}
