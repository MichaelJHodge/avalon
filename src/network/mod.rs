// The network module - responsible for setting up and managing the P2P network.
use futures::channel::{mpsc, oneshot}; // Asynchronous channels for communication.
use futures::prelude::*; // Importing futures utilities for async operations.

// Importing necessary components from libp2p.
use libp2p::{
    core::Multiaddr,     // Multiaddress for network addresses.
    identity,            // For generating identity keys.
    kad,                 // Kademlia DHT for peer discovery and content distribution.
    multiaddr::Protocol, // Networking protocols.
    noise,               // Noise protocol for encryption.
    request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel}, // For request-response communication pattern.
    swarm::{NetworkBehaviour, Swarm, SwarmEvent}, // Core libp2p components for networking.
    tcp,
    yamux,
    PeerId, // TCP protocol, yamux for multiplexing, and PeerId type.
};

use libp2p::StreamProtocol; // Protocol for streaming data.
use serde::{Deserialize, Serialize}; // For serializing and deserializing data.
use std::collections::{hash_map, HashMap, HashSet}; // Standard collections.
use std::error::Error; // Error handling.
use std::time::Duration; // For specifying durations.

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
    //Convert public key to peer ID.
    let peer_id = id_keys.public().to_peer_id();

    //Building the libp2p swarm.
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(id_keys)
        // .with_tokio()
        .with_async_std()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| Behaviour {
            //Setting up the Kademlia DHT for peer dsicovery and content distribution.
            kademlia: kad::Behaviour::new(
                peer_id,
                kad::store::MemoryStore::new(key.public().to_peer_id()),
            ),
            //Setting up the request-response protocol for file exchange.
            request_response: request_response::cbor::Behaviour::new(
                [(StreamProtocol::new("/avalon/kad/1"), ProtocolSupport::Full)],
                request_response::Config::default(),
            ),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm
        .behaviour_mut()
        .kademlia
        .set_mode(Some(kad::Mode::Server)); // Setting the Kademlia DHT to server mode.

    //Channels for command and event communication.
    let (command_sender, command_receiver) = mpsc::channel(0);
    let (event_sender, event_receiver) = mpsc::channel(0);

    //Returning the network client, event receiver, and event loop.
    Ok((
        Client {
            sender: command_sender,
        },
        event_receiver,
        EventLoop::new(swarm, command_receiver, event_sender),
    ))
}

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

    /// Advertise the local node as the provider of the given file on the DHT.
    pub(crate) async fn start_providing(&mut self, file_name: String) {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.

        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::StartProviding { file_name, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.");
    }

    /// Find the providers for the given file on the DHT.
    pub(crate) async fn get_providers(&mut self, file_name: String) -> HashSet<PeerId> {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.

        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::GetProviders { file_name, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }

    /// Request the content of the given file from the given peer.
    pub(crate) async fn request_file(
        &mut self,
        peer: PeerId,      //The peer to request the file from.
        file_name: String, //The name of the file to request.
    ) -> Result<Vec<u8>, Box<dyn Error + Send>> {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.

        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::RequestFile {
                file_name,
                peer,
                sender,
            })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not be dropped.")
    }

    /// Respond with the provided file content to the given request.
    pub(crate) async fn respond_file(
        &mut self,
        file: Vec<u8>, //The file content to send. This is a vector of bytes.
        channel: ResponseChannel<FileResponse>, //The channel to send the response on.
    ) {
        //Similar pattern as start_listening in that we send a command to the network and wait for the response.

        self.sender
            .send(Command::RespondFile { file, channel })
            .await
            .expect("Command receiver not to be dropped.");
    }
}

//Event loop struct manages the network's event loop.
pub(crate) struct EventLoop {
    swarm: Swarm<Behaviour>, // The libp2p Swarm managing network behaviors.
    command_receiver: mpsc::Receiver<Command>, // Receiver for network commands.
    event_sender: mpsc::Sender<Event>, // Sender for network events.
    // Maps and hashes for tracking various network operations.
    pending_dial: HashMap<PeerId, oneshot::Sender<Result<(), Box<dyn Error + Send>>>>, // Pending dial operations.
    pending_start_providing: HashMap<kad::QueryId, oneshot::Sender<()>>, // Pending start providing operations.
    pending_get_providers: HashMap<kad::QueryId, oneshot::Sender<HashSet<PeerId>>>, // Pending get providers operations.
    pending_request_file:
        HashMap<OutboundRequestId, oneshot::Sender<Result<Vec<u8>, Box<dyn Error + Send>>>>, // Pending request file operations.
}

//Implementation of the event loop.
impl EventLoop {
    //The new function initializes the event loop.
    // It takes the libp2p Swarm, command receiver, and event sender as arguments.
    // It returns the event loop.

    fn new(
        swarm: Swarm<Behaviour>,                   // The libp2p Swarm.
        command_receiver: mpsc::Receiver<Command>, // Command receiver.
        event_sender: mpsc::Sender<Event>,         // Event sender.
    ) -> Self {
        Self {
            swarm,
            command_receiver,
            event_sender,
            // Initialize the maps and hashes.
            pending_dial: Default::default(),
            pending_start_providing: Default::default(),
            pending_get_providers: Default::default(),
            pending_request_file: Default::default(),
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
    async fn handle_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            //Handling Kademlia events related to outbound queries.
            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(
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
            //More Kademlia events for finding providers of a file.
            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(
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
            //Handling other Kademlia events and request-response protocol events.
            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(
                kad::Event::OutboundQueryProgressed {
                    result:
                        kad::QueryResult::GetProviders(Ok(
                            kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                        )),
                    ..
                },
            )) => {}
            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(_)) => {}

            SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(
                request_response::Event::Message { message, .. },
            )) => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    self.event_sender
                        .send(Event::InboundRequest {
                            request: request.0,
                            channel,
                        })
                        .await
                        .expect("Event receiver not to be dropped.");
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    let _ = self
                        .pending_request_file
                        .remove(&request_id)
                        .expect("Request to still be pending.")
                        .send(Ok(response.0));
                }
            },
            SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(
                request_response::Event::OutboundFailure {
                    request_id, error, ..
                },
            )) => {
                let _ = self
                    .pending_request_file
                    .remove(&request_id)
                    .expect("Request to still be pending.")
                    .send(Err(Box::new(error)));
            }
            SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(
                request_response::Event::ResponseSent { .. },
            )) => {}

            SwarmEvent::NewListenAddr { address, .. } => {
                //Logging the new listen address of the local node.
                let local_peer_id = *self.swarm.local_peer_id();
                eprintln!(
                    "Local node is listening on {:?}",
                    address.with(Protocol::P2p(local_peer_id))
                );
            }
            //Handling more connection-related events
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
            //Handling command to start providing a file.
            Command::StartProviding { file_name, sender } => {
                //Initiate providing the file through Kademlia DHT.
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .start_providing(file_name.into_bytes().into())
                    .expect("No store error.");
                self.pending_start_providing.insert(query_id, sender);
            }
            //Handling command to get providers of a file.
            //This is a query to the Kademlia DHT.
            //The result is returned through the sender.
            //The sender is stored in a map for later retrieval.
            Command::GetProviders { file_name, sender } => {
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .get_providers(file_name.into_bytes().into());
                self.pending_get_providers.insert(query_id, sender);
            }
            //Handling command to request a file from a peer.
            //This is a request-response operation.
            //The result is returned through the sender.
            //The sender is stored in a map for later retrieval.
            Command::RequestFile {
                file_name,
                peer,
                sender,
            } => {
                let request_id = self
                    .swarm
                    .behaviour_mut()
                    .request_response
                    .send_request(&peer, FileRequest(file_name));
                self.pending_request_file.insert(request_id, sender);
            }
            //Handling command to respond to a file request.
            //This is a request-response operation.
            //The response is sent through the channel.
            //The channel is provided by the request-response protocol.
            //The channel is stored in a map for later retrieval.
            //The request-response protocol handles the rest of the operation.
            Command::RespondFile { file, channel } => {
                self.swarm
                    .behaviour_mut()
                    .request_response
                    .send_response(channel, FileResponse(file))
                    .expect("Connection to peer to be still open.");
            }
        }
    }
}

//Defining the network behavior combining multiple libp2p protocols.
#[derive(NetworkBehaviour)]
struct Behaviour {
    request_response: request_response::cbor::Behaviour<LimitOrderRequest, LimitOrderResponse>,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
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
        file_name: String,
        sender: oneshot::Sender<()>,
    },
    ProvideOrder {
        order_details: String,
        sender: oneshot::Sender<()>,
    },
    GetProviders {
        file_name: String,
        sender: oneshot::Sender<HashSet<PeerId>>,
    },
    RequestFile {
        file_name: String,
        peer: PeerId,
        sender: oneshot::Sender<Result<Vec<u8>, Box<dyn Error + Send>>>,
    },
    RespondFile {
        file: Vec<u8>,
        channel: ResponseChannel<FileResponse>,
    },
}

//Enum for different types of events that can be emitted.

#[derive(Debug)]
pub(crate) enum Event {
    InboundRequest {
        request: String,
        channel: ResponseChannel<FileResponse>,
    },
}

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

// Simple file exchange protocol
// The request-response protocol is used for file exchange.
// The request is the file name, and the response is the file content.
// The request-response protocol is a generic protocol that can be used for any type of request and response.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileRequest(String);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileResponse(Vec<u8>);
