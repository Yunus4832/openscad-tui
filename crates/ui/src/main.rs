//! OpenSCAD TUI - Terminal User Interface for OpenSCAD
//!
//! A command-driven OpenSCAD editor with real-time preview

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use ratatui_image::picker::Picker;
use std::error::Error;
use std::io;

use openscad_ui::{app::App, input::handle_key, ui::draw};

fn main() -> Result<(), Box<dyn Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((10, 20)));
    let mut app = App::new();
    app.configure_image_picker(picker);
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
        app.poll_render_events();
        app.model_preview.tick(std::time::Instant::now());
        if app.take_terminal_clear_request() {
            terminal.clear()?;
        }
        terminal.draw(|f| {
            draw(f, &mut app);
        })?;

        if app.should_quit {
            return Ok(());
        }

        let poll_interval = if app.model_preview.auto_rotate {
            std::time::Duration::from_millis(33)
        } else {
            std::time::Duration::from_millis(100)
        };
        if crossterm::event::poll(poll_interval)? {
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
                    app.calculate_help_modal_size();
                }
                _ => {}
            }
        }
    }
}
