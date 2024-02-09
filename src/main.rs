// mod network;
mod orderbook;

// use async_std::task::spawn;
// use clap::Parser;

// use std::error::Error;
// use tracing_subscriber::EnvFilter;
// // The network module - responsible for setting up and managing the P2P network.

// // Importing necessary components from libp2p.
// use libp2p::{
//     core::Multiaddr,     // Multiaddress for network addresses.
//     multiaddr::Protocol, // Networking protocols.
// };
// use serde::{Deserialize, Serialize}; // For serializing and deserializing data.
use axum::{extract::Json, http::StatusCode, routing::post, Router};
use clap::Parser;
use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::{
    autonat, gossipsub, identify, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, StreamProtocol,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{io, select, time};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

//The main function is the entry point of the program. It is asynchronous,
//which means it can perform non-blocking operations. It returns a Result type,
//which can either be Ok(()) if the program runs successfully, or an error of type
//Box<dyn Error> if something goes wrong.

#[derive(NetworkBehaviour)]
struct AvalonBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
    pub autonat: autonat::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

    //Create a channel for sending offers to the network.
    //The channel has a buffer size of 100, which means it can hold up to 100 offers
    //before it starts blocking. The order_tx variable is used to send offers to the
    //channel, and the order_rx variable is used to receive offers from the channel.

    let (offer_tx, mut offer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

    let id_keys = match opt.identity_file {
        Some(ref file_path) if fs::metadata(file_path).is_ok() => load_keypair_file(file_path)?,
        _ => {
            let keypair = identity::Keypair::generate_ed25519();
            if let Some(ref file_path) = opt.identity_file {
                save_keypair_file(&keypair, file_path)?;
            }
            keypair
        }
    };

    //Configure autonat
    let autonat_config = autonat::Config::default();
    let autonat = autonat::Behaviour::new(id_keys.public().to_peer_id(), autonat_config);

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(id_keys)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_dns()?
        .with_behaviour(|key| {
            //Set up a custom gossipsub message id function
            //This is used to identify/prevent duplicate messages
            let offer_id = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };
            //Set up a custom gossipsub config
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10))
                //No dupllicate offers will be sent
                .message_id_fn(offer_id)
                .build()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

            let gossipsub_privacy = gossipsub::MessageAuthenticity::Signed(key.clone());

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(gossipsub_privacy, gossipsub_config)?;

            //Setting up the Kademlia behaviour for peer dsicovery and content distribution.

            let mut kademlia_config = kad::Config::default();

            // Set the protocol name for Kademlia protocol.
            kademlia_config.set_protocol_names(vec![StreamProtocol::try_from_owned(
                "/avalon/kad/1".to_string(),
            )?]);

            // Set the query timeout to 60 seconds.
            kademlia_config.set_query_timeout(Duration::from_secs(60));

            let store = kad::store::MemoryStore::new(key.public().to_peer_id());

            // Build the Kademlia behaviour.
            let mut kademlia =
                kad::Behaviour::with_config(key.public().to_peer_id(), store, kademlia_config);

            // In case the user provided an known peer, use it to enter the network
            if let Some(addr) = opt.known_peer {
                let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
                    return Err("Expect peer multiaddr to contain peer ID.".into());
                };

                kademlia.add_address(&peer_id, addr);
            } else {
                // Otherwise, use the default bootstrap nodes.
                for node in BOOTSTRAP_NODES.iter() {
                    let addr: Multiaddr = format!("/p2p/{}", node).parse()?;
                    let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
                        return Err("Expect peer multiaddr to contain peer ID.".into());
                    };
                    kademlia.add_address(&peer_id, addr);
                }
            }

            // Bootstrap the Kademlia DHT.
            kademlia.bootstrap().unwrap();

            // Setting up the Identify behaviour for peer identification.
            let identify = identify::Behaviour::new(identify::Config::new(
                "/avalon/id/1".into(),
                key.public().clone(),
            ));

            // Return the behaviour.
            Ok(AvalonBehaviour {
                gossipsub,
                kademlia,
                identify,
                autonat,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    //Set the Kademlia DHT to server mode.
    swarm
        .behaviour_mut()
        .kademlia
        .set_mode(Some(kad::Mode::Server)); // Setting the Kademlia DHT to server mode.

    // Create a Gossipsub topic
    let topic = gossipsub::IdentTopic::new("/avalon/orders/1");

    // Subscribes to our topic
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    if !opt.listen_address.is_empty() {
        for addr in opt.listen_address.iter() {
            swarm.listen_on(addr.clone())?;
        }
    } else {
        // Fallback to default addresses if no listen addresses are provided
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        swarm.listen_on("/ip6/::/tcp/0".parse()?)?;
    }

    // Start the Axum server for offer submission if the option is provided.

    if let Some(submission_addr_str) = opt.listen_offer_submission {
        tokio::spawn(async move {
            if let Err(e) = run_axum_server(offer_tx.clone(), Some(submission_addr_str)).await {
                eprintln!("Server error: {}", e);
            }
        });
    }

    let mut peer_discovery_interval = time::interval(time::Duration::from_secs(10));

    println!("Entering loop...");

    loop {
        select! {
            Some(offer) = offer_rx.recv() => {
                println!("Broadcasting Offer: {}", String::from_utf8_lossy(&offer));

                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), offer) {
                    eprintln!("Broadcasting offer failed: {}", e);
                }
            },
            _ = peer_discovery_interval.tick() => {
                swarm.behaviour_mut().kademlia.get_closest_peers(PeerId::random());
            },
            event = swarm.select_next_some() => match event {
                  SwarmEvent::IncomingConnection { .. } => {}

                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    println!("Connected to peer: {peer_id}");
                },

                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    println!("Disconnected from peer: {peer_id}");
                },

                SwarmEvent::Behaviour(AvalonBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: _,
                    message_id: _,
                    message,
                })) => {
                    let data_clone = message.data.clone();
                    let msg_str = String::from_utf8_lossy(&data_clone).into_owned();
                    println!("Received Offer: {}", msg_str);

                    if let Some(ref endpoint_url) = opt.offer_hook {
                            let endpoint_url_clone = endpoint_url.clone();
                            tokio::spawn(async move {
                                if let Err(e) = post_limit_order(&endpoint_url_clone, &msg_str).await {
                                    eprintln!("Error posting to offer hook: {}", e);
                                }
                            });
                        }
                },
                SwarmEvent::Behaviour(AvalonBehaviourEvent::Identify(identify::Event::Received { info: identify::Info { observed_addr, listen_addrs, .. }, peer_id })) => {
                    for addr in listen_addrs {
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                    }
                    // Mark the address observed for us by the external peer as confirmed.
                    // TODO: We shouldn't trust this, instead we should confirm our own address manually or using
                    // `libp2p-autonat`.
                    swarm.add_external_address(observed_addr);
                },
                      SwarmEvent::NewListenAddr { address, .. } => {
                    let local_peer_id = *swarm.local_peer_id();
                    eprintln!(
                        "Local node is listening on {:?}",
                        address.with(Protocol::P2p(local_peer_id))
                    );
                }
                _ => {}
            }
        }
    }
}

// This function configures and runs the Axum server
async fn run_axum_server(
    offer_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    listen_offer_submission: Option<String>,
) -> Result<(), Box<dyn Error>> {
    if let Some(submission_addr_str) = listen_offer_submission {
        let submission_addr: SocketAddr =
            submission_addr_str.parse().expect("Invalid socket address");

        let app = Router::new()
            .route(
                "/submit_offer",
                post(move |json| offer_handler(json, offer_tx.clone())),
            )
            .layer(TraceLayer::new_for_http());

        // Start the Axum server
        let listener = tokio::net::TcpListener::bind(submission_addr)
            .await
            .unwrap();

        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();

        println!("Axum server running at {}", submission_addr);

        // axum::serve::bind(&submission_addr)
        //     .serve(app.into_make_service())
        //     .await?;
    }
    Ok(())
}

async fn offer_handler(
    Json(payload): Json<LimitOrder>,
    offer_tx_clone: tokio::sync::mpsc::Sender<Vec<u8>>,
) -> StatusCode {
    println!("Received order: {:?}", payload);
    match bech32::decode(&payload.order_details) {
        Ok(_) => {
            let offer_bytes = payload.order_details.as_bytes().to_vec();
            if offer_tx_clone.send(offer_bytes).await.is_err() {
                eprintln!("Failed to send offer through the channel");
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::OK
            }
        }
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

async fn post_limit_order(endpoint: &str, offer: &str) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();

    let offer_json = json!({ "offer": offer });
    client.post(endpoint).json(&offer_json).send().await?;

    Ok(())
}

fn save_keypair_file(keypair: &identity::Keypair, file_path: &str) -> io::Result<()> {
    let encoded = keypair.to_protobuf_encoding().unwrap();
    let keypair_json: IdentityJson = IdentityJson { identity: encoded };
    let json = serde_json::to_string(&keypair_json)?;
    let mut file = File::create(file_path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn load_keypair_file(file_path: &str) -> io::Result<identity::Keypair> {
    let contents = fs::read_to_string(file_path)?;
    let keypair_json: IdentityJson = serde_json::from_str(&contents)?;
    identity::Keypair::from_protobuf_encoding(&keypair_json.identity)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid keypair data"))
}

const BOOTSTRAP_NODES: [&str; 3] = [
    "12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Rg3sBYbU1Nft9HyQ6X",
    "12D3KooWCLvBXPohyMUKhbRrkcfRRkMLDfnCqyCjNSk6qyfjLMJ8",
    "12D3KooWP6QDYTCccwfUQVAc6jQDvzVY1FtU3WVsAxmVratbbC5V",
];

#[derive(Serialize, Deserialize)]
struct IdentityJson {
    identity: Vec<u8>,
}

#[derive(Parser, Debug)]
#[clap(name = "Splash!")]
struct Opt {
    #[clap(
        long,
        short,
        value_name = "MULTIADDR",
        help = "Set initial peer, if missing use dexies DNS introducer"
    )]
    known_peer: Option<Multiaddr>,

    #[clap(
        long,
        short,
        value_name = "MULTIADDR",
        help = "Set listen address, defaults to all interfaces, use multiple times for multiple addresses"
    )]
    listen_address: Vec<Multiaddr>,

    #[clap(
        long,
        short,
        help = "Store and reuse peer identity (only useful for known peers)"
    )]
    identity_file: Option<String>,

    #[clap(
        long,
        help = "HTTP endpoint where incoming offers are posted to, sends JSON body {\"offer\":\"offer1...\"} (defaults to STDOUT)"
    )]
    offer_hook: Option<String>,

    #[clap(
        long,
        help = "Start a HTTP API for offer submission, expects JSON body {\"offer\":\"offer1...\"}",
        value_name = "HOST:PORT"
    )]
    listen_offer_submission: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LimitOrder {
    order_details: String,
}

// #[async_std::main]
// async fn main() -> Result<(), Box<dyn Error>> {
//     //Parse the command line arguments.
//     let opt = Opt::parse();

//     //Used for initializing a global tracing subscriber, which is used for application-level logging.
//     let _ = tracing_subscriber::fmt()
//         .with_env_filter(EnvFilter::from_default_env())
//         .try_init();

//     //Create a new network client, network events stream, and network event loop.
//     //The network event loop is then spawnwed as a task to run in the background.
//     let (mut network_client, mut network_events, network_event_loop) =
//         network::new(opt.secret_key_seed).await?;

//     // Spawn the network task for it to run in the background.
//     spawn(network_event_loop.run());

//     //The program then starts listening for incoming connections. If a listening address
//     //is provided, it uses that; otherwise, it listens on any address.

//     // In case a listen address was provided use it, otherwise listen on any
//     // address.
//     match opt.listen_address {
//         Some(addr) => network_client
//             .start_listening(addr)
//             .await
//             .expect("Listening not to fail."),
//         None => network_client
//             .start_listening("/ip4/0.0.0.0/tcp/0".parse()?)
//             .await
//             .expect("Listening not to fail."),
//     };

//     // In case the user provided an address of a peer on the CLI, dial it.
//     if let Some(addr) = opt.peer {
//         let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
//             return Err("Expect peer multiaddr to contain peer ID.".into());
//         };
//         network_client
//             .dial(peer_id, addr)
//             .await
//             .expect("Dial to succeed");
//     }

//     //Question: Do I still need these arguments?

//     match opt.argument {
//         //Provide an order and propogate it to other peers
//         CliArgument::ProvideOrder { order_details } => {}
//         //Rceive an order
//         CliArgument::ReceiveOrder { order_details } => {}
//     }

//     Ok(())
// }

// #[derive(Serialize, Deserialize)]
// struct IdentityJson {
//     identity: Vec<u8>,
// }

// #[derive(Parser, Debug)]
// #[clap(name = "avalon")]
// /// Represents the command line options for the program.
// struct Opt {
//     /// Fixed value to generate deterministic peer ID.
//     #[clap(long)]
//     secret_key_seed: Option<u8>,

//     /// The peer address to connect to.
//     #[clap(long)]
//     peer: Option<Multiaddr>,

//     //Store peer id in a file for resuse if necessary
//     #[clap(
//         long,
//         short,
//         help = "Store peer id in a file for resuse if necessary (only useful for known peers)"
//     )]
//     peer_id_file: Option<String>,

//     //
//     #[clap(
//         long,
//         help = "HTTP endpoint where incoming offers are posted to, sends JSON body {\"offer\":\"offer1...\"} (defaults to STDOUT)"
//     )]
//     offer: Option<String>,

//     /// The address to listen on for incoming connections.
//     #[clap(long)]
//     listen_address: Option<Multiaddr>,

//     /// The subcommand to execute.
//     #[clap(subcommand)]
//     argument: CliArgument,
// }

// #[derive(Debug, Parser)]

// /// Represents the subcommands of the program.
// enum CliArgument {
//     ProvideOrder {
//         #[clap(long)]
//         order_details: String,
//     },
//     ReceiveOrder {
//         #[clap(long)]
//         order_details: String,
//     },
// }
