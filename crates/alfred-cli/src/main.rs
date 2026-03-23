//! alfred — CLI tool for scripting Alfred terminal multiplexer.
//!
//! Phase 1: stub that prints usage. IPC transport (named pipe / Unix socket)
//! is implemented in Phase 5.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "alfred", about = "Alfred terminal multiplexer CLI", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List open workspaces.
    #[command(name = "workspace")]
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Manage panes in the active workspace.
    #[command(name = "pane")]
    Pane {
        #[command(subcommand)]
        action: PaneAction,
    },
    /// List or clear agent notifications.
    #[command(name = "notification")]
    Notification {
        #[command(subcommand)]
        action: NotificationAction,
    },
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// Create a new workspace.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        dir: Option<String>,
    },
    /// List all workspaces.
    List,
    /// Switch to a workspace.
    Switch {
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum PaneAction {
    /// Split the active pane.
    Split {
        #[arg(long, default_value = "vertical")]
        direction: String,
    },
    /// Send keystrokes to a pane.
    SendKeys {
        keys: String,
        #[arg(long)]
        pane_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum NotificationAction {
    /// List unacknowledged notifications.
    List,
    /// Clear all notifications.
    Clear,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Phase 5 will connect via IPC. For now, print a stub message.
    eprintln!("alfred-cli: IPC not yet implemented (Phase 5).");
    eprintln!("Command received: {:?}", std::env::args().collect::<Vec<_>>());

    match cli.command {
        Command::Workspace { action } => match action {
            WorkspaceAction::List => println!("(no workspaces — Alfred not running)"),
            WorkspaceAction::Create { name, dir } => {
                println!("Would create workspace '{}' at {:?}", name, dir);
            }
            WorkspaceAction::Switch { name } => {
                println!("Would switch to workspace '{}'", name);
            }
        },
        Command::Pane { action } => match action {
            PaneAction::Split { direction } => {
                println!("Would split pane ({})", direction);
            }
            PaneAction::SendKeys { keys, pane_id } => {
                println!("Would send keys {:?} to pane {:?}", keys, pane_id);
            }
        },
        Command::Notification { action } => match action {
            NotificationAction::List => println!("(no notifications)"),
            NotificationAction::Clear => println!("Cleared."),
        },
    }

    Ok(())
}
