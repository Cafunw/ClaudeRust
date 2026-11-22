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

/// Serializes read-modify-write of the stored list (a run being recorded
/// while a routine is saved or deleted would otherwise lose one of them).
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One stored routine: one this version understands, or the JSON of one it
/// doesn't (written by a newer version: an unknown schedule kind...), kept
/// as it is so saving the list doesn't drop it.
enum Stored {
    Known(Box<Routine>),
    Raw(Value),
}

/// The stored list, item by item. An error only when the stored value as a
/// whole can't be read: then nothing may be saved over it.
fn load_stored(db: &crate::db::Db) -> AppResult<Vec<Stored>> {
    let Some(text) = db.setting_get(KEY)? else { return Ok(vec![]) };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => Ok(items
            .into_iter()
            .map(|v| match serde_json::from_value::<Routine>(v.clone()) {
                Ok(r) => Stored::Known(Box::new(r)),
                Err(e) => {
                    log::warn!("keeping a routine this version can't read: {e}");
                    Stored::Raw(v)
                }
            })
            .collect()),
        Ok(Value::Null) => Ok(vec![]),
        _ => Err(AppError::coded("routines_unreadable", Value::Null, "The saved routines can't be read, so they weren't changed.")),
    }
}

/// The routines this version can run. (Unreadable entries are left out here
/// but kept in storage.)
pub fn load(db: &crate::db::Db) -> Vec<Routine> {
    match load_stored(db) {
        Ok(items) => items.into_iter().filter_map(|s| if let Stored::Known(r) = s { Some(*r) } else { None }).collect(),
        Err(e) => {
            log::warn!("{e}");
            vec![]
        }
    }
}

/// Load the list, let `f` change the routines in it, and save it, as one
/// step. Entries this version can't read stay where they were.
fn modify<T>(db: &crate::db::Db, f: impl FnOnce(&mut Vec<Routine>) -> AppResult<T>) -> AppResult<T> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stored = load_stored(db)?;
    let mut list: Vec<Routine> = stored.iter().filter_map(|s| if let Stored::Known(r) = s { Some((**r).clone()) } else { None }).collect();
    let out = f(&mut list)?;
    let mut by_id: std::collections::HashMap<String, Routine> = std::collections::HashMap::new();
    let mut order: Vec<String> = vec![];
    for r in list {
        order.push(r.id.clone());
        by_id.insert(r.id.clone(), r);
    }
    let mut items: Vec<Value> = Vec::with_capacity(stored.len() + 1);
    for s in stored {
        match s {
            Stored::Raw(v) => items.push(v),
            Stored::Known(r) => {
                if let Some(r) = by_id.remove(&r.id) {
                    items.push(serde_json::to_value(r)?);
                }
            }
        }
    }
    // new ones go last
    for id in order {
        if let Some(r) = by_id.remove(&id) {
            items.push(serde_json::to_value(r)?);
        }
    }
    db.setting_set(KEY, &serde_json::to_string(&items)?)?;
    Ok(out)
}

pub fn validate(r: &Routine) -> AppResult<()> {
    if r.name.trim().is_empty() || r.prompt.trim().is_empty() {
        return Err(AppError::coded("routine_incomplete", Value::Null, "A routine needs a name and a prompt."));
    }
    if !std::path::Path::new(&r.cwd).is_dir() {
        return Err(AppError::coded("folder_not_found", json!({"path": r.cwd}), format!("Folder not found: {}", r.cwd)));
    }
    match &r.schedule {
        Schedule::Interval { minutes } if *minutes < 5 => Err(AppError::coded("routine_too_often", json!({"n": 5}), "Run at most every 5 minutes.")),
        Schedule::Daily { time, weekdays } => {
            NaiveTime::parse_from_str(time, "%H:%M").map_err(|_| AppError::coded("routine_bad_time", Value::Null, "Use a time like 09:30."))?;
            if weekdays.iter().any(|d| !(1..=7).contains(d)) {
                return Err(AppError::coded("routine_bad_weekday", Value::Null, "Weekdays are 1 (Monday) to 7 (Sunday)."));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The first run time (ms) strictly after `after_ms`.
pub fn next_after(s: &Schedule, after_ms: i64) -> Option<i64> {
    next_after_in(&Local, s, after_ms)
}

/// `next_after` in time zone `tz`.
fn next_after_in<Tz: TimeZone>(tz: &Tz, s: &Schedule, after_ms: i64) -> Option<i64> {
    match s {
        Schedule::Interval { minutes } => Some(after_ms + *minutes as i64 * 60_000),
        Schedule::Daily { time, weekdays } => {
            let t = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
            let after = tz.timestamp_millis_opt(after_ms).earliest()?;
            for d in 0..8 {
                let day = after.date_naive() + chrono::Duration::days(d);
                let wd = day.weekday().number_from_monday() as u8;
                if !weekdays.is_empty() && !weekdays.contains(&wd) {
                    continue;
                }
                // a day whose time can't be placed is skipped, never the
                // routine (`?` here made it never due again)
                let Some(ms) = run_time_on(tz, day, t) else { continue };
                if ms > after_ms {
                    return Some(ms);
                }
            }
            None
        }
    }
}

/// When local time `t` on `day` happens, in ms. Twice (clocks going back):
/// the first. Not at all (clocks going forward over it, e.g. 02:30 on the
/// night summer time starts): the moment the clocks jump to, so the routine
/// still runs that day.
fn run_time_on<Tz: TimeZone>(tz: &Tz, day: chrono::NaiveDate, t: NaiveTime) -> Option<i64> {
    let at = day.and_time(t.with_second(0).unwrap_or(t));
    if let Some(dt) = tz.from_local_datetime(&at).earliest() {
        return Some(dt.timestamp_millis());
    }
    // in a gap: the first minute that exists after it (gaps last minutes
    // to hours, a day at most)
    (1..=24 * 60).find_map(|m| tz.from_local_datetime(&(at + chrono::Duration::minutes(m))).earliest()).map(|dt| dt.timestamp_millis())
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
    modify(db, |list| {
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
            return Err(AppError::coded("routine_not_found", Value::Null, "Routine not found."));
        }
        Ok(r)
    })
}

pub fn delete(db: &crate::db::Db, id: &str) -> AppResult<()> {
    modify(db, |list| {
        list.retain(|r| r.id != id);
        Ok(())
    })
}

pub fn mark_run(db: &crate::db::Db, id: &str, conv: Option<String>, error: Option<String>) {
    let r = modify(db, |list| {
        if let Some(r) = list.iter_mut().find(|r| r.id == id) {
            r.last_run_at = Some(now_ms());
            if conv.is_some() {
                r.last_conversation_id = conv;
            }
            r.last_error = error;
        }
        Ok(())
    });
    if let Err(e) = r {
        log::warn!("recording a routine run failed: {e}");
    }
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

    /// Central European time without the tz database: UTC+1, and UTC+2 from
    /// 2026-03-29 01:00 UTC (02:00 local jumps to 03:00).
    #[derive(Clone, Copy)]
    struct Cet;

    impl Cet {
        fn switch() -> chrono::NaiveDateTime {
            chrono::NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(1, 0, 0).unwrap()
        }
        fn off(summer: bool) -> chrono::FixedOffset {
            chrono::FixedOffset::east_opt(if summer { 7200 } else { 3600 }).unwrap()
        }
    }

    impl TimeZone for Cet {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &chrono::FixedOffset) -> Self {
            Cet
        }
        fn offset_from_local_date(&self, d: &chrono::NaiveDate) -> chrono::LocalResult<chrono::FixedOffset> {
            self.offset_from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, l: &chrono::NaiveDateTime) -> chrono::LocalResult<chrono::FixedOffset> {
            let gap_start = Cet::switch() + chrono::Duration::hours(1); // 02:00 local
            if *l < gap_start {
                chrono::LocalResult::Single(Cet::off(false))
            } else if *l < gap_start + chrono::Duration::hours(1) {
                chrono::LocalResult::None
            } else {
                chrono::LocalResult::Single(Cet::off(true))
            }
        }
        fn offset_from_utc_date(&self, d: &chrono::NaiveDate) -> chrono::FixedOffset {
            self.offset_from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, u: &chrono::NaiveDateTime) -> chrono::FixedOffset {
            Cet::off(*u >= Cet::switch())
        }
    }

    #[test]
    fn daily_routine_survives_the_spring_forward_gap() {
        let s = Schedule::Daily { time: "02:30".into(), weekdays: vec![] };
        // Saturday 03:00 local, the day before summer time starts
        let sat = Cet.with_ymd_and_hms(2026, 3, 28, 3, 0, 0).unwrap().timestamp_millis();
        let next = next_after_in(&Cet, &s, sat).expect("still due");
        // 02:30 doesn't exist on Sunday: it runs when the clocks reach 03:00
        assert_eq!(Cet.timestamp_millis_opt(next).unwrap().naive_local(), chrono::NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(3, 0, 0).unwrap());
        // and at 02:30 again the day after
        let then = next_after_in(&Cet, &s, next).unwrap();
        assert_eq!(Cet.timestamp_millis_opt(then).unwrap().naive_local(), chrono::NaiveDate::from_ymd_opt(2026, 3, 30).unwrap().and_hms_opt(2, 30, 0).unwrap());
    }

    fn db_with(json: &str) -> crate::db::Db {
        let db = crate::db::Db::open_in_memory().unwrap();
        db.setting_set(KEY, json).unwrap();
        db
    }

    #[test]
    fn unreadable_routines_are_kept_when_saving() {
        let mut known = routine(Schedule::Interval { minutes: 60 });
        known.id = "known".into();
        let future = json!({"id": "future", "name": "f", "prompt": "p", "cwd": "/", "schedule": {"kind": "cron", "expr": "0 9 * * *"}});
        let db = db_with(&serde_json::to_string(&json!([future, known])).unwrap());
        let list = load(&db);
        assert_eq!(list.len(), 1, "the readable one is listed");
        let mut added = routine(Schedule::Interval { minutes: 30 });
        added.id = String::new();
        let added = upsert(&db, added).unwrap();
        mark_run(&db, "known", Some("c1".into()), None);
        delete(&db, &added.id).unwrap();
        let stored: Value = serde_json::from_str(&db.setting_get(KEY).unwrap().unwrap()).unwrap();
        assert_eq!(stored[0], future, "kept as it was, in its place");
        assert_eq!(stored[1]["id"], "known");
        assert_eq!(stored[1]["last_conversation_id"], "c1");
        assert_eq!(stored.as_array().unwrap().len(), 2);
        // a stored value that can't be read at all isn't saved over
        let db = db_with("{not json");
        assert!(load(&db).is_empty());
        let mut fresh = routine(Schedule::Interval { minutes: 30 });
        fresh.id = String::new();
        assert!(upsert(&db, fresh).unwrap_err().to_string().contains("can't be read"));
        assert_eq!(db.setting_get(KEY).unwrap().unwrap(), "{not json");
    }

    #[test]
    fn concurrent_changes_are_not_lost() {
        let db = std::sync::Arc::new(db_with("[]"));
        let mut ids = vec![];
        for _ in 0..4 {
            let mut r = routine(Schedule::Interval { minutes: 60 });
            r.id = String::new();
            ids.push(upsert(&db, r).unwrap().id);
        }
        let threads: Vec<_> = ids
            .iter()
            .cloned()
            .map(|id| {
                let db = db.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        mark_run(&db, &id, Some(format!("{id}-{i}")), None);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        for r in load(&db) {
            assert_eq!(r.last_conversation_id, Some(format!("{}-19", r.id)), "every routine's last run was recorded");
        }
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
