```rust
// Import necessary modules and crates
use std::{collections::HashMap, fmt, str::FromStr};
use anyhow::Result;
use clap::Parser;
use futures_lite::StreamExt;
use iroh::{protocol::Router, Endpoint, NodeAddr, NodeId};
use iroh_gossip::{
    net::{Event, Gossip, GossipEvent, GossipReceiver},
    proto::TopicId,
};
use serde::{Deserialize, Serialize};
use mdns_sd::{ServiceDaemon, ServiceEvent}; // Import mDNS service for local discovery

/// CLI arguments structure for setting user options
#[derive(Parser, Debug)]
struct Args {
    /// Set a user nickname
    #[clap(short, long)]
    name: Option<String>,
    /// Set the bind port for the socket (default: random port)
    #[clap(short, long, default_value = "0")]
    bind_port: u16,
    /// Subcommands for either opening or joining a chat room
    #[clap(subcommand)]
    command: Command,
}

/// Enumeration of available chat commands
#[derive(Parser, Debug)]
enum Command {
    /// Open a chat room and print a ticket for others to join
    Open,
    /// Join a chat room using a ticket
    Join {
        /// Ticket provided as a base32 string
        ticket: String,
    },
}

#[tokio::main] // Marks the main function as async with Tokio runtime
async fn main() -> Result<()> {
    let args = Args::parse(); // Parse command-line arguments

    // Determine the topic and known nodes based on the command
    let (topic, nodes) = match &args.command {
        Command::Open => {
            // Generate a random topic ID
            let topic = TopicId::from_bytes(rand::random());
            println!("> opening chat room for topic {topic}");
            (topic, vec![])
        }
        Command::Join { ticket } => {
            // Parse the ticket to extract topic and node addresses
            let Ticket { topic, nodes } = Ticket::from_str(ticket)?;
            println!("> joining chat room for topic {topic}");
            (topic, nodes)
        }
    };

    // Set up the P2P networking endpoint with discovery
    let endpoint = Endpoint::builder().discovery().bind().await?;
    println!("> our node id: {}", endpoint.node_id());

    // Initialize mDNS service for LAN discovery
    let mdns = ServiceDaemon::new()?;
    mdns.register("iroh-chat", "_iroh._tcp", 1200, &[])?;
    println!("> mDNS discovery enabled for local network");

    // Initialize the gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone()).await?;

    // Start the router to accept gossip connections
    let router = Router::builder(endpoint.clone())
        .accept(iroh_gossip::ALPN, gossip.clone())
        .spawn()
        .await?;

    // Generate a ticket containing the topic and node address
    let ticket = {
        let me = endpoint.node_addr().await?;
        let nodes = vec![me];
        Ticket { topic, nodes }
    };
    println!("> ticket to join us: {ticket}");

    // Connect to known nodes if any are available
    let node_ids = nodes.iter().map(|p| p.node_id).collect();
    if nodes.is_empty() {
        println!("> waiting for nodes to join us...");
    } else {
        println!("> trying to connect to {} nodes...", nodes.len());
        for node in nodes.into_iter() {
            endpoint.add_node_addr(node)?;
        }
    }

    // Subscribe to the gossip topic
    let (sender, receiver) = gossip.subscribe_and_join(topic, node_ids).await?.split();
    println!("> connected!");

    // Broadcast nickname if set
    if let Some(name) = args.name {
        let message = Message::AboutMe {
            from: endpoint.node_id(),
            name,
        };
        sender.broadcast(message.to_vec().into()).await?;
    }

    // Start listening for incoming messages
    tokio::spawn(subscribe_loop(receiver));

    // Set up input handling for user messages
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || input_loop(line_tx));

    println!("> type a message and hit enter to broadcast...");
    while let Some(text) = line_rx.recv().await {
        let message = Message::Message {
            from: endpoint.node_id(),
            text: text.clone(),
        };
        sender.broadcast(message.to_vec().into()).await?;
        println!("> sent: {text}");
    }
    router.shutdown().await?;
    Ok(())
}

/// Enum representing message types
#[derive(Debug, Serialize, Deserialize)]
enum Message {
    AboutMe { from: NodeId, name: String },
    Message { from: NodeId, text: String },
}

impl Message {
    /// Deserialize message from bytes
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(Into::into)
    }

    /// Serialize message to byte vector
    pub fn to_vec(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serialization should not fail")
    }
}

/// Function to handle incoming gossip messages
async fn subscribe_loop(mut receiver: GossipReceiver) -> Result<()> {
    let mut names = HashMap::new(); // Store node IDs and corresponding names
    while let Some(event) = receiver.try_next().await? {
        if let Event::Gossip(GossipEvent::Received(msg)) = event {
            match Message::from_bytes(&msg.content)? {
                Message::AboutMe { from, name } => {
                    names.insert(from, name.clone());
                    println!("> {} is now known as {}", from.fmt_short(), name);
                }
                Message::Message { from, text } => {
                    let name = names.get(&from).map_or_else(|| from.fmt_short(), String::to_string);
                    println!("{}: {}", name, text);
                }
            }
        }
    }
    Ok(())
}


