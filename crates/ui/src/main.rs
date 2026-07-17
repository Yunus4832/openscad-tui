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
use ratatui_image::picker::{Picker, ProtocolType};
use std::error::Error;
use std::io;

use openscad_ui::{
    app::App,
    input::{handle_key, handle_mouse},
    ui::draw,
};

const IMAGE_PROTOCOL_ENV: &str = "OPENSCAD_TUI_IMAGE_PROTOCOL";

fn main() -> Result<(), Box<dyn Error>> {
    let protocol_override = image_protocol_override_from_env()
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((10, 20)));
    if let Some(protocol_type) = protocol_override {
        picker.set_protocol_type(protocol_type);
    }
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

fn image_protocol_override_from_env() -> Result<Option<ProtocolType>, String> {
    let Some(value) = std::env::var_os(IMAGE_PROTOCOL_ENV) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{IMAGE_PROTOCOL_ENV} must contain valid UTF-8"))?;
    parse_image_protocol_override(&value)
}

fn parse_image_protocol_override(value: &str) -> Result<Option<ProtocolType>, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(None),
        "kitty" => Ok(Some(ProtocolType::Kitty)),
        "sixel" => Ok(Some(ProtocolType::Sixel)),
        "halfblocks" => Ok(Some(ProtocolType::Halfblocks)),
        "iterm2" => Ok(Some(ProtocolType::Iterm2)),
        _ => Err(format!(
            "invalid {IMAGE_PROTOCOL_ENV} value {value:?}; expected auto, kitty, sixel, halfblocks, or iterm2"
        )),
    }
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    loop {
        app.poll_render_events();
        app.model_preview.tick(std::time::Instant::now());
        if app.take_terminal_clear_request() {
            terminal.clear()?;
        }
        let draw_started = std::time::Instant::now();
        terminal.draw(|f| {
            draw(f, &mut app);
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
                handle_event(event::read()?, &mut app);
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
    use ratatui_image::picker::ProtocolType;

    #[test]
    fn image_protocol_override_accepts_supported_values() {
        assert_eq!(parse_image_protocol_override("auto"), Ok(None));
        assert_eq!(
            parse_image_protocol_override(" KITTY "),
            Ok(Some(ProtocolType::Kitty))
        );
        assert_eq!(
            parse_image_protocol_override("sixel"),
            Ok(Some(ProtocolType::Sixel))
        );
        assert_eq!(
            parse_image_protocol_override("halfblocks"),
            Ok(Some(ProtocolType::Halfblocks))
        );
        assert_eq!(
            parse_image_protocol_override("iterm2"),
            Ok(Some(ProtocolType::Iterm2))
        );
    }

    #[test]
    fn image_protocol_override_rejects_unknown_or_empty_values() {
        let unknown = parse_image_protocol_override("png").unwrap_err();
        assert!(unknown.contains("auto, kitty, sixel, halfblocks, or iterm2"));
        assert!(parse_image_protocol_override("").is_err());
    }
}
