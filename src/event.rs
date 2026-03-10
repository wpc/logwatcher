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
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _crossterm_task: tokio::task::JoinHandle<()>,
    _notify_task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(
        tick_rate: std::time::Duration,
        watch_dir: PathBuf,
        glob_pattern: Option<String>,
    ) -> anyhow::Result<Self> {
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
        let tx_fs = tx.clone();
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<NotifyEvent>();

        let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
            if let Ok(event) = res {
                let _ = notify_tx.send(event);
            }
        })?;
        watcher.watch(&watch_dir, RecursiveMode::Recursive)?;

        let notify_task = tokio::spawn(async move {
            let _watcher = watcher; // keep alive
            while let Some(event) = notify_rx.recv().await {
                for path in &event.paths {
                    // Skip directories
                    if path.is_dir() {
                        continue;
                    }

                    // Apply glob filter
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
                        let _ = tx_fs.send(AppEvent::FileChanged(path.clone()));
                    } else if event.kind.is_remove() {
                        let _ = tx_fs.send(AppEvent::FileDeleted(path.clone()));
                    }
                }
            }
        });

        Ok(Self {
            rx,
            _crossterm_task: crossterm_task,
            _notify_task: notify_task,
        })
    }

    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }
}
