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
    let mut notices = HashMap::new();
    let shutdown = shutdown();
    tokio::pin!(shutdown);
    println!("Server PeerId: {peer}");
    log(format_args!(
        "server_started peer={peer} max_clients={} ttl_seconds={}",
        args.max_clients, args.ttl_seconds
    ));
    loop {
        tokio::select! {
            result = &mut shutdown => { result?; log(format_args!("server_shutdown")); break; },
            _ = tick.tick() => { let count = registry.expire(); if count > 0 { log(format_args!("registrations_expired count={count}")); } },
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => println!("Control address: {}", address.with(Protocol::P2p(peer))),
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    if let Some(address) = chatmask_server::socket_address(endpoint.get_remote_address()) {
                        endpoints.insert(peer_id, address);
                        log(format_args!("client_connected peer={peer_id} endpoint={address}"));
                    }
                },
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } if !swarm.is_connected(&peer_id) => {
                    registry.remove(peer_id); endpoints.remove(&peer_id);
                    log(format_args!("client_disconnected peer={peer_id} cause={cause:?}"));
                },
                SwarmEvent::Behaviour(BehaviourEvent::Control(request_response::Event::Message { peer, message: request_response::Message::Request { request, channel, .. }, .. })) => {
                    let response = match request {
                        Request::Register => {
                            let response = match endpoints.get(&peer) { Some(endpoint) => registry.register(peer, *endpoint), None => Response::Error { message: "QUIC connection required".into() } };
                            log(format_args!("registration peer={peer} response={response:?}")); response
                        },
                        Request::Connect { peer: target } => {
                            log(format_args!("node_query source={peer} target={target:?}"));
                            if notices.len() >= 2048 { Response::Error { message: "Coordination busy".into() } }
                            else { match target.parse().map_err(|_| "Invalid target".into()).and_then(|target| registry.connect(peer, target).map(|pair| (target, pair))) {
                                Ok((target, (a, b, attempt))) => {
                                    let to_target = swarm.behaviour_mut().control.send_request(&target, Request::Punch { peer: peer.to_string(), endpoint: a, attempt: attempt.clone(), timeout_ms: 5000 });
                                    let to_source = swarm.behaviour_mut().control.send_request(&peer, Request::Punch { peer: target.to_string(), endpoint: b, attempt: attempt.clone(), timeout_ms: 5000 });
                                    notices.insert(to_target, (attempt.clone(), target)); notices.insert(to_source, (attempt.clone(), peer));
                                    log(format_args!("punch_coordination_dispatched source={peer} target={target} source_endpoint={a} target_endpoint={b} attempt={attempt}"));
                                    Response::Started { attempt }
                                },
                                Err(error) => { log(format_args!("node_query_failed source={peer} target={target:?} reason={error:?}")); Response::Error { message: error.to_string() } },
                            } }
                        },
                        Request::Punch { .. } => Response::Error { message: "Server-only request".into() },
                    };
                    if swarm.behaviour_mut().control.send_response(channel, response).is_err() { log(format_args!("response_delivery_failed peer={peer}")); }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Control(request_response::Event::Message { peer, message: request_response::Message::Response { request_id, response }, .. })) => {
                    if let Some((attempt, expected)) = notices.remove(&request_id) { log(format_args!("punch_notice_response peer={peer} expected={expected} attempt={attempt} response={response:?}")); }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Control(request_response::Event::OutboundFailure { peer, request_id, error, .. })) => {
                    if let Some((attempt, _)) = notices.remove(&request_id) { log(format_args!("punch_notice_failed peer={peer} attempt={attempt} reason={error:?}")); }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Control(request_response::Event::InboundFailure { peer, error, .. })) => log(format_args!("control_request_failed peer={peer} reason={error:?}")),
                SwarmEvent::IncomingConnectionError { error, .. } => log(format_args!("incoming_connection_failed reason={error:?}")),
                SwarmEvent::ListenerError { error, .. } => return Err(error.into()),
                SwarmEvent::ListenerClosed { reason, .. } => { reason?; return Err("Listener closed".into()); },
                _ => {},
            }
        }
    }
    Ok(())
}
fn log(message: std::fmt::Arguments<'_>) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    println!("[unix={timestamp}] {message}");
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
