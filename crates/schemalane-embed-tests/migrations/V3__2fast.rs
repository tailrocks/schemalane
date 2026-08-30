use tokio_postgres::Client;

pub async fn migration(client: &Client) -> Result<(), tokio_postgres::Error> {
    let _ = client;
    std::future::ready(()).await;
    Ok(())
}
