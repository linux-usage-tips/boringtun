// Copyright (c) 2019 Cloudflare, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause

use parking_lot::RwLock;
use socket2::{Domain, Protocol, Type};
use std::io::{Read, Write};

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};
use std::str::FromStr;

use crate::device::{AllowedIps, Error, ProxyConfig};
use crate::noise::{Tunn, TunnResult};

#[derive(Default, Debug)]
pub struct Endpoint {
    pub addr: Option<SocketAddr>,
    pub conn: Option<socket2::Socket>,
}

pub struct Peer {
    /// The associated tunnel struct
    pub(crate) tunnel: Tunn,
    /// The index the tunnel uses
    index: u32,
    endpoint: RwLock<Endpoint>,
    allowed_ips: AllowedIps<()>,
    preshared_key: Option<[u8; 32]>,
}

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct AllowedIP {
    pub addr: IpAddr,
    pub cidr: u8,
}

impl FromStr for AllowedIP {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ip: Vec<&str> = s.split('/').collect();
        if ip.len() != 2 {
            return Err("Invalid IP format".to_owned());
        }

        let (addr, cidr) = (ip[0].parse::<IpAddr>(), ip[1].parse::<u8>());
        match (addr, cidr) {
            (Ok(addr @ IpAddr::V4(_)), Ok(cidr)) if cidr <= 32 => Ok(AllowedIP { addr, cidr }),
            (Ok(addr @ IpAddr::V6(_)), Ok(cidr)) if cidr <= 128 => Ok(AllowedIP { addr, cidr }),
            _ => Err("Invalid IP format".to_owned()),
        }
    }
}

impl Peer {
    pub fn new(
        tunnel: Tunn,
        index: u32,
        endpoint: Option<SocketAddr>,
        allowed_ips: &[AllowedIP],
        preshared_key: Option<[u8; 32]>,
    ) -> Peer {
        Peer {
            tunnel,
            index,
            endpoint: RwLock::new(Endpoint {
                addr: endpoint,
                conn: None,
            }),
            allowed_ips: allowed_ips.iter().map(|ip| (ip, ())).collect(),
            preshared_key,
        }
    }

    pub fn update_timers<'a>(&mut self, dst: &'a mut [u8]) -> TunnResult<'a> {
        self.tunnel.update_timers(dst)
    }

    pub fn endpoint(&self) -> parking_lot::RwLockReadGuard<'_, Endpoint> {
        self.endpoint.read()
    }

    pub(crate) fn endpoint_mut(&self) -> parking_lot::RwLockWriteGuard<'_, Endpoint> {
        self.endpoint.write()
    }

    pub fn shutdown_endpoint(&self) {
        if let Some(conn) = self.endpoint.write().conn.take() {
            tracing::info!("Disconnecting from endpoint");
            conn.shutdown(Shutdown::Both).unwrap();
        }
    }

    pub fn set_endpoint(&self, addr: SocketAddr) {
        let mut endpoint = self.endpoint.write();
        if endpoint.addr != Some(addr) {
            // We only need to update the endpoint if it differs from the current one
            if let Some(conn) = endpoint.conn.take() {
                conn.shutdown(Shutdown::Both).unwrap();
            }

            endpoint.addr = Some(addr);
        }
    }

    pub fn connect_endpoint(
        &self,
        port: u16,
        fwmark: Option<u32>,
        proxy: Option<ProxyConfig>,
    ) -> Result<socket2::Socket, Error> {
        let mut endpoint = self.endpoint.write();

        if endpoint.conn.is_some() {
            return Err(Error::Connect("Connected".to_owned()));
        }

        let addr = endpoint
            .addr
            .expect("Attempt to connect to undefined endpoint");

        let udp_conn = if let Some(proxy_cfg) = proxy {
            // Implement SOCKS5 UDP associate
            match proxy_cfg.proxy_type.as_str() {
                "socks5" => {
                    tracing::info!("Connecting via SOCKS5 proxy: {}", proxy_cfg.address);
                    
                    // Parse proxy address
                    let proxy_addr: SocketAddr = proxy_cfg.address.parse()
                        .map_err(|e| Error::Connect(format!("Invalid proxy address: {}", e)))?;
                    
                    // Create TCP connection to SOCKS5 server for UDP ASSOCIATE
                    let mut stream = TcpStream::connect(proxy_addr)
                        .map_err(|e| Error::Connect(format!("Failed to connect to proxy: {}", e)))?;
                    
                    // SOCKS5 handshake
                    // Send greeting with no authentication
                    stream.write_all(&[0x05, 0x01, 0x00])?;
                    
                    // Read server response
                    let mut response = [0u8; 2];
                    stream.read_exact(&mut response)?;
                    if response[0] != 0x05 || response[1] != 0x00 {
                        return Err(Error::Connect("SOCKS5 handshake failed".to_owned()));
                    }
                    
                    // Send UDP ASSOCIATE request
                    // Format: VER(1) | CMD(1) | RSV(1) | ATYP(1) | DST.ADDR(var) | DST.PORT(2)
                    // CMD = 0x03 for UDP ASSOCIATE
                    // DST.ADDR should be 0.0.0.0 for UDP ASSOCIATE
                    let mut request = vec![0x05, 0x03, 0x00, 0x01]; // VER, CMD, RSV, ATYP(IPv4)
                    request.extend_from_slice(&[0u8; 4]); // 0.0.0.0
                    request.extend_from_slice(&port.to_be_bytes()); // Port
                    
                    stream.write_all(&request)?;
                    
                    // Read UDP ASSOCIATE response
                    // Format: VER(1) | REP(1) | RSV(1) | ATYP(1) | BND.ADDR(var) | BND.PORT(2)
                    let mut resp = [0u8; 4];
                    stream.read_exact(&mut resp)?;
                    if resp[0] != 0x05 || resp[1] != 0x00 {
                        return Err(Error::Connect(format!("SOCKS5 UDP ASSOCIATE failed: REP={}", resp[1])));
                    }
                    
                    // Parse relay address from response
                    let relay_addr = match resp[3] {
                        0x01 => { // IPv4
                            let mut addr_bytes = [0u8; 4];
                            stream.read_exact(&mut addr_bytes)?;
                            let mut port_bytes = [0u8; 2];
                            stream.read_exact(&mut port_bytes)?;
                            SocketAddr::new(IpAddr::V4(Ipv4Addr::from(addr_bytes)), u16::from_be_bytes(port_bytes))
                        }
                        0x04 => { // IPv6
                            let mut addr_bytes = [0u8; 16];
                            stream.read_exact(&mut addr_bytes)?;
                            let mut port_bytes = [0u8; 2];
                            stream.read_exact(&mut port_bytes)?;
                            SocketAddr::new(IpAddr::V6(Ipv6Addr::from(addr_bytes)), u16::from_be_bytes(port_bytes))
                        }
                        _ => return Err(Error::Connect("Unsupported address type in SOCKS5 response".to_owned())),
                    };
                    
                    tracing::info!("SOCKS5 UDP relay address: {}", relay_addr);
                    
                    // Close TCP connection (we don't need it anymore for UDP)
                    drop(stream);
                    
                    // Create UDP socket and connect to relay address
                    let udp_socket = socket2::Socket::new(
                        Domain::for_address(relay_addr),
                        Type::STREAM,
                        Some(Protocol::UDP),
                    ).map_err(|e| Error::Connect(format!("Failed to create UDP socket: {}", e)))?;
                    
                    udp_socket.set_reuse_address(true)?;
                    let bind_addr = if relay_addr.is_ipv4() {
                        SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into()
                    } else {
                        SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0).into()
                    };
                    udp_socket.bind(&bind_addr)?;
                    udp_socket.connect(&relay_addr.into())?;
                    
                    udp_socket
                }
                "http" => {
                    tracing::warn!("HTTP proxy not supported for UDP, using direct connection");
                    socket2::Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::UDP))?
                }
                _ => {
                    tracing::warn!("Unknown proxy type: {}, using direct connection", proxy_cfg.proxy_type);
                    socket2::Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::UDP))?
                }
            }
        } else {
            socket2::Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::UDP))?
        };
        
        udp_conn.set_nonblocking(true)?;

        #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
        if let Some(fwmark) = fwmark {
            udp_conn.set_mark(fwmark)?;
        }

        tracing::info!(
            message="Connected endpoint",
            port=port,
            endpoint=?endpoint.addr.unwrap()
        );

        endpoint.conn = Some(udp_conn.try_clone().unwrap());

        Ok(udp_conn)
    }

    pub fn is_allowed_ip<I: Into<IpAddr>>(&self, addr: I) -> bool {
        self.allowed_ips.find(addr.into()).is_some()
    }

    pub fn allowed_ips(&self) -> impl Iterator<Item = (IpAddr, u8)> + '_ {
        self.allowed_ips.iter().map(|(_, ip, cidr)| (ip, cidr))
    }

    pub fn time_since_last_handshake(&self) -> Option<std::time::Duration> {
        self.tunnel.time_since_last_handshake()
    }

    pub fn persistent_keepalive(&self) -> Option<u16> {
        self.tunnel.persistent_keepalive()
    }

    pub fn preshared_key(&self) -> Option<&[u8; 32]> {
        self.preshared_key.as_ref()
    }

    pub fn index(&self) -> u32 {
        self.index
    }
}
