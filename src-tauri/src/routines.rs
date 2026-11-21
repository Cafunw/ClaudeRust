//! Routines: prompts that run on a schedule as new code sessions (e.g.
//! "every morning at 9: update dependencies and run the tests").
//!
//! Stored in the settings table (key `routines`). A scheduler checks every
//! 30 seconds; a run starts a new code conversation in the routine's
//! folder and sends its prompt. A run missed while the app was closed is
//! made up once at startup.

use crate::error::{AppError, AppResult};
use crate::util::{new_id, now_ms};
use chrono::{Datelike, Local, NaiveTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Schedule {
    /// Every `minutes` minutes.
    Interval { minutes: u32 },
    /// At `time` (HH:MM, local) on the given weekdays (1 = Monday … 7 = Sunday; empty = every day).
    Daily {
        time: String,
        #[serde(default)]
        weekdays: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub cwd: String,
    pub schedule: Schedule,
    #[serde(default)]
    pub engine: String,
    #[serde(default = "default_mode")]
    pub permission_mode: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub worktree: bool,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub last_conversation_id: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

fn default_mode() -> String {
    "auto".into()
}
fn yes() -> bool {
    true
}

const KEY: &str = "routines";

pub fn load(db: &crate::db::Db) -> Vec<Routine> {
    db.setting_get(KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn store(db: &crate::db::Db, list: &[Routine]) -> AppResult<()> {
    db.setting_set(KEY, &serde_json::to_string(list)?)
}

pub fn validate(r: &Routine) -> AppResult<()> {
    if r.name.trim().is_empty() || r.prompt.trim().is_empty() {
        return Err(AppError::msg("A routine needs a name and a prompt."));
    }
    if !std::path::Path::new(&r.cwd).is_dir() {
        return Err(AppError::msg(format!("Folder not found: {}", r.cwd)));
    }
    match &r.schedule {
        Schedule::Interval { minutes } if *minutes < 5 => Err(AppError::msg("Run at most every 5 minutes.")),
        Schedule::Daily { time, weekdays } => {
            NaiveTime::parse_from_str(time, "%H:%M").map_err(|_| AppError::msg("Use a time like 09:30."))?;
            if weekdays.iter().any(|d| !(1..=7).contains(d)) {
                return Err(AppError::msg("Weekdays are 1 (Monday) to 7 (Sunday)."));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The first run time (ms) strictly after `after_ms`.
pub fn next_after(s: &Schedule, after_ms: i64) -> Option<i64> {
    match s {
        Schedule::Interval { minutes } => Some(after_ms + *minutes as i64 * 60_000),
        Schedule::Daily { time, weekdays } => {
            let t = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
            let after = Local.timestamp_millis_opt(after_ms).single()?;
            for d in 0..8 {
                let day = after.date_naive() + chrono::Duration::days(d);
                let wd = day.weekday().number_from_monday() as u8;
                if !weekdays.is_empty() && !weekdays.contains(&wd) {
                    continue;
                }
                let dt = Local.from_local_datetime(&day.and_hms_opt(t.hour(), t.minute(), 0)?).earliest()?;
                if dt.timestamp_millis() > after_ms {
                    return Some(dt.timestamp_millis());
                }
            }
            None
        }
    }
}

/// When a routine is due next.
pub fn next_run(r: &Routine) -> Option<i64> {
    if !r.enabled {
        return None;
    }
    next_after(&r.schedule, r.last_run_at.unwrap_or(r.created_at))
}

pub fn to_json(r: &Routine) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or(json!({}));
    v["next_run_at"] = json!(next_run(r));
    v
}

/// Insert or update; returns the saved routine.
pub fn upsert(db: &crate::db::Db, mut r: Routine) -> AppResult<Routine> {
    validate(&r)?;
    let mut list = load(db);
    if r.id.is_empty() {
        r.id = new_id();
        r.created_at = now_ms();
        list.push(r.clone());
    } else if let Some(x) = list.iter_mut().find(|x| x.id == r.id) {
        // keep run history; a schedule change counts from now
        r.created_at = x.created_at;
        r.last_conversation_id = x.last_conversation_id.clone();
        r.last_error = x.last_error.clone();
        r.last_run_at = x.last_run_at;
        if x.schedule != r.schedule || (!x.enabled && r.enabled) {
            // counts from now, so changing the schedule doesn't trigger a run
            r.created_at = now_ms();
            r.last_run_at = None;
        }
        *x = r.clone();
    } else {
        return Err(AppError::msg("Routine not found."));
    }
    store(db, &list)?;
    Ok(r)
}

pub fn delete(db: &crate::db::Db, id: &str) -> AppResult<()> {
    let mut list = load(db);
    list.retain(|r| r.id != id);
    store(db, &list)
}

pub fn mark_run(db: &crate::db::Db, id: &str, conv: Option<String>, error: Option<String>) {
    let mut list = load(db);
    if let Some(r) = list.iter_mut().find(|r| r.id == id) {
        r.last_run_at = Some(now_ms());
        if conv.is_some() {
            r.last_conversation_id = conv;
        }
        r.last_error = error;
    }
    let _ = store(db, &list);
}

/// Routines due at `now`.
pub fn due(list: &[Routine], now: i64) -> Vec<Routine> {
    list.iter().filter(|r| next_run(r).map(|t| t <= now).unwrap_or(false)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(s: Schedule) -> Routine {
        Routine {
            id: "r".into(), name: "n".into(), prompt: "p".into(), cwd: std::env::temp_dir().to_string_lossy().to_string(), schedule: s,
            engine: String::new(), permission_mode: "auto".into(), model: None, enabled: true, worktree: false,
            last_run_at: None, last_conversation_id: None, last_error: None, created_at: 1_000_000,
        }
    }

    #[test]
    fn interval_and_daily_schedules() {
        let r = routine(Schedule::Interval { minutes: 30 });
        assert_eq!(next_run(&r), Some(1_000_000 + 30 * 60_000));
        assert_eq!(due(std::slice::from_ref(&r), 1_000_000 + 30 * 60_000).len(), 1);
        assert!(due(&[r], 1_000_000).is_empty());
        // daily at 09:30 → the next 09:30 after a moment
        let base = Local.with_ymd_and_hms(2026, 11, 20, 10, 0, 0).unwrap().timestamp_millis(); // Friday
        let next = next_after(&Schedule::Daily { time: "09:30".into(), weekdays: vec![] }, base).unwrap();
        let n = Local.timestamp_millis_opt(next).unwrap();
        assert_eq!((n.day(), n.hour(), n.minute()), (21, 9, 30));
        // weekdays only (Mon-Fri): Friday 10:00 → Monday 09:30
        let next = next_after(&Schedule::Daily { time: "09:30".into(), weekdays: vec![1, 2, 3, 4, 5] }, base).unwrap();
        assert_eq!(Local.timestamp_millis_opt(next).unwrap().weekday().number_from_monday(), 1);
    }

    #[test]
    fn validation() {
        assert!(validate(&routine(Schedule::Interval { minutes: 1 })).is_err());
        assert!(validate(&routine(Schedule::Daily { time: "25:99".into(), weekdays: vec![] })).is_err());
        assert!(validate(&routine(Schedule::Daily { time: "07:05".into(), weekdays: vec![6, 7] })).is_ok());
        let mut r = routine(Schedule::Interval { minutes: 60 });
        r.enabled = false;
        assert_eq!(next_run(&r), None);
    }
}
