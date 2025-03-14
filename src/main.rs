use std::{collections::HashMap, fmt, str::FromStr};
use anyhow::Result;
use clap::Parser;
use futures_lite::StreamExt;
use reqwest::Client; 
use iroh::{
    discovery::{dns::DnsDiscovery, local_swarm_discovery::LocalSwarmDiscovery, ConcurrentDiscovery},
    protocol::Router, Endpoint, NodeAddr, NodeId, SecretKey,
};
use iroh_gossip::{
    net::{Event, Gossip, GossipEvent, GossipReceiver},
    proto::TopicId,
};
use serde::{Deserialize, Serialize};

// Function to retrieve OpenHAB item state
pub async fn get_item_state() -> Result<String> {
    let client = Client::new();
    let url = "http://192.168.38.59:8080/rest/items/TestItem"; 
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await?
        .text()
        .await?;

    Ok(response)
}

#[derive(Parser, Debug)]
struct Args {
    #[clap(short, long)]
    name: Option<String>,

    #[clap(short, long, default_value = "0")]
    bind_port: u16,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    Open,
    Join { ticket: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct Ticket {
    topic: TopicId,
    nodes: Vec<NodeAddr>,
}

impl FromStr for Ticket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(Into::into)
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Node ID: {}", self.nodes[0].node_id)
    }
}

fn simplify_ticket(ticket: &Ticket) -> String {
    ticket.nodes[0].node_id.to_string()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let (topic, nodes) = match &args.command {
        Command::Open => {
            let topic = TopicId::from_bytes(rand::random());
            println!("> opening chat room for topic {topic}");
            (topic, vec![])
        }
        Command::Join { ticket } => {
            let Ticket { topic, nodes } = Ticket::from_str(ticket)?;
            println!("> joining chat room for topic {topic}");
            (topic, nodes)
        }
    };

    let secret_key = SecretKey::generate(rand::rngs::OsRng);
    
    let discovery = ConcurrentDiscovery::from_services(vec![
        Box::new(DnsDiscovery::n0_dns()),
        Box::new(LocalSwarmDiscovery::new(secret_key.public())?),
    ]);
    
    let endpoint = Endpoint::builder()
        .discovery(Box::new(discovery))
        .bind()
        .await?;
    println!("> our node id: {}", endpoint.node_id());

    let gossip = Gossip::builder().spawn(endpoint.clone()).await?;

    let router = Router::builder(endpoint.clone())
        .accept(iroh_gossip::ALPN, gossip.clone())
        .spawn()
        .await?;

    let ticket = {
        let me = endpoint.node_addr().await?;
        let nodes = vec![me];
        Ticket { topic, nodes }
    };
    let ticket_str = serde_json::to_string(&ticket)?;
    println!("> ticket to join us: {}", ticket_str);
    
    let node_ids = nodes.iter().map(|p| p.node_id).collect();
    if nodes.is_empty() {
        println!("> waiting for nodes to join us...");
    } else {
        println!("> trying to connect to {} nodes...", nodes.len());
        for node in nodes.into_iter() {
            endpoint.add_node_addr(node)?;
        }
    }

    let (sender, receiver) = gossip.subscribe_and_join(topic, node_ids).await?.split();
    println!("> connected!");

    if let Some(name) = args.name.clone() {
        let message = Message::AboutMe {
            from: endpoint.node_id(),
            name,
        };
        sender.broadcast(message.to_vec().into()).await?;
    }

    tokio::spawn(subscribe_loop(receiver));

    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || input_loop(line_tx));

    println!("> type a message and hit enter to broadcast...");
    while let Some(text) = line_rx.recv().await {
        // Fetch the state of the OpenHAB item
        let openhab_state = get_item_state().await.unwrap_or_else(|_| "Error fetching state".to_string());

        // Send message with OpenHAB state
        let message = Message::Message {
            from: endpoint.node_id(),
            text: format!("{} - OpenHAB state: {}", text, openhab_state),
        };
        sender.broadcast(message.to_vec().into()).await?;
        println!("> sent: {text} - OpenHAB state: {openhab_state}");
    }

    router.shutdown().await?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
enum Message {
    AboutMe { from: NodeId, name: String },
    Message { from: NodeId, text: String },
}

impl Message {
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(Into::into)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serialization should not fail")
    }
}

async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
    let mut names = HashMap::new();
    while let Some(event) = receiver.try_next().await? {
        if let Event::Gossip(GossipEvent::Received(msg)) = event {
            match Message::from_bytes(&msg.content)? {
                Message::AboutMe { from, name } => {
                    names.insert(from, name.clone());
                    println!("> {} is now known as {}", from.fmt_short(), name);
                }
                Message::Message { from, text } => {
                    // Fetch OpenHAB state when receiving a message
                    let openhab_state = get_item_state().await.unwrap_or_else(|_| "Error fetching state".to_string());

                    // Print received message with OpenHAB state
                    let name = names.get(&from).map_or_else(|| from.fmt_short(), String::to_string);
                    println!("{}: {} - OpenHAB state: {}", name, text, openhab_state);
                }
            }
        }
    }
    Ok(())
}

fn input_loop(tx: tokio::sync::mpsc::Sender<String>) {
    let stdin = std::io::stdin();
    let mut buffer = String::new();
    while stdin.read_line(&mut buffer).is_ok() {
        let text = buffer.trim().to_string();
        if tx.blocking_send(text).is_err() {
            break;
        }
        buffer.clear();
    }
}