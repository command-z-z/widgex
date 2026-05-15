use std::{env, path::PathBuf};

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "widgexd", version, about = "Widgex daemon")]
struct Args {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    cli_path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let socket = args.socket.unwrap_or_else(widgex_ipc::default_socket_path);
    let cli_path = args.cli_path.unwrap_or_else(default_cli_path);

    widgexd::run_socket_daemon(args.config, socket, cli_path)
}

fn default_cli_path() -> PathBuf {
    env::current_exe()
        .map(|path| path.with_file_name("widgex"))
        .unwrap_or_else(|_| PathBuf::from("widgex"))
}
