//! OpenSCAD TUI - Terminal User Interface for OpenSCAD
//!
//! A command-driven OpenSCAD editor with real-time preview

mod app;
mod commands;
mod input;
mod ui;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::error::Error;
use std::io;

use app::App;
use input::handle_key;
use ui::draw;

fn main() -> Result<(), Box<dyn Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let app = App::new();
    let res = run_app(&mut terminal, app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    loop {
        terminal.draw(|f| {
            draw(f, &app);
        })?;

        if app.should_quit {
            return Ok(());
        }

        if crossterm::event::poll(std::time::Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    handle_key(key, &mut app);
                    // Validate tree state after each command to ensure path is still valid
                    app.validate_tree_state();
                }
                // Mouse events are handled implicitly by CrossTerm
                // Users can select and copy text using standard terminal mouse support
                Event::Mouse(_mouse_event) => {
                    // Mouse events are primarily for text selection and copying
                    // which is handled by the terminal emulator, not the application
                    // Future: could implement mouse clicks for UI navigation if needed
                }
                Event::Resize(_, _) => {
                    // Terminal was resized, next draw will handle it automatically
                }
                _ => {}
            }
        }
    }
}
