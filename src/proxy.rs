use crate::Config;

use anyhow::Result;

use tokio::net::TcpListener;

pub async fn run(config: Config) -> Result<()> {
    let listener: TcpListener = TcpListener::bind(&config.listen_addr).await?;
    tracing::info!(
        addr = %config.listen_addr,
        "Proxy listening on {}",
        config.listen_addr
    );

    loop {
        let (socket, addr) = listener.accept().await?;
        tracing::debug!(?addr, "Accepted connection from {}", addr);
    }
}
