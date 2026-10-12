use chatmask_server::{Request, Response, PROTOCOL};
use futures::StreamExt;
use libp2p::{
    identity, noise, request_response, swarm::SwarmEvent, tcp, yamux, StreamProtocol, Swarm,
    SwarmBuilder,
};
use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

fn client(key: identity::Keypair) -> Swarm<request_response::cbor::Behaviour<Request, Response>> {
    SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .unwrap()
        .with_quic()
        .with_behaviour(|_| {
            request_response::cbor::Behaviour::new(
                [(
                    StreamProtocol::new(PROTOCOL),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default(),
            )
        })
        .unwrap()
        .build()
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        // The target is the exact unique temporary directory created by this test.
        if self.0.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
#[tokio::test]
async fn real_server_registers_observed_udp_addresses_signals_both_peers_and_clients_send_directly()
{
    let dir = Directory(std::env::temp_dir().join(format!(
        "chatmask-coordination-{}",
        libp2p::PeerId::random()
    )));
    fs::create_dir(&dir.0).unwrap();
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_chatmask-server"))
            .args(["--listen", "/ip4/127.0.0.1/udp/0/quic-v1"])
            .arg("--key")
            .arg(dir.0.join("server.key"))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let (tx, rx) = mpsc::channel();
    let stdout = server.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });

    let control = loop {
        let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        if let Some(value) = line.strip_prefix("Control address: ") {
            break value.parse::<libp2p::Multiaddr>().unwrap();
        }
    };
    let key_a = identity::Keypair::generate_ed25519();
    let key_b = identity::Keypair::generate_ed25519();
    let a_peer = key_a.public().to_peer_id();
    let b_peer = key_b.public().to_peer_id();
    let server_peer = match control.iter().last().unwrap() {
        libp2p::multiaddr::Protocol::P2p(peer) => peer,
        _ => panic!(),
    };
    let mut a = client(key_a.clone());
    let mut b = client(key_b.clone());
    a.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap())
        .unwrap();
    b.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap())
        .unwrap();
    a.dial(control.clone()).unwrap();
    b.dial(control).unwrap();
    a.behaviour_mut().send_request(
        &server_peer,
        Request::Register {
            candidates: vec!["10.20.30.40:9001".parse().unwrap()],
        },
    );
    b.behaviour_mut().send_request(
        &server_peer,
        Request::Register {
            candidates: vec!["10.20.30.40:9002".parse().unwrap()],
        },
    );
    let (mut endpoint_a, mut endpoint_b) = (None, None);
    tokio::time::timeout(Duration::from_secs(10), async {
        while endpoint_a.is_none() || endpoint_b.is_none() {
            tokio::select! {
                event = a.select_next_some() => if let SwarmEvent::Behaviour(request_response::Event::Message { message: request_response::Message::Response { response: Response::Registered { endpoint, .. }, .. }, .. }) = event { endpoint_a = Some(endpoint); },
                event = b.select_next_some() => if let SwarmEvent::Behaviour(request_response::Event::Message { message: request_response::Message::Response { response: Response::Registered { endpoint, .. }, .. }, .. }) = event { endpoint_b = Some(endpoint); },
            }
        }
    }).await.unwrap();
    let endpoint_a = endpoint_a.unwrap();
    let endpoint_b = endpoint_b.unwrap();
    a.behaviour_mut().send_request(
        &server_peer,
        Request::Connect {
            peer: b_peer.to_string(),
        },
    );
    let (mut notice_a, mut notice_b, mut started) = (None, None, None);
    tokio::time::timeout(Duration::from_secs(10), async {
        while notice_a.is_none() || notice_b.is_none() || started.is_none() {
            tokio::select! {
                event = a.select_next_some() => if let SwarmEvent::Behaviour(request_response::Event::Message { peer, message, .. }) = event {
                    assert_eq!(peer, server_peer);
                    match message {
                        request_response::Message::Request { request: Request::Punch { peer, endpoint, candidates, attempt, timeout_ms }, channel, .. } => {
                            assert_eq!(peer, b_peer.to_string()); assert_eq!(endpoint, endpoint_b); assert_eq!(candidates, vec!["10.20.30.40:9002".parse().unwrap()]); assert_eq!(timeout_ms, 12000); notice_a = Some(attempt);
                            a.behaviour_mut().send_response(channel, Response::Ack).unwrap();
                        },
                        request_response::Message::Response { response: Response::Started { attempt }, .. } => started = Some(attempt),
                        _ => {},
                    }
                },
                event = b.select_next_some() => if let SwarmEvent::Behaviour(request_response::Event::Message { peer: sender, message: request_response::Message::Request { request: Request::Punch { peer, endpoint, candidates, attempt, timeout_ms }, channel, .. }, .. }) = event {
                    assert_eq!(sender, server_peer); assert_eq!(peer, a_peer.to_string()); assert_eq!(endpoint, endpoint_a); assert_eq!(candidates, vec!["10.20.30.40:9001".parse().unwrap()]); assert_eq!(timeout_ms, 12000); notice_b = Some(attempt);
                    b.behaviour_mut().send_response(channel, Response::Ack).unwrap();
                }
            }
        }
    }).await.unwrap();
    assert_eq!(notice_a, notice_b);
    assert_eq!(notice_a, started);
    assert_ne!(endpoint_a.port(), 0);
    assert_ne!(endpoint_b.port(), 0);
}

#[test]
fn identity_is_stable_and_corruption_does_not_rotate_it() {
    let dir =
        Directory(std::env::temp_dir().join(format!("chatmask-key-{}", libp2p::PeerId::random())));
    let path = dir.0.join("server.key");
    let key = chatmask_server::load_identity(&path).unwrap();
    assert_eq!(
        key.public().to_peer_id(),
        chatmask_server::load_identity(&path)
            .unwrap()
            .public()
            .to_peer_id()
    );
    fs::write(&path, b"damaged").unwrap();
    assert!(chatmask_server::load_identity(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"damaged");
}
