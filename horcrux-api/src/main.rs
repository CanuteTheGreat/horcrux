//! The `horcrux-api` binary: everything lives in the `horcrux_api` library;
//! this only starts the server.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    horcrux_api::app::run().await
}
