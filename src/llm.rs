use crate::event::AppEvent;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Spawn a background task to summarize a process command via an LLM API.
/// On success, sends `ProcessSummaryReady` through `tx`.
/// On any error, silently does nothing (panel falls back to raw cmd).
pub fn spawn_summary(
    tx: mpsc::UnboundedSender<AppEvent>,
    path: PathBuf,
    cmd: String,
    api_url: String,
    log_file: Option<PathBuf>,
) {
    tokio::spawn(async move {
        let summary = match call_llm(&cmd, &api_url, log_file.as_deref()).await {
            Ok(s) => s,
            Err(e) => format!("[LLM error: {}]", e),
        };
        let _ = tx.send(AppEvent::ProcessSummaryReady { path, summary });
    });
}

/// Detect file paths in command args and read the first one found (up to 10KB).
fn read_script_content(cmd: &str) -> Option<(String, String)> {
    for arg in cmd.split_whitespace() {
        let p = std::path::Path::new(arg);
        if p.is_absolute() && p.is_file() {
            if let Ok(content) = std::fs::read_to_string(p) {
                let truncated = if content.len() > 10_000 {
                    format!("{}...(truncated)", &content[..10_000])
                } else {
                    content
                };
                return Some((arg.to_string(), truncated));
            }
        }
    }
    None
}

fn append_log(log_file: &std::path::Path, entry: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
    {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] {}", timestamp, entry);
    }
}

async fn call_llm(
    cmd: &str,
    api_url: &str,
    log_file: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    let mut user_message = format!("Command line: {}", cmd);

    if let Some((path, content)) = read_script_content(cmd) {
        user_message.push_str(&format!("\n\nScript file ({}):\n{}", path, content));
    }

    let body = serde_json::json!({
        "messages": [
            {
                "role": "system",
                "content": "You are a Unix process identifier. Given a command line (and optionally the script it runs), reply with ONLY a single short summary (under 100 chars) of what the process does. No explanation, no quotes, no prefixes."
            },
            {
                "role": "user",
                "content": user_message
            }
        ],
        "max_tokens": 16384
    });

    if let Some(lf) = log_file {
        append_log(lf, &format!("REQUEST to {}\n{}", api_url, serde_json::to_string_pretty(&body).unwrap_or_default()));
    }

    // Extract hostname from URL to bypass proxy for internal endpoints
    let no_proxy = reqwest::Url::parse(api_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()?;

    // Set NO_PROXY for this specific host
    std::env::set_var("NO_PROXY", &no_proxy);

    let resp = client
        .post(api_url)
        .json(&body)
        .send()
        .await?;

    let json: serde_json::Value = resp.json().await?;

    if let Some(lf) = log_file {
        append_log(lf, &format!("RESPONSE\n{}", serde_json::to_string_pretty(&json).unwrap_or_default()));
    }

    let choice = &json["choices"][0]["message"];

    // Try content first, fall back to reasoning_content (for models like Kimi)
    let summary = choice["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| choice["reasoning_content"].as_str())
        .ok_or_else(|| anyhow::anyhow!("unexpected LLM response format"))?
        .trim()
        .to_string();

    if let Some(lf) = log_file {
        append_log(lf, &format!("SUMMARY: {}", summary));
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_script_content_nonexistent_path() {
        let result = read_script_content("node /nonexistent/path/file.js");
        assert!(result.is_none());
    }

    #[test]
    fn read_script_content_no_absolute_paths() {
        let result = read_script_content("echo hello world");
        assert!(result.is_none());
    }

    #[test]
    fn read_script_content_finds_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, b"#!/bin/bash\necho hello").unwrap();
        let cmd = format!("bash {}", tmp.path().display());
        let result = read_script_content(&cmd);
        assert!(result.is_some());
        let (path, content) = result.unwrap();
        assert_eq!(path, tmp.path().display().to_string());
        assert!(content.contains("echo hello"));
    }

    #[test]
    fn read_script_content_truncates_large_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let large_content = "x".repeat(20_000);
        std::io::Write::write_all(&mut tmp, large_content.as_bytes()).unwrap();
        let cmd = format!("bash {}", tmp.path().display());
        let result = read_script_content(&cmd);
        assert!(result.is_some());
        let (_, content) = result.unwrap();
        assert!(content.ends_with("...(truncated)"));
        assert!(content.len() < 20_000);
    }
}
