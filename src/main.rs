// Copyright 2018 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

// #![doc = include_str!("../README.md")]

use futures::{executor::block_on, prelude::*, select};
use libp2p::{
    core::muxing::StreamMuxerBox,
    dns::TokioDnsConfig,
    gossipsub, identity,
    kad::{store::MemoryStore, Kademlia, KademliaConfig, KademliaEvent},
    mdns, quic,
    swarm::{
        dial_opts::{DialOpts, PeerCondition},
        NetworkBehaviour,
    },
    swarm::{DialError, SwarmBuilder, SwarmEvent},
    Multiaddr, PeerId, StreamProtocol, Swarm, Transport,
};
use log::{error, info};
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use std::{collections::hash_map::DefaultHasher, str::FromStr};
use tokio_stream::{Stream, StreamExt};

// We create a custom network behaviour that combines Gossipsub and Mdns.
#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    kad: libp2p::kad::Kademlia<MemoryStore>,
    // mdns: mdns::async_io::Behaviour,
}
pub const MAINNET_BOOTSTRAP_ADDRS: &str = "/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWQp644DK27fd3d4Km3jr7gHiuJJ5ZGmy8hH4py7fP4FP7,/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWNQ9tVrcb64tw6bNs2CaNrUGPM7yRrKvBBheQ5yCyPHKC";
pub const TESTNET_BOOTSTRAP_ADDRS: &str = "/dns4/wormhole-testnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWAkB9ynDur1Jtoa97LBUp8RXdhzS5uHgAfdTquJbrbN7i";
pub const MAINNET_BOOTSTRAP_MULTIADDR: &str =
    "/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic";
pub const TESTNET_BOOTSTRAP_MULTIADDR: &str =
    "/dns4/wormhole-testnet-v2-bootstrap.certus.one/udp/8999/quic";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Create a random PeerId
    let id_keys = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(id_keys.public());
    let bootstrappers = bootstrap_addrs(MAINNET_BOOTSTRAP_ADDRS, &local_peer_id);
    println!("Local peer id: {local_peer_id}");

    let stream_protocol = StreamProtocol::new("/wormhole/mainnet/2");
    let mut cfg = KademliaConfig::default()
        .set_protocol_names(vec![stream_protocol])
        .to_owned();

    cfg.set_query_timeout(Duration::from_secs(60));
    let store = MemoryStore::new(local_peer_id);
    let mut kad_behaviour = Kademlia::with_config(local_peer_id, store, cfg);
    kad_behaviour.set_mode(Some(libp2p::kad::Mode::Server));
    for p in bootstrappers.0.iter() {
        kad_behaviour.add_address(&p, MAINNET_BOOTSTRAP_MULTIADDR.parse()?);
    }
    kad_behaviour.bootstrap()?;

    let mut quic_config = quic::Config::new(&id_keys);
    quic_config.support_draft_29 = true;
    let transport = {
        let mut quic_config = quic::Config::new(&id_keys);
        quic_config.support_draft_29 = true;
        let quic_transport = quic::tokio::Transport::new(quic_config);
        tokio::task::spawn_blocking(|| TokioDnsConfig::system(quic_transport))
            .await
            .expect("Dns configuration failed")
            .unwrap()
            .map(|either_output, _| match either_output {
                (peer_id, muxer) => (peer_id, StreamMuxerBox::new(muxer)),
            })
            .boxed()
    };
    // To content-address message, we can take the hash of message and use it as an ID.
    let message_id_fn = |message: &gossipsub::Message| {
        let mut s = DefaultHasher::new();
        message.data.hash(&mut s);
        gossipsub::MessageId::from(s.finish().to_string())
    };

    // Set a custom gossipsub configuration
    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(60)) // This is set to aid debugging by not cluttering the log space
        .validation_mode(gossipsub::ValidationMode::Strict) // This sets the kind of message validation. The default is Strict (enforce message signing)
        .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
        .build()
        .expect("Valid config");

    // build a gossipsub network behaviour
    let mut gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(id_keys),
        gossipsub_config,
    )
    .expect("Correct configuration");
    // Create a Gossipsub topic
    let topic = gossipsub::IdentTopic::new(format!("{}/{}", "/wormhole/mainnet/2", "broadcast"));

    // subscribes to our topic
    gossipsub.subscribe(&topic)?;

    // Create a Swarm to manage peers and events
    let mut swarm = {
        let mut behaviour = MyBehaviour {
            gossipsub,
            kad: kad_behaviour,
        };
        behaviour.gossipsub.subscribe(&topic)?;
        SwarmBuilder::with_tokio_executor(transport, behaviour, local_peer_id).build()
    };
    // swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/udp/8999/quic".parse()?)?;
    swarm.listen_on("/ip6/::/udp/8999/quic".parse()?)?;
    // println!("reaches here atleast");
    //
    // println!("bootstrap ids: {:?}", bootstrappers.0);
    if bootstrappers.1 {
        println!("We're a bootstrap node lol");
    }
    // let dial_addr: Multiaddr = MAINNET_BOOTSTRAP_MULTIADDR.parse().unwrap();
    // for p in bootstrappers.0  {
    //     swarm.dial(p)?
    // }
    // let ma = Multiaddr::from_str(BOOTSTRAP_ADDRS)?;
    // let peer_id =  ma.iter().find_map(|proto| {
    //     if let libp2p::multiaddr::Protocol::P2p(hash) = proto {
    //         Some(PeerId::from_multihash(hash.clone().into()))
    //     } else {
    //         None
    //     }}).unwrap().unwrap();

    // println!("peer: {:?}", peer_id);
    // {
    //     Some(id) => id,
    //     None => {
    //         error!("Invalid bootstrap address: {}", addr_str);
    //     }
    // };
    // let bootstrappers = bootstrap_addrs(MAINNET_BOOTSTRAP_ADDRS, &local_peer_id);

    // swarm.dial("12D3KooWQp644DK27fd3d4Km3jr7gHiuJJ5ZGmy8hH4py7fP4FP7".parse::<PeerId>()?)?;
    // swarm.dial("12D3KooWNQ9tVrcb64tw6bNs2CaNrUGPM7yRrKvBBheQ5yCyPHKC".parse::<PeerId>()?)?;

    let succ = connect(bootstrappers.0, &mut swarm)?;
    println!("success new: {:?}", succ);
    // let successful_connections = connect_peers(bootstrappers.0, &mut swarm)?;
    // let success_connections_v2 = connect_peers_v2(local_peer_id, bootstrappers.0, &mut swarm)?;
    // let ninfo =  swarm.network_info();
    // println!("Network INFO: {:?}", ninfo);
    // // if swarm.dial(peer_id).is_ok(){
    // //     println!("dialed to wormhole");
    // // }
    // println!("IT DOES CONNECT TO THE MAINNET BOOTSTRAP PEERS: {}", successful_connections);
    // println!("IT DOES CONNECT TO THE MAINNET BOOTSTRAP PEERS V2: {}", success_connections_v2);

    // println!("Enter messages via STDIN and they will be sent to connected peers using Gossipsub");

    // Kick it off
    loop {
        tokio::select! {
            // line = stdin.select_next_some() => {
            //     if let Err(e) = swarm
            //         .behaviour_mut().gossipsub
            //         .publish(topic.clone(), line.expect("Stdin not to close").as_bytes()) {
            //         println!("Publish error: {e:?}");
            //     }
            // },
            event = swarm.select_next_some() => match event {

                SwarmEvent::Dialing{peer_id, connection_id} => {
                    print!("Dialing TOOO: {:?}",peer_id )
                },
                SwarmEvent::OutgoingConnectionError{peer_id,error, ..} => {
                    println!("FAILED PEER ID: {:?}", peer_id);
                    println!("PROLLY ERRORRED!: {:?}", error );
                }
                SwarmEvent::ConnectionEstablished{peer_id, connection_id, endpoint, num_established, concurrent_dial_errors, established_in } => {
                    println!("DID IT HAPPEN ACTUALY?:  {:?}, Connected ID: {:?}", num_established, peer_id);
                    println!("or maybe NOOTTT? {:?}", concurrent_dial_errors);
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Kad(KademliaEvent::RoutingUpdated { peer, is_new_peer, addresses, bucket_range, old_peer })) => {
                    // for addr in addresses.iter(){
                    //     println!("Kademlia added a new peer: {addr}");
                    // }
                    println!("Any peers?: {:?}", peer);
                    println!("IS NEW PEER?: {:?}", is_new_peer);
                    println!("Addresses = {:?}", addresses);

                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Kad(KademliaEvent::OutboundQueryProgressed { id, result, stats, step })) => {
                    println!("QUERY RESULT?: {:?}", result);
                },
                // SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                //     for (peer_id, _multiaddr) in list {
                //         println!("mDNS discover peer has expired: {peer_id}");
                //         swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                //     }
                // },
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => println!(
                        "Got message: '{}' with id: {id} from peer: {peer_id}",
                        String::from_utf8_lossy(&message.data),
                    ),
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed{
                    peer_id,
                    topic,
                })) => println!(
                    "Subscribed to: '{}' from id: {peer_id}",
                    topic.to_string(),
                ),
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Local node is listening on {address}");
                }

                _ => {}
            }
        }
    }
}

pub fn bootstrap_addrs(bootstrap_peers: &str, self_id: &PeerId) -> (Vec<PeerId>, bool) {
    let mut bootstrappers = Vec::new();
    let mut is_bootstrap_node = false;

    for addr_str in bootstrap_peers.split(",") {
        if addr_str.is_empty() {
            continue;
        }

        let multi_address = match Multiaddr::from_str(addr_str) {
            Ok(addr) => addr,
            Err(e) => {
                error!("Invalid bootstrap address: {}, Error: {}", addr_str, e);
                continue;
            }
        };

        let peer_id = match multi_address.iter().find_map(|proto| {
            if let libp2p::multiaddr::Protocol::P2p(hash) = proto {
                Some(PeerId::from_multihash(hash.clone().into()))
            } else {
                None
            }
        }) {
            Some(id) => id,
            None => {
                error!("Invalid bootstrap address: {}", addr_str);
                continue;
            }
        };

        if peer_id.unwrap() == *self_id {
            info!("We're a bootstrap node");
            is_bootstrap_node = true;
            continue;
        }

        bootstrappers.push(peer_id.unwrap());
    }

    (bootstrappers, is_bootstrap_node)
}

pub fn connect_peers(
    peers: Vec<PeerId>,
    swarm: &mut Swarm<MyBehaviour>,
) -> Result<usize, DialError> {
    let mut success_counter = 0usize;
    for &peer in &peers {
        let dial_opts = DialOpts::peer_id(peer)
        .condition(PeerCondition::Disconnected)
        .addresses(
            vec![
                "/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWQp644DK27fd3d4Km3jr7gHiuJJ5ZGmy8hH4py7fP4FP7".parse::<Multiaddr>().unwrap(),
                "/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWNQ9tVrcb64tw6bNs2CaNrUGPM7yRrKvBBheQ5yCyPHKC".parse::<Multiaddr>().unwrap(),
            ]
        )
        .extend_addresses_through_behaviour()
        // .override_role()
        .build();
        match swarm.dial(dial_opts) {
            Ok(_) => success_counter += 1,
            Err(_) => return Err(DialError::Aborted),
        }
    }
    Ok(success_counter)
}

pub fn connect(peers: Vec<PeerId>, swarm: &mut Swarm<MyBehaviour>) -> Result<usize, DialError> {
    let mut success_counter = 0usize;
    for &peer in &peers {
        match swarm.dial(peer) {
            Ok(_) => success_counter += 1,
            Err(_) => return Err(DialError::Aborted),
        }
    }
    Ok(success_counter)
}

pub fn connect_peers_v2(
    local_id: PeerId,
    peers: Vec<PeerId>,
    swarm: &mut Swarm<MyBehaviour>,
) -> Result<usize, DialError> {
    let mut success_counter = 0usize;
    for _i in 0..peers.len() {
        let dial_opts = DialOpts::peer_id(local_id)
        .condition(PeerCondition::Disconnected)
        .addresses(
            vec![
                "/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWQp644DK27fd3d4Km3jr7gHiuJJ5ZGmy8hH4py7fP4FP7".parse::<Multiaddr>().unwrap(),
                "/dns4/wormhole-mainnet-v2-bootstrap.certus.one/udp/8999/quic/p2p/12D3KooWNQ9tVrcb64tw6bNs2CaNrUGPM7yRrKvBBheQ5yCyPHKC".parse::<Multiaddr>().unwrap(),
            ]
        )
        .extend_addresses_through_behaviour()
        .override_role()
        .build();
        match swarm.dial(dial_opts) {
            Ok(_) => success_counter += 1,
            Err(_) => return Err(DialError::Aborted),
        }
    }
    Ok(success_counter)
}
