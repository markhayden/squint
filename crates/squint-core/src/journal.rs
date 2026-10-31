//! The append-only run journal: one JSON object per line, fsynced per event, written from the
//! first event so a failure at any step leaves a durable record. Artifacts an event references are
//! written first (see `artifacts`). `read` turns a journal, complete or partial, back into events.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::events::Event;
use crate::{Error, Result};

/// Called after each line is written, with the run directory and the event's name.
pub type Observer = std::sync::Arc<dyn Fn(&Path, &str) + Send + Sync>;

/// A run journal under `<run dir>/journal.jsonl`.
pub struct Journal {
    file: File,
    path: PathBuf,
    seq: u64,
    last_event: Option<&'static str>,
    fail_at: Option<u64>,
    observer: Option<Observer>,
}

/// One journal line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    pub unix_ms: u64,
    #[serde(flatten)]
    pub event: Event,
}

impl Journal {
    /// Creates `journal.jsonl` in `run_dir` (which must exist). Fails if it already exists.
    pub fn create(run_dir: &Path) -> Result<Journal> {
        let path = run_dir.join("journal.jsonl");
        let mut options = OpenOptions::new();
        options.create_new(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Journal {
            file,
            path,
            seq: 0,
            last_event: None,
            fail_at: None,
            observer: None,
        })
    }

    /// Test knob: the append at this sequence number fails as if the process had died between an
    /// artifact write and its event.
    pub fn fail_at(&mut self, seq: u64) {
        self.fail_at = Some(seq);
    }

    /// The last event appended: the state a run reached.
    pub fn last_event(&self) -> Option<&'static str> {
        self.last_event
    }

    /// Appends one event and flushes it to disk.
    pub fn append(&mut self, event: &Event) -> Result<()> {
        if self.fail_at == Some(self.seq) {
            return Err(std::io::Error::other(format!(
                "injected journal fault at seq {}",
                self.seq
            ))
            .into());
        }
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let line = Line {
            seq: self.seq,
            unix_ms,
            event: event.clone(),
        };
        let mut text = serde_json::to_string(&line)?;
        text.push('\n');
        self.file.write_all(text.as_bytes())?;
        self.file.sync_data()?;
        self.seq += 1;
        self.last_event = Some(event.name());
        if let (Some(observer), Some(run_dir)) = (&self.observer, self.path.parent()) {
            observer(run_dir, event.name());
        }
        Ok(())
    }

    /// Calls `observer` after every line this journal writes from now on.
    pub fn observe(&mut self, observer: Observer) {
        self.observer = Some(observer);
    }

    /// Reads a journal. A torn final line (the process died mid-write) is dropped; any other
    /// malformed line is an error.
    pub fn read(path: &Path) -> Result<Vec<Line>> {
        let text = std::fs::read_to_string(path)?;
        let raw: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut lines = Vec::with_capacity(raw.len());
        for (i, l) in raw.iter().enumerate() {
            match serde_json::from_str::<Line>(l) {
                Ok(line) => lines.push(line),
                Err(e) if i + 1 == raw.len() => {
                    let _ = e;
                    break;
                }
                Err(e) => {
                    return Err(Error::Io(std::io::Error::other(format!(
                        "{}:{}: malformed journal line: {e}",
                        path.display(),
                        i + 1
                    ))));
                }
            }
        }
        Ok(lines)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of events appended so far.
    pub fn len(&self) -> u64 {
        self.seq
    }

    pub fn is_empty(&self) -> bool {
        self.seq == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::Verdict;
    use crate::events::{Prepared, VerdictReached};

    #[test]
    fn appends_numbered_lines_in_order_and_reads_them_back() {
        let dir = std::env::temp_dir().join(format!("squint-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        let mut j = Journal::create(&dir).expect("create");
        j.append(&Event::Prepared(Prepared {
            prep_version: "canonical".into(),
            images: 2,
        }))
        .expect("append");
        j.append(&Event::Verdict(VerdictReached {
            verdict: Verdict::Pass,
        }))
        .expect("append");
        assert_eq!(j.last_event(), Some("verdict"));
        let text = std::fs::read_to_string(j.path()).expect("read");
        let raw: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        assert_eq!(raw[0]["seq"], 0);
        assert_eq!(raw[0]["event"], "prepared");
        assert_eq!(raw[1]["data"]["verdict"], "pass");
        let lines = Journal::read(j.path()).expect("read back");
        assert_eq!(lines.len(), 2);
        assert!(matches!(lines[1].event, Event::Verdict(_)));
        assert!(
            Journal::create(&dir).is_err(),
            "a second journal in the same run dir is refused"
        );
        // A torn final line is dropped.
        std::fs::OpenOptions::new()
            .append(true)
            .open(j.path())
            .expect("open")
            .write_all(b"{\"seq\":2,\"unix_ms\":1,\"event\":\"err")
            .expect("write");
        assert_eq!(Journal::read(j.path()).expect("tolerant").len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
