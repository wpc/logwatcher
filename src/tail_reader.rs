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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn read_tail_returns_last_n_lines() {
        let mut f = NamedTempFile::new().unwrap();
        for i in 1..=10 {
            writeln!(f, "line {}", i).unwrap();
        }
        f.flush().unwrap();

        let (lines, size) = read_tail(f.path(), 3).unwrap();
        assert_eq!(lines, vec!["line 8", "line 9", "line 10"]);
        assert!(size > 0);
    }

    #[test]
    fn read_tail_returns_all_when_fewer_than_n() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "only").unwrap();
        writeln!(f, "two").unwrap();
        f.flush().unwrap();

        let (lines, _) = read_tail(f.path(), 50).unwrap();
        assert_eq!(lines, vec!["only", "two"]);
    }

    #[test]
    fn read_tail_empty_file() {
        let f = NamedTempFile::new().unwrap();
        let (lines, size) = read_tail(f.path(), 10).unwrap();
        assert!(lines.is_empty());
        assert_eq!(size, 0);
    }

    #[test]
    fn read_new_content_appended() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello\n").unwrap();
        f.flush().unwrap();
        let initial_size = std::fs::metadata(f.path()).unwrap().len();

        write!(f, "world\n").unwrap();
        f.flush().unwrap();

        let (lines, new_size) = read_new_content(f.path(), initial_size, 50).unwrap();
        assert_eq!(lines, vec!["world"]);
        assert!(new_size > initial_size);
    }

    #[test]
    fn read_new_content_no_change() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello\n").unwrap();
        f.flush().unwrap();
        let size = std::fs::metadata(f.path()).unwrap().len();

        let (lines, returned_size) = read_new_content(f.path(), size, 50).unwrap();
        assert!(lines.is_empty());
        assert_eq!(returned_size, size);
    }

    #[test]
    fn read_new_content_detects_truncation() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "long content here\nand more\n").unwrap();
        f.flush().unwrap();
        let old_size = std::fs::metadata(f.path()).unwrap().len();

        // Truncate and write shorter content
        let path = f.path().to_path_buf();
        std::fs::write(&path, "short\n").unwrap();

        let (lines, new_size) = read_new_content(&path, old_size, 50).unwrap();
        assert_eq!(lines, vec!["short"]);
        assert!(new_size < old_size);
    }

    #[test]
    fn read_tail_single_line_no_newline() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "no trailing newline").unwrap();
        f.flush().unwrap();

        let (lines, size) = read_tail(f.path(), 10).unwrap();
        assert_eq!(lines, vec!["no trailing newline"]);
        assert!(size > 0);
    }

    #[test]
    fn read_new_content_multiple_lines() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "line1\n").unwrap();
        f.flush().unwrap();
        let initial_size = std::fs::metadata(f.path()).unwrap().len();

        write!(f, "line2\nline3\nline4\n").unwrap();
        f.flush().unwrap();

        let (lines, _) = read_new_content(f.path(), initial_size, 50).unwrap();
        assert_eq!(lines, vec!["line2", "line3", "line4"]);
    }
}

