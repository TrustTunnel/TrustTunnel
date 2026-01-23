extern "C" {
    #[cfg(target_os = "macos")]
    fn bind_to_interface_by_index(
        fd: libc::c_int,
        family: libc::c_int,
        idx: libc::c_uint,
    ) -> libc::c_int;
}

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};

pub(crate) const MIN_LINK_MTU: usize = 1280;
pub(crate) const MIN_IPV4_HEADER_SIZE: usize = 20;
pub(crate) const MIN_IPV6_HEADER_SIZE: usize = 40;
pub(crate) const MAX_IP_PACKET_SIZE: usize = 2_usize.pow(16);
pub(crate) const UDP_HEADER_SIZE: usize = 8;
/// IPv6 allows sending slightly bigger datagrams, but assume it does not matter
pub(crate) const MAX_UDP_PAYLOAD_SIZE: usize =
    MAX_IP_PACKET_SIZE - MIN_IPV4_HEADER_SIZE - UDP_HEADER_SIZE;
pub(crate) const PLAIN_DNS_PORT_NUMBER: u16 = 53;
pub(crate) const PLAIN_HTTP_PORT_NUMBER: u16 = 80;

pub(crate) const IPV4_WIRE_LENGTH: usize = 4;
pub(crate) const IPV6_WIRE_LENGTH: usize = 16;
const FIXED_LENGTH_IP_WIRE_LENGTH: usize = IPV6_WIRE_LENGTH;
const IPV4_PADDING_WIRE_LENGTH: usize = FIXED_LENGTH_IP_WIRE_LENGTH - IPV4_WIRE_LENGTH;

pub const HTTP1_ALPN: &str = "http/1.1";
pub const HTTP2_ALPN: &str = "h2";
pub const HTTP3_ALPN: &str = "h3";

pub(crate) const HTTP3_DATA_FRAME_TYPE_WIRE_LENGTH: usize = varint_len(0);
/// The minimum value of a stream capacity which allows to send a data chunk.
/// Consists of 1 byte for frame ID, 1 byte for the shortest frame length, and
/// 1 byte for the chunk itself.
pub(crate) const MIN_USABLE_QUIC_STREAM_CAPACITY: usize = http3_data_frame_overhead(1) + 1;

const SCRUBBED_PLACEHOLDER: &str = "scrubbed";

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum Channel {
    /// The connection is used for tunneling client's connections (see [`crate::tunnel`])
    Tunnel,
    /// The connection is used just for measuring a ping
    Ping,
    /// The connection is used for a speedtest (see [`crate::http_speedtest_handler`])
    Speedtest,
    /// The connection is used for proxying requests further (see [`crate::reverse_proxy`])
    ReverseProxy,
}

pub(crate) type HostnamePort = (String, u16);

#[derive(Debug, Clone)]
pub(crate) enum TcpDestination {
    Address(SocketAddr),
    HostName(HostnamePort),
}

pub(crate) trait PeerAddr {
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

impl PeerAddr for tokio::net::TcpStream {
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.peer_addr()
    }
}

impl<IO> PeerAddr for tokio_rustls::server::TlsStream<IO>
where
    IO: PeerAddr,
{
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.get_ref().0.peer_addr()
    }
}

pub(crate) fn make_udp_socket(is_v4: bool) -> io::Result<UdpSocket> {
    if is_v4 {
        UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
    } else {
        UdpSocket::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)))
    }
}

/// https://www.rfc-editor.org/rfc/rfc9000.html#section-16
pub(crate) const fn varint_len(x: usize) -> usize {
    if x <= 63 {
        1
    } else if x <= 16_383 {
        2
    } else if x <= 1_073_741_823 {
        4
    } else if x <= 4_611_686_018_427_387_903 {
        8
    } else {
        unreachable!()
    }
}

pub(crate) const fn http3_data_frame_overhead(payload_size: usize) -> usize {
    HTTP3_DATA_FRAME_TYPE_WIRE_LENGTH + varint_len(payload_size)
}

pub(crate) fn get_fixed_size_ip(bytes: &mut Bytes) -> IpAddr {
    let ip = bytes.split_to(IPV6_WIRE_LENGTH);
    if ip[..IPV4_PADDING_WIRE_LENGTH].iter().all(|x| *x == 0) {
        let address: [u8; IPV4_WIRE_LENGTH] = ip[IPV4_PADDING_WIRE_LENGTH..].try_into().unwrap();
        IpAddr::from(address)
    } else {
        let address: [u8; FIXED_LENGTH_IP_WIRE_LENGTH] = ip[..].try_into().unwrap();
        IpAddr::from(address)
    }
}

pub(crate) fn put_fixed_size_ip(bytes: &mut BytesMut, ip: &IpAddr) {
    match ip {
        IpAddr::V4(ip) => {
            bytes.put_slice(&[0; IPV4_PADDING_WIRE_LENGTH]);
            bytes.put_slice(&ip.octets());
        }
        IpAddr::V6(ip) => bytes.put_slice(&ip.octets()),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn bind_to_interface(
    fd: libc::c_int,
    _family: libc::c_int,
    name: &str,
) -> io::Result<()> {
    unsafe {
        let r = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_bytes().as_ptr() as *const libc::c_void,
            name.len() as libc::socklen_t,
        );
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn bind_to_interface(
    fd: libc::c_int,
    family: libc::c_int,
    name: &str,
) -> io::Result<()> {
    unsafe {
        let idx = libc::if_nametoindex(name.as_ptr() as *const libc::c_char);
        if idx == 0 {
            return Err(io::Error::last_os_error());
        }
        if 0 != bind_to_interface_by_index(fd, family, idx) {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

pub(crate) fn set_socket_ttl(fd: libc::c_int, is_ipv4: bool, ttl: u8) -> io::Result<()> {
    unsafe {
        let (level, name) = if is_ipv4 {
            (libc::IPPROTO_IP, libc::IP_TTL)
        } else {
            (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS)
        };

        let ttl = ttl as libc::c_int;
        let r = libc::setsockopt(
            fd,
            level,
            name,
            &ttl as *const _ as *const libc::c_void,
            std::mem::size_of_val(&ttl) as _,
        );

        if r < 0 {
            return Err(io::Error::last_os_error());
        }
    }

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn send_udp_gso_to(
    socket: &tokio::net::UdpSocket,
    data: &[u8],
    peer: &SocketAddr,
    segment_size: usize,
) -> io::Result<()> {
    if segment_size >= data.len() {
        return match socket.try_send_to(data, *peer) {
            Ok(_) => Ok(()),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        };
    }

    use std::os::unix::io::AsRawFd;

    const UDP_SEGMENT: libc::c_int = 103;

    let fd = socket.as_raw_fd();
    let (mut sockaddr, sockaddr_len) = socket_addr_to_libc(peer);
    
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };

    let mut control = [0u8; unsafe { libc::CMSG_SPACE(std::mem::size_of::<u16>() as u32) as usize }];
    
    let mut msg = libc::msghdr {
        msg_name: &mut sockaddr as *mut _ as *mut libc::c_void,
        msg_namelen: sockaddr_len,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr() as *mut libc::c_void,
        msg_controllen: control.len() as _,
        msg_flags: 0,
    };

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if !cmsg.is_null() {
            (*cmsg).cmsg_level = libc::SOL_UDP;
            (*cmsg).cmsg_type = UDP_SEGMENT;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<u16>() as u32) as _;
            *(libc::CMSG_DATA(cmsg) as *mut u16) = segment_size as u16;
            msg.msg_controllen = (*cmsg).cmsg_len as _;
        }

        let ret = libc::sendmsg(fd, &msg, 0);
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(err);
        }
    }
    
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) fn send_udp_gso_to(
    socket: &tokio::net::UdpSocket,
    data: &[u8],
    peer: &SocketAddr,
    _segment_size: usize,
) -> io::Result<()> {
    match socket.try_send_to(data, *peer) {
        Ok(_) => Ok(()),
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e),
    }
}


pub(crate) fn socket_addr_to_libc(addr: &SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    unsafe {
        let mut storage = std::mem::zeroed();

        let len = match addr {
            SocketAddr::V4(addr) => {
                let storage = &mut storage as *mut _ as *mut libc::sockaddr_in;
                (*storage).sin_family = libc::AF_INET as libc::sa_family_t;
                (*storage).sin_port = addr.port().to_be();
                (*storage).sin_addr.s_addr = u32::from_ne_bytes((*addr).ip().octets());
                std::mem::size_of::<libc::sockaddr_in>()
            }
            SocketAddr::V6(addr) => {
                let storage = &mut storage as *mut _ as *mut libc::sockaddr_in6;
                (*storage).sin6_family = libc::AF_INET6 as libc::sa_family_t;
                (*storage).sin6_port = addr.port().to_be();
                (*storage).sin6_flowinfo = addr.flowinfo();
                (*storage).sin6_addr.s6_addr = addr.ip().octets();
                (*storage).sin6_scope_id = addr.scope_id();
                std::mem::size_of::<libc::sockaddr_in6>()
            }
        };

        (storage, len as libc::socklen_t)
    }
}

pub(crate) fn libc_to_socket_addr(addr: &libc::sockaddr_storage) -> SocketAddr {
    match addr.ss_family as libc::c_int {
        libc::AF_INET => unsafe {
            let addr = &*(addr as *const _ as *const libc::sockaddr_in);
            SocketAddrV4::new(
                Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()),
                u16::from_be(addr.sin_port),
            )
            .into()
        },
        libc::AF_INET6 => unsafe {
            let addr = &*(addr as *const _ as *const libc::sockaddr_in6);
            SocketAddrV6::new(
                Ipv6Addr::from(addr.sin6_addr.s6_addr),
                u16::from_be(addr.sin6_port),
                addr.sin6_flowinfo,
                addr.sin6_scope_id,
            )
            .into()
        },
        _ => unreachable!(),
    }
}

/// Do [`libc::recvfrom`] over `fd` in a buffer of `buffer_size` size.
/// If [`None`], `buffer_size` defaults to [`MIN_LINK_MTU`].
pub(crate) fn recv_from(
    fd: libc::c_int,
    buffer_size: Option<usize>,
) -> io::Result<(IpAddr, Bytes)> {
    let mut buffer = BytesMut::zeroed(buffer_size.unwrap_or(MIN_LINK_MTU));

    unsafe {
        let mut peer = std::mem::zeroed::<libc::sockaddr_storage>();
        let mut peer_len = std::mem::size_of_val(&peer) as libc::socklen_t;
        let flags = libc::MSG_DONTWAIT;
        let r = libc::recvfrom(
            fd,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len(),
            flags,
            &mut peer as *mut libc::sockaddr_storage as *mut libc::sockaddr,
            &mut peer_len as *mut _,
        );
        if r < 0 {
            return Err(io::Error::last_os_error());
        }

        buffer.truncate(r as usize);

        Ok((libc_to_socket_addr(&peer).ip(), buffer.freeze()))
    }
}

/// # Return
///
/// [`None`] in case of packet is invalid, or
/// the next header protocol ID ([`libc::IPPROTO_*`]) and IP packet payload otherwise.
pub(crate) fn skip_ipv4_header(mut packet: Bytes) -> Option<(libc::c_int, Bytes)> {
    if packet.len() < MIN_IPV4_HEADER_SIZE {
        return None;
    }

    let x = packet.get_u8(); // Version + Header length
    let header_length = ((x & 0x0f) * 4) as usize;
    if header_length < 20 || header_length > packet.len() + 1 {
        return None;
    }

    packet.advance(1 + 2 + 2 + 2 + 1); // DSCP + ECN + Total length + ID + Flags + Frag. Offset + TTL
    let next_protocol = packet.get_u8() as libc::c_int;
    packet.advance(2 + 4 + 4 + (header_length - MIN_IPV4_HEADER_SIZE)); // Checksum + Source + Destination + Options

    Some((next_protocol, packet))
}

/// # Return
///
/// [`None`] in case of packet is invalid, or
/// the next header protocol ID ([`libc::IPPROTO_*`]) and IP packet payload otherwise.
pub(crate) fn skip_ipv6_header(mut packet: Bytes) -> Option<(libc::c_int, Bytes)> {
    if packet.len() < MIN_IPV6_HEADER_SIZE {
        return None;
    }

    packet.advance(4 + 2); // Version + Traffic class + Flow label + Payload length
    let mut next_protocol = packet.get_u8() as libc::c_int;
    packet.advance(1 + 16 + 16); // Hop limit + Source + Destination

    loop {
        match next_protocol {
            libc::IPPROTO_HOPOPTS | libc::IPPROTO_ROUTING | libc::IPPROTO_DSTOPTS => {
                if packet.len() < 2 {
                    return None;
                }
                next_protocol = packet.get_u8() as libc::c_int;
                let header_ext_length = packet.get_u8() as usize;
                packet.advance(header_ext_length);
            }
            libc::IPPROTO_FRAGMENT => {
                const IPV6_FRAGMENT_EXT_LENGTH: usize = 8;
                if packet.len() < IPV6_FRAGMENT_EXT_LENGTH {
                    return None;
                }

                next_protocol = packet.get_u8() as libc::c_int;
                packet.advance(IPV6_FRAGMENT_EXT_LENGTH - 1);
            }
            _ => break,
        }
    }

    Some((next_protocol, packet))
}

/// Calculates the checksum for the provided byte array
/// in accordance with https://datatracker.ietf.org/doc/html/rfc1071
pub(crate) fn rfc1071_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    let is_even = bytes.len() % 2 == 0;
    for i in (0..bytes.len()).step_by(2) {
        sum += (bytes[i] as u32) << 8;
        if is_even || i + 1 < bytes.len() - 1 {
            sum += bytes[i + 1] as u32;
        }
    }
    !((sum >> 16) + sum) as u16
}

/// Returns [`true`] if the address appears to be globally routable.
/// See [iana-ipv4-special-registry][ipv4-sr].
///
/// The following return [`false`]:
///
/// - private addresses (see [`Ipv4Addr::is_private()`])
/// - the loopback address (see [`Ipv4Addr::is_loopback()`])
/// - the link-local address (see [`Ipv4Addr::is_link_local()`])
/// - the broadcast address (see [`Ipv4Addr::is_broadcast()`])
/// - addresses used for documentation (see [`Ipv4Addr::is_documentation()`])
/// - the unspecified address (see [`Ipv4Addr::is_unspecified()`]), and the whole
///   `0.0.0.0/8` block
/// - addresses reserved for future protocols, except
///   `192.0.0.9/32` and `192.0.0.10/32` which are globally routable
/// - addresses reserved for future use (see [IETF RFC 1112])
/// - addresses reserved for networking devices benchmarking (see [IETF RFC 2544])
///
/// [ipv4-sr]: https://www.iana.org/assignments/iana-ipv4-special-registry/iana-ipv4-special-registry.xhtml
///
/// @todo: replace with [`Ipv4Addr::is_global`] when it becomes stable
#[allow(clippy::nonminimal_bool)]
pub(crate) const fn is_global_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    // check if this address is 192.0.0.9 or 192.0.0.10. These addresses are the only two
    // globally routable addresses in the 192.0.0.0/24 range.
    {
        let ip = u32::from_be_bytes(octets);
        if ip == 0xc0000009 || ip == 0xc000000a {
            return true;
        }
    }
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        // address is part of the Shared Address Space defined in [IETF RFC 6598] (`100.64.0.0/10`)
        && !(octets[0] == 100 && (octets[1] & 0b1100_0000 == 0b0100_0000))
        // addresses reserved for future protocols (`192.0.0.0/24`)
        && !(octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        // address is reserved by IANA for future use [IETF RFC 1112]
        && !(octets[0] & 240 == 240 && !ip.is_broadcast())
        // reserved for network devices benchmarking [IETF RFC 2544]
        && !(octets[0] == 198 && (octets[1] & 0xfe) == 18)
        // Make sure the address is not in 0.0.0.0/8
        && octets[0] != 0
}

/// Returns [`true`] if the address is a globally routable unicast address.
///
/// The following return false:
///
/// - the loopback address
/// - the link-local addresses
/// - unique local addresses
/// - the unspecified address
/// - the address range reserved for documentation
///
/// This method returns [`true`] for site-local addresses as per [RFC 4291 section 2.5.7]
///
/// [RFC 4291 section 2.5.7]: https://tools.ietf.org/html/rfc4291#section-2.5.7
///
/// @todo: replace with [`Ipv6Addr::is_unicast_global`] when it becomes stable
#[must_use]
#[inline]
pub(crate) const fn is_unicast_global_ipv6(ip: &Ipv6Addr) -> bool {
    let segments = ip.segments();

    !(ip.is_multicast()
        || ip.is_loopback()
        // unicast address with link-local scope [RFC 4291]
        || (segments[0] & 0xffc0) == 0xfe80
        // unique local address (`fc00::/7`) [IETF RFC 4193]
        || (segments[0] & 0xfe00) == 0xfc00
        || ip.is_unspecified()
        // address reserved for documentation (`2001:db8::/32`) [IETF RFC 3849]
        || (segments[0] == 0x2001) && (segments[1] == 0xdb8))
}

/// Returns [`true`] if the address appears to be globally routable.
///
/// The following return [`false`]:
///
/// - the loopback address
/// - link-local and unique local unicast addresses
/// - interface-, link-, realm-, admin- and site-local multicast addresses
///
/// @todo: replace with [`Ipv6Addr::is_global`] when it becomes stable
#[must_use]
#[inline]
pub(crate) const fn is_global_ipv6(ip: &Ipv6Addr) -> bool {
    match ip.segments()[0] & 0x000f {
        1 // Interface-local scope (same node)
        | 2 // Link-local scope (same link)
        | 3 // Subnet-local scope
        | 4 // Admin-local scope
        | 5 // Site-local scope (same site)
        | 8 // Organization-local scope
        => false,
        0x0e => true, // Global scope
        _ => is_unicast_global_ipv6(ip),
    }
}

/// Returns [`true`] if the address appears to be globally routable.
#[must_use]
#[inline]
pub(crate) const fn is_global_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(x) => is_global_ipv4(x),
        IpAddr::V6(x) => is_global_ipv6(x),
    }
}

/// Returns HTTP request with sensitive fields removed.
#[inline]
pub(crate) fn scrub_request(request: &http::request::Parts) -> http::request::Parts {
    let mut r = http::request::Request::new(()).into_parts().0;
    r.method.clone_from(&request.method);
    r.uri.clone_from(&request.uri);
    r.version.clone_from(&request.version);
    r.headers.clone_from(&request.headers);
    for header in &[
        http::header::AUTHORIZATION,
        http::header::PROXY_AUTHORIZATION,
        http::header::COOKIE,
    ] {
        if r.headers.contains_key(header) {
            r.headers
                .insert(header, http::HeaderValue::from_static(SCRUBBED_PLACEHOLDER));
        }
    }
    r
}

/// Returns SNI with sensitive data removed.
#[inline]
pub(crate) fn scrub_sni(mut sni: String) -> String {
    if let Some(to) = sni.find('.') {
        sni.replace_range(..to, SCRUBBED_PLACEHOLDER)
    }
    sni
}

#[cfg(test)]
mod tests {
    use crate::net_utils::{
        libc_to_socket_addr, scrub_request, scrub_sni, socket_addr_to_libc, SCRUBBED_PLACEHOLDER,
    };
    use http::uri;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn sockaddr_conversion_v4() {
        let ip = Ipv4Addr::from([1, 2, 3, 4]);
        let port = 1234;

        let (sa, sa_len) = socket_addr_to_libc(&(ip, port).into());
        assert_eq!(sa.ss_family as libc::c_int, libc::AF_INET);
        assert_eq!(std::mem::size_of::<libc::sockaddr_in>(), sa_len as usize);
        assert_eq!(port.to_be(), unsafe {
            (*(&sa as *const libc::sockaddr_storage as *const libc::sockaddr_in)).sin_port
        });

        let sa = libc_to_socket_addr(&sa);
        assert_eq!(sa.ip(), ip);
        assert_eq!(sa.port(), port);
    }

    #[test]
    fn sockaddr_conversion_v6() {
        let ip = Ipv6Addr::from(0x102030405060708090a0b0c0d0e0f00d_u128);
        let port = 1234;

        let (sa, sa_len) = socket_addr_to_libc(&(ip, port).into());
        assert_eq!(sa.ss_family as libc::c_int, libc::AF_INET6);
        assert_eq!(std::mem::size_of::<libc::sockaddr_in6>(), sa_len as usize);
        assert_eq!(port.to_be(), unsafe {
            (*(&sa as *const libc::sockaddr_storage as *const libc::sockaddr_in6)).sin6_port
        });

        let sa = libc_to_socket_addr(&sa);
        assert_eq!(sa.ip(), ip);
        assert_eq!(sa.port(), port);
    }

    #[test]
    fn scrubbing_of_sni() {
        assert_eq!("one", scrub_sni("one".to_string()));
        assert_eq!(
            format!("{}.", SCRUBBED_PLACEHOLDER),
            crate::net_utils::scrub_sni("one.".to_string())
        );
        assert_eq!(
            format!("{}.one", SCRUBBED_PLACEHOLDER),
            crate::net_utils::scrub_sni(".one".to_string())
        );
        assert_eq!(
            format!("{}.two", SCRUBBED_PLACEHOLDER),
            crate::net_utils::scrub_sni("one.two".to_string())
        );
        assert_eq!(
            format!("{}.two.three", SCRUBBED_PLACEHOLDER),
            crate::net_utils::scrub_sni("one.two.three".to_string())
        );
    }

    #[test]
    fn scrubbing_of_headers() {
        let mut original = http::request::Request::new(()).into_parts().0;
        original.method.clone_from(&http::Method::GET);
        original
            .uri
            .clone_from(&uri::Uri::from_static("https://httpbin.agrd.dev/abcd"));
        original.version.clone_from(&http::Version::HTTP_11);
        original.headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("secret1"),
        );
        original.headers.insert(
            http::header::PROXY_AUTHORIZATION,
            http::HeaderValue::from_static("secret2"),
        );
        original.headers.insert(
            http::header::COOKIE,
            http::HeaderValue::from_static("cookie1"),
        );
        original.headers.append(
            http::header::COOKIE,
            http::HeaderValue::from_static("cookie2"),
        );
        original.headers.append(
            http::header::COOKIE,
            http::HeaderValue::from_static("cookie3"),
        );
        original.headers.insert(
            http::header::ACCEPT,
            http::HeaderValue::from_static("text/html"),
        );
        let mut scrubbed = scrub_request(&original);
        assert_eq!(original.method, scrubbed.method);
        assert_eq!(original.uri, scrubbed.uri);
        assert_eq!(original.version, scrubbed.version);
        assert_eq!(
            vec![SCRUBBED_PLACEHOLDER],
            scrubbed
                .headers
                .get_all(http::header::AUTHORIZATION)
                .iter()
                .map(|x| x.to_str().unwrap())
                .collect::<Vec<&str>>()
        );
        assert_eq!(
            vec![SCRUBBED_PLACEHOLDER],
            scrubbed
                .headers
                .get_all(http::header::PROXY_AUTHORIZATION)
                .iter()
                .map(|x| x.to_str().unwrap())
                .collect::<Vec<&str>>()
        );
        assert_eq!(
            vec![SCRUBBED_PLACEHOLDER],
            scrubbed
                .headers
                .get_all(http::header::COOKIE)
                .iter()
                .map(|x| x.to_str().unwrap())
                .collect::<Vec<&str>>()
        );
        original.headers.remove(http::header::AUTHORIZATION);
        scrubbed.headers.remove(http::header::AUTHORIZATION);
        original.headers.remove(http::header::PROXY_AUTHORIZATION);
        scrubbed.headers.remove(http::header::PROXY_AUTHORIZATION);
        original.headers.remove(http::header::COOKIE);
        scrubbed.headers.remove(http::header::COOKIE);
        assert_eq!(original.headers, scrubbed.headers);
    }

    #[test]
    fn scrubbing_of_headers_do_not_add() {
        let mut original = http::request::Request::new(()).into_parts().0;
        original.headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("secret1"),
        );
        original.headers.insert(
            http::header::PROXY_AUTHORIZATION,
            http::HeaderValue::from_static("secret2"),
        );
        let scrubbed = scrub_request(&original);
        assert_eq!(
            vec![SCRUBBED_PLACEHOLDER],
            scrubbed
                .headers
                .get_all(http::header::AUTHORIZATION)
                .iter()
                .map(|x| x.to_str().unwrap())
                .collect::<Vec<&str>>()
        );
        assert_eq!(
            vec![SCRUBBED_PLACEHOLDER],
            scrubbed
                .headers
                .get_all(http::header::PROXY_AUTHORIZATION)
                .iter()
                .map(|x| x.to_str().unwrap())
                .collect::<Vec<&str>>()
        );
        assert_eq!(
            0,
            scrubbed
                .headers
                .get_all(http::header::COOKIE)
                .iter()
                .count()
        );
    }
}
