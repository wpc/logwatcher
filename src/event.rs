use crossterm::event::{EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use notify::{Event as NotifyEvent, RecursiveMode, Watcher};
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Resize,
    Tick,
    FileChanged(PathBuf),
    FileDeleted(PathBuf),
    ProcessSummaryReady { path: PathBuf, summary: String },
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _crossterm_task: tokio::task::JoinHandle<()>,
    _notify_task: tokio::task::JoinHandle<()>,
}

/// Process a single notify filesystem event: apply glob filter, dispatch to channel.
fn process_notify_event(
    event: &NotifyEvent,
    glob_pattern: &Option<String>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    for path in &event.paths {
        if path.is_dir() {
            continue;
        }
        if let Some(ref pattern) = glob_pattern {
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if !glob_match::glob_match(pattern, filename) {
                    continue;
                }
            } else {
                continue;
            }
        }
        if event.kind.is_create() || event.kind.is_modify() {
            let _ = tx.send(AppEvent::FileChanged(path.clone()));
        } else if event.kind.is_remove() {
            let _ = tx.send(AppEvent::FileDeleted(path.clone()));
        }
    }
}

/// Spawn a tokio task that bridges notify filesystem events to the AppEvent channel.
/// Returns (task_handle, watcher_guard) — the watcher is kept alive inside the task.
fn spawn_notify_task(
    watch_dir: &std::path::Path,
    glob_pattern: Option<String>,
    tx: mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<NotifyEvent>();

    let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
        if let Ok(event) = res {
            let _ = notify_tx.send(event);
        }
    })?;
    watcher.watch(watch_dir, RecursiveMode::Recursive)?;

    let notify_task = tokio::spawn(async move {
        let _watcher = watcher; // keep alive
        while let Some(event) = notify_rx.recv().await {
            process_notify_event(&event, &glob_pattern, &tx);
        }
    });

    Ok(notify_task)
}

impl EventHandler {
    pub fn new(
        tick_rate: std::time::Duration,
        watch_dir: PathBuf,
        glob_pattern: Option<String>,
    ) -> anyhow::Result<(Self, mpsc::UnboundedSender<AppEvent>)> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Crossterm events (keyboard + resize + tick)
        let tx_ct = tx.clone();
        let crossterm_task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_rate);
            loop {
                let crossterm_event = reader.next();
                let tick = tick_interval.tick();
                tokio::select! {
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(crossterm::event::Event::Key(key))) => {
                                if key.kind == KeyEventKind::Press {
                                    let _ = tx_ct.send(AppEvent::Key(key));
                                }
                            }
                            Some(Ok(crossterm::event::Event::Resize(_, _))) => {
                                let _ = tx_ct.send(AppEvent::Resize);
                            }
                            Some(Err(_)) => {}
                            None => break,
                            _ => {}
                        }
                    }
                    _ = tick => {
                        let _ = tx_ct.send(AppEvent::Tick);
                    }
                }
            }
        });

        // File system notify events
        let notify_task = spawn_notify_task(&watch_dir, glob_pattern, tx.clone())?;

        Ok((Self {
            rx,
            _crossterm_task: crossterm_task,
            _notify_task: notify_task,
        }, tx))
    }

    /// Create an EventHandler for testing (no crossterm/terminal needed).
    /// Returns the handler and a sender for injecting events manually.
    #[cfg(test)]
    fn new_for_test(
        watch_dir: PathBuf,
        glob_pattern: Option<String>,
    ) -> anyhow::Result<(Self, mpsc::UnboundedSender<AppEvent>)> {
        let (tx, rx) = mpsc::unbounded_channel();

        let notify_task = spawn_notify_task(&watch_dir, glob_pattern, tx.clone())?;
        let dummy_crossterm = tokio::spawn(async {});

        Ok((
            Self {
                rx,
                _crossterm_task: dummy_crossterm,
                _notify_task: notify_task,
            },
            tx,
        ))
    }

    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_create_event(path: PathBuf) -> NotifyEvent {
        NotifyEvent {
            kind: notify::EventKind::Create(notify::event::CreateKind::File),
            paths: vec![path],
            attrs: Default::default(),
        }
    }

    fn make_modify_event(path: PathBuf) -> NotifyEvent {
        NotifyEvent {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![path],
            attrs: Default::default(),
        }
    }

    fn make_remove_event(path: PathBuf) -> NotifyEvent {
        NotifyEvent {
            kind: notify::EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![path],
            attrs: Default::default(),
        }
    }

    #[test]
    fn process_create_event_sends_file_changed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let event = make_create_event(PathBuf::from("/tmp/test.txt"));
        process_notify_event(&event, &None, &tx);

        let received = rx.try_recv().unwrap();
        assert!(matches!(received, AppEvent::FileChanged(p) if p == PathBuf::from("/tmp/test.txt")));
    }

    #[test]
    fn process_modify_event_sends_file_changed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let event = make_modify_event(PathBuf::from("/tmp/test.txt"));
        process_notify_event(&event, &None, &tx);

        let received = rx.try_recv().unwrap();
        assert!(matches!(received, AppEvent::FileChanged(_)));
    }

    #[test]
    fn process_remove_event_sends_file_deleted() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let event = make_remove_event(PathBuf::from("/tmp/test.txt"));
        process_notify_event(&event, &None, &tx);

        let received = rx.try_recv().unwrap();
        assert!(matches!(received, AppEvent::FileDeleted(_)));
    }

    #[test]
    fn process_event_skips_directories() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // Use /tmp which is a directory
        let event = make_create_event(PathBuf::from("/tmp"));
        process_notify_event(&event, &None, &tx);

        assert!(rx.try_recv().is_err()); // nothing sent
    }

    #[test]
    fn process_event_glob_filters_non_matching() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let event = make_create_event(PathBuf::from("/tmp/test.txt"));
        process_notify_event(&event, &Some("*.log".to_string()), &tx);

        assert!(rx.try_recv().is_err()); // filtered out
    }

    #[test]
    fn process_event_glob_allows_matching() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let event = make_create_event(PathBuf::from("/tmp/test.log"));
        process_notify_event(&event, &Some("*.log".to_string()), &tx);

        let received = rx.try_recv().unwrap();
        assert!(matches!(received, AppEvent::FileChanged(_)));
    }

    #[test]
    fn process_event_multiple_paths() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let event = NotifyEvent {
            kind: notify::EventKind::Create(notify::event::CreateKind::File),
            paths: vec![
                PathBuf::from("/tmp/a.txt"),
                PathBuf::from("/tmp/b.txt"),
            ],
            attrs: Default::default(),
        };
        process_notify_event(&event, &None, &tx);

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err()); // only 2
    }

    #[tokio::test]
    async fn event_handler_integration() {
        use tempfile::TempDir;
        use std::io::Write;

        let tmp = TempDir::new().unwrap();

        let (mut handler, _tx) = EventHandler::new_for_test(
            tmp.path().to_path_buf(),
            None,
        ).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut f = std::fs::File::create(tmp.path().join("test.txt")).unwrap();
        f.write_all(b"hello\n").unwrap();
        f.flush().unwrap();

        let mut got_file_event = false;
        for _ in 0..20 {
            if let Ok(event) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                handler.next(),
            ).await {
                if matches!(event, Some(AppEvent::FileChanged(_))) {
                    got_file_event = true;
                    break;
                }
            }
        }
        assert!(got_file_event);
    }

    #[tokio::test]
    async fn event_handler_with_glob_filter() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();

        let (mut handler, tx) = EventHandler::new_for_test(
            tmp.path().to_path_buf(),
            Some("*.log".to_string()),
        ).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Create a non-matching file — should be filtered out
        std::fs::write(tmp.path().join("test.txt"), "hello\n").unwrap();

        // Inject a manual tick so we have something to receive
        let _ = tx.send(AppEvent::Tick);

        let event = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            handler.next(),
        ).await.unwrap();

        // Should get Tick, not FileChanged (the .txt file was filtered)
        assert!(matches!(event, Some(AppEvent::Tick)));
    }
}
