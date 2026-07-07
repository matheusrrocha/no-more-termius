mod app;
mod model;
mod screens;
mod sftp;
mod ssh;
mod ssh_config;
mod store;

use app::App;

fn main() -> anyhow::Result<()> {
    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = App::new().and_then(|mut app| app.run(&mut terminal));
    ratatui::restore();
    result
}
