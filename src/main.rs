use futures::prelude::*;
use libp2p::swarm::SwarmEvent;
use libp2p::{ping, Multiaddr};
use std::{error::Error, time::Duration};
use tracing_subscriber::EnvFilter;

//Transport defines HOW to send bytes on the network.
//NetworkBehaviour defines WHAT to send on the network.
//Swarm combines the two and actually sends the data.
//The Swarm is the main entry point to the libp2p stack.
//It is the object that you will interact with most of the time.

//With the Swarm in place, we can lsiten for incoming connections and dial other peers if necessary.
//We can also send and receive data from other peers.
//The Swarm is generic over the Transport and NetworkBehaviour traits.
//This means that we can use any transport and any network behaviour we want.
//In this example, we will use the TCP transport and the Ping behaviour.

//We just need to pass an address to the Swarm.
//The Swarm will then listen on that address for incoming connections.

#[async_std::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_async_std()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::tls::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_behaviour(|_| ping::Behaviour::default())?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30))) // Allows us to observe pings for 30 seconds.
        .build();

    //We can now listen on a random port on all interfaces.
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Dial the peer identified by the multi-address given as the second
    // command-line argument, if any.

    //A Multiaddr is a self-describing network address.
    //It can be used to describe how to reach a node in the network.
    //For example, /ip4/

    if let Some(addr) = std::env::args().nth(1) {
        let remote: Multiaddr = addr.parse()?;
        swarm.dial(remote)?;
        println!("Dialed {addr}")
    }

    //Now we just need to drive the Swarm in a loop
    //so it can listen for incoming connections and establish
    //outgoing connections if we specify an address on the CLI.

    //The two nodes will establish a connection and send each other ping and pong messages every 15 seconds.
    //We can observe the ping and pong messages in the logs.

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => println!("Listening on {address:?}"),
            SwarmEvent::Behaviour(event) => println!("{event:?}"),
            _ => {}
        }
    }
}
