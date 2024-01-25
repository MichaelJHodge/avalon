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
        // Providing a file.
        CliArgument::Provide { path, name } => {
            // Advertise oneself as a provider of the file on the DHT.
            network_client.start_providing(name.clone()).await;

            loop {
                match network_events.next().await {
                    // Reply with the content of the file on incoming requests.
                    Some(network::Event::InboundRequest { request, channel }) => {
                        if request == name {
                            network_client
                                .respond_file(std::fs::read(&path)?, channel)
                                .await;
                        }
                    }
                    e => todo!("{:?}", e),
                }
            }
        }

        //If the Get variant is provided, the program first locates all nodes that provide
        //the file. It then requests the content of the file from each node and ignores
        //the remaining requests once a single one succeeds. The content of the file is
        //then written to the standard output (stdout).

        // Locating and getting a file.
        CliArgument::Get { name } => {
            // Locate all nodes providing the file.
            let providers = network_client.get_providers(name.clone()).await;
            if providers.is_empty() {
                return Err(format!("Could not find provider for file {name}.").into());
            }

            // Request the content of the file from each node.
            let requests = providers.into_iter().map(|p| {
                let mut network_client = network_client.clone();
                let name = name.clone();
                async move { network_client.request_file(p, name).await }.boxed()
            });

            // Await the requests, ignore the remaining once a single one succeeds.
            let file_content = futures::future::select_ok(requests)
                .await
                .map_err(|_| "None of the providers returned file.")?
                .0;

            std::io::stdout().write_all(&file_content)?;
        }
    }

    Ok(())
}

#[derive(Parser, Debug)]
#[clap(name = "libp2p file sharing")]
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
    Provide {
        #[clap(long)]
        path: PathBuf,
        #[clap(long)]
        name: String,
    },
    Get {
        #[clap(long)]
        name: String,
    },
}
