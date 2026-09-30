mod app;
mod askpass;
mod bundle;
mod crypto;
mod model;
mod screens;
mod sftp;
mod ssh;
mod ssh_config;
mod store;
mod theme;

use app::App;

fn main() -> anyhow::Result<()> {
    // Started by ssh as SSH_ASKPASS for a connection with a stored password.
    if let Some(name) = std::env::var_os(ssh::ASKPASS_ENV) {
        std::process::exit(askpass::run(&name.to_string_lossy()));
    }
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // ratatui::init installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = App::new().and_then(|mut app| app.run(&mut terminal));
    ratatui::restore();
    result
}
