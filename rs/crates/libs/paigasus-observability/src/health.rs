// SPDX-License-Identifier: Apache-2.0

//! The liveness/readiness probe the container images' `HEALTHCHECK` runs.
//!
//! The images are shell-less (`FROM scratch` over a chiseled Ubuntu rootfs, SMA-500), so
//! `curl`/`wget` do not exist and each service binary probes itself instead. One
//! implementation lives here rather than in each `main.rs`, the same single-site discipline
//! `repo:redis-connect-single-site` enforces elsewhere.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Probe `path` on `addr` over plaintext HTTP/1.1, within a TOTAL `deadline`.
///
/// `Ok(true)` iff the response status is 2xx; `Ok(false)` for any other status (a 503 from
/// `/readyz` is a healthy *answer*, not a failure); `Err` if the service could not be reached,
/// answered nothing, or answered something that is not HTTP.
///
/// Neither service terminates TLS (`axum-server` is a dev-dependency only), so plaintext is
/// correct today; both images require a TLS-terminating ingress.
pub fn probe(addr: SocketAddr, path: &str, deadline: Duration) -> io::Result<bool> {
    let started = Instant::now();
    let target = connectable(addr);

    let mut stream = TcpStream::connect_timeout(&target, remaining(deadline, started)?)?;

    // `deadline` is a TOTAL budget, not a connect timeout. A server that has accepted but
    // wedged (a saturated axum, a blocked handler) would otherwise block this call forever.
    stream.set_write_timeout(Some(remaining(deadline, started)?))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {target}\r\nUser-Agent: paigasus-healthcheck\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    stream.set_read_timeout(Some(remaining(deadline, started)?))?;
    let mut status_line = String::new();
    BufReader::new(&stream).read_line(&mut status_line)?;
    status_is_success(&status_line)
}

/// The budget left, or `TimedOut` once it is gone. Never returns `Duration::ZERO`: std reads a
/// zero timeout as "no timeout at all", which would silently remove the bound this enforces.
fn remaining(deadline: Duration, started: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_sub(started.elapsed());
    if left.is_zero() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "health probe deadline exceeded"));
    }
    Ok(left)
}

/// Both services default to `0.0.0.0`, which is an unspecified address, not a destination.
fn connectable(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port()),
        _ => addr,
    }
}

fn status_is_success(status_line: &str) -> io::Result<bool> {
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("malformed HTTP status line: {status_line:?}"))
        })?;
    Ok((200..300).contains(&code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::thread;
    use std::time::Duration;

    /// Serve exactly one request with `status_line`, then close. Returns the bound address.
    fn serve_once(status_line: &'static str) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(format!("{status_line}\r\nContent-Length: 0\r\n\r\n").as_bytes());
            }
        });
        addr
    }

    #[test]
    fn a_2xx_response_is_healthy() {
        let addr = serve_once("HTTP/1.1 200 OK");
        assert!(probe(addr, "/healthz", Duration::from_secs(2)).expect("probe ran"));
    }

    #[test]
    fn a_503_response_is_unhealthy_but_not_an_error() {
        let addr = serve_once("HTTP/1.1 503 Service Unavailable");
        assert!(!probe(addr, "/readyz", Duration::from_secs(2)).expect("probe ran"));
    }

    #[test]
    fn a_refused_connection_is_an_error() {
        // Bind then drop, so the port is almost certainly closed.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);
        assert!(probe(addr, "/healthz", Duration::from_secs(2)).is_err());
    }

    #[test]
    fn an_unspecified_address_is_probed_on_loopback() {
        // Services bind 0.0.0.0 by default, which is NOT a destination. The probe must
        // rewrite it to loopback rather than dial it.
        let addr = serve_once("HTTP/1.1 200 OK");
        let unspecified = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED), addr.port());
        assert!(probe(unspecified, "/healthz", Duration::from_secs(2)).expect("probe ran"));
    }

    #[test]
    fn a_server_that_accepts_but_never_responds_hits_the_total_deadline() {
        // The regression this pins: a connect-only timeout would block here forever.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            let held = listener.accept();
            thread::sleep(Duration::from_secs(30));
            drop(held);
        });
        let started = std::time::Instant::now();
        assert!(probe(addr, "/healthz", Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5), "probe must honour its deadline, took {:?}", started.elapsed());
    }

    #[test]
    fn a_malformed_status_line_is_an_error() {
        let addr = serve_once("NOT-HTTP");
        assert!(probe(addr, "/healthz", Duration::from_secs(2)).is_err());
    }
}
