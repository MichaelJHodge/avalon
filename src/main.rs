// mod network;
mod db;
mod orderbook;
use axum::{extract::Json, http::StatusCode, routing::get, routing::post, Router};
use clap::Parser;
use ethers::types::{Address, U256};
use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::{
    autonat, gossipsub, identify, identity, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, StreamProtocol,
};
use orderbook::LimitOrder;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{io, select, time};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::db::insert_offer;

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

// const DATABASE_URL: &str = "ckb_limit_order_book.db";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

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

    let db_pool = db::setup_database(&database_url)
        .await
        .expect("Failed to setup database");

    let mut peer_discovery_interval = time::interval(time::Duration::from_secs(10));

    // Get the local peer ID as a string
    let local_peer_id_str = swarm.local_peer_id();

    // If the send_order flag is set, call the client function
    if opt.send_order {
        // If the send_order flag is set, call the client function
        send_limit_order_from_client(*local_peer_id_str).await?;
        return Ok(());
    }

    // If the fetch_orders flag is set, fetch all orders from the database and print them
    // if opt.fetch_orders {
    //     let orders = db::get_all_orders(&db_pool).await?;
    //     for order in orders.iter() {
    //         println!("{:?}", order);
    //     }
    // }

    // If the get_closest_peers flag is set, get the closest peers to the provided peer ID
    if let Some(peer_id) = opt.get_closest_peers {
        // let peer_id = peer_id.unwrap_or_else(|| PeerId::random());
        // let peer_id = PeerId::random();
        println!("Searching for the closest peers to {peer_id}");
        swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
    }

    println!("Entering loop...");

    // The main event loop
    // The loop listens for incoming offers and broadcasts them to the network.
    // It also listens for network events and logs them to the console.

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

                       // Deserialize JSON string back to LimitOrder
                    if let Ok(offer) = serde_json::from_str::<LimitOrder>(&msg_str) {
                        // Assuming you have `db_pool` accessible here, e.g., passed along with the context or globally accessible
                        match insert_offer(&offer, &db_pool).await {
                            Ok(_) => println!("Offer inserted into the database successfully."),
                            Err(e) => eprintln!("Failed to insert offer into the database: {}", e),
                        }
                    } else {
                        eprintln!("Failed to deserialize received offer.");
                    }

                     // Post the offer to the offer hook if provided in the command line arguments
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
                    // TODO: Confirm our own address manually or using
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
                post(move |json| order_handler(json, offer_tx.clone())),
            )
            // .route("/get_orders", get(get_orders_handler))
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

async fn order_handler(
    Json(payload): Json<LimitOrder>,
    offer_tx_clone: tokio::sync::mpsc::Sender<Vec<u8>>,
) -> StatusCode {
    //works with String
    // println!("Received order: {:?}", payload);
    // // Directly use the payload string as the offer message
    // let offer_bytes = payload.as_bytes().to_vec();
    // if offer_tx_clone.send(offer_bytes).await.is_err() {
    //     eprintln!("Failed to send offer through the channel");
    //     StatusCode::INTERNAL_SERVER_ERROR
    // } else {
    //     StatusCode::OK
    // }

    // Serialize LimitOrder into JSON and then into bytes
    if let Ok(offer_bytes) = serde_json::to_vec(&payload) {
        if offer_tx_clone.send(offer_bytes).await.is_err() {
            eprintln!("Failed to send offer through the channel");
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        }
    } else {
        StatusCode::BAD_REQUEST
    }

    //Doesnt work right now
    // match bech32::decode(&payload.order_details) {
    //     Ok(_) => {
    //         let offer_bytes = payload.order_details.as_bytes().to_vec();
    //         if offer_tx_clone.send(offer_bytes).await.is_err() {
    //             eprintln!("Failed to send offer through the channel");
    //             StatusCode::INTERNAL_SERVER_ERROR
    //         } else {
    //             StatusCode::OK
    //         }
    //     }
    //     Err(_) => StatusCode::BAD_REQUEST,
    // }
}

// async fn get_orders_handler(db_pool: SqlitePool) -> Result<Json<Vec<LimitOrder>>, StatusCode> {
//     match db::get_all_orders(&db_pool).await {
//         Ok(orders) => Ok(Json(orders)),
//         Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
//     }
// }

async fn post_limit_order(endpoint: &str, offer: &str) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();

    let offer_json = json!({ "offer": offer });
    client.post(endpoint).json(&offer_json).send().await?;

    Ok(())
}

async fn send_limit_order_from_client(node_id: PeerId) -> Result<(), Box<dyn Error>> {
    print!("Sending limit order from client");
    let limit_order = LimitOrder::new(
        orderbook::OrderSide::Buy, // or OrderSide::Sell
        "ETH".to_string(),
        U256::from(10),  // amount
        U256::from(500), // price
        node_id,         // Replace with a valid Ethereum address
        U256::from(0),
    );

    //Serialize the limit order into JSON
    let json_payload = serde_json::to_string(&limit_order)?;

    //Create HTTP Client and send through the network
    let client = reqwest::Client::new();
    let response = client
        .post("http://localhost:3000/submit_offer")
        .header("Content-Type", "application/json")
        .body(json_payload)
        .send()
        .await?;

    println!("Response: {:?}", response.text().await?);

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

    #[clap(long, help = "Send a limit order as a client")]
    send_order: bool,

    #[clap(
        long,
        help = "Start a HTTP API for offer submission, expects JSON body {\"offer\":\"offer1...\"}",
        value_name = "HOST:PORT"
    )]
    listen_offer_submission: Option<String>,

    #[clap(long, help = "Fetch and display all current orders from the database")]
    fetch_orders: bool,

    #[clap(long, help = "Get closest peers")]
    get_closest_peers: Option<PeerId>,
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
