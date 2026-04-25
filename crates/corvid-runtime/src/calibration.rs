use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MIN_SAMPLES_TO_FLAG: u64 = 3;
const MIS_CALIBRATION_DRIFT: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationObservation {
    pub actual_correct: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationStats {
    pub prompt: String,
    pub model: String,
    pub samples: u64,
    pub correct: u64,
    pub mean_confidence: f64,
    pub accuracy: f64,
    pub drift: f64,
    pub miscalibrated: bool,
}

#[derive(Default)]
struct CalibrationAccumulator {
    samples: u64,
    correct: u64,
    confidence_sum: f64,
}

#[derive(Clone, Default)]
pub struct CalibrationStore {
    inner: Arc<Mutex<HashMap<(String, String), CalibrationAccumulator>>>,
}

impl CalibrationStore {
    pub fn record(&self, prompt: &str, model: &str, confidence: f64, actual_correct: bool) {
        let mut inner = self.inner.lock().expect("calibration store poisoned");
        let entry = inner
            .entry((prompt.to_string(), model.to_string()))
            .or_default();
        entry.samples += 1;
        entry.correct += u64::from(actual_correct);
        entry.confidence_sum += confidence.clamp(0.0, 1.0);
    }

    pub fn stats(&self, prompt: &str, model: &str) -> Option<CalibrationStats> {
        let inner = self.inner.lock().expect("calibration store poisoned");
        let entry = inner.get(&(prompt.to_string(), model.to_string()))?;
        let samples = entry.samples.max(1);
        let mean_confidence = entry.confidence_sum / samples as f64;
        let accuracy = entry.correct as f64 / samples as f64;
        let drift = (mean_confidence - accuracy).abs();
        Some(CalibrationStats {
            prompt: prompt.to_string(),
            model: model.to_string(),
            samples: entry.samples,
            correct: entry.correct,
            mean_confidence,
            accuracy,
            drift,
            miscalibrated: entry.samples >= MIN_SAMPLES_TO_FLAG
                && drift > MIS_CALIBRATION_DRIFT,
        })
    }
}
