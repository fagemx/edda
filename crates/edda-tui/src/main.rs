mod app;
mod ui;

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};

use app::App;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let repo_root = std::env::current_dir()?;
    let project_id = edda_store::project_id(&repo_root);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, project_id, repo_root);
    ratatui::restore();

    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    project_id: String,
    repo_root: std::path::PathBuf,
) -> color_eyre::Result<()> {
    let mut app = App::new(project_id, repo_root);
    let interval = Duration::from_secs(1);
    let mut last_refresh = Instant::now();

    // Initial load — errors are non-fatal, show empty state
    let _ = app.refresh_data();

    loop {
        terminal.draw(|f| ui::render(f, &app))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key);
                }
                _ => {}
            }
        }

        if last_refresh.elapsed() >= interval {
            let _ = app.refresh_data();
            last_refresh = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
