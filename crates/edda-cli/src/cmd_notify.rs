use std::io::Read;
use std::path::Path;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum NotifyCmd {
    /// Send test notification to all configured channels
    Test,
    /// Show configured notification channels
    Status,
    /// Send a free-text message to every configured channel (event name "digest")
    Send {
        /// Message title (first line)
        #[arg(long)]
        title: String,
        /// Read the message body from this file ("-" for stdin)
        #[arg(long, value_name = "PATH")]
        file: String,
    },
}

pub fn run(cmd: NotifyCmd, repo_root: &Path) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    let config = edda_notify::NotifyConfig::load(&paths);

    match cmd {
        NotifyCmd::Test => run_test(&config),
        NotifyCmd::Status => run_status(&config),
        NotifyCmd::Send { title, file } => run_send(&config, &title, &file),
    }
}

fn run_send(config: &edda_notify::NotifyConfig, title: &str, file: &str) -> anyhow::Result<()> {
    let body = if file == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(file)?
    };

    if config.channels.is_empty() {
        // Delivery is best-effort: an unconfigured operator is a log line,
        // not a failure (GH-765 doneWhen).
        println!("No notification channels configured; digest not sent.");
        return Ok(());
    }

    let results = edda_notify::send_text(config, title, &body);
    for (name, result) in results {
        match result {
            Ok(()) => println!("  OK  {name}"),
            Err(e) => println!("  ERR {name}: {e}"),
        }
    }
    Ok(())
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
