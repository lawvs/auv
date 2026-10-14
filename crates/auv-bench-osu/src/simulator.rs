use serde::{Deserialize, Serialize};

use crate::model::{BeatmapModel, HitObject};
use crate::synthesizer::ActionPlan;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Judgement {
  Hit300,
  Hit100,
  Hit50,
  Miss,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectJudgement {
  pub object_index: usize,
  pub kind: String,
  pub scheduled_time_ms: f64,
  pub actual_hit_time_ms: Option<f64>,
  pub timing_error_ms: Option<f64>,
  pub spatial_error_px: Option<f64>,
  pub judgement: Judgement,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationReport {
  pub map_name: String,
  pub total_objects: usize,
  pub count_300: usize,
  pub count_100: usize,
  pub count_50: usize,
  pub count_miss: usize,
  pub accuracy: f64,
  pub mean_absolute_timing_error_ms: f64,
  pub max_timing_error_ms: f64,
  pub judgements: Vec<ObjectJudgement>,
}

pub fn simulate_plan_offline(model: &BeatmapModel, plan: &ActionPlan) -> SimulationReport {
  let hit_window_300 = model.difficulty.hit_window_300();
  let hit_window_100 = model.difficulty.hit_window_100();
  let hit_window_50 = model.difficulty.hit_window_50();
  let hit_radius = model.difficulty.circle_radius_px();

  let mut judgements = Vec::with_capacity(model.hit_objects.len());
  let mut total_timing_err = 0.0;
  let mut hit_count = 0;
  let mut max_timing_err = 0.0f64;

  for (idx, obj) in model.hit_objects.iter().enumerate() {
    let target_time = obj.start_time_ms();
    let target_pos = obj.start_pos();

    // Find the closest key down event within the outer hit window (hit_window_50)
    let candidate_key = plan.key_schedule.iter().filter(|e| e.event_type == crate::synthesizer::KeyEventType::Down).min_by(|a, b| {
      let diff_a = (a.time_ms - target_time).abs();
      let diff_b = (b.time_ms - target_time).abs();
      diff_a.partial_cmp(&diff_b).unwrap()
    });

    let mut object_judgement = Judgement::Miss;
    let mut actual_hit_time = None;
    let mut timing_error = None;
    let mut spatial_error = None;

    if let Some(key_event) = candidate_key {
      let dt = (key_event.time_ms - target_time).abs();
      if dt <= hit_window_50 {
        // Find cursor position at keypress time
        let cursor_pos = sample_trajectory_at(&plan.trajectory, key_event.time_ms).unwrap_or(target_pos);
        let dist = (cursor_pos.0 - target_pos.0).hypot(cursor_pos.1 - target_pos.1);

        spatial_error = Some(dist);
        actual_hit_time = Some(key_event.time_ms);
        let err = key_event.time_ms - target_time;
        timing_error = Some(err);

        // Under osu! rules: hit must land within circle radius for circles and sliders;
        // spinners cover the entire playfield so any click during spinner is spatially valid.
        let is_spatial_valid = match obj {
          HitObject::Spinner { .. } => true,
          _ => dist <= hit_radius,
        };

        if is_spatial_valid {
          if dt <= hit_window_300 {
            object_judgement = Judgement::Hit300;
          } else if dt <= hit_window_100 {
            object_judgement = Judgement::Hit100;
          } else {
            object_judgement = Judgement::Hit50;
          }

          total_timing_err += err.abs();
          hit_count += 1;
          if err.abs() > max_timing_err {
            max_timing_err = err.abs();
          }
        }
      }
    }

    judgements.push(ObjectJudgement {
      object_index: idx,
      kind: obj.kind_str().to_string(),
      scheduled_time_ms: target_time,
      actual_hit_time_ms: actual_hit_time,
      timing_error_ms: timing_error,
      spatial_error_px: spatial_error,
      judgement: object_judgement,
    });
  }

  let count_300 = judgements.iter().filter(|j| j.judgement == Judgement::Hit300).count();
  let count_100 = judgements.iter().filter(|j| j.judgement == Judgement::Hit100).count();
  let count_50 = judgements.iter().filter(|j| j.judgement == Judgement::Hit50).count();
  let count_miss = judgements.iter().filter(|j| j.judgement == Judgement::Miss).count();

  let total_scored = (count_300 + count_100 + count_50 + count_miss) as f64;
  let accuracy = if total_scored > 0.0 {
    (300.0 * (count_300 as f64) + 100.0 * (count_100 as f64) + 50.0 * (count_50 as f64)) / (300.0 * total_scored)
  } else {
    0.0
  };

  let mean_timing_err = if hit_count > 0 {
    total_timing_err / (hit_count as f64)
  } else {
    0.0
  };

  SimulationReport {
    map_name: format!("{} - {} [{}]", model.artist, model.title, model.version),
    total_objects: model.total_objects(),
    count_300,
    count_100,
    count_50,
    count_miss,
    accuracy,
    mean_absolute_timing_error_ms: mean_timing_err,
    max_timing_error_ms: max_timing_err,
    judgements,
  }
}

fn sample_trajectory_at(trajectory: &[crate::synthesizer::CursorTrajectoryPoint], time_ms: f64) -> Option<(f64, f64)> {
  if trajectory.is_empty() {
    return None;
  }
  if time_ms <= trajectory[0].time_ms {
    return Some((trajectory[0].x, trajectory[0].y));
  }
  if time_ms >= trajectory.last().unwrap().time_ms {
    let p = trajectory.last().unwrap();
    return Some((p.x, p.y));
  }

  let idx = match trajectory.binary_search_by(|p| p.time_ms.partial_cmp(&time_ms).unwrap()) {
    Ok(i) => return Some((trajectory[i].x, trajectory[i].y)),
    Err(i) => i.saturating_sub(1),
  };

  if idx >= trajectory.len() - 1 {
    let p = trajectory.last().unwrap();
    return Some((p.x, p.y));
  }

  let p0 = &trajectory[idx];
  let p1 = &trajectory[idx + 1];
  let dt = p1.time_ms - p0.time_ms;
  if dt <= 0.0 {
    return Some((p0.x, p0.y));
  }
  let t = (time_ms - p0.time_ms) / dt;
  Some((p0.x + t * (p1.x - p0.x), p0.y + t * (p1.y - p0.y)))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::parser::parse_beatmap_file;
  use crate::synthesizer::synthesize_plan;

  #[test]
  fn test_simulator_on_synthetic_plan_achieves_perfect_score() {
    let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/sample-beatmap-osu.osu");
    let model = parse_beatmap_file(fixture_path).expect("parse sample beatmap");
    let plan = synthesize_plan(&model);

    let report = simulate_plan_offline(&model, &plan);

    assert_eq!(report.total_objects, 12);
    assert_eq!(report.count_300, 12, "all 12 objects must hit 300 on synthetic plan");
    assert_eq!(report.count_100, 0);
    assert_eq!(report.count_50, 0);
    assert_eq!(report.count_miss, 0);
    assert!((report.accuracy - 1.0).abs() < 1e-6);
  }

  #[test]
  fn test_simulator_on_tear_rain_fixture_achieves_perfect_score() {
    let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/351190-cYsmix-Tear-Rain-Normal.osu");
    let model = parse_beatmap_file(fixture_path).expect("parse real tear rain beatmap");
    let plan = synthesize_plan(&model);

    let report = simulate_plan_offline(&model, &plan);

    assert_eq!(report.total_objects, 295);
    assert_eq!(report.count_300, 295, "all 295 objects must hit 300 on synthetic plan");
    assert_eq!(report.count_miss, 0);
    assert!((report.accuracy - 1.0).abs() < 1e-6);
  }
}
