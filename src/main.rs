mod network;

use async_std::task::spawn;
use clap::Parser;

use futures::prelude::*;
use futures::StreamExt;
use libp2p::{core::Multiaddr, multiaddr::Protocol};
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

//The main function is the entry point of the program. It is asynchronous,
//which means it can perform non-blocking operations. It returns a Result type,
//which can either be Ok(()) if the program runs successfully, or an error of type
//Box<dyn Error> if something goes wrong.
#[async_std::main]
async fn main() -> Result<(), Box<dyn Error>> {
    //Used for initializing a global tracing subscriber, which is used for application-level logging.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    //Parse the command line arguments.
    let opt = Opt::parse();

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
