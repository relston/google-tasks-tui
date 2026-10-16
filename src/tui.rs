use std::{
    io,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use ratatui::{
    crossterm::{
        event::{
            self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags,
        },
        execute, terminal,
    },
    DefaultTerminal,
};

use crate::api::Client;
use crate::auth;
use crate::cache;
use crate::config;
use crate::paths::Paths;

mod app;
mod board;
mod render;
mod text;
mod worker;

use app::{App, QuitState};
use worker::{worker, Msg};

const THEME_POLL: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(100);

/// Returns whether shutdown should join the worker after draining writes.
fn run(term: &mut DefaultTerminal, mut app: App, msgs: Receiver<Msg>) -> std::io::Result<bool> {
    let mut last_poll = Instant::now();
    while !app.should_exit() {
        term.draw(|f| app.draw(f))?;
        // Block on input, but wake often enough to pick up API results.
        if event::poll(TICK)? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.on_key(k);
                }
            }
        }
        loop {
            match msgs.try_recv() {
                Ok(msg) => app.on_msg(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.worker_disconnected();
                    break;
                }
            }
        }
        if last_poll.elapsed() >= THEME_POLL {
            app.check_theme();
            last_poll = Instant::now();
        }
    }
    Ok(app.quit == QuitState::Drained)
}

struct TerminalGuard<T> {
    terminal: T,
    detailed_keys: bool,
    restore: fn(bool),
}

impl<T> TerminalGuard<T> {
    #[cfg(test)]
    fn with_restore(terminal: T, detailed_keys: bool, restore: fn(bool)) -> Self {
        Self {
            terminal,
            detailed_keys,
            restore,
        }
    }

    fn terminal_mut(&mut self) -> &mut T {
        &mut self.terminal
    }
}

impl TerminalGuard<DefaultTerminal> {
    fn init() -> Self {
        let terminal = ratatui::init();
        let mut guard = Self {
            terminal,
            detailed_keys: false,
            restore: restore_terminal,
        };
        guard.detailed_keys = enable_key_detail();
        guard
    }
}

impl<T> Drop for TerminalGuard<T> {
    fn drop(&mut self) {
        (self.restore)(self.detailed_keys);
    }
}

fn restore_terminal(detailed_keys: bool) {
    if detailed_keys {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
}

/// Ask the terminal to report Shift+Enter and friends (kitty keyboard
/// protocol). Returns whether the flags were pushed, so they can be popped.
fn enable_key_detail() -> bool {
    if !terminal::supports_keyboard_enhancement().unwrap_or(false) {
        return false;
    }
    execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

#[derive(Debug)]
pub enum Error {
    Auth(auth::Error),
    Terminal(io::Error),
    WorkerPanicked,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(error) => write!(f, "{error}"),
            Self::Terminal(error) => write!(f, "{error}"),
            Self::WorkerPanicked => f.write_str("network worker panicked"),
        }
    }
}

impl std::error::Error for Error {}

impl From<auth::Error> for Error {
    fn from(error: auth::Error) -> Self {
        Self::Auth(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Terminal(error)
    }
}

pub fn start(paths: &Paths) -> Result<(), Error> {
    // Reuse the stored token so nothing blocks the first frame; only a first
    // run (or a wiped token) needs the browser, which wants the terminal.
    let auth = auth::Auth::load(paths)?;
    let account = auth.fingerprint();
    let (cached, cache_meta, warning) = match cache::load_for(paths, &account) {
        Ok((cached, meta)) => (cached, meta, None),
        Err(error) => (
            cache::Cache::default(),
            cache::Meta {
                account,
                generation: 0,
            },
            Some(format!("cache ignored: {error}")),
        ),
    };
    let (config, config_warning) = config::load(paths);
    let warning = match (warning, config_warning) {
        (Some(cache), Some(config)) => Some(format!("{cache}; {config}")),
        (warning, None) | (None, warning) => warning,
    };
    let client = Client::new(auth);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (msg_tx, msg_rx) = mpsc::channel();
    let worker_handle = thread::spawn(move || worker(client, cmd_rx, msg_tx));

    let mut terminal = TerminalGuard::init();
    let res = run(
        terminal.terminal_mut(),
        App::new_with_storage(cmd_tx, cached, config, paths.clone(), cache_meta, warning),
        msg_rx,
    );
    drop(terminal);
    let join_worker = res.as_ref().copied().unwrap_or(true);
    if join_worker && worker_handle.join().is_err() && res.is_ok() {
        return Err(Error::WorkerPanicked);
    }
    res?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static TERMINAL_RESTORED: AtomicBool = AtomicBool::new(false);

    fn record_terminal_restore(detailed_keys: bool) {
        TERMINAL_RESTORED.store(detailed_keys, Ordering::SeqCst);
    }

    #[test]
    fn terminal_guard_restores_state_during_panic_unwinding() {
        TERMINAL_RESTORED.store(false, Ordering::SeqCst);

        let result = std::panic::catch_unwind(|| {
            let _guard = TerminalGuard::with_restore((), true, record_terminal_restore);
            panic!("exercise terminal cleanup");
        });

        assert!(result.is_err());
        assert!(TERMINAL_RESTORED.load(Ordering::SeqCst));
    }
}
