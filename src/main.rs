use std::path::PathBuf;

use clap::{Parser, Subcommand};

use emr::cli::config_cmd::{cmd_config_init, cmd_config_show, cmd_config_validate};
use emr::cli::down::cmd_down;
use emr::cli::env::{default_env_path, resolve_admin_secret};
use emr::cli::health_cmd::cmd_health;
use emr::cli::serve::cmd_serve;
use emr::cli::status::cmd_status;
use emr::cli::up::cmd_up;
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
    Status {
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
    },

    /// Show provider health
    Health {
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
    },

    /// Start the emr Docker container
    Up {
        /// Host port to bind
        #[arg(long, default_value = "3200")]
        port: u16,
        /// Path to .env file
        #[arg(long)]
        env_file: Option<std::path::PathBuf>,
        /// Path to config.toml to mount into container
        #[arg(long)]
        config: Option<std::path::PathBuf>,
    },

    /// Stop the emr Docker container
    Down {
        /// Kill immediately instead of graceful stop
        #[arg(long)]
        force: bool,
        /// Graceful stop timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
    },
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
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// List all API keys
    List {
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// Revoke an API key by id
    Revoke {
        /// Key id to revoke
        id: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// Rotate an API key (revoke old, issue new)
    Rotate {
        /// Key id to rotate
        id: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
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
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// List configured providers
    List {
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// Remove a provider
    Remove {
        /// Provider name to remove
        name: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// Test provider connectivity
    Test {
        /// Provider name to test
        name: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
    },
    /// Probe provider latency as a function of batch size
    Probe {
        /// Provider name to probe
        name: String,
        /// Server base URL
        #[arg(long, default_value = "http://localhost:3200")]
        server: String,
        /// Admin secret (or set EMR_ADMIN_SECRET, or create ~/.config/emr/.env)
        #[arg(long)]
        admin_secret: Option<String>,
        /// Caller API key for /v1/embeddings requests
        #[arg(long)]
        api_key: String,
        /// Number of sample requests per batch size
        #[arg(long, default_value = "3")]
        samples: u32,
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

        Commands::Status { server } => {
            cmd_status(&server).await?;
            Ok(())
        }

        Commands::Health { server } => {
            cmd_health(&server).await?;
            Ok(())
        }

        Commands::Up { port, env_file, config } => {
            cmd_up(port, env_file, config).await?;
            Ok(())
        }

        Commands::Down { force, timeout } => {
            cmd_down(force, timeout).await?;
            Ok(())
        }
    }
}

async fn dispatch_keys(command: KeysCommands) -> Result<(), error::ConfigError> {
    use emr::cli::keys::{cmd_keys_create, cmd_keys_list, cmd_keys_revoke, cmd_keys_rotate};

    match command {
        KeysCommands::Create { name, server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_keys_create(&name, &server, &secret).await
        }
        KeysCommands::List { server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_keys_list(&server, &secret).await
        }
        KeysCommands::Revoke { id, server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_keys_revoke(&id, &server, &secret).await
        }
        KeysCommands::Rotate { id, server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_keys_rotate(&id, &server, &secret).await
        }
    }
}

async fn dispatch_providers(command: ProvidersCommands) -> Result<(), error::ConfigError> {
    use emr::cli::probe::cmd_providers_probe;
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
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_providers_add(&server, &secret, &name, &provider_type, &api_key_env, &endpoint, &model).await
        }
        ProvidersCommands::List { server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_providers_list(&server, &secret).await
        }
        ProvidersCommands::Remove { name, server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_providers_remove(&server, &secret, &name).await
        }
        ProvidersCommands::Test { name, server, admin_secret } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_providers_test(&server, &secret, &name).await
        }
        ProvidersCommands::Probe { name, server, admin_secret, api_key, samples } => {
            let secret = resolve_admin_secret(admin_secret.as_deref(), &default_env_path())?;
            cmd_providers_probe(&server, &secret, &api_key, &name, samples).await
        }
    }
}
