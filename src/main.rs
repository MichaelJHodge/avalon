mod network;

use async_std::task::spawn;
use clap::Parser;

use futures::StreamExt;
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;
// The network module - responsible for setting up and managing the P2P network.
use futures::channel::{mpsc, oneshot}; // Asynchronous channels for communication.
use futures::prelude::*; // Importing futures utilities for async operations.

// Importing necessary components from libp2p.
use libp2p::StreamProtocol; // Protocol for streaming data.
use libp2p::{
    core::Multiaddr, // Multiaddress for network addresses.
    gossipsub,       // Gossipsub for pub-sub messaging. Used for gossiping order details.
    identify,        // For generating identity keys.
    identity,
    kad,                 // Kademlia DHT for peer discovery and content distribution.
    multiaddr::Protocol, // Networking protocols.
    noise,               // Noise protocol for encryption.
    request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel}, // For request-response communication pattern.
    swarm::{NetworkBehaviour, Swarm, SwarmEvent}, // Core libp2p components for networking.
    tcp,
    yamux,
    PeerId, // TCP protocol, yamux for multiplexing, and PeerId type.
};
use serde::{Deserialize, Serialize}; // For serializing and deserializing data.
use std::collections::{hash_map, HashMap, HashSet}; // Standard collections.
use std::time::Duration;
use tokio::{io, select, time};

//Defining the network behavior combining multiple libp2p protocols.
#[derive(NetworkBehaviour)]
struct AvalonBehaviour {
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
}

//The main function is the entry point of the program. It is asynchronous,
//which means it can perform non-blocking operations. It returns a Result type,
//which can either be Ok(()) if the program runs successfully, or an error of type
//Box<dyn Error> if something goes wrong.
#[async_std::main]
async fn main() -> Result<(), Box<dyn Error>> {
    //Parse the command line arguments.
    let opt = Opt::parse();

    // Create a public/private key pair, either random or based on a seed, for node identity.
    let id_keys = match opt.peer_id_file {
        Some(ref file_path) if fs::metadata(file_path).is_ok() => get_keypair_from_file(file_path)?,
        _ => {
            let keypair = identity::Keypair::generate_ed25519();
            if let Some(ref file_path) = opt.peer_id_file {
                save_keypair_file(&keypair, file_path)?;
            }
            keypair
        }
    };
    //Convert public key to peer ID.
    let peer_id = id_keys.public().to_peer_id();

    //Used for initializing a global tracing subscriber, which is used for application-level logging.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    //Building the libp2p swarm.
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(id_keys)
        // .with_tokio()
        .with_async_std()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
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

            let mut cfg = kad::Config::default();

            cfg.set_protocol_names(vec![StreamProtocol::try_from_owned(
                "/avalon/kad/1".to_string(),
            )?]);

            cfg.set_query_timeout(Duration::from_secs(60));
            let store = kad::store::MemoryStore::new(key.public().to_peer_id());

            let mut kademlia = kad::Behaviour::with_config(key.public().to_peer_id(), store, cfg);

            kademlia.bootstrap().unwrap();

            let identify = identify::Behaviour::new(identify::Config::new(
                "/avalon/id/1".into(),
                key.public().clone(),
            ));

            Ok(AvalonBehaviour {
                gossipsub,
                kademlia,
                identify,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm
        .behaviour_mut()
        .kademlia
        .set_mode(Some(kad::Mode::Server)); // Setting the Kademlia DHT to server mode.

    //Create a new network client, network events stream, and network event loop.
    //The network event loop is then spawnwed as a task to run in the background.
    let (mut network_client, mut network_events, network_event_loop) =
        network::new(opt.secret_key_seed).await?;

    // Spawn the network task for it to run in the background.
    spawn(network_event_loop.run());

    //The program then starts listening for incoming connections. If a listening address
    //is provided, it uses that; otherwise, it listens on any address.

    // In case a listen address was provided use it, otherwise listen on any
    // address.
    match opt.listen_address {
        Some(addr) => network_client
            .start_listening(addr)
            .await
            .expect("Listening not to fail."),
        None => network_client
            .start_listening("/ip4/0.0.0.0/tcp/0".parse()?)
            .await
            .expect("Listening not to fail."),
    };

    // In case the user provided an address of a peer on the CLI, dial it.
    if let Some(addr) = opt.peer {
        let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
            return Err("Expect peer multiaddr to contain peer ID.".into());
        };
        network_client
            .dial(peer_id, addr)
            .await
            .expect("Dial to succeed");
    }

    //Then check the argument field of opt to see what the user wants to do.
    //If the Provide variant is provided, the program advertises itself as a provider
    //of the file on the DHT. It then waits for incoming requests for the file and
    //responds with the content of the file on incoming requests.

    match opt.argument {
        //Provide an order and propogate it to other peers
        CliArgument::ProvideOrder { order_details } => {}
        //Rceive an order
        CliArgument::ReceiveOrder { order_details } => {}
    }

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

fn get_keypair_from_file(file_path: &str) -> io::Result<identity::Keypair> {
    let contents = fs::read_to_string(file_path)?;
    let keypair_json: IdentityJson = serde_json::from_str(&contents)?;
    identity::Keypair::from_protobuf_encoding(&keypair_json.identity)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid keypair data"))
}

#[derive(Serialize, Deserialize)]
struct IdentityJson {
    identity: Vec<u8>,
}

#[derive(Parser, Debug)]
#[clap(name = "avalon")]
/// Represents the command line options for the program.
struct Opt {
    /// Fixed value to generate deterministic peer ID.
    #[clap(long)]
    secret_key_seed: Option<u8>,

    /// The peer address to connect to.
    #[clap(long)]
    peer: Option<Multiaddr>,

    //Store peer id in a file for resuse if necessary
    #[clap(
        long,
        short,
        help = "Store peer id in a file for resuse if necessary (only useful for known peers)"
    )]
    peer_id_file: Option<String>,

    /// The address to listen on for incoming connections.
    #[clap(long)]
    listen_address: Option<Multiaddr>,

    /// The subcommand to execute.
    #[clap(subcommand)]
    argument: CliArgument,
}

#[derive(Debug, Parser)]

/// Represents the subcommands of the program.
enum CliArgument {
    ProvideOrder {
        #[clap(long)]
        order_details: String,
    },
    ReceiveOrder {
        #[clap(long)]
        order_details: String,
    },
}
