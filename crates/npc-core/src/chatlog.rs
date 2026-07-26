//! Raw chat transcript log: a flat, append-only JSONL history of every chat
//! turn, kept at `{data_dir}/chat-log.jsonl`. This is deliberately separate
//! from npc-memory's embedded/searchable memory store -- that store is for
//! semantic recall, this is just "what was actually said, in order," used
//! e.g. to replay recent conversation in the web UI without touching
//! embeddings at all.
//!
//! One JSON object per line ([`ChatLogEntry`]), oldest entries first in the
//! file. [`append`] adds new turns and quietly caps growth via rotation;
//! [`load_recent`] reads the tail back out, tolerating a corrupt or missing
//! file the same way [`crate::person`]'s store tolerates a corrupt or
//! missing record.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// One turn of chat, serialized as a single line of JSON in the log file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLogEntry {
    /// RFC3339 timestamp of the turn.
    pub time: String,
    /// Who spoke, if known. Omitted from the JSON line entirely when
    /// `None` (rather than written as `null`), so the field can be added
    /// to entries incrementally without changing the shape of older lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub input: String,
    pub output: String,
}

/// Hard cap on how many lines the log is allowed to grow to before
/// [`append`] rotates old entries out. See [`track_append_and_maybe_rotate`]
/// for how the check itself stays cheap.
const MAX_LINES: usize = 5000;

/// `{data_dir}/chat-log.jsonl`.
fn chat_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("chat-log.jsonl")
}

/// Append one entry to `{data_dir}/chat-log.jsonl`, creating `data_dir` and
/// the file itself as needed. Each entry is written as exactly one JSON
/// line. After writing, [`track_append_and_maybe_rotate`] is given a chance
/// to trim the log if it's grown past the configured limit -- see that
/// function's doc comment for why this doesn't mean re-reading the whole
/// file on every call.
pub fn append(data_dir: &Path, entry: &ChatLogEntry) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = chat_log_path(data_dir);

    let line = serde_json::to_string(entry)?;
    {
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{line}")?;
    }

    track_append_and_maybe_rotate(&path, MAX_LINES)?;
    Ok(())
}

/// Load up to `limit` most recent entries from `{data_dir}/chat-log.jsonl`,
/// returned oldest-first (chronological order). A missing file yields an
/// empty list rather than an error. Lines that fail to parse are logged and
/// skipped so one corrupt line doesn't take down the rest of the log --
/// same "best effort" spirit as [`crate::person::list_people`].
pub fn load_recent(data_dir: &Path, limit: usize) -> Vec<ChatLogEntry> {
    let path = chat_log_path(data_dir);
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut entries = Vec::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<ChatLogEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                tracing::warn!(error = %e, "skipping unreadable chat log line");
            }
        }
    }

    if entries.len() > limit {
        entries.split_off(entries.len() - limit)
    } else {
        entries
    }
}

/// Per-path cache of how many lines are currently in a given chat log file,
/// so [`track_append_and_maybe_rotate`] can tell whether rotation is due
/// without reading the file on every [`append`] call.
///
/// Keyed by path rather than kept as a single bare counter because a
/// process can be tracking more than one chat log at once -- the unit
/// tests below each use their own throwaway `data_dir`, and nothing rules
/// out a future multi-tenant deployment doing the same for real. A single
/// `static AtomicUsize` would conflate unrelated logs' line counts.
///
/// This cache is otherwise process-local and best-effort: if a file is
/// ever appended to by something other than [`track_append_and_maybe_rotate`]
/// (as the tests below do deliberately, and as could happen across a
/// process restart), the tracked count can undercount reality until the
/// next rotation check re-measures it from disk. See that function's doc
/// comment for why an undercount only delays a rotation rather than
/// risking one being skipped forever.
static LINE_COUNTS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();

fn line_counts() -> &'static Mutex<HashMap<PathBuf, usize>> {
    LINE_COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Real (uncached) line count of `path`, treating a missing file as empty.
fn count_lines(path: &Path) -> anyhow::Result<usize> {
    match std::fs::read_to_string(path) {
        Ok(data) => Ok(data.lines().count()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

/// Record that one more line was just appended to `path`, and rotate it
/// down to `max_lines` if the tracked line count has grown past
/// `max_lines * 2`.
///
/// This replaces an earlier version that gated the (expensive) real line
/// count behind a *byte size* heuristic: since a real JSONL chat line is
/// comfortably larger than the conservative per-line byte estimate that
/// heuristic used (more so with non-ASCII text), the size gate would swing
/// open long before the file actually reached `max_lines * 2` lines --
/// after which *every single append* re-read and re-counted the whole file
/// just to find there was still nothing to do, for potentially thousands of
/// turns in a row.
///
/// Instead of estimating from size, we track the real line count directly
/// in memory (see [`LINE_COUNTS`]) and only touch the filesystem for an
/// actual count in two cases: the first time a given path is seen in this
/// process (nothing cached yet, so it's measured once and cached), and
/// when the tracked count crosses `max_lines * 2` -- at which point
/// [`maybe_rotate`] was going to read the file anyway to rotate it, so the
/// read isn't wasted. Every other call is a single hash-map lookup and
/// increment, no I/O at all.
///
/// Because [`maybe_rotate`] always re-derives the real line count from disk
/// whenever it's invoked (never trusting the cache for the rotation
/// decision itself), a stale/undercounted cache entry can only *delay* a
/// rotation check -- never cause one to be silently skipped forever, since
/// the tracked count keeps climbing by one on every subsequent append and
/// will eventually cross the threshold and trigger a fresh, accurate read.
fn track_append_and_maybe_rotate(path: &Path, max_lines: usize) -> anyhow::Result<()> {
    let mut counts = line_counts().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    // `path` was just appended to by the caller before this runs, so a
    // fresh (uncached) measurement already reflects that new line -- only
    // the cached-hit path needs the explicit `+ 1`.
    let count = match counts.get(path) {
        Some(&cached) => cached + 1,
        None => count_lines(path)?,
    };

    if count > max_lines * 2 {
        let actual = maybe_rotate(path, max_lines)?;
        counts.insert(path.to_path_buf(), actual);
    } else {
        counts.insert(path.to_path_buf(), count);
    }

    Ok(())
}

/// Rotate `path` down to the most recent `max_lines` lines, but only if
/// it's actually over `max_lines * 2` lines. Returns the file's resulting
/// line count either way (used by [`track_append_and_maybe_rotate`] to
/// re-sync its cache with reality).
///
/// This always does a real read of the whole file -- callers are expected
/// to only invoke it when a rotation is actually plausible (see
/// [`track_append_and_maybe_rotate`]), not on every append.
fn maybe_rotate(path: &Path, max_lines: usize) -> anyhow::Result<usize> {
    let data = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = data.lines().collect();
    if lines.len() <= max_lines * 2 {
        return Ok(lines.len());
    }

    let kept = &lines[lines.len() - max_lines..];
    let mut out = kept.join("\n");
    out.push('\n');

    let tmp_path = tmp_path_for(path);
    std::fs::write(&tmp_path, out)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(max_lines)
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "chat-log.jsonl".to_string());
    name.push_str(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("npc-core-chatlog-test-{}", uuid::Uuid::new_v4()))
    }

    fn entry(input: &str, output: &str) -> ChatLogEntry {
        ChatLogEntry {
            time: chrono::Utc::now().to_rfc3339(),
            speaker: None,
            input: input.to_string(),
            output: output.to_string(),
        }
    }

    #[test]
    fn append_and_load_round_trip() {
        let dir = temp_data_dir();
        let mut e = entry("こんにちは", "やあ");
        e.speaker = Some("太郎".to_string());
        append(&dir, &e).unwrap();

        let loaded = load_recent(&dir, 10);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].input, "こんにちは");
        assert_eq!(loaded[0].output, "やあ");
        assert_eq!(loaded[0].speaker.as_deref(), Some("太郎"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_recent_truncates_to_the_tail() {
        let dir = temp_data_dir();
        for i in 0..5 {
            append(&dir, &entry(&format!("in{i}"), &format!("out{i}"))).unwrap();
        }

        let loaded = load_recent(&dir, 2);
        assert_eq!(loaded.len(), 2);
        // Oldest-first ordering preserved, and it's the *last* two entries.
        assert_eq!(loaded[0].input, "in3");
        assert_eq!(loaded[1].input, "in4");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_log_is_empty() {
        let dir = temp_data_dir();
        let loaded = load_recent(&dir, 10);
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_recent_skips_corrupt_lines_and_keeps_the_rest() {
        let dir = temp_data_dir();
        append(&dir, &entry("good1", "ok1")).unwrap();

        // Append a corrupt line directly, bypassing `append`'s JSON encoding.
        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(chat_log_path(&dir))
                .unwrap();
            writeln!(file, "{{ not json").unwrap();
        }

        append(&dir, &entry("good2", "ok2")).unwrap();

        let loaded = load_recent(&dir, 10);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].input, "good1");
        assert_eq!(loaded[1].input, "good2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_trims_to_the_most_recent_max_lines() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = chat_log_path(&dir);

        // Write 10 lines directly (cheaper than driving `append` in a
        // loop) then rotate with a small `max_lines` to exercise the same
        // trimming code path `append` uses in production, without needing
        // thousands of writes to cross the real MAX_LINES * 2 threshold.
        {
            let mut file = OpenOptions::new().create(true).append(true).open(&path).unwrap();
            for i in 0..10 {
                let line = serde_json::to_string(&entry(&format!("in{i}"), &format!("out{i}"))).unwrap();
                writeln!(file, "{line}").unwrap();
            }
        }

        maybe_rotate(&path, 3).unwrap();

        let loaded = load_recent(&dir, 100);
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].input, "in7");
        assert_eq!(loaded[1].input, "in8");
        assert_eq!(loaded[2].input, "in9");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_is_a_no_op_below_the_threshold() {
        let dir = temp_data_dir();
        append(&dir, &entry("only", "one")).unwrap();
        let path = chat_log_path(&dir);

        // max_lines = 5000 in production; a single tiny entry never gets
        // near the real line-count threshold.
        maybe_rotate(&path, MAX_LINES).unwrap();

        let loaded = load_recent(&dir, 10);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].input, "only");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracked_append_rotates_once_the_counted_threshold_is_crossed() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = chat_log_path(&dir);
        let max_lines = 3;

        // Drive the exact same per-append sequence `append()` uses --
        // write one line, then let the counting gate decide whether to
        // rotate -- with a small `max_lines` so crossing `max_lines * 2`
        // takes 7 lines instead of thousands.
        for i in 0..7 {
            let line = serde_json::to_string(&entry(&format!("in{i}"), &format!("out{i}"))).unwrap();
            {
                let mut file = OpenOptions::new().create(true).append(true).open(&path).unwrap();
                writeln!(file, "{line}").unwrap();
            }
            track_append_and_maybe_rotate(&path, max_lines).unwrap();
        }

        // The 7th line pushes the tracked count to 7 (> max_lines * 2 == 6),
        // so rotation should have fired exactly once, trimming to the last
        // `max_lines` entries.
        let loaded = load_recent(&dir, 100);
        assert_eq!(loaded.len(), max_lines);
        assert_eq!(loaded[0].input, "in4");
        assert_eq!(loaded[1].input, "in5");
        assert_eq!(loaded[2].input, "in6");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gate_does_not_reread_the_file_once_the_cache_is_warm() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = chat_log_path(&dir);

        // Write one line and let the gate seed its in-memory count for this
        // path from disk (the one-time "first time we see this path" read).
        {
            let mut file = OpenOptions::new().create(true).append(true).open(&path).unwrap();
            writeln!(file, "{}", serde_json::to_string(&entry("in0", "out0")).unwrap()).unwrap();
        }
        track_append_and_maybe_rotate(&path, 5).unwrap();

        // Corrupt the file on disk with invalid UTF-8, bypassing the cache
        // entirely. `max_lines = 5` means the rotation threshold is 10
        // lines, so the tracked count (now 1) is nowhere near it.
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(&[0xFF, 0xFE, b'\n']).unwrap();
        }

        // If this call re-read the file (e.g. to re-measure or re-verify
        // the count) it would hit invalid UTF-8 and return an error. It
        // succeeding proves the warm cache was used instead of the disk.
        track_append_and_maybe_rotate(&path, 5).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
