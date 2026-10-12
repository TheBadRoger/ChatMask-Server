use chatmask_server::{Args, BehaviourEvent, Registry, Request, Response, Result};
use clap::Parser;
use futures::StreamExt;
use libp2p::{multiaddr::Protocol, request_response, swarm::SwarmEvent};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let key = chatmask_server::load_identity(&args.key)?;
    let peer = key.public().to_peer_id();
    let mut swarm = chatmask_server::build(&args, key)?;
    let mut registry = Registry::new(args.max_clients, args.ttl_seconds);
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut endpoints = HashMap::new();
    let shutdown = shutdown();
    tokio::pin!(shutdown);
    println!("Server PeerId: {peer}");
    loop {
        tokio::select! {
            result = &mut shutdown => { result?; println!("Shutting down"); break; },
            _ = tick.tick() => registry.expire(),
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => println!("Control address: {}", address.with(Protocol::P2p(peer))),
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => { if let Some(address) = chatmask_server::socket_address(endpoint.get_remote_address()) { endpoints.insert(peer_id, address); } },
                SwarmEvent::ConnectionClosed { peer_id, .. } if !swarm.is_connected(&peer_id) => { registry.remove(peer_id); endpoints.remove(&peer_id); },
                SwarmEvent::Behaviour(BehaviourEvent::Control(request_response::Event::Message { peer, message: request_response::Message::Request { request, channel, .. }, .. })) => {
                    let response = match request {
                        Request::Register => match endpoints.get(&peer) { Some(endpoint) => registry.register(peer, *endpoint), None => Response::Error { message: "QUIC connection required".into() } },
                        Request::Connect { peer: target } => match target.parse().map_err(|_| "Invalid target".into()).and_then(|target| registry.connect(peer, target).map(|pair| (target, pair))) {
                            Ok((target, (a, b, attempt))) => {
                                swarm.behaviour_mut().control.send_request(&target, Request::Punch { peer: peer.to_string(), endpoint: a, attempt: attempt.clone(), timeout_ms: 5000 });
                                swarm.behaviour_mut().control.send_request(&peer, Request::Punch { peer: target.to_string(), endpoint: b, attempt: attempt.clone(), timeout_ms: 5000 });
                                Response::Started { attempt }
                            },
                            Err(error) => Response::Error { message: error.to_string() },
                        },
                        Request::Punch { .. } => Response::Error { message: "Server-only request".into() },
                    };
                    let _ = swarm.behaviour_mut().control.send_response(channel, response);
                },
                SwarmEvent::ListenerError { error, .. } => return Err(error.into()),
                SwarmEvent::ListenerClosed { reason, .. } => { reason?; return Err("Listener closed".into()); },
                _ => {},
            }
        }
    }
    Ok(())
}
async fn shutdown() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { r = tokio::signal::ctrl_c() => r, _ = term.recv() => Ok(()) }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
