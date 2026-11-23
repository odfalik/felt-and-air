//! The saved/editor setup format. Baking consumes the same snapshot, not a second settings schema.
use piano_engine::{params::PARAMS, GrandOptions};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Setup {
    pub values: Vec<f64>,
    pub grid: i32,
    pub volume: f32,
}

impl Default for Setup {
    fn default() -> Self {
        crate::PianoParams::new().capture_setup()
    }
}

pub(crate) fn grid_scale(index: i32) -> Option<f64> {
    [1.0, 1.25, 1.5, 2.0].get(index as usize).copied()
}

impl Setup {
    /// Accept current-length setups and older setups (padding missing trailing defaults),
    /// while rejecting corrupt/out-of-range values.
    pub fn options(&self) -> Result<GrandOptions, String> {
        let len = self.values.len();
        let valid_len = len == PARAMS.len()
            || len == PARAMS.len().saturating_sub(3)
            || len == PARAMS.len().saturating_sub(5);
        if !valid_len
            || !(0..=3).contains(&self.grid)
            || !self.volume.is_finite()
            || !(0.0..=3.0).contains(&self.volume)
        {
            return Err("Invalid setup length, grid or volume".into());
        }
        let mut options = GrandOptions::default();
        for (p, &value) in PARAMS.iter().zip(&self.values) {
            if !value.is_finite() || value < p.lo || value > p.hi {
                return Err(format!("Invalid setup value for {}", p.name));
            }
            p.write(&mut options, value);
        }
        Ok(options)
    }

    pub fn grid_scale(&self) -> Result<f64, String> {
        grid_scale(self.grid).ok_or_else(|| "Invalid setup grid".into())
    }
}
