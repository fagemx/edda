use std::path::Path;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum NotifyCmd {
    /// Send test notification to all configured channels
    Test,
    /// Show configured notification channels
    Status,
}

pub fn run(cmd: NotifyCmd, repo_root: &Path) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    let config = edda_notify::NotifyConfig::load(&paths);

    match cmd {
        NotifyCmd::Test => run_test(&config),
        NotifyCmd::Status => run_status(&config),
    }
}

fn run_test(config: &edda_notify::NotifyConfig) -> anyhow::Result<()> {
    if config.channels.is_empty() {
        println!("No notification channels configured.");
        println!();
        println!("Add channels in .edda/config.json under \"notify_channels\":");
        println!(
            "  edda config set notify_channels '[{{\"type\":\"ntfy\",\"url\":\"https://ntfy.sh/my-topic\",\"events\":[\"*\"]}}]'"
        );
        return Ok(());
    }

    println!(
        "Sending test notification to {} channel(s)...",
        config.channels.len()
    );
    let results = edda_notify::test_channels(config);
    for (name, result) in results {
        match result {
            Ok(()) => println!("  OK  {name}"),
            Err(e) => println!("  ERR {name}: {e}"),
        }
    }
    Ok(())
}

fn run_status(config: &edda_notify::NotifyConfig) -> anyhow::Result<()> {
    if config.channels.is_empty() {
        println!("No notification channels configured.");
        return Ok(());
    }

    println!("{} channel(s) configured:", config.channels.len());
    for ch in &config.channels {
        println!("  - {}", ch.display_name());
    }
    Ok(())
}
