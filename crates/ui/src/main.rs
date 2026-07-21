//! OpenSCAD TUI - Terminal User Interface for OpenSCAD
//!
//! A command-driven OpenSCAD editor with real-time preview

use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use openscad_terminal::DisplayProtocol;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use ratatui_image::picker::Picker;
use std::error::Error;
use std::io;
use std::path::PathBuf;

use openscad_ui::{
    app::{App, PreviewCloseAction},
    commands::{cmd_edit_scad, cmd_load_force, cmd_view},
    input::{handle_key, handle_mouse},
    ui::draw,
};

const IMAGE_PROTOCOL_ENV: &str = "OPENSCAD_TUI_IMAGE_PROTOCOL";
const HISTORY_FILE_NAME: &str = "history.json";

#[derive(Debug, Parser)]
#[command(
    name = "openscad-tui",
    version,
    about = "A structured terminal editor for OpenSCAD"
)]
struct Cli {
    /// .scadtui project, .scad source, or .off/.stl model to open
    file: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let protocol_override = image_protocol_override_from_env()
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;

    let mut app = App::new();
    let history_path = command_history_path();
    if let Some(path) = &history_path {
        if let Err(error) = app.load_command_history(path) {
            eprintln!("warning: could not load command history: {error}");
        }
    }
    if let Some(file) = cli.file {
        let filename = file.to_string_lossy();
        let extension = file
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("scad") => cmd_edit_scad(&mut app, &filename)?,
            Some("off" | "stl") => {
                cmd_view(&mut app, &filename)?;
                app.preview_close_action = PreviewCloseAction::Quit;
            }
            _ => cmd_load_force(&mut app, &filename)?,
        }
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((10, 20)));
    app.configure_image_picker(picker);
    if let Some(protocol_type) = protocol_override {
        app.model_preview.set_protocol_type(protocol_type);
    }
    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Some(path) = &history_path {
        if let Err(error) = app.save_command_history(path) {
            eprintln!("warning: could not save command history: {error}");
        }
    }

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

fn command_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|directory| directory.join("openscad-tui").join(HISTORY_FILE_NAME))
}

fn image_protocol_override_from_env() -> Result<Option<DisplayProtocol>, String> {
    let Some(value) = std::env::var_os(IMAGE_PROTOCOL_ENV) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{IMAGE_PROTOCOL_ENV} must contain valid UTF-8"))?;
    parse_image_protocol_override(&value)
}

fn parse_image_protocol_override(value: &str) -> Result<Option<DisplayProtocol>, String> {
    if value.trim().eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    value.parse::<DisplayProtocol>().map(Some).map_err(|_| {
        format!(
            "invalid {IMAGE_PROTOCOL_ENV} value {value:?}; expected auto or one of {}",
            DisplayProtocol::NAMES.join(", ")
        )
    })
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    loop {
        app.poll_render_events();
        app.model_preview.tick(std::time::Instant::now());
        if app.take_terminal_clear_request() {
            terminal.clear()?;
        }
        let draw_started = std::time::Instant::now();
        terminal.draw(|f| {
            draw(f, app);
        })?;
        app.model_preview.record_ui_draw(draw_started.elapsed());

        if app.should_quit {
            return Ok(());
        }

        let poll_interval = if app.model_preview.auto_rotate || app.mouse_drag.is_some() {
            std::time::Duration::from_millis(33)
        } else {
            std::time::Duration::from_millis(100)
        };
        if crossterm::event::poll(poll_interval)? {
            // A terminal image can take long enough to write that several input events queue up.
            // Drain a bounded batch before drawing another image so a stop key is not trapped
            // behind mouse movement or resize events.
            for _ in 0..64 {
                handle_event(event::read()?, app);
                if !crossterm::event::poll(std::time::Duration::ZERO)? {
                    break;
                }
            }
        }
    }
}

fn handle_event(event: Event, app: &mut App) {
    match event {
        Event::Key(key) => {
            handle_key(key, app);
            app.validate_tree_state();
        }
        Event::Mouse(mouse_event) => handle_mouse(mouse_event, app),
        Event::Resize(_, _) => app.calculate_help_modal_size(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_image_protocol_override;
    use openscad_terminal::DisplayProtocol;

    #[test]
    fn image_protocol_override_accepts_supported_values() {
        assert_eq!(parse_image_protocol_override("auto"), Ok(None));
        assert_eq!(
            parse_image_protocol_override(" KITTY "),
            Ok(Some(DisplayProtocol::Kitty))
        );
        assert_eq!(
            parse_image_protocol_override("sixel"),
            Ok(Some(DisplayProtocol::Sixel))
        );
        assert_eq!(
            parse_image_protocol_override("halfblocks"),
            Ok(Some(DisplayProtocol::Halfblocks))
        );
        assert_eq!(
            parse_image_protocol_override("braille"),
            Ok(Some(DisplayProtocol::Braille))
        );
        assert_eq!(
            parse_image_protocol_override("iterm2"),
            Ok(Some(DisplayProtocol::Iterm2))
        );
        assert_eq!(
            parse_image_protocol_override("ascii"),
            Ok(Some(DisplayProtocol::Ascii))
        );
    }

    #[test]
    fn image_protocol_override_rejects_unknown_or_empty_values() {
        let unknown = parse_image_protocol_override("png").unwrap_err();
        assert!(unknown.contains("auto or one of kitty, sixel, iterm2, halfblocks, braille, ascii"));
        assert!(parse_image_protocol_override("").is_err());
    }
}
