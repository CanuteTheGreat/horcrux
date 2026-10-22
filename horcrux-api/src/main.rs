//! The `horcrux-api` binary: everything lives in the `horcrux_api` library;
//! this only starts the server, or runs the host-integration command
//! `horcrux-api guests <stop|boot|status>` (see api/guest_state/agent.rs).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("guests") {
        std::process::exit(horcrux_api::api::guest_state::agent::cli_main(&args[1..]).await);
    }
    horcrux_api::app::run().await
}
