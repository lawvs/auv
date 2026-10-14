use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::curve::SliderCurve;
use crate::model::{BeatmapModel, HitObject};

// Amended constants per Amendment package v1
pub const ARRIVAL_LEAD_MS: f64 = 12.0;
pub const POST_HIT_DWELL_MS: f64 = 8.0;
pub const STREAM_ALT_THRESHOLD_MS: f64 = 125.0;
pub const SLIDER_HOLD_TAIL_PAD_MS: f64 = 10.0;
pub const SPINNER_RADIUS_OSUPX: f64 = 50.0;
pub const SPINNER_RPS: f64 = 7.5;
pub const STRAIGHT_LINE_THRESHOLD_MS: f64 = 80.0;
pub const TRAJECTORY_SAMPLE_INTERVAL_MS: f64 = 4.0; // 250 Hz

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCode {
  K1,
  K2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyEventType {
  Down,
  Up,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledKeyEvent {
  pub time_ms: f64,
  pub key: KeyCode,
  pub event_type: KeyEventType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorTrajectoryPoint {
  pub time_ms: f64,
  pub x: f64,
  pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanHeader {
  pub map_title: String,
  pub map_artist: String,
  pub map_version: String,
  pub total_objects: usize,
  pub arrival_lead_ms: f64,
  pub post_hit_dwell_ms: f64,
  pub stream_alt_threshold_ms: f64,
  pub slider_hold_tail_pad_ms: f64,
  pub spinner_radius_osupx: f64,
  pub spinner_rps: f64,
  pub straight_line_threshold_ms: f64,
  pub trajectory_sample_interval_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionPlan {
  pub header: PlanHeader,
  pub trajectory: Vec<CursorTrajectoryPoint>,
  pub key_schedule: Vec<ScheduledKeyEvent>,
}

pub fn synthesize_plan(beatmap: &BeatmapModel) -> ActionPlan {
  let header = PlanHeader {
    map_title: beatmap.title.clone(),
    map_artist: beatmap.artist.clone(),
    map_version: beatmap.version.clone(),
    total_objects: beatmap.total_objects(),
    arrival_lead_ms: ARRIVAL_LEAD_MS,
    post_hit_dwell_ms: POST_HIT_DWELL_MS,
    stream_alt_threshold_ms: STREAM_ALT_THRESHOLD_MS,
    slider_hold_tail_pad_ms: SLIDER_HOLD_TAIL_PAD_MS,
    spinner_radius_osupx: SPINNER_RADIUS_OSUPX,
    spinner_rps: SPINNER_RPS,
    straight_line_threshold_ms: STRAIGHT_LINE_THRESHOLD_MS,
    trajectory_sample_interval_ms: TRAJECTORY_SAMPLE_INTERVAL_MS,
  };

  let key_schedule = synthesize_key_schedule(&beatmap.hit_objects);
  let trajectory = synthesize_trajectory(&beatmap.hit_objects);

  ActionPlan {
    header,
    trajectory,
    key_schedule,
  }
}

fn synthesize_key_schedule(objects: &[HitObject]) -> Vec<ScheduledKeyEvent> {
  let mut events = Vec::new();
  let mut last_hit_time: Option<f64> = None;
  let mut active_key = KeyCode::K1;
  let mut prev_was_slider = false;
  let mut slider_key = KeyCode::K1;

  for obj in objects {
    let hit_time = obj.start_time_ms();

    // Forced key alternation rules:
    // 1. Forced key switch for the first note after a slider hold
    // 2. Consecutive notes closer than STREAM_ALT_THRESHOLD_MS alternate K1/K2
    let key = if prev_was_slider {
      match slider_key {
        KeyCode::K1 => KeyCode::K2,
        KeyCode::K2 => KeyCode::K1,
      }
    } else if let Some(last_time) = last_hit_time {
      if (hit_time - last_time) <= STREAM_ALT_THRESHOLD_MS {
        match active_key {
          KeyCode::K1 => KeyCode::K2,
          KeyCode::K2 => KeyCode::K1,
        }
      } else {
        KeyCode::K1
      }
    } else {
      KeyCode::K1
    };

    active_key = key;
    last_hit_time = Some(hit_time);

    match obj {
      HitObject::Circle { .. } => {
        prev_was_slider = false;
        events.push(ScheduledKeyEvent {
          time_ms: hit_time,
          key,
          event_type: KeyEventType::Down,
        });
        events.push(ScheduledKeyEvent {
          time_ms: hit_time + POST_HIT_DWELL_MS,
          key,
          event_type: KeyEventType::Up,
        });
      }
      HitObject::Slider { duration_ms, .. } => {
        prev_was_slider = true;
        slider_key = key;
        events.push(ScheduledKeyEvent {
          time_ms: hit_time,
          key,
          event_type: KeyEventType::Down,
        });
        events.push(ScheduledKeyEvent {
          time_ms: hit_time + duration_ms + SLIDER_HOLD_TAIL_PAD_MS,
          key,
          event_type: KeyEventType::Up,
        });
      }
      HitObject::Spinner { end_time_ms, .. } => {
        prev_was_slider = false;
        events.push(ScheduledKeyEvent {
          time_ms: hit_time,
          key,
          event_type: KeyEventType::Down,
        });
        events.push(ScheduledKeyEvent {
          time_ms: *end_time_ms,
          key,
          event_type: KeyEventType::Up,
        });
      }
    }
  }

  events.sort_by(|a, b| a.time_ms.partial_cmp(&b.time_ms).unwrap_or(std::cmp::Ordering::Equal));
  events
}

fn synthesize_trajectory(objects: &[HitObject]) -> Vec<CursorTrajectoryPoint> {
  if objects.is_empty() {
    return Vec::new();
  }

  let mut trajectory = Vec::new();

  // First object start point (arrive ARRIVAL_LEAD_MS early)
  let first_obj = &objects[0];
  let first_pos = first_obj.start_pos();
  let first_arrival_time = (first_obj.start_time_ms() - ARRIVAL_LEAD_MS).max(0.0);

  let mut current_time = first_arrival_time;
  let mut current_pos = first_pos;

  trajectory.push(CursorTrajectoryPoint {
    time_ms: current_time,
    x: current_pos.0,
    y: current_pos.1,
  });

  for obj in objects.iter() {
    // 1. Move to object start pos (arriving ARRIVAL_LEAD_MS early)
    let target_arrival_time = (obj.start_time_ms() - ARRIVAL_LEAD_MS).max(current_time);
    let target_pos = obj.start_pos();

    if target_arrival_time > current_time {
      let duration = target_arrival_time - current_time;
      let steps = (duration / TRAJECTORY_SAMPLE_INTERVAL_MS).ceil() as usize;

      // Degenerates to direct straight lines when consecutive gap < STRAIGHT_LINE_THRESHOLD_MS
      for step in 1..=steps {
        let t = (step as f64) / (steps as f64);
        let sample_time = current_time + t * duration;
        let x = current_pos.0 + t * (target_pos.0 - current_pos.0);
        let y = current_pos.1 + t * (target_pos.1 - current_pos.1);
        trajectory.push(CursorTrajectoryPoint {
          time_ms: sample_time,
          x,
          y,
        });
      }
      current_time = target_arrival_time;
      current_pos = target_pos;
    }

    // 2. Handle object-specific trajectory
    match obj {
      HitObject::Circle { x, y, time_ms } => {
        // Hold on circle until hit time + dwell
        let dwell_time = *time_ms + POST_HIT_DWELL_MS;
        if dwell_time > current_time {
          current_time = dwell_time;
          current_pos = (*x, *y);
          trajectory.push(CursorTrajectoryPoint {
            time_ms: current_time,
            x: current_pos.0,
            y: current_pos.1,
          });
        }
      }
      HitObject::Slider {
        x,
        y,
        time_ms,
        curve_type,
        control_points,
        repeat_count,
        pixel_length,
        duration_ms,
        end_time_ms,
        ..
      } => {
        let curve = SliderCurve::new(*curve_type, (*x, *y), control_points, *pixel_length);
        let steps = (duration_ms / TRAJECTORY_SAMPLE_INTERVAL_MS).ceil() as usize;

        for step in 1..=steps {
          let elapsed = (step as f64) * TRAJECTORY_SAMPLE_INTERVAL_MS;
          let sample_time = (time_ms + elapsed).min(*end_time_ms);
          let progress = if *duration_ms > 0.0 {
            (elapsed / duration_ms).clamp(0.0, 1.0)
          } else {
            1.0
          };

          // Target is ball center (0 px offset)
          let pos = curve.position_at_slide_progress(progress, *repeat_count, *pixel_length);
          trajectory.push(CursorTrajectoryPoint {
            time_ms: sample_time,
            x: pos.0,
            y: pos.1,
          });
          current_pos = pos;
        }
        current_time = *end_time_ms;
      }
      HitObject::Spinner {
        x,
        y,
        time_ms,
        end_time_ms,
        duration_ms,
      } => {
        // Orbit center at SPINNER_RADIUS_OSUPX, rate SPINNER_RPS
        let steps = (duration_ms / TRAJECTORY_SAMPLE_INTERVAL_MS).ceil() as usize;
        let rev_speed = 2.0 * PI * SPINNER_RPS;

        for step in 1..=steps {
          let elapsed = (step as f64) * TRAJECTORY_SAMPLE_INTERVAL_MS;
          let sample_time = (time_ms + elapsed).min(*end_time_ms);
          let theta = (elapsed / 1000.0) * rev_speed;

          let sx = x + SPINNER_RADIUS_OSUPX * theta.cos();
          let sy = y + SPINNER_RADIUS_OSUPX * theta.sin();

          trajectory.push(CursorTrajectoryPoint {
            time_ms: sample_time,
            x: sx,
            y: sy,
          });
          current_pos = (sx, sy);
        }
        current_time = *end_time_ms;
      }
    }
  }

  trajectory
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::parser::parse_beatmap_file;

  #[test]
  fn test_synthesize_sample_plan_amended_constants() {
    let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/sample-beatmap-osu.osu");
    let model = parse_beatmap_file(fixture_path).expect("must parse sample beatmap");
    let plan = synthesize_plan(&model);

    // Verify amended header constants
    assert_eq!(plan.header.total_objects, 12);
    assert_eq!(plan.header.arrival_lead_ms, 12.0);
    assert_eq!(plan.header.post_hit_dwell_ms, 8.0);
    assert_eq!(plan.header.stream_alt_threshold_ms, 125.0);
    assert_eq!(plan.header.slider_hold_tail_pad_ms, 10.0);
    assert_eq!(plan.header.spinner_radius_osupx, 50.0);
    assert_eq!(plan.header.spinner_rps, 7.5);
    assert_eq!(plan.header.straight_line_threshold_ms, 80.0);

    // Verify key schedule
    assert_eq!(plan.key_schedule.len(), 24);
    assert_eq!(plan.key_schedule[0].key, KeyCode::K1);
    assert_eq!(plan.key_schedule[0].time_ms, 24.0);
    assert_eq!(plan.key_schedule[1].time_ms, 32.0); // 24 + 8 (POST_HIT_DWELL_MS)
  }

  #[test]
  fn test_forced_key_switch_after_slider_hold() {
    // Slider followed immediately by a circle
    let objects = vec![
      HitObject::Slider {
        x: 100.0,
        y: 100.0,
        time_ms: 100.0,
        curve_type: crate::model::CurveType::Linear,
        control_points: vec![(200.0, 100.0)],
        repeat_count: 1,
        pixel_length: 100.0,
        span_duration_ms: 200.0,
        duration_ms: 200.0,
        end_time_ms: 300.0,
      },
      HitObject::Circle {
        x: 250.0,
        y: 100.0,
        time_ms: 500.0, // gap > stream threshold, BUT must force switch because prev was slider!
      },
    ];

    let schedule = synthesize_key_schedule(&objects);
    assert_eq!(schedule[0].key, KeyCode::K1, "slider should use K1");
    // Next note after slider must force switch to K2!
    assert_eq!(schedule[2].key, KeyCode::K2, "first note after slider must switch to K2");
  }
}
