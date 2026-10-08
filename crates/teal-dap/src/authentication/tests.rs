use std::{
    io::{BufRead, Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use hmac::Mac;

use super::{Message, Role, authenticate, mac, remaining};

fn connected_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connector = thread::spawn(move || TcpStream::connect(address).unwrap());
    let (listener, _) = listener.accept().unwrap();
    (connector.join().unwrap(), listener)
}

#[test]
fn matching_token_authenticates_both_roles() {
    let (mut connector, mut listener) = connected_pair();
    let connector_thread = thread::spawn(move || {
        authenticate(&mut connector, "shared-token", Role::Connector).map(|_| ())
    });
    authenticate(&mut listener, "shared-token", Role::Listener).unwrap();
    connector_thread.join().unwrap().unwrap();
}

#[test]
fn mismatched_token_fails_both_roles() {
    let (mut connector, mut listener) = connected_pair();
    let connector_thread = thread::spawn(move || {
        authenticate(&mut connector, "connector-token", Role::Connector).map(|_| ())
    });
    assert!(authenticate(&mut listener, "listener-token", Role::Listener).is_err());
    drop(listener);
    assert!(connector_thread.join().unwrap().is_err());
}

#[test]
fn listener_rejects_wrong_version_and_oversized_packets() {
    let mut wrong_version = br#"{"type":"clientProof","version":1,"nonce":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"proof":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}"#.to_vec();
    wrong_version.push(b'\n');
    for payload in [
        br#"{"type":"hello","token":"legacy-token"}"#
            .iter()
            .copied()
            .chain(std::iter::once(b'\n'))
            .collect(),
        wrong_version,
        vec![b'x'; super::MAX_PACKET + 1],
    ] {
        let (mut connector, mut listener) = connected_pair();
        let peer = thread::spawn(move || {
            let mut challenge = Vec::new();
            loop {
                let mut byte = [0];
                connector.read_exact(&mut byte).unwrap();
                challenge.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            connector.write_all(&payload).unwrap();
        });
        assert!(authenticate(&mut listener, "shared-token", Role::Listener).is_err());
        peer.join().unwrap();
    }
}

#[test]
fn listener_rejects_a_truncated_packet() {
    let (mut connector, mut listener) = connected_pair();
    let peer = thread::spawn(move || {
        let mut challenge = String::new();
        std::io::BufReader::new(connector.try_clone().unwrap())
            .read_line(&mut challenge)
            .unwrap();
        connector.write_all(br#"{"type":"clientProof""#).unwrap();
        connector.shutdown(std::net::Shutdown::Write).unwrap();
    });
    assert!(authenticate(&mut listener, "shared-token", Role::Listener).is_err());
    peer.join().unwrap();
}

#[test]
fn listener_applies_one_total_handshake_deadline() {
    let (mut connector, mut listener) = connected_pair();
    let peer = thread::spawn(move || {
        let mut challenge = String::new();
        std::io::BufReader::new(connector.try_clone().unwrap())
            .read_line(&mut challenge)
            .unwrap();
        thread::sleep(super::TIMEOUT + Duration::from_millis(250));
    });
    let started = Instant::now();
    assert!(authenticate(&mut listener, "shared-token", Role::Listener).is_err());
    assert!(started.elapsed() < super::TIMEOUT + Duration::from_secs(2));
    peer.join().unwrap();
}

#[test]
fn listener_preserves_a_command_coalesced_after_client_proof() {
    let (mut connector, mut listener) = connected_pair();
    let peer = thread::spawn(move || {
        let mut challenge = String::new();
        std::io::BufReader::new(connector.try_clone().unwrap())
            .read_line(&mut challenge)
            .unwrap();
        let Message::Challenge { nonce: server, .. } = serde_json::from_str(&challenge).unwrap()
        else {
            panic!("listener must send a challenge");
        };
        let client = [7; 32];
        let proof = mac("shared-token", b"connector\0", &server, &client)
            .finalize()
            .into_bytes()
            .into();
        let proof = serde_json::to_vec(&Message::ClientProof {
            version: super::VERSION,
            nonce: client,
            proof,
        })
        .unwrap();
        let mut bytes = proof;
        bytes.extend_from_slice(b"\nBUSINESS\n");
        connector.write_all(&bytes).unwrap();

        let mut server_proof = String::new();
        std::io::BufReader::new(connector)
            .read_line(&mut server_proof)
            .unwrap();
        assert!(matches!(
            serde_json::from_str(&server_proof).unwrap(),
            Message::ServerProof {
                version: super::VERSION,
                ..
            }
        ));
    });

    let mut reader = authenticate(&mut listener, "shared-token", Role::Listener).unwrap();
    let mut command = String::new();
    reader.read_line(&mut command).unwrap();
    assert_eq!(command, "BUSINESS\n");
    peer.join().unwrap();
}

#[test]
fn expired_total_deadline_and_empty_token_fail_closed() {
    assert_eq!(
        remaining(Instant::now() - Duration::from_millis(1))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::TimedOut
    );
    let (mut connector, _listener) = connected_pair();
    assert_eq!(
        authenticate(&mut connector, "", Role::Connector)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
}
