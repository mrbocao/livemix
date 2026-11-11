// Adapted from LiveCraft (4a1eca7), MIT. See docs/LICENSE-LiveCraft-MIT.txt.
/// Camera moves sent over VISCA, the common PTZ protocol on IP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ptz {
    Move { pan: i8, tilt: i8 },
    Stop,
    ZoomIn,
    ZoomOut,
    ZoomStop,
    Home,
    Recall(u8),
    Store(u8),
}

pub fn visca_payload(cmd: Ptz) -> Vec<u8> {
    let (speed_p, speed_t) = (0x0c, 0x0a);
    match cmd {
        Ptz::Move { pan, tilt } => {
            let x = match pan {
                p if p < 0 => 0x01,
                p if p > 0 => 0x02,
                _ => 0x03,
            };
            let y = match tilt {
                t if t > 0 => 0x01,
                t if t < 0 => 0x02,
                _ => 0x03,
            };
            vec![0x81, 0x01, 0x06, 0x01, speed_p, speed_t, x, y, 0xff]
        }
        Ptz::Stop => vec![0x81, 0x01, 0x06, 0x01, speed_p, speed_t, 0x03, 0x03, 0xff],
        Ptz::ZoomIn => vec![0x81, 0x01, 0x04, 0x07, 0x25, 0xff],
        Ptz::ZoomOut => vec![0x81, 0x01, 0x04, 0x07, 0x35, 0xff],
        Ptz::ZoomStop => vec![0x81, 0x01, 0x04, 0x07, 0x00, 0xff],
        Ptz::Home => vec![0x81, 0x01, 0x06, 0x04, 0xff],
        Ptz::Recall(n) => vec![0x81, 0x01, 0x04, 0x3f, 0x02, n, 0xff],
        Ptz::Store(n) => vec![0x81, 0x01, 0x04, 0x3f, 0x01, n, 0xff],
    }
}

/// Wraps a command in the VISCA over IP header, on UDP port 52381.
pub fn visca_packet(seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![0x01, 0x00];
    p.extend((payload.len() as u16).to_be_bytes());
    p.extend(seq.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

pub fn send(address: &str, command: Ptz) -> Result<(), String> {
    use std::net::{IpAddr, SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    static SEQUENCE: AtomicU32 = AtomicU32::new(1);
    let address = address.trim();
    let target = address.parse::<SocketAddr>().or_else(|_| {
        address.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 52381))
    }).map_err(|_| "Enter the camera's IP address, optionally with its UDP port".to_owned())?;
    let socket = UdpSocket::bind(if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
        .map_err(|error| error.to_string())?;
    let send = |command| {
        let packet = visca_packet(SEQUENCE.fetch_add(1, Ordering::Relaxed), &visca_payload(command));
        socket.send_to(&packet, target).map_err(|error| error.to_string())
    };
    send(command)?;
    let stop = match command {
        Ptz::Move { .. } => Some(Ptz::Stop),
        Ptz::ZoomIn | Ptz::ZoomOut => Some(Ptz::ZoomStop),
        _ => None,
    };
    if let Some(stop) = stop {
        std::thread::sleep(Duration::from_millis(250));
        for _ in 0..3 {
            send(stop)?;
        }
    }
    Ok(())
}

#[test]
fn home_reaches_the_configured_udp_endpoint() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
    send(&socket.local_addr().unwrap().to_string(), Ptz::Home).unwrap();
    let mut packet = [0; 32];
    let count = socket.recv(&mut packet).unwrap();
    assert_eq!(&packet[..4], &[1, 0, 0, 5]);
    assert_eq!(&packet[8..count], &[0x81, 0x01, 0x06, 0x04, 0xff]);
}
