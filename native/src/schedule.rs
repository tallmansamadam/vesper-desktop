//! Always-on schedules, backed by the Windows Task Scheduler.
//!
//! Each schedule is a real OS task (`schtasks`) that runs `vesper.exe --apply
//! "<scene>"` at a time of day, so it fires even when the app is closed (while
//! the user is logged in - user-context tasks need no elevation). The task
//! metadata is also mirrored to schedules.json so the app can list and manage
//! them without parsing schtasks output.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

pub const DAY_CODES: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

pub struct Schedule {
    pub id: String,
    pub scene: String,
    pub hour: u32,
    pub minute: u32,
    pub days: [bool; 7], // Mon..Sun
}

impl Schedule {
    pub fn task_name(&self) -> String {
        format!("Vesper_{}", self.id)
    }

    pub fn time_str(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    pub fn days_label(&self) -> String {
        if self.days.iter().all(|&d| d) || self.days.iter().all(|&d| !d) {
            return "Every day".into();
        }
        DAY_CODES
            .iter()
            .zip(self.days)
            .filter(|(_, on)| *on)
            .map(|(c, _)| *c)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn load(path: &Path) -> Vec<Schedule> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    for it in arr {
        let mut days = [false; 7];
        if let Some(a) = it.get("days").and_then(Value::as_array) {
            for (i, d) in a.iter().take(7).enumerate() {
                days[i] = d.as_bool().unwrap_or(false);
            }
        }
        out.push(Schedule {
            id: it.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            scene: it.get("scene").and_then(Value::as_str).unwrap_or("").to_string(),
            hour: it.get("hour").and_then(Value::as_u64).unwrap_or(7) as u32,
            minute: it.get("minute").and_then(Value::as_u64).unwrap_or(0) as u32,
            days,
        });
    }
    out.retain(|s| !s.id.is_empty() && !s.scene.is_empty());
    out
}

pub fn save(path: &Path, list: &[Schedule]) {
    let arr: Vec<Value> = list
        .iter()
        .map(|s| {
            json!({
                "id": s.id, "scene": s.scene, "hour": s.hour, "minute": s.minute,
                "days": s.days.to_vec(),
            })
        })
        .collect();
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default(),
    );
}

fn run(mut cmd: Command) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW - no console flash
    }
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr);
            let msg = if msg.trim().is_empty() {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            } else {
                msg.trim().to_string()
            };
            Err(if msg.is_empty() { "schtasks failed".into() } else { msg })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Register (or replace) the OS task for a schedule.
pub fn create_task(exe: &Path, s: &Schedule) -> Result<(), String> {
    let tr = format!("\"{}\" --apply \"{}\"", exe.display(), s.scene);
    let mut cmd = Command::new("schtasks");
    cmd.args(["/create", "/tn", &s.task_name(), "/tr", &tr, "/f", "/st", &s.time_str()]);
    if s.days.iter().all(|&d| d) || s.days.iter().all(|&d| !d) {
        cmd.args(["/sc", "DAILY"]);
    } else {
        let d = DAY_CODES
            .iter()
            .zip(s.days)
            .filter(|(_, on)| *on)
            .map(|(c, _)| *c)
            .collect::<Vec<_>>()
            .join(",");
        cmd.args(["/sc", "WEEKLY", "/d", &d]);
    }
    run(cmd)
}

pub fn delete_task(id: &str) -> Result<(), String> {
    let mut cmd = Command::new("schtasks");
    cmd.args(["/delete", "/tn", &format!("Vesper_{id}"), "/f"]);
    run(cmd)
}
