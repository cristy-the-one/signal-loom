//! Reading the open log: series, values, frames, bus load and window stats.

use super::Session;
use crate::dto::{BusLoad, FrameDto, PointDto, Query, SeriesDto, StepDir, ValueDto, WindowStats};
use crate::error::Result;
use crate::index::{QueryWindow, Series};

impl Session {
    /// Physical series are bucketed by the index; math channels are evaluated
    /// by the deck (see `Deck::evaluate`) and the compare log's series follow.
    pub fn query(&self, query: &Query) -> Result<Vec<SeriesDto>> {
        let log = self.log()?;
        let (physical, derived) = self.deck.split(&query.signals)?;
        let mut series = if physical.is_empty() {
            Vec::new()
        } else {
            log.query(&QueryWindow {
                t0_us: query.t0_us,
                t1_us: query.t1_us,
                signals: physical.clone(),
                max_points: query.max_points,
            })?
        };
        if query.include_compare {
            series.extend(self.compare.series(&physical, query)?);
        }
        series.extend(self.deck.evaluate(log, &derived, query)?);
        let wanted: Vec<&str> = query.signals.iter().map(String::as_str).collect();
        Ok(series
            .into_iter()
            .filter(|series| {
                wanted.iter().any(|name| series.name == *name)
                    || (query.include_compare && series.name.ends_with(" · B"))
            })
            .map(series_dto)
            .collect())
    }

    pub fn stats(&self, name: &str, t0_us: u64, t1_us: u64) -> Result<WindowStats> {
        let log = self.log()?;
        if self.deck.is_math(name) {
            return self.deck.math_stats(log, name, t0_us, t1_us);
        }
        log.stats(name, t0_us, t1_us)
    }

    pub fn bus_load(&self, t0_us: u64, t1_us: u64) -> Result<BusLoad> {
        self.log()?.bus_load(t0_us, t1_us)
    }

    pub fn values_at(&self, t_us: u64) -> Result<Vec<ValueDto>> {
        let log = self.log()?;
        Ok(log
            .values_at(t_us)?
            .into_iter()
            .map(|value| ValueDto {
                name: value.name,
                unit: value.unit,
                value: value.value,
                label: value.label,
            })
            .collect())
    }

    pub fn frame_at(&self, t_us: u64) -> Result<Option<FrameDto>> {
        self.step(t_us.saturating_add(1), StepDir::Prev)
    }

    pub fn step(&self, t_us: u64, dir: StepDir) -> Result<Option<FrameDto>> {
        let log = self.log()?;
        let Some((hit, values)) = log.step_frame_with_values(t_us, matches!(dir, StepDir::Next))?
        else {
            return Ok(None);
        };
        let values = values
            .into_iter()
            .map(|value| ValueDto {
                name: value.name,
                unit: value.unit,
                value: value.value,
                label: value.label,
            })
            .collect();
        Ok(Some(FrameDto {
            t_us: hit.t_us,
            ordinal: hit.ordinal,
            message_id: hit.message_id,
            message_name: hit.message_name,
            extended: hit.extended,
            dlc: hit.dlc,
            data_hex: hit.data_hex,
            values,
        }))
    }
}

fn series_dto(series: Series) -> SeriesDto {
    SeriesDto {
        name: series.name,
        unit: series.unit,
        points: series
            .points
            .into_iter()
            .map(|(t, v)| PointDto { t, v })
            .collect(),
    }
}
