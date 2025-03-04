use reqwest::Client;
use anyhow::Result;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::StreamExt;
use url::Url;

// Fonction pour récupérer l'état de l'item "test_item" depuis OpenHAB
pub async fn get_item_state() -> Result<String> {
    let client = Client::new();
    let url = "http://192.168.247.59/:8080/rest/items/test_item/state";
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await?
        .text()
        .await?;

    Ok(response)
}

// Fonction pour se connecter au WebSocket d'OpenHAB
pub async fn connect_websocket() -> Result<()> {
    let (ws_stream, _) = connect_async(Url::parse("ws://192.168.247.59/:8080/ws")?).await?;
    let (_write, mut read) = ws_stream.split();

    println!("> Connecté au WebSocket OpenHAB");

    while let Some(message) = read.next().await {
        let msg = message?;
        if msg.is_text() {
            println!("> Message reçu: {}", msg.to_text()?);
        }
    }

    Ok(())
}
