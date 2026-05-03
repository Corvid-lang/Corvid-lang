use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum DeployCommand {
    /// Emit a deploy package containing Dockerfile and OCI metadata.
    Package {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit Docker Compose deployment artifacts.
    Compose {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit Fly.io and Render-style single-service deployment artifacts.
    Paas {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit Kubernetes manifests.
    K8s {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit systemd service, sysusers, and tmpfiles artifacts.
    Systemd {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}
