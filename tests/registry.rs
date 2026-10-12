use super::*;
#[test]
fn authenticated_observed_endpoints_expire_and_capacity_and_retry_rates_are_bounded() {
    let (a, b) = (PeerId::random(), PeerId::random());
    let mut registry = Registry::new(2, 60);
    let ep_a = "127.0.0.1:9000".parse().unwrap();
    let ep_b = "127.0.0.1:9001".parse().unwrap();
    assert!(registry.connect(a, b).is_err());
    assert!(
        matches!(registry.register(a,ep_a),Response::Registered{endpoint,..} if endpoint==ep_a)
    );
    registry.register(b, ep_b);
    assert!(matches!(
        registry.register(PeerId::random(), ep_a),
        Response::Error { .. }
    ));
    let (actual_a, actual_b, _) = registry.connect(a, b).unwrap();
    assert_eq!((actual_a, actual_b), (ep_a, ep_b));
    registry.register(a, ep_a);
    assert!(registry.connect(a, b).is_err());
    assert!(registry.connect(a, a).is_err());
    registry.clients.get_mut(&b).unwrap().seen -= Duration::from_secs(61);
    registry.expire();
    assert!(registry.connect(a, b).is_err());
    registry.remove(a);
    assert!(registry.clients.is_empty());
}
#[test]
fn only_quic_addresses_produce_udp_candidates() {
    assert!(socket_address(&"/ip4/127.0.0.1/tcp/9000".parse().unwrap()).is_none());
    assert_eq!(
        socket_address(&"/ip4/127.0.0.1/udp/9000/quic-v1".parse().unwrap()).unwrap(),
        "127.0.0.1:9000".parse().unwrap()
    );
}

#[test]
fn private_candidates_are_bounded_filtered_and_removed_with_registration() {
    let mut registry = Registry::new(2, 60);
    let peer = PeerId::random();
    let observed = "203.0.113.10:5000".parse().unwrap();
    let private: SocketAddr = "192.168.1.5:4000".parse().unwrap();
    let candidates = [
        private,
        private,
        "127.0.0.1:4000".parse().unwrap(),
        "8.8.8.8:4000".parse().unwrap(),
        "0.0.0.0:4000".parse().unwrap(),
        "10.0.0.1:0".parse().unwrap(),
    ];
    assert!(
        matches!(registry.register_candidates(peer, observed, &candidates), Response::Registered { endpoint, .. } if endpoint == observed)
    );
    assert_eq!(registry.candidates(peer), vec![private]);
    assert!(matches!(
        registry.register_candidates(peer, observed, &[private; 9]),
        Response::Error { .. }
    ));
    registry.remove(peer);
    assert!(registry.candidates(peer).is_empty());
}
