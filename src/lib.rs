use clap::Parser;
use libp2p::{
    connection_limits, identity, request_response, swarm::NetworkBehaviour, Multiaddr, PeerId,
    StreamProtocol, Swarm, SwarmBuilder,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
pub const PROTOCOL: &str = "/chatmask/punch/3.0.0";
#[derive(Parser, Debug)]
#[command(version, about = "ChatMask QUIC hole-punch coordination server")]
pub struct Args {
    #[arg(long, default_value = "/ip4/0.0.0.0/udp/7001/quic-v1")]
    pub listen: Multiaddr,
    #[arg(long, default_value = "data/server.key")]
    pub key: std::path::PathBuf,
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u32).range(1..))]
    pub max_clients: u32,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(10..3601))]
    pub ttl_seconds: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Register {
        candidates: Vec<SocketAddr>,
    },
    Connect {
        peer: String,
    },
    Punch {
        peer: String,
        endpoint: SocketAddr,
        candidates: Vec<SocketAddr>,
        attempt: String,
        timeout_ms: u32,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Registered {
        endpoint: SocketAddr,
        ttl_seconds: u32,
    },
    Started {
        attempt: String,
    },
    Ack,
    Error {
        message: String,
    },
}
struct Registration {
    endpoint: SocketAddr,
    candidates: Vec<SocketAddr>,
    seen: Instant,
    last_connect: Option<Instant>,
}
pub struct Registry {
    clients: HashMap<PeerId, Registration>,
    max: usize,
    ttl: Duration,
}
impl Registry {
    pub fn new(max: u32, ttl_seconds: u32) -> Self {
        Self {
            clients: HashMap::new(),
            max: max as usize,
            ttl: Duration::from_secs(ttl_seconds.into()),
        }
    }
    pub fn remove(&mut self, peer: PeerId) {
        self.clients.remove(&peer);
    }
    pub fn expire(&mut self) -> usize {
        let before = self.clients.len();
        let ttl = self.ttl;
        self.clients.retain(|_, r| r.seen.elapsed() < ttl);
        before - self.clients.len()
    }
    pub fn candidates(&self, peer: PeerId) -> Vec<SocketAddr> {
        self.clients
            .get(&peer)
            .map(|r| r.candidates.clone())
            .unwrap_or_default()
    }
    pub fn register(&mut self, peer: PeerId, endpoint: SocketAddr) -> Response {
        self.register_candidates(peer, endpoint, &[])
    }
    pub fn register_candidates(
        &mut self,
        peer: PeerId,
        endpoint: SocketAddr,
        candidates: &[SocketAddr],
    ) -> Response {
        if candidates.len() > 8 {
            return Response::Error {
                message: "Too many local candidates".into(),
            };
        }
        let mut filtered = Vec::new();
        for candidate in candidates {
            let local = match candidate.ip() {
                std::net::IpAddr::V4(ip) => ip.is_private(),
                std::net::IpAddr::V6(ip) => ip.is_unique_local(),
            };
            if candidate.port() != 0
                && candidate.is_ipv4() == endpoint.is_ipv4()
                && (local || (endpoint.ip().is_loopback() && candidate.ip().is_loopback()))
                && !filtered.contains(candidate)
            {
                filtered.push(*candidate);
            }
        }
        self.expire();
        if !self.clients.contains_key(&peer) && self.clients.len() >= self.max {
            return Response::Error {
                message: "Server full".into(),
            };
        }
        let last_connect = self.clients.get(&peer).and_then(|r| r.last_connect);
        self.clients.insert(
            peer,
            Registration {
                endpoint,
                candidates: filtered,
                seen: Instant::now(),
                last_connect,
            },
        );
        Response::Registered {
            endpoint,
            ttl_seconds: self.ttl.as_secs() as u32,
        }
    }
    pub fn connect(
        &mut self,
        source: PeerId,
        target: PeerId,
    ) -> Result<(SocketAddr, SocketAddr, String)> {
        self.expire();
        if source == target {
            return Err("Cannot connect to self".into());
        }
        let a = self.clients.get(&source).ok_or("Register first")?.endpoint;
        let b = self
            .clients
            .get(&target)
            .ok_or("Target not available")?
            .endpoint;
        let r = self.clients.get_mut(&source).expect("registered source");
        if r.last_connect
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return Err("Try again later".into());
        }
        r.last_connect = Some(Instant::now());
        Ok((
            a,
            b,
            identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id()
                .to_string(),
        ))
    }
}
pub fn socket_address(address: &Multiaddr) -> Option<SocketAddr> {
    use libp2p::multiaddr::Protocol;
    let mut ip = None;
    let mut port = None;
    let mut quic = false;
    for p in address.iter() {
        match p {
            Protocol::Ip4(v) => ip = Some(v.into()),
            Protocol::Ip6(v) => ip = Some(v.into()),
            Protocol::Udp(v) => port = Some(v),
            Protocol::QuicV1 => quic = true,
            _ => {}
        }
    }
    if quic {
        Some(SocketAddr::new(ip?, port?))
    } else {
        None
    }
}
#[derive(NetworkBehaviour)]
// Check connection limits before notifying protocol behaviours about a new connection.
pub struct Behaviour {
    limits: connection_limits::Behaviour,
    pub control: request_response::cbor::Behaviour<Request, Response>,
}
pub fn build(args: &Args, key: identity::Keypair) -> Result<Swarm<Behaviour>> {
    let mut swarm = SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_quic()
        .with_behaviour(|_| Behaviour {
            control: request_response::cbor::Behaviour::with_codec(
                request_response::cbor::codec::Codec::default()
                    .set_request_size_maximum(2048)
                    .set_response_size_maximum(2048),
                [(
                    StreamProtocol::new(PROTOCOL),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default()
                    .with_request_timeout(Duration::from_secs(5))
                    .with_max_concurrent_streams(8),
            ),
            limits: connection_limits::Behaviour::new(
                connection_limits::ConnectionLimits::default()
                    .with_max_pending_incoming(Some(32))
                    .with_max_pending_outgoing(Some(32))
                    .with_max_established(Some(args.max_clients))
                    .with_max_established_per_peer(Some(1)),
            ),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();
    swarm.listen_on(args.listen.clone())?;
    Ok(swarm)
}
pub fn load_identity(path: &Path) -> Result<identity::Keypair> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(4097).read_to_end(&mut bytes)?;
            if bytes.len() > 4096 {
                return Err("Identity file too large".into());
            }
            let key = identity::Keypair::from_protobuf_encoding(&bytes)?;
            if key.key_type() != identity::KeyType::Ed25519 {
                return Err("Expected an Ed25519 identity".into());
            }
            return Ok(key);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(parent)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(parent)?;
    }
    let key = identity::Keypair::generate_ed25519();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // Never replace an existing or damaged identity with a new identity silently.
    file.write_all(&key.to_protobuf_encoding()?)?;
    file.sync_all()?;
    Ok(key)
}

#[cfg(test)]
#[path = "../tests/registry.rs"]
mod tests;
