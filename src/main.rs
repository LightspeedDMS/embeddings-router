mod cli;
mod config;
mod error;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use cli::config_cmd::{cmd_config_init, cmd_config_show, cmd_config_validate};
use cli::serve::cmd_serve;

/// Embeddings Router — a unified routing and multiplexing layer for embedding providers.
#[derive(Parser)]
#[command(
    name = "emr",
    version,
    about = "Embeddings Router: unified interface for multiple embedding providers",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the embeddings router server
    Serve,

    /// Manage API keys
    Keys {
        #[command(subcommand)]
        command: KeysCommands,
    },

    /// Manage embedding providers
    Providers {
        #[command(subcommand)]
        command: ProvidersCommands,
    },

    /// Manage router configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Show router status
    Status,

    /// Show provider health
    Health,

    /// Start the router (alias for serve with daemon mode)
    Up,

    /// Stop the router
    Down,
}

// ── Keys subcommands ─────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum KeysCommands {
    /// Create a new API key
    Create,
    /// List all API keys
    List,
    /// Revoke an API key
    Revoke,
    /// Rotate an API key
    Rotate,
}

// ── Providers subcommands ────────────────────────────────────────────────────

#[derive(Subcommand)]
enum ProvidersCommands {
    /// Add an embedding provider
    Add,
    /// List configured providers
    List,
    /// Remove a provider
    Remove,
    /// Test provider connectivity
    Test,
}

// ── Config subcommands ───────────────────────────────────────────────────────

#[derive(Subcommand)]
enum ConfigCommands {
    /// Generate a default config.toml with all documented sections and sensible defaults
    Init {
        /// Path to write the config file
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },

    /// Print the effective configuration (merging defaults with file values)
    Show {
        /// Path to the config file
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },

    /// Validate the configuration file and report errors
    Validate {
        /// Path to the config file
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
}

// ── Entry point ──────────────────────────────────────────────────────────────

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    let cli = Cli::parse();

    let result = dispatch(cli.command);
    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn dispatch(command: Commands) -> Result<(), error::ConfigError> {
    match command {
        Commands::Serve => {
            cmd_serve();
            Ok(())
        }

        Commands::Keys { command } => {
            match command {
                KeysCommands::Create => println!("keys create: not yet implemented"),
                KeysCommands::List => println!("keys list: not yet implemented"),
                KeysCommands::Revoke => println!("keys revoke: not yet implemented"),
                KeysCommands::Rotate => println!("keys rotate: not yet implemented"),
            }
            Ok(())
        }

        Commands::Providers { command } => {
            match command {
                ProvidersCommands::Add => println!("providers add: not yet implemented"),
                ProvidersCommands::List => println!("providers list: not yet implemented"),
                ProvidersCommands::Remove => println!("providers remove: not yet implemented"),
                ProvidersCommands::Test => println!("providers test: not yet implemented"),
            }
            Ok(())
        }

        Commands::Config { command } => match command {
            ConfigCommands::Init { config } => cmd_config_init(&config),
            ConfigCommands::Show { config } => cmd_config_show(&config),
            ConfigCommands::Validate { config } => cmd_config_validate(&config),
        },

        Commands::Status => {
            println!("status: not yet implemented");
            Ok(())
        }

        Commands::Health => {
            println!("health: not yet implemented");
            Ok(())
        }

        Commands::Up => {
            println!("up: not yet implemented (planned for Story #10)");
            Ok(())
        }

        Commands::Down => {
            println!("down: not yet implemented (planned for Story #10)");
            Ok(())
        }
    }
}
