// The network module - responsible for setting up and managing the P2P network.
use futures::channel::{mpsc, oneshot}; // Asynchronous channels for communication.
use futures::prelude::*; // Importing futures utilities for async operations.

// Importing necessary components from libp2p.
use crate::orderbook::LimitOrder;
use libp2p::{
    autonat,
    core::Multiaddr,     // Multiaddress for network addresses.
    gossipsub,           // Gossipsub for pub-sub messaging. Used for gossiping order details.
    identify,            // Identify protocol for peer identification.
    identity,            // For generating identity keys.
    kad,                 // Kademlia DHT for peer discovery and content distribution.
    multiaddr::Protocol, // Networking protocols.
    noise,               // Noise protocol for encryption.
    swarm::{NetworkBehaviour, Swarm, SwarmEvent}, // Core libp2p components for networking.
    tcp,
    yamux,
    PeerId, // TCP protocol, yamux for multiplexing, and PeerId type.
};

use libp2p::StreamProtocol; // Protocol for streaming data.
use serde::{Deserialize, Serialize}; // For serializing and deserializing data.
use std::collections::hash_map::DefaultHasher;
use std::collections::{hash_map, HashMap, HashSet}; // Standard collections.

use std::error::Error; // Error handling.
use std::hash::{Hash, Hasher};
use std::time::Duration; // For specifying durations.
use tokio::io; // Used for I/O operations, meaning input/output operations.

//Defines the network architecture, behaviors, and client interface for managing peer-to-peer interactions.

/// Creates the network components, namely:
///
/// - The network client to interact with the network layer from anywhere
///   within your application.
///
/// - The network event stream, e.g. for incoming requests.
///
/// - The network task driving the network itself.
///

//Defining the network behavior combining multiple libp2p protocols.
#[derive(NetworkBehaviour)]
pub struct AvalonBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
    pub autonat: autonat::Behaviour,
}

//function to create and initialize the network components.
pub(crate) async fn new(
    secret_key_seed: Option<u8>, // Optional seed for deterministic key generation.
) -> Result<(Client, impl Stream<Item = Event>, EventLoop), Box<dyn Error>> {
    // Create a public/private key pair, either random or based on a seed, for node identity.
    let id_keys = match secret_key_seed {
        Some(seed) => {
            let mut bytes = [0u8; 32];
            bytes[0] = seed;
            identity::Keypair::ed25519_from_bytes(bytes).unwrap()
        }
        None => identity::Keypair::generate_ed25519(),
    };

    //Create a channel for sending offers to the network.
    //The channel has a buffer size of 100, which means it can hold up to 100 offers
    //before it starts blocking. The order_tx variable is used to send offers to the
    //channel, and the order_rx variable is used to receive offers from the channel.
    let (order_tx, mut order_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

    //Configure autonat
    let autonat_config = autonat::Config::default();
    let autonat = autonat::Behaviour::new(id_keys.public().to_peer_id(), autonat_config);

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

    //Channels for command and event communication.
    let (command_sender, command_receiver) = mpsc::channel(0);
    let (event_sender, event_receiver) = mpsc::channel(0);

    //Returning the network client, event receiver, and event loop.
    Ok((
        Client {
            sender: command_sender,
        },
        event_receiver,
        EventLoop::new(swarm, command_receiver, event_sender, topic),
    ))
}

//TODO: EDIT THE BELOW CODE TO FIT THE NETWORK MODULE AND ADD EVENT HANDLERS:
//THIS WILL BE THE BUSINESS LOGIC OF THE NETWORK

//The Client struct provides a simple interface for interacting with the network layer.
#[derive(Clone)]
pub(crate) struct Client {
    sender: mpsc::Sender<Command>, //sender for sending commands to the network.
}

impl Client {
    /// Listen for incoming connections on the given address.
    pub(crate) async fn start_listening(
        &mut self,       //mutably borrow the client.
        addr: Multiaddr, //network address to listen on.
    ) -> Result<(), Box<dyn Error + Send>> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::StartListening { addr, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }

    /// Function to dial the given peer at the given address.
    pub(crate) async fn dial(
        &mut self,            //mutably borrow the client.
        peer_id: PeerId,      //ID of the peer to connect to.
        peer_addr: Multiaddr, // Address of the peer to connect to.
    ) -> Result<(), Box<dyn Error + Send>> {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::Dial {
                peer_id,
                peer_addr,
                sender,
            })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }

    /// Advertise the local node as the provider of the given order on the DHT.

    pub(crate) async fn broadcast_order(&mut self, order_details: String) {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.

        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::ProvideOrder {
                order_details,
                sender,
            })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.");
    }
    /// Find the providers for the given order on the DHT.
    pub(crate) async fn get_providers(&mut self, order_details: String) -> HashSet<PeerId> {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.

        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::GetProviders {
                order_details,
                sender,
            })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }
}

//Event loop struct manages the network's event loop.
pub(crate) struct EventLoop {
    swarm: Swarm<AvalonBehaviour>, // The libp2p Swarm managing network behaviors.
    command_receiver: mpsc::Receiver<Command>, // Receiver for network commands.
    event_sender: mpsc::Sender<Event>, // Sender for network events.
    // Maps and hashes for tracking various network operations.
    pending_dial: HashMap<PeerId, oneshot::Sender<Result<(), Box<dyn Error + Send>>>>, // Pending dial operations.
    pending_start_providing: HashMap<kad::QueryId, oneshot::Sender<()>>, // Pending start providing operations.
    pending_get_providers: HashMap<kad::QueryId, oneshot::Sender<HashSet<PeerId>>>, // Pending get providers operations.
    topic: gossipsub::IdentTopic, // Gossipsub topic.
}

//Implementation of the event loop.
impl EventLoop {
    //The new function initializes the event loop.
    // It takes the libp2p Swarm, command receiver, and event sender as arguments.
    // It returns the event loop.

    fn new(
        swarm: Swarm<AvalonBehaviour>,             // The libp2p Swarm.
        command_receiver: mpsc::Receiver<Command>, // Command receiver.
        event_sender: mpsc::Sender<Event>,         // Event sender.
        topic: gossipsub::IdentTopic,              // Gossipsub topic.
    ) -> Self {
        Self {
            swarm,
            command_receiver,
            event_sender,
            // Initialize the maps and hashes.
            pending_dial: Default::default(),
            pending_start_providing: Default::default(),
            pending_get_providers: Default::default(),
            topic,
        }
    }

    // The run function drives the network event loop.
    pub(crate) async fn run(mut self) {
        loop {
            //Selecting between swarm events and command reception. (This is a common pattern in async Rust.)
            //The select! macro allows us to wait for multiple futures at the same time.
            //It returns the first future that completes.
            //The futures::select! macro is similar to the match statement, but instead of matching on values, it matches on futures.

            futures::select! {
                event = self.swarm.next() => self.handle_event(event.expect("Swarm stream to be infinite.")).await  ,
                command = self.command_receiver.next() => match command {
                    Some(c) => self.handle_command(c).await,
                    // Command channel closed, thus shutting down the network event loop.
                    None=>  return,
                },
            }
        }
    }

    //Handling various swarm events - THIS IS WHERE
    //MUCH OF THE LOGIC WILL GO
    async fn handle_event(&mut self, event: SwarmEvent<AvalonBehaviourEvent>) {
        match event {
            //Handling  connection-related events
            SwarmEvent::IncomingConnection { .. } => {}
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                //Handling a successfully established connection.
                if endpoint.is_dialer() {
                    //If the local node initiated the connection, notify the corresponding sender.
                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        let _ = sender.send(Ok(()));
                    }
                }
            }
            SwarmEvent::ConnectionClosed { .. } => {}

            //Handling Kademlia events related to outbound queries.
            SwarmEvent::Behaviour(AvalonBehaviourEvent::Kademlia(
                kad::Event::OutboundQueryProgressed {
                    id,
                    result: kad::QueryResult::StartProviding(_),
                    ..
                },
            )) => {
                //When a StartProviding query completes, notify the corresponding sender.

                let sender: oneshot::Sender<()> = self
                    .pending_start_providing
                    .remove(&id)
                    .expect("Completed query to be previously pending.");
                let _ = sender.send(()); //Sending cempletion notification.
            }
            //More Kademlia events for finding providers of an order?
            SwarmEvent::Behaviour(AvalonBehaviourEvent::Kademlia(
                kad::Event::OutboundQueryProgressed {
                    id,
                    result:
                        kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                            providers,
                            ..
                        })),
                    ..
                },
            )) => {
                //When a GetProviders query completes, notify the corresponding sender.
                if let Some(sender) = self.pending_get_providers.remove(&id) {
                    sender.send(providers).expect("Receiver not to be dropped");

                    // Finish the query. We are only interested in the first result (ie the first set of providers).
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .query_mut(&id)
                        .unwrap()
                        .finish();
                }
            }

            SwarmEvent::Behaviour(AvalonBehaviourEvent::Kademlia(_)) => {}
            //Handling gossipsub events.
            //This is where I handle received limit orders.
            //The received limit order is deserialized and handled as needed.
            SwarmEvent::Behaviour(AvalonBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                if let Ok(limit_order) = serde_json::from_slice::<LimitOrder>(&message.data) {
                    // Process the received limit order
                    // e.g., add it to the order book?
                    // Broadcast the order to the network
                    println!(
                        "Broadcasting Limit Order: {}",
                        String::from_utf8_lossy(&limit_order)
                    );

                    if let Err(e) = self
                        .swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(self.topic.clone(), limit_order)
                    {
                        eprintln!("Error broadcasting order: {:?}", e);
                    }
                }
            }

            // Handling Identify events for dynamic network adjustments
            SwarmEvent::Behaviour(AvalonBehaviourEvent::Identify(identify::Event::Received {
                info,
                peer_id,
                ..
            })) => {
                // Mark the address observed for us by the external peer as confirmed.
                for addr in info.listen_addrs {
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr);
                }
                // Optionally confirm your own address (consider libp2p-autonat)
            }
            // Add handling for autonat events
            SwarmEvent::Behaviour(AvalonBehaviourEvent::Autonat(event)) => {
                // Handle autonat events here
                // e.g., log NAT status or public address
            }

            SwarmEvent::NewListenAddr { address, .. } => {
                //Logging the new listen address of the local node.
                let local_peer_id = *self.swarm.local_peer_id();
                eprintln!(
                    "Local node is listening on {:?}",
                    address.with(Protocol::P2p(local_peer_id))
                );
            }

            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(peer_id) = peer_id {
                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        let _ = sender.send(Err(Box::new(error)));
                    }
                }
            }
            SwarmEvent::IncomingConnectionError { .. } => {}
            SwarmEvent::Dialing {
                peer_id: Some(peer_id),
                ..
            } => eprintln!("Dialing {peer_id}"),
            e => panic!("{e:?}"),
        }
    }

    //Handling various network commands sent to the event loop.
    async fn handle_command(&mut self, command: Command) {
        match command {
            //Handling command to start listening for incoming connections.
            Command::StartListening { addr, sender } => {
                //Attempt to listen on the given address .
                let _ = match self.swarm.listen_on(addr) {
                    Ok(_) => sender.send(Ok(())),            //Notify success
                    Err(e) => sender.send(Err(Box::new(e))), //Notify error
                };
            }
            //Handling command to dial/connect to a peer.
            Command::Dial {
                peer_id,
                peer_addr,
                sender,
            } => {
                //If the peer is not already being dialed, dial the peer and instiate a connection.
                if let hash_map::Entry::Vacant(e) = self.pending_dial.entry(peer_id) {
                    //Adding peer address to Kademlia DHT and attempting to dial the peer.
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, peer_addr.clone());
                    match self.swarm.dial(peer_addr.with(Protocol::P2p(peer_id))) {
                        Ok(()) => {
                            e.insert(sender); //Storing the sender for notifying the caller of the result.
                        }
                        Err(e) => {
                            let _ = sender.send(Err(Box::new(e))); //Notify error if dial fails
                        }
                    }
                } else {
                    todo!("Already dialing peer."); //Placeholder: Handle already dialing peer.
                }
            }

            Command::ProvideOrder {
                order_details,
                sender,
            } => {
                //Initiate providing the order through Kademlia DHT.
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .start_providing(order_details.into_bytes().into())
                    .expect("No store error.");
                self.pending_start_providing.insert(query_id, sender);
            }
            Command::ReceiveOrder {
                order_details,
                sender,
            } => {
                //Initiate receiving the order through Kademlia DHT.
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .start_providing(order_details.into_bytes().into())
                    .expect("No store error.");
                self.pending_start_providing.insert(query_id, sender);
            }
            //Handling command to start providing a file.
            Command::StartProviding {
                order_details,
                sender,
            } => {
                //Initiate providing the file through Kademlia DHT.
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .start_providing(order_details.into_bytes().into())
                    .expect("No store error.");
                self.pending_start_providing.insert(query_id, sender);
            }
            //Handling command to get providers of a file.
            //This is a query to the Kademlia DHT.
            //The result is returned through the sender.
            //The sender is stored in a map for later retrieval.
            Command::GetProviders {
                order_details,
                sender,
            } => {
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .get_providers(order_details.into_bytes().into());
                self.pending_get_providers.insert(query_id, sender);
            }
        }
    }
}

//Enum defining possible commands
#[derive(Debug)]
enum Command {
    // Different types of commands for network operations.
    StartListening {
        addr: Multiaddr,
        sender: oneshot::Sender<Result<(), Box<dyn Error + Send>>>,
    },
    Dial {
        peer_id: PeerId,
        peer_addr: Multiaddr,
        sender: oneshot::Sender<Result<(), Box<dyn Error + Send>>>,
    },
    StartProviding {
        order_details: String,
        sender: oneshot::Sender<()>,
    },
    ProvideOrder {
        order_details: String,
        sender: oneshot::Sender<()>,
    },
    ReceiveOrder {
        order_details: String,
        sender: oneshot::Sender<()>,
    },
    GetProviders {
        order_details: String,
        sender: oneshot::Sender<HashSet<PeerId>>,
    },
}

//Enum for different types of events that can be emitted.

#[derive(Debug)]
pub(crate) enum Event {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LimitOrderRequest {
    // Define the fields for a limit order request
    // e.g., order type, amount, price, asset, etc.
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LimitOrderResponse {
    // Define the fields for a limit order response
    // e.g., confirmation of order received, order status, etc.
}
