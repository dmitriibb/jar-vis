//! Scheduled prompts.
//!
//! The agent asks for one by ending its reply with a fenced block:
//!
//! ````text
//! ```jarvis-schedule
//! {"prompt": "Check whether the service is running", "in_minutes": 60}
//! ```
//! ````
//!
//! Jar-vis removes the block from the reply, stores a task bound to the session
//! the reply came from, and later sends the prompt to that session.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use chrono::{Duration, Local, NaiveDateTime, NaiveTime, TimeZone};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::backend::SessionRef;
use crate::config;

const FENCE: &str = "jarvis-schedule";
const MINUTE_MS: i64 = 60_000;
/// Repeating tasks end after this long unless the request says otherwise.
const DEFAULT_REPEAT_MINUTES: i64 = 24 * 60;
const MAX_PROMPT_CHARS: usize = 2000;
const MAX_TASKS: usize = 100;

/// What the agent writes inside the fenced block.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Directive {
    pub prompt: String,
    /// Run once, this many minutes from now. With `every_minutes`: delay of the first run.
    #[serde(default)]
    pub in_minutes: Option<f64>,
    /// Run once at a local time: `HH:MM` (next occurrence) or `YYYY-MM-DD HH:MM`.
    #[serde(default)]
    pub at: Option<String>,
    /// Repeat at this interval.
    #[serde(default)]
    pub every_minutes: Option<f64>,
    /// Stop a repeating task after this many runs.
    #[serde(default)]
    pub times: Option<u32>,
    /// Stop a repeating task after this many minutes.
    #[serde(default)]
    pub for_minutes: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub session: SessionRef,
    #[serde(default)]
    pub session_title: Option<String>,
    pub prompt: String,
    /// All times are milliseconds since the Unix epoch.
    pub created: i64,
    pub next_run: i64,
    /// Interval of a repeating task.
    #[serde(default)]
    pub every_minutes: Option<f64>,
    /// Runs completed so far.
    #[serde(default)]
    pub runs: u32,
    #[serde(default)]
    pub max_runs: Option<u32>,
    /// A repeating task is dropped once its next run would fall after this.
    #[serde(default)]
    pub expires: Option<i64>,
}

impl Task {
    /// "in 60 minutes", "every 15 minutes", for the note sent with the prompt.
    fn cadence(&self) -> String {
        match self.every_minutes {
            Some(every) => format!("every {}", minutes_text(every)),
            None => "once".to_string(),
        }
    }
}

fn minutes_text(minutes: f64) -> String {
    let rounded = minutes.round() as i64;
    if rounded == 1 {
        "minute".to_string()
    } else if rounded % 60 == 0 && rounded >= 60 {
        let hours = rounded / 60;
        if hours == 1 { "hour".to_string() } else { format!("{hours} hours") }
    } else {
        format!("{rounded} minutes")
    }
}

/// Split a reply into the text to show and the scheduling requests in it.
/// Blocks that cannot be parsed are still removed, and reported as errors.
pub fn extract(text: &str) -> (String, Vec<Result<Directive, String>>) {
    let re = Regex::new(&format!(r"(?s)```+\s*{FENCE}[^\n]*\n(.*?)```+")).expect("valid regex");
    let mut found = Vec::new();
    for cap in re.captures_iter(text) {
        let body = cap[1].trim();
        // One object, or an array of them.
        match serde_json::from_str::<Vec<Directive>>(body) {
            Ok(list) => found.extend(list.into_iter().map(Ok)),
            Err(_) => found.push(serde_json::from_str::<Directive>(body).map_err(|e| e.to_string())),
        }
    }
    let mut clean = re.replace_all(text, "").into_owned();
    // An unterminated block at the very end of a cut-off reply.
    if let Some(start) = clean.find(&format!("```{FENCE}")) {
        clean.truncate(start);
    }
    (clean.trim().to_string(), found)
}

fn parse_at(at: &str, now_ms: i64) -> Result<i64> {
    let now = Local.timestamp_millis_opt(now_ms).single().ok_or_else(|| anyhow!("bad clock"))?;
    let at = at.trim().replace('T', " ");
    let local = |naive: NaiveDateTime| {
        Local.from_local_datetime(&naive).earliest().ok_or_else(|| anyhow!("\"{at}\" is not a valid local time"))
    };
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&at, format) {
            return Ok(local(naive)?.timestamp_millis());
        }
    }
    let time = NaiveTime::parse_from_str(&at, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(&at, "%H:%M:%S"))
        .map_err(|_| anyhow!("\"{at}\" is not a time (use HH:MM or YYYY-MM-DD HH:MM)"))?;
    let mut when = local(now.date_naive().and_time(time))?;
    // A clock time that already passed today means tomorrow.
    if when <= now {
        when = local((now.date_naive() + Duration::days(1)).and_time(time))?;
    }
    Ok(when.timestamp_millis())
}

fn to_ms(minutes: f64) -> i64 {
    (minutes * MINUTE_MS as f64).round() as i64
}

/// Turn a request into a task for `session`.
pub fn task_from(directive: &Directive, session: &SessionRef, title: Option<String>, now_ms: i64) -> Result<Task> {
    let prompt = directive.prompt.trim();
    if prompt.is_empty() {
        bail!("the scheduled prompt is empty");
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        bail!("the scheduled prompt is too long");
    }
    let positive = |value: Option<f64>, name: &str| -> Result<Option<f64>> {
        match value {
            Some(v) if !v.is_finite() || v <= 0.0 => bail!("{name} must be greater than zero"),
            other => Ok(other),
        }
    };
    let in_minutes = positive(directive.in_minutes, "in_minutes")?;
    let every = positive(directive.every_minutes, "every_minutes")?;
    let for_minutes = positive(directive.for_minutes, "for_minutes")?;
    if every.is_some_and(|e| e < 1.0) {
        bail!("every_minutes must be at least 1");
    }

    let next_run = match (&directive.at, in_minutes, every) {
        (Some(at), _, _) => parse_at(at, now_ms)?,
        (None, Some(delay), _) => now_ms + to_ms(delay),
        (None, None, Some(every)) => now_ms + to_ms(every),
        (None, None, None) => bail!("say when: in_minutes, at or every_minutes"),
    };
    if next_run <= now_ms {
        bail!("the scheduled time is in the past");
    }
    let (max_runs, expires) = match every {
        Some(_) => {
            let lifetime = for_minutes.map(to_ms).unwrap_or(DEFAULT_REPEAT_MINUTES * MINUTE_MS);
            (directive.times.filter(|t| *t > 0), Some(now_ms + lifetime))
        }
        None => (None, None),
    };
    Ok(Task {
        id: format!("task_{now_ms:x}_{:04x}", rand_suffix()),
        session: session.clone(),
        session_title: title,
        prompt: prompt.to_string(),
        created: now_ms,
        next_run,
        every_minutes: every,
        runs: 0,
        max_runs,
        expires,
    })
}

/// Enough to keep ids apart when several tasks are created in one millisecond.
fn rand_suffix() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static COUNTER: AtomicU16 = AtomicU16::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

const WIRE_PREFIX: &str = "[scheduled task:";
const VOICE_PREFIX: &str = "[voice mode:";

/// The prompt as sent to the agent, with a note that it was scheduled.
pub fn wire_prompt(task: &Task, now_ms: i64) -> String {
    let asked = Local
        .timestamp_millis_opt(task.created)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let now = Local
        .timestamp_millis_opt(now_ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let run = match task.every_minutes {
        Some(_) => format!(" This is run {} of a repeating task.", task.runs + 1),
        None => String::new(),
    };
    format!(
        "{WIRE_PREFIX} the user is not typing this now. On {asked} they asked for it to be sent to you {}; it is now {now}.{run} \
         Do what it says and report the result briefly. Do not schedule it again.]\n\n{}",
        task.cadence(),
        task.prompt
    )
}

/// Undo the notes Jar-vis adds to messages, for showing them in the panel.
/// Returns the user's text and whether it was a scheduled prompt.
pub fn display_text(wire: &str) -> (String, bool) {
    let mut text = wire.trim();
    let mut scheduled = false;
    loop {
        let is_schedule = text.starts_with(WIRE_PREFIX);
        if !is_schedule && !text.starts_with(VOICE_PREFIX) {
            break;
        }
        let Some(end) = text.find("]\n\n") else { break };
        scheduled |= is_schedule;
        text = text[end + 3..].trim_start();
    }
    (text.to_string(), scheduled)
}

/// The tasks of every session, persisted as `schedule.json`.
pub struct Schedule {
    path: PathBuf,
    tasks: Vec<Task>,
}

impl Schedule {
    pub fn load() -> Self {
        Self::load_from(config::data_dir().join("schedule.json"))
    }

    pub fn load_from(path: PathBuf) -> Self {
        let tasks = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path, tasks }
    }

    fn save(&self) {
        match serde_json::to_vec_pretty(&self.tasks) {
            Ok(bytes) => {
                if let Err(e) = config::write_atomic(&self.path, &bytes) {
                    tracing::warn!("could not save the schedule: {e:#}");
                }
            }
            Err(e) => tracing::warn!("could not serialize the schedule: {e}"),
        }
    }

    /// Tasks ordered by their next run.
    pub fn tasks(&self) -> Vec<Task> {
        let mut tasks = self.tasks.clone();
        tasks.sort_by_key(|t| t.next_run);
        tasks
    }

    pub fn add(&mut self, task: Task) -> Result<()> {
        if self.tasks.len() >= MAX_TASKS {
            bail!("there are already {MAX_TASKS} scheduled tasks");
        }
        self.tasks.push(task);
        self.save();
        Ok(())
    }

    /// Returns false when there is no such task.
    pub fn cancel(&mut self, id: &str) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.id != id);
        let removed = self.tasks.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    /// Drop the tasks of a session that no longer exists. Returns how many.
    pub fn remove_session(&mut self, session_id: &str) -> usize {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.session.id != session_id);
        let removed = before - self.tasks.len();
        if removed > 0 {
            self.save();
        }
        removed
    }

    pub fn set_title(&mut self, session_id: &str, title: &str) {
        let mut changed = false;
        for task in self.tasks.iter_mut().filter(|t| t.session.id == session_id) {
            if task.session_title.as_deref() != Some(title) {
                task.session_title = Some(title.to_string());
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Take the tasks that are due. One-shot tasks are removed. Repeating tasks
    /// are moved to their next run, counted from now, so runs missed while the
    /// machine slept are skipped rather than fired in a burst.
    pub fn take_due(&mut self, now_ms: i64) -> Vec<Task> {
        let mut due = Vec::new();
        let mut keep = Vec::with_capacity(self.tasks.len());
        for mut task in std::mem::take(&mut self.tasks) {
            if task.next_run > now_ms {
                keep.push(task);
                continue;
            }
            due.push(task.clone());
            if let Some(every) = task.every_minutes {
                task.runs += 1;
                task.next_run = now_ms + to_ms(every);
                let exhausted = task.max_runs.is_some_and(|max| task.runs >= max);
                let expired = task.expires.is_some_and(|end| task.next_run > end);
                if !exhausted && !expired {
                    keep.push(task);
                }
            }
        }
        self.tasks = keep;
        if !due.is_empty() {
            self.save();
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionRef {
        SessionRef { id: "ses_1".into(), directory: PathBuf::from("/ws") }
    }

    fn directive(json: &str) -> Directive {
        serde_json::from_str(json).unwrap()
    }

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn extracts_and_strips_blocks() {
        let reply = "Okay, I will check in an hour.\n\n```jarvis-schedule\n{\"prompt\": \"Check the service\", \"in_minutes\": 60}\n```\n";
        let (clean, found) = extract(reply);
        assert_eq!(clean, "Okay, I will check in an hour.");
        assert_eq!(found.len(), 1);
        let d = found[0].as_ref().unwrap();
        assert_eq!(d.prompt, "Check the service");
        assert_eq!(d.in_minutes, Some(60.0));
    }

    #[test]
    fn extracts_arrays_and_keeps_other_code() {
        let reply = "Done.\n---\n```bash\nls\n```\n```jarvis-schedule\n[{\"prompt\":\"a\",\"in_minutes\":5},{\"prompt\":\"b\",\"every_minutes\":15}]\n```";
        let (clean, found) = extract(reply);
        assert!(clean.contains("```bash\nls\n```"));
        assert!(!clean.contains("jarvis-schedule"));
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].as_ref().unwrap().every_minutes, Some(15.0));
    }

    #[test]
    fn broken_blocks_are_removed_and_reported() {
        let (clean, found) = extract("Sure.\n```jarvis-schedule\n{not json}\n```");
        assert_eq!(clean, "Sure.");
        assert!(found[0].is_err());
        let (clean, found) = extract("Sure.\n```jarvis-schedule\n{\"prompt\": \"cut off");
        assert_eq!(clean, "Sure.");
        assert!(found.is_empty());
        assert_eq!(extract("No blocks here."), ("No blocks here.".to_string(), vec![]));
    }

    #[test]
    fn one_shot_task() {
        let task = task_from(&directive(r#"{"prompt":" Check it ","in_minutes":60}"#), &session(), Some("T".into()), NOW).unwrap();
        assert_eq!(task.prompt, "Check it");
        assert_eq!(task.next_run, NOW + 60 * MINUTE_MS);
        assert_eq!((task.every_minutes, task.expires, task.max_runs), (None, None, None));
        assert_eq!(task.session_title.as_deref(), Some("T"));
    }

    #[test]
    fn repeating_task_defaults_to_a_day() {
        let task = task_from(&directive(r#"{"prompt":"x","every_minutes":15}"#), &session(), None, NOW).unwrap();
        assert_eq!(task.next_run, NOW + 15 * MINUTE_MS);
        assert_eq!(task.expires, Some(NOW + 24 * 60 * MINUTE_MS));
        let limited = task_from(&directive(r#"{"prompt":"x","every_minutes":15,"in_minutes":1,"times":3,"for_minutes":120}"#), &session(), None, NOW).unwrap();
        assert_eq!(limited.next_run, NOW + MINUTE_MS);
        assert_eq!(limited.max_runs, Some(3));
        assert_eq!(limited.expires, Some(NOW + 120 * MINUTE_MS));
    }

    #[test]
    fn rejects_bad_requests() {
        for json in [
            r#"{"prompt":"","in_minutes":5}"#,
            r#"{"prompt":"x"}"#,
            r#"{"prompt":"x","in_minutes":0}"#,
            r#"{"prompt":"x","in_minutes":-5}"#,
            r#"{"prompt":"x","every_minutes":0.2}"#,
            r#"{"prompt":"x","at":"soon"}"#,
            r#"{"prompt":"x","at":"2001-01-01 10:00"}"#,
        ] {
            assert!(task_from(&directive(json), &session(), None, NOW).is_err(), "{json}");
        }
    }

    #[test]
    fn clock_times_roll_over_to_tomorrow() {
        let now = Local.timestamp_millis_opt(NOW).single().unwrap();
        let earlier = (now - Duration::minutes(30)).format("%H:%M").to_string();
        let later = (now + Duration::minutes(30)).format("%H:%M").to_string();
        let soon = parse_at(&later, NOW).unwrap();
        assert!(soon > NOW && soon - NOW <= 31 * MINUTE_MS, "{later}");
        let tomorrow = parse_at(&earlier, NOW).unwrap();
        assert!(tomorrow - NOW > 22 * 60 * MINUTE_MS, "{earlier}");
        let exact = (now + Duration::days(2)).format("%Y-%m-%dT%H:%M").to_string();
        assert!(parse_at(&exact, NOW).unwrap() - NOW > 47 * 60 * MINUTE_MS);
    }

    #[test]
    fn wire_prompt_round_trips_for_display() {
        let task = task_from(&directive(r#"{"prompt":"Check the service","every_minutes":60}"#), &session(), None, NOW).unwrap();
        let wire = wire_prompt(&task, NOW + 60 * MINUTE_MS);
        assert!(wire.starts_with("[scheduled task:"));
        assert!(wire.contains("every hour"));
        assert!(wire.contains("run 1 of a repeating task"));
        assert!(wire.ends_with("\n\nCheck the service"));
        assert_eq!(display_text(&wire), ("Check the service".to_string(), true));
        let attached = format!("[voice mode: be brief. Use --- then details.]\n\n{wire}");
        assert_eq!(display_text(&attached), ("Check the service".to_string(), true));
        assert_eq!(display_text("[voice mode: rules]\n\nhello"), ("hello".to_string(), false));
        assert_eq!(display_text("plain [text] here"), ("plain [text] here".to_string(), false));
    }

    #[test]
    fn schedule_runs_and_persists() {
        let path = std::env::temp_dir().join(format!("jarvis-schedule-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut schedule = Schedule::load_from(path.clone());
        let once = task_from(&directive(r#"{"prompt":"once","in_minutes":10}"#), &session(), None, NOW).unwrap();
        let repeat = task_from(&directive(r#"{"prompt":"repeat","every_minutes":15,"times":2}"#), &session(), None, NOW).unwrap();
        let other = SessionRef { id: "ses_2".into(), directory: PathBuf::from("/other") };
        let elsewhere = task_from(&directive(r#"{"prompt":"elsewhere","in_minutes":500}"#), &other, None, NOW).unwrap();
        let once_id = once.id.clone();
        for task in [once, repeat, elsewhere] {
            schedule.add(task).unwrap();
        }
        assert_eq!(schedule.tasks().iter().map(|t| t.prompt.as_str()).collect::<Vec<_>>(), ["once", "repeat", "elsewhere"]);

        assert!(schedule.take_due(NOW + 5 * MINUTE_MS).is_empty());
        // The one-shot fires and is gone; the repeating one fires and moves on.
        let due = schedule.take_due(NOW + 16 * MINUTE_MS);
        assert_eq!(due.iter().map(|t| t.prompt.as_str()).collect::<Vec<_>>(), ["once", "repeat"]);
        assert!(!schedule.cancel(&once_id));
        let left = schedule.tasks();
        assert_eq!(left.len(), 2);
        assert_eq!((left[0].prompt.as_str(), left[0].runs, left[0].next_run), ("repeat", 1, NOW + 31 * MINUTE_MS));

        // Survives a restart.
        let mut reloaded = Schedule::load_from(path.clone());
        assert_eq!(reloaded.tasks(), left);
        // The second run was the last one allowed.
        let due = reloaded.take_due(NOW + 40 * MINUTE_MS);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].runs, 1);
        assert_eq!(reloaded.tasks().len(), 1);

        reloaded.set_title("ses_2", "Other project");
        assert_eq!(reloaded.tasks()[0].session_title.as_deref(), Some("Other project"));
        assert_eq!(reloaded.remove_session("ses_2"), 1);
        assert!(reloaded.tasks().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn repeating_task_expires() {
        let path = std::env::temp_dir().join(format!("jarvis-schedule-expire-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut schedule = Schedule::load_from(path.clone());
        schedule
            .add(task_from(&directive(r#"{"prompt":"x","every_minutes":30,"for_minutes":45}"#), &session(), None, NOW).unwrap())
            .unwrap();
        // After the first run the next one would land past the 45-minute limit.
        assert_eq!(schedule.take_due(NOW + 30 * MINUTE_MS).len(), 1);
        assert!(schedule.tasks().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missed_runs_are_not_fired_in_a_burst() {
        let path = std::env::temp_dir().join(format!("jarvis-schedule-missed-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut schedule = Schedule::load_from(path.clone());
        schedule.add(task_from(&directive(r#"{"prompt":"x","every_minutes":15}"#), &session(), None, NOW).unwrap()).unwrap();
        // The machine slept for five hours.
        let woke = NOW + 300 * MINUTE_MS;
        assert_eq!(schedule.take_due(woke).len(), 1);
        assert!(schedule.take_due(woke + MINUTE_MS).is_empty());
        assert_eq!(schedule.tasks()[0].next_run, woke + 15 * MINUTE_MS);
        let _ = std::fs::remove_file(&path);
    }
}
