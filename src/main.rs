use std::path::PathBuf;

use clap::{Parser, Subcommand};

use emr::cli::config_cmd::{cmd_config_init, cmd_config_show, cmd_config_validate};
use emr::cli::serve::cmd_serve;
use emr::error;

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
    Create {
        /// Human-readable name for this key (e.g. "ci-pipeline")
        #[arg(long)]
        name: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
    /// List all API keys
    List {
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
    /// Revoke an API key by id
    Revoke {
        /// Key id to revoke
        id: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
    /// Rotate an API key (revoke old, issue new)
    Rotate {
        /// Key id to rotate
        id: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
}

// ── Providers subcommands ────────────────────────────────────────────────────

#[derive(Subcommand)]
enum ProvidersCommands {
    /// Add an embedding provider
    Add {
        /// Provider name (e.g. "voyage-prod")
        #[arg(long)]
        name: String,
        /// Provider type: voyage or cohere
        #[arg(long = "type")]
        provider_type: String,
        /// Environment variable that holds the API key
        #[arg(long)]
        api_key_env: String,
        /// Provider endpoint URL
        #[arg(long)]
        endpoint: String,
        /// Model identifier
        #[arg(long)]
        model: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
    /// List configured providers
    List {
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
    /// Remove a provider
    Remove {
        /// Provider name to remove
        name: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
    /// Test provider connectivity
    Test {
        /// Provider name to test
        name: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET)
        #[arg(long, env = "EMR_ADMIN_SECRET")]
        admin_secret: String,
    },
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    let cli = Cli::parse();

    let result = dispatch(cli.command).await;
    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

async fn dispatch(command: Commands) -> Result<(), error::ConfigError> {
    match command {
        Commands::Serve => {
            cmd_serve().await?;
            Ok(())
        }

        Commands::Keys { command } => {
            dispatch_keys(command).await?;
            Ok(())
        }

        Commands::Providers { command } => {
            dispatch_providers(command).await?;
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

async fn dispatch_keys(command: KeysCommands) -> Result<(), error::ConfigError> {
    use emr::cli::keys::{cmd_keys_create, cmd_keys_list, cmd_keys_revoke, cmd_keys_rotate};

    match command {
        KeysCommands::Create { name, server, admin_secret } => {
            cmd_keys_create(&name, &server, &admin_secret).await
        }
        KeysCommands::List { server, admin_secret } => {
            cmd_keys_list(&server, &admin_secret).await
        }
        KeysCommands::Revoke { id, server, admin_secret } => {
            cmd_keys_revoke(&id, &server, &admin_secret).await
        }
        KeysCommands::Rotate { id, server, admin_secret } => {
            cmd_keys_rotate(&id, &server, &admin_secret).await
        }
    }
}

async fn dispatch_providers(command: ProvidersCommands) -> Result<(), error::ConfigError> {
    use emr::cli::providers::{
        cmd_providers_add, cmd_providers_list, cmd_providers_remove, cmd_providers_test,
    };

    match command {
        ProvidersCommands::Add {
            name,
            provider_type,
            api_key_env,
            endpoint,
            model,
            server,
            admin_secret,
        } => {
            cmd_providers_add(&server, &admin_secret, &name, &provider_type, &api_key_env, &endpoint, &model).await
        }
        ProvidersCommands::List { server, admin_secret } => {
            cmd_providers_list(&server, &admin_secret).await
        }
        ProvidersCommands::Remove { name, server, admin_secret } => {
            cmd_providers_remove(&server, &admin_secret, &name).await
        }
        ProvidersCommands::Test { name, server, admin_secret } => {
            cmd_providers_test(&server, &admin_secret, &name).await
        }
    }
}
