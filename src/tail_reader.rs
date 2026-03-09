use std::path::Path;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::fs::File;
use anyhow::Result;

/// Read the last `n` lines of a file.
/// Returns (lines, file_size_after_read).
pub fn read_tail(path: &Path, n: usize) -> Result<(Vec<String>, u64)> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    if size == 0 {
        return Ok((vec![], 0));
    }

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader
        .lines()
        .filter_map(|l| l.ok())
        .collect();

    let start = if all_lines.len() > n {
        all_lines.len() - n
    } else {
        0
    };

    Ok((all_lines[start..].to_vec(), size))
}

/// Read new content appended since `last_size`.
/// Detects truncation: if current size < last_size, re-read tail.
pub fn read_new_content(path: &Path, last_size: u64, max_lines: usize) -> Result<(Vec<String>, u64)> {
    let metadata = std::fs::metadata(path)?;
    let current_size = metadata.len();

    if current_size < last_size {
        // File was truncated (log rotation). Re-read tail.
        return read_tail(path, max_lines);
    }

    if current_size == last_size {
        return Ok((vec![], current_size));
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(last_size))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

    Ok((lines, current_size))
}
