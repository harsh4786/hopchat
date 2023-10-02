## Hopchat

An open source implementation of wormhole's p2p stack in Rust language. 
This is a bare minimum example of how wormhole uses libp2p stack for its
peer-to-peer networking between the guardian nodes in the network. 

Here's how the p2p stack works at a high level: 
- The node first generates an Ed25519 keypair if a keypair doesn't exist already.
- The node takes in a string slice parses it into a libp2p Multiaddress and a PeerID,
  this is the PeerId and multiaddress of the node that we will be dialing to later in
  the process. This would be the address of our bootstrap node.
- Then we setup [Kademlia](https://en.wikipedia.org/wiki/Kademlia), which is a distributed 
  Hash table designed for decentralized peer-to-peer networks, using libp2p's implementation. 
  This adds the multiaddress and the peer id of the bootstrap node to our Kademlia DHT.
- Then we setup the transport layer for our node, we use an older version of Quic as the nodes
  use an older version of quic as described in [draft-29](https://datatracker.ietf.org/doc/html/draft-ietf-quic-transport-29). However wormhole will soon upgrade to quic-v1 based on my chats
  with the wormhole core devs.
- After setting up quic, we move on to setting up the Gossipsub protocol, this is the protocol
  that our test node will use to communicate and listen to messages from the wormhole guardian
  network, we make a gossipsub topic called `/wormhole/mainnet/2/broadcast`, if we need to 
  connect to a testnet node then: `/wormhole/testnet/broadcast`.
- We also have a ping protocol just to examine the liveness of our connection to the bootstrap
  node, if the bootstrap node is still connected to our test node, the pings will continue to
  log at regular intervals.
- We build the `Swarm` object with above configurations and start listening on these addresses
  `/ip4/0.0.0.0/udp/8999/quic` and `/ip6/::/udp/8999/quic`
- Lastly, we set an async event loop to listen for messages through the event interface provided by libp2p.