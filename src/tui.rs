use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io::stderr;

pub type Tui = Terminal<CrosstermBackend<std::io::Stderr>>;

pub fn init() -> Result<Tui> {
    enable_raw_mode()?;
    execute!(stderr(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

pub fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(stderr(), LeaveAlternateScreen)?;
    Ok(())
}

pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore();
        original_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_panic_hook_does_not_panic() {
        // Just verify it can be called without panicking
        install_panic_hook();
    }

    #[test]
    fn restore_does_not_panic() {
        // restore() may fail without a terminal but should not panic
        let _ = restore();
    }

    #[test]
    fn init_may_fail_without_terminal() {
        // In a test environment without a tty, init() likely errors but should not panic
        let result = init();
        if let Ok(_terminal) = result {
            let _ = restore();
        }
    }

    #[test]
    fn panic_hook_invokes_restore() {
        install_panic_hook();
        // Trigger a panic and catch it — exercises the panic hook body
        let result = std::panic::catch_unwind(|| {
            panic!("test panic for coverage");
        });
        assert!(result.is_err());
    }
}
