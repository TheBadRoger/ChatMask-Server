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
