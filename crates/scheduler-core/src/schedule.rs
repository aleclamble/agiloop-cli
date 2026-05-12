use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use cron::Schedule as CronSchedule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    Cron {
        expression: String,
        timezone: String,
        #[serde(default)]
        misfire_policy: MisfirePolicy,
    },
    Interval {
        every: u64,
        unit: IntervalUnit,
        #[serde(default)]
        timezone: Option<String>,
        #[serde(default)]
        start_at: Option<DateTime<Utc>>,
        #[serde(default)]
        misfire_policy: MisfirePolicy,
    },
    Once {
        at: DateTime<Utc>,
        #[serde(default)]
        timezone: Option<String>,
    },
    Manual {},
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicy {
    Skip,
    #[default]
    RunOnce,
    Backfill,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntervalUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl ScheduleSpec {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ScheduleSpec::Cron {
                expression,
                timezone,
                ..
            } => {
                parse_timezone(timezone)?;
                parse_five_field_cron(expression)?;
                Ok(())
            }
            ScheduleSpec::Interval { every, unit, .. } => {
                if *every == 0 {
                    return Err("interval value must be greater than zero".to_string());
                }
                let duration = interval_duration(*every, *unit)?;
                if duration < Duration::seconds(60) {
                    return Err("minimum interval is 60 seconds".to_string());
                }
                Ok(())
            }
            ScheduleSpec::Once { .. } | ScheduleSpec::Manual {} => Ok(()),
        }
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, String> {
        match self {
            ScheduleSpec::Cron {
                expression,
                timezone,
                ..
            } => {
                let tz = parse_timezone(timezone)?;
                let schedule = parse_five_field_cron(expression)?;
                let after_in_tz = after.with_timezone(&tz);
                let next = schedule
                    .after(&after_in_tz)
                    .next()
                    .ok_or_else(|| "cron expression has no future occurrence".to_string())?;
                Ok(Some(next.with_timezone(&Utc)))
            }
            ScheduleSpec::Interval {
                every,
                unit,
                start_at,
                ..
            } => {
                let duration = interval_duration(*every, *unit)?;
                let mut next = start_at.unwrap_or(after + duration);
                if next <= after {
                    let elapsed = after.signed_duration_since(next);
                    let steps = elapsed.num_seconds() / duration.num_seconds() + 1;
                    next += duration * steps as i32;
                }
                Ok(Some(next))
            }
            ScheduleSpec::Once { at, .. } => {
                if *at > after {
                    Ok(Some(*at))
                } else {
                    Ok(None)
                }
            }
            ScheduleSpec::Manual {} => Ok(None),
        }
    }
}

pub fn parse_timezone(value: &str) -> Result<Tz, String> {
    value
        .parse::<Tz>()
        .map_err(|_| format!("invalid timezone `{value}`"))
}

fn parse_five_field_cron(expression: &str) -> Result<CronSchedule, String> {
    if expression.split_whitespace().count() != 5 {
        return Err("cron expression must have exactly five fields".to_string());
    }
    let with_seconds = format!("0 {expression}");
    CronSchedule::from_str(&with_seconds)
        .map_err(|error| format!("invalid cron expression `{expression}`: {error}"))
}

fn interval_duration(every: u64, unit: IntervalUnit) -> Result<Duration, String> {
    let every = i64::try_from(every).map_err(|_| "interval value is too large".to_string())?;
    match unit {
        IntervalUnit::Seconds => Ok(Duration::seconds(every)),
        IntervalUnit::Minutes => Ok(Duration::minutes(every)),
        IntervalUnit::Hours => Ok(Duration::hours(every)),
        IntervalUnit::Days => Ok(Duration::days(every)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn validates_five_field_cron_with_timezone() {
        let schedule = ScheduleSpec::Cron {
            expression: "0 8 * * *".to_string(),
            timezone: "Africa/Johannesburg".to_string(),
            misfire_policy: MisfirePolicy::RunOnce,
        };

        assert!(schedule.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_cron_without_panicking() {
        let schedule = ScheduleSpec::Cron {
            expression: "not a cron".to_string(),
            timezone: "Africa/Johannesburg".to_string(),
            misfire_policy: MisfirePolicy::RunOnce,
        };

        assert!(schedule.validate().is_err());
    }

    #[test]
    fn rejects_sub_minute_intervals() {
        let schedule = ScheduleSpec::Interval {
            every: 30,
            unit: IntervalUnit::Seconds,
            timezone: None,
            start_at: None,
            misfire_policy: MisfirePolicy::RunOnce,
        };

        assert_eq!(
            schedule.validate(),
            Err("minimum interval is 60 seconds".to_string())
        );
    }

    #[test]
    fn calculates_next_interval_after_anchor() {
        let start_at = Utc.with_ymd_and_hms(2026, 5, 12, 8, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 5, 12, 9, 5, 0).unwrap();
        let schedule = ScheduleSpec::Interval {
            every: 30,
            unit: IntervalUnit::Minutes,
            timezone: None,
            start_at: Some(start_at),
            misfire_policy: MisfirePolicy::RunOnce,
        };

        assert_eq!(
            schedule.next_after(after).unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 5, 12, 9, 30, 0).unwrap())
        );
    }

    #[test]
    fn manual_schedule_has_no_next_time() {
        assert_eq!(
            ScheduleSpec::Manual {}
                .next_after(Utc.with_ymd_and_hms(2026, 5, 12, 9, 5, 0).unwrap())
                .unwrap(),
            None
        );
    }
}
