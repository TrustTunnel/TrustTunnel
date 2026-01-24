use crate::net_utils;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use smallvec::{smallvec, SmallVec};
use std::borrow::Cow;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

const PROTOCOL_VERSION: u8 = 0x05;
const RESERVED: u8 = 0x00;
const MAX_AUTH_METHODS_NUM: usize = u8::MAX as usize;
const MAX_DOMAIN_NAME_LENGTH: usize = u8::MAX as usize;
const ADDRESS_TYPE_IP_V4: u8 = 0x01;
const ADDRESS_TYPE_DOMAIN_NAME: u8 = 0x03;
const ADDRESS_TYPE_IP_V6: u8 = 0x04;
const UDP_HEADER_FRAG: u8 = 0x00;

const AUTHENTICATION_STATUS_SUCCESS: u8 = 0x00;
const AUTHENTICATION_CODE_NO_AUTH: u8 = 0x00;
const AUTHENTICATION_CODE_USERNAME_PASSWORD: u8 = 0x02;
const AUTHENTICATION_CODE_EXTENDED_AUTH: u8 = 0x80;
const AUTHENTICATION_CODE_NO_ACCEPTABLE: u8 = 0xff;

const USERNAME_PASSWORD_AUTHENTICATION_VER: u8 = 0x01;

const EXTENDED_AUTHENTICATION_TERM_TYPE_CODE: u8 = 0x00;
const EXTENDED_AUTHENTICATION_TERM_VAL_LENGTH: u16 = 0x00;

// smallvec does not allow bigger arrays
type MaxStackSmallVec = SmallVec<[u8; 512]>;

#[derive(Debug)]
pub(crate) enum Error {
    /// A socket error
    Io(io::Error),
    /// A SOCKS protocol error
    Protocol(String),
    /// An authentication failure
    Authentication(String),
}

pub(crate) struct UdpAssociation<S> {
    socket: UdpSocket,
    _stream: S,
}

pub(crate) enum ConnectResult<IO> {
    /// A TCP SOCKS tunnel is successfully established
    TcpConnection(IO),
    /// A UDP association is successfully established
    UdpAssociation(UdpAssociation<IO>),
    /// A server replied with unsuccessful reply code
    Failure(ReplyCode),
}

/// https://datatracker.ietf.org/doc/html/rfc1928#section-3
#[derive(PartialEq)]
enum AuthenticationMethod {
    /// X'00' NO AUTHENTICATION REQUIRED
    NoAuth,
    /// X'02' USERNAME/PASSWORD
    UsernamePassword,
    /// X'80' Custom extended authentication
    ExtendedAuth,
    /// X'FF' NO ACCEPTABLE METHODS
    NoAcceptable,
}

impl AuthenticationMethod {
    const fn to_u8(&self) -> u8 {
        match self {
            Self::NoAuth => AUTHENTICATION_CODE_NO_AUTH,
            Self::UsernamePassword => AUTHENTICATION_CODE_USERNAME_PASSWORD,
            Self::ExtendedAuth => AUTHENTICATION_CODE_EXTENDED_AUTH,
            Self::NoAcceptable => AUTHENTICATION_CODE_NO_ACCEPTABLE,
        }
    }

    const fn from_u8(x: u8) -> Option<Self> {
        match x {
            AUTHENTICATION_CODE_NO_AUTH => Some(Self::NoAuth),
            AUTHENTICATION_CODE_USERNAME_PASSWORD => Some(Self::UsernamePassword),
            AUTHENTICATION_CODE_EXTENDED_AUTH => Some(Self::ExtendedAuth),
            AUTHENTICATION_CODE_NO_ACCEPTABLE => Some(Self::NoAcceptable),
            _ => None,
        }
    }
}

/// The set of available extensions of the extended authentication procedure
#[derive(Clone)]
pub(crate) enum ExtendedAuthenticationValue<'this> {
    /// The domain name which the client used for TLS session (SNI).
    /// The value is a UTF-8 string.
    Domain(Cow<'this, str>),
    /// Public IP address of the VPN client.
    /// The value is an IP address (4 or 16 bytes) in the network byte order.
    ClientAddress(IpAddr),
    /// The `User-Agent` sent by the VPN client.
    /// The value is a UTF-8 string.
    UserAgent(Cow<'this, str>),
    /// The value of the `Proxy-Authorization` header sent by the VPN client,
    /// with the `Basic` keyword stripped.
    /// The value is a base64 encoded string.
    /// **MUST NOT** come together with the [`ExtendedAuthenticationValue::SniAuth`]
    /// in the same message.
    BasicProxyAuth(Cow<'this, str>),
    /// Used as a marker that the VPN client tries to authenticate using the TLS
    /// domain name.
    /// Has no value (the length is zero).
    SniAuth,
}

impl ExtendedAuthenticationValue<'_> {
    const fn type_code(&self) -> u8 {
        match self {
            Self::Domain(_) => 0x01,
            Self::ClientAddress(_) => 0x02,
            Self::UserAgent(_) => 0x03,
            Self::BasicProxyAuth(_) => 0x04,
            Self::SniAuth => 0x05,
        }
    }

    fn into_owned(self) -> ExtendedAuthenticationValue<'static> {
        match self {
            Self::Domain(x) => ExtendedAuthenticationValue::Domain(Cow::Owned(x.into_owned())),
            Self::ClientAddress(x) => ExtendedAuthenticationValue::ClientAddress(x),
            Self::UserAgent(x) => {
                ExtendedAuthenticationValue::UserAgent(Cow::Owned(x.into_owned()))
            }
            Self::BasicProxyAuth(x) => {
                ExtendedAuthenticationValue::BasicProxyAuth(Cow::Owned(x.into_owned()))
            }
            Self::SniAuth => ExtendedAuthenticationValue::SniAuth,
        }
    }
}

#[derive(Clone)]
pub(crate) enum Authentication<'this> {
    /// https://datatracker.ietf.org/doc/html/rfc1929#section-2
    UsernamePassword(Cow<'this, str>, Cow<'this, str>),
    /// Custom extended authentication.
    ///
    /// The extended authentication uses [`AUTHENTICATION_CODE_EXTENDED_AUTH`] as
    /// an authentication method. After a server selects this authentication method,
    /// a client sends a message in the following format:
    /// ```text
    /// +-----+-----------+-----+--------+
    /// | VER |   EXT(0)  |     | EXT(n) |
    /// +-----+-----------+ ... +--------+
    /// |  1  | see below |     |        |
    /// +-----+-----------+-----+--------+
    /// ```
    /// Where:
    ///  * `VER` - the current extended authentication version: 0x01
    ///  * `EXT[i]` - an extension in the following format:
    ///    ```text
    ///    +------+--------+----------+
    ///    | TYPE | LENGTH |   VALUE  |
    ///    +------+--------+----------+
    ///    |  1   |    2   | Variable |
    ///    +------+--------+----------+
    ///    ```
    ///    Where:
    ///     * `TYPE` - a type of the extension value (see [`ExtendedAuthenticationValue`])
    ///     * `LENGTH` - the length of the extension value
    ///     * `VALUE` - the extension value
    ///
    /// A message **MUST** end with a special extension - `TERM` with the
    /// [`EXTENDED_AUTHENTICATION_TERM_TYPE_CODE`] type and zero length.
    ///
    /// The server responds with a standard message as in
    /// [the RFC](https://datatracker.ietf.org/doc/html/rfc1929#section-2).
    Extended(Vec<ExtendedAuthenticationValue<'this>>),
}

impl Authentication<'_> {
    const fn to_method(&self) -> AuthenticationMethod {
        match self {
            Self::UsernamePassword(..) => AuthenticationMethod::UsernamePassword,
            Self::Extended(_) => AuthenticationMethod::ExtendedAuth,
        }
    }

    pub fn into_owned(self) -> Authentication<'static> {
        match self {
            Self::UsernamePassword(u, p) => Authentication::UsernamePassword(
                Cow::Owned(u.into_owned()),
                Cow::Owned(p.into_owned()),
            ),
            Self::Extended(values) => {
                Authentication::Extended(values.into_iter().map(|x| x.into_owned()).collect())
            }
        }
    }
}

/// https://datatracker.ietf.org/doc/html/rfc1928#section-5
#[derive(Clone)]
pub(crate) enum Address<'this> {
    /// X'01' the address is a version-4 IP address, with a length of 4 octets
    /// X'04' the address is a version-6 IP address, with a length of 16 octets
    IpAddress(IpAddr),
    /// X'03' the address field contains a fully-qualified domain name. The first
    /// octet of the address field contains the number of octets of name that
    /// follow, there is no terminating NUL octet.
    DomainName(Cow<'this, str>),
}

impl Address<'_> {
    const fn address_type(&self) -> u8 {
        match self {
            Self::IpAddress(IpAddr::V4(_)) => ADDRESS_TYPE_IP_V4,
            Self::IpAddress(IpAddr::V6(_)) => ADDRESS_TYPE_IP_V6,
            Self::DomainName(_) => ADDRESS_TYPE_DOMAIN_NAME,
        }
    }
}

/// https://datatracker.ietf.org/doc/html/rfc1928#section-4
pub(crate) enum Request<'this> {
    /// CONNECT X'01'
    Connect(Address<'this>, u16),
    /// UDP ASSOCIATE X'03'
    UdpAssociate,
}

impl Request<'_> {
    const fn command_code(&self) -> u8 {
        match self {
            Self::Connect(..) => 0x01,
            Self::UdpAssociate => 0x03,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum ReplyCode {
    /// X'00' succeeded
    Succeeded,
    /// X'01' general SOCKS server failure
    GeneralFailure,
    /// X'02' connection not allowed by ruleset
    NotAllowed,
    /// X'03' Network unreachable
    NetworkUnreachable,
    /// X'04' Host unreachable
    HostUnreachable,
    /// X'05' Connection refused
    ConnectionRefused,
    /// X'06' TTL expired
    TtlExpired,
    /// X'07' Command not supported
    CommandNotSupported,
    /// X'08' Address type not supported
    AddressTypeNotSupported,
}

impl ReplyCode {
    const fn from_u8(x: u8) -> Option<Self> {
        match x {
            0x00 => Some(Self::Succeeded),
            0x01 => Some(Self::GeneralFailure),
            0x02 => Some(Self::NotAllowed),
            0x03 => Some(Self::NetworkUnreachable),
            0x04 => Some(Self::HostUnreachable),
            0x05 => Some(Self::ConnectionRefused),
            0x06 => Some(Self::TtlExpired),
            0x07 => Some(Self::CommandNotSupported),
            0x08 => Some(Self::AddressTypeNotSupported),
            _ => None,
        }
    }
}

/// https://datatracker.ietf.org/doc/html/rfc1928#section-6
struct Reply {
    /// REP Reply field
    code: ReplyCode,
    /// BND.ADDR server bound address
    bound_address: Address<'static>,
    /// BND.PORT server bound port
    bound_port: u16,
}

#[async_trait]
trait SocksWriter: AsyncWriteExt + Sized + Unpin {
    async fn write_selection_message(
        &mut self,
        methods: &[AuthenticationMethod],
    ) -> Result<(), Error> {
        if methods.len() > MAX_AUTH_METHODS_NUM {
            return Err(Error::Protocol("Too many methods".to_string()));
        }

        let mut buf = SmallVec::<[u8; 32]>::with_capacity(
            std::mem::size_of_val(&PROTOCOL_VERSION) + std::mem::size_of::<u8>() + methods.len(),
        );

        buf.push(PROTOCOL_VERSION);
        buf.push(methods.len() as u8);
        buf.extend(methods.iter().map(AuthenticationMethod::to_u8));

        self.write_all(&buf).await.map_err(Error::Io)
    }

    async fn write_authentication_message(&mut self, auth: &Authentication) -> Result<(), Error> {
        let buf = match auth {
            Authentication::UsernamePassword(username, password) => {
                let mut buf = MaxStackSmallVec::with_capacity(
                    std::mem::size_of_val(&USERNAME_PASSWORD_AUTHENTICATION_VER)
                        + std::mem::size_of::<u8>()
                        + username.len()
                        + std::mem::size_of::<u8>()
                        + password.len(),
                );

                buf.push(USERNAME_PASSWORD_AUTHENTICATION_VER);
                buf.push(username.len() as u8);
                buf.extend_from_slice(username.as_bytes());
                buf.push(password.len() as u8);
                buf.extend_from_slice(password.as_bytes());

                buf
            }
            Authentication::Extended(values) => {
                let mut buf: MaxStackSmallVec = smallvec![USERNAME_PASSWORD_AUTHENTICATION_VER];

                for value in values {
                    write_extended_authentication_value(&mut buf, value)?;
                }
                write_extended_authentication_term_value(&mut buf)?;

                buf
            }
        };

        self.write_all(&buf).await.map_err(Error::Io)
    }

    async fn write_request(
        &mut self,
        command: u8,
        destination: &Address<'_>,
        port: u16,
    ) -> Result<(), Error> {
        let mut buf = MaxStackSmallVec::with_capacity(
            std::mem::size_of_val(&PROTOCOL_VERSION)
                + std::mem::size_of_val(&command)
                + std::mem::size_of_val(&RESERVED)
                + std::mem::size_of_val(&destination.address_type())
                + match destination {
                    Address::IpAddress(IpAddr::V4(_)) => net_utils::IPV4_WIRE_LENGTH,
                    Address::IpAddress(IpAddr::V6(_)) => net_utils::IPV6_WIRE_LENGTH,
                    Address::DomainName(x) => {
                        std::mem::size_of::<u8>() + x.len().min(MAX_DOMAIN_NAME_LENGTH)
                    }
                }
                + std::mem::size_of_val(&port),
        );

        buf.push(PROTOCOL_VERSION);
        buf.push(command);
        buf.push(RESERVED);

        buf.push(destination.address_type());
        match destination {
            Address::IpAddress(IpAddr::V4(x)) => buf.extend_from_slice(&x.octets()),
            Address::IpAddress(IpAddr::V6(x)) => buf.extend_from_slice(&x.octets()),
            Address::DomainName(x) if x.len() <= MAX_DOMAIN_NAME_LENGTH => {
                buf.push(x.len() as u8);
                buf.extend_from_slice(x.as_bytes());
            }
            Address::DomainName(_) => {
                return Err(Error::Protocol("Too long domain name".to_string()))
            }
        }

        put_u16(&mut buf, port);

        self.write_all(&buf).await.map_err(Error::Io)
    }
}

#[async_trait]
trait SocksReader: AsyncReadExt + Unpin {
    async fn read_version(&mut self) -> Result<(), Error> {
        let version = self.read_u8().await.map_err(Error::Io)?;
        if version != PROTOCOL_VERSION {
            return Err(Error::Protocol(format!(
                "Unexpected protocol version: {}",
                version
            )));
        }

        Ok(())
    }

    async fn read_selection_response(&mut self) -> Result<AuthenticationMethod, Error> {
        self.read_version().await?;

        let method = self.read_u8().await.map_err(Error::Io)?;
        AuthenticationMethod::from_u8(method)
            .ok_or_else(|| Error::Protocol(format!("Unexpected authentication method: {}", method)))
    }

    async fn read_authentication_response(&mut self) -> Result<(), Error> {
        let version = self.read_u8().await.map_err(Error::Io)?;
        if version != USERNAME_PASSWORD_AUTHENTICATION_VER {
            return Err(Error::Protocol(format!(
                "Unexpected authentication version: {}",
                version
            )));
        }

        let status = self.read_u8().await.map_err(Error::Io)?;
        if status != AUTHENTICATION_STATUS_SUCCESS {
            return Err(Error::Authentication(format!("Status={}", status)));
        }

        Ok(())
    }

    async fn read_reply(&mut self) -> Result<Reply, Error> {
        self.read_version().await?;
        let reply_code = self.read_u8().await.map_err(Error::Io)?;
        let reply_code = ReplyCode::from_u8(reply_code)
            .ok_or_else(|| Error::Protocol(format!("Unexpected reply code: {}", reply_code)))?;

        let reserved = self.read_u8().await.map_err(Error::Io)?;
        if reserved != RESERVED {
            return Err(Error::Protocol(format!(
                "Unexpected reserved field value: {}",
                reserved
            )));
        }

        let address = match self.read_u8().await.map_err(Error::Io)? {
            ADDRESS_TYPE_IP_V4 => {
                let mut bytes = [0; net_utils::IPV4_WIRE_LENGTH];
                self.read_exact(&mut bytes).await.map_err(Error::Io)?;
                Address::IpAddress(IpAddr::from(bytes))
            }
            ADDRESS_TYPE_IP_V6 => {
                let mut bytes = [0; net_utils::IPV6_WIRE_LENGTH];
                self.read_exact(&mut bytes).await.map_err(Error::Io)?;
                Address::IpAddress(IpAddr::from(bytes))
            }
            ADDRESS_TYPE_DOMAIN_NAME => {
                let length = self.read_u8().await.map_err(Error::Io)?;
                let mut buf = vec![0; length as usize];
                self.read_exact(&mut buf).await.map_err(Error::Io)?;
                Address::DomainName(Cow::from(String::from_utf8(buf).map_err(|e| {
                    Error::Protocol(format!("Domain name parse failure: {}", e))
                })?))
            }
            x => return Err(Error::Protocol(format!("Unexpected address type: {}", x))),
        };

        Ok(Reply {
            code: reply_code,
            bound_address: address,
            bound_port: self.read_u16().await.map_err(Error::Io)?,
        })
    }
}

pub(crate) async fn connect<IO>(
    io: IO,
    auth: Option<Authentication<'_>>,
    request: Request<'_>,
) -> Result<ConnectResult<IO>, Error>
where
    IO: AsyncWriteExt + AsyncReadExt + Send + Unpin,
{
    connect_inner(io, auth, request).await
}

#[async_trait]
impl<T: AsyncWriteExt + Unpin> SocksWriter for T {}

#[async_trait]
impl<T: AsyncReadExt + Unpin> SocksReader for T {}

async fn connect_inner<IO>(
    mut io: IO,
    auth: Option<Authentication<'_>>,
    request: Request<'_>,
) -> Result<ConnectResult<IO>, Error>
where
    IO: SocksWriter + SocksReader + Send,
{
    io.write_selection_message(&[
        auth.as_ref()
            .map(Authentication::to_method)
            .unwrap_or(AuthenticationMethod::NoAuth),
        AuthenticationMethod::NoAuth,
    ])
    .await?;

    match (io.read_selection_response().await?, auth.as_ref()) {
        (AuthenticationMethod::NoAuth, _) => {}
        (AuthenticationMethod::UsernamePassword, Some(Authentication::UsernamePassword(..)))
        | (AuthenticationMethod::ExtendedAuth, Some(Authentication::Extended(_))) => {
            io.write_authentication_message(auth.as_ref().unwrap())
                .await?;
            io.read_authentication_response().await?;
        }
        (AuthenticationMethod::NoAcceptable, _) => {
            return Err(Error::Authentication(
                "Server rejected offered authentication methods".to_string(),
            ))
        }
        _ => {
            return Err(Error::Authentication(
                "Server selected non-offered authentication method".to_string(),
            ))
        }
    }

    let (destination, port, udp_socket) = match &request {
        Request::Connect(d, p) => (d.clone(), *p, None),
        Request::UdpAssociate => {
            let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
                .await
                .map_err(Error::Io)?;
            let bound_address = socket.local_addr().map_err(Error::Io)?;
            (
                Address::IpAddress(bound_address.ip()),
                bound_address.port(),
                Some(socket),
            )
        }
    };

    io.write_request(request.command_code(), &destination, port)
        .await?;

    let reply = io.read_reply().await?;
    if reply.code != ReplyCode::Succeeded {
        return Ok(ConnectResult::Failure(reply.code));
    }

    match request {
        Request::Connect(..) => Ok(ConnectResult::TcpConnection(io)),
        Request::UdpAssociate => {
            let bound_address = match reply.bound_address {
                Address::IpAddress(x) => SocketAddr::from((x, reply.bound_port)),
                Address::DomainName(x) => {
                    return Err(Error::Protocol(format!("Unexpected bound address: {}", x)))
                }
            };
            udp_socket
                .as_ref()
                .unwrap()
                .connect(bound_address)
                .await
                .map_err(Error::Io)?;
            Ok(ConnectResult::UdpAssociation(UdpAssociation {
                socket: udp_socket.unwrap(),
                _stream: io,
            }))
        }
    }
}

impl<S> UdpAssociation<S> {
    pub fn get_ref(&self) -> &UdpSocket {
        &self.socket
    }

    pub async fn send_to(&self, data: &[u8], destination: SocketAddr) -> Result<(), Error> {
        let mut buf = BytesMut::with_capacity(udp_buffer_size(
            if destination.is_ipv4() {
                net_utils::IPV4_WIRE_LENGTH
            } else {
                net_utils::IPV6_WIRE_LENGTH
            },
            data.len(),
        ));

        buf.put_u8(RESERVED);
        buf.put_u8(RESERVED);
        // Fragmentation is not supported for now
        buf.put_u8(UDP_HEADER_FRAG);

        match destination.ip() {
            IpAddr::V4(x) => {
                buf.put_u8(ADDRESS_TYPE_IP_V4);
                buf.put(x.octets().as_slice());
            }
            IpAddr::V6(x) => {
                buf.put_u8(ADDRESS_TYPE_IP_V6);
                buf.put(x.octets().as_slice());
            }
        }

        buf.put_u16(destination.port());
        buf.put(data);

        self.socket.send(&buf).await.map(|_| ()).map_err(Error::Io)
    }

    pub async fn recv_from(&self, data: &mut [u8]) -> Result<(usize, SocketAddr), Error> {
        const MIN_UDP_PACKET_SIZE: usize = udp_buffer_size(net_utils::IPV4_WIRE_LENGTH, 0);

        let mut buf = vec![0; udp_buffer_size(net_utils::IPV6_WIRE_LENGTH, data.len())];
        let n = self.socket.recv(&mut buf).await.map_err(Error::Io)?;
        buf.truncate(n);
        if n < MIN_UDP_PACKET_SIZE {
            return Err(Error::Protocol("Too short packet".to_string()));
        }

        let mut buf = Bytes::from(buf);
        if RESERVED != buf.get_u8() || RESERVED != buf.get_u8() {
            return Err(Error::Protocol("Unexpected reserved bytes".to_string()));
        }
        // Fragmentation is not supported for now
        if UDP_HEADER_FRAG != buf.get_u8() {
            return Err(Error::Protocol("Unexpected fragmentation byte".to_string()));
        }

        let source = match buf.get_u8() {
            // SAFETY: enough capacity is guaranteed by checking length against `MIN_UDP_PACKET_SIZE`
            ADDRESS_TYPE_IP_V4 => {
                let mut addr_octets = [0; net_utils::IPV4_WIRE_LENGTH];
                buf.copy_to_slice(&mut addr_octets);
                IpAddr::from(addr_octets)
            }
            ADDRESS_TYPE_IP_V6 => {
                if buf.remaining() < net_utils::IPV6_WIRE_LENGTH {
                    return Err(Error::Protocol(
                        "Packet length doesn't conform to announced address type".to_string(),
                    ));
                }

                let mut addr_octets = [0; net_utils::IPV6_WIRE_LENGTH];
                buf.copy_to_slice(&mut addr_octets);
                IpAddr::from(addr_octets)
            }
            x => return Err(Error::Protocol(format!("Unexpected address type: {}", x))),
        };

        if buf.len() < std::mem::size_of::<u16>() {
            return Err(Error::Protocol("Too short packet".to_string()));
        }

        let source = SocketAddr::from((source, buf.get_u16()));
        let n = buf.remaining();
        let cap = data.len();
        buf.copy_to_slice(&mut data[..std::cmp::min(n, cap)]);

        Ok((n, source))
    }
}

const fn udp_buffer_size(address_size: usize, data_cap: usize) -> usize {
    std::mem::size_of::<u16>() // reserved
        + std::mem::size_of::<u8>() // fragmentation
        + std::mem::size_of::<u8>() // address type
        + address_size
        + std::mem::size_of::<u16>() // port
        + data_cap
}

fn write_extended_authentication_value<A>(
    buf: &mut SmallVec<A>,
    value: &ExtendedAuthenticationValue,
) -> Result<(), Error>
where
    A: smallvec::Array<Item = u8>,
{
    buf.push(value.type_code());

    match value {
        ExtendedAuthenticationValue::Domain(x) => {
            if x.len() > u16::MAX as usize {
                return Err(Error::Protocol("Too long domain name".to_string()));
            }

            put_u16(buf, x.len() as u16);
            buf.extend_from_slice(x.as_bytes());
        }
        ExtendedAuthenticationValue::ClientAddress(x) => match x {
            IpAddr::V4(x) => {
                put_u16(buf, net_utils::IPV4_WIRE_LENGTH as u16);
                buf.extend_from_slice(&x.octets());
            }
            IpAddr::V6(x) => {
                put_u16(buf, net_utils::IPV6_WIRE_LENGTH as u16);
                buf.extend_from_slice(&x.octets());
            }
        },
        ExtendedAuthenticationValue::UserAgent(x) => {
            if x.len() > u16::MAX as usize {
                return Err(Error::Protocol("Too long User-Agent".to_string()));
            }

            put_u16(buf, x.len() as u16);
            buf.extend_from_slice(x.as_bytes());
        }
        ExtendedAuthenticationValue::BasicProxyAuth(x) => {
            if x.len() > u16::MAX as usize {
                return Err(Error::Protocol("Too long Proxy-Authorization".to_string()));
            }

            put_u16(buf, x.len() as u16);
            buf.extend_from_slice(x.as_bytes());
        }
        ExtendedAuthenticationValue::SniAuth => {
            put_u16(buf, 0_u16);
        }
    }

    Ok(())
}

fn write_extended_authentication_term_value<A>(buf: &mut SmallVec<A>) -> Result<(), Error>
where
    A: smallvec::Array<Item = u8>,
{
    buf.push(EXTENDED_AUTHENTICATION_TERM_TYPE_CODE);
    put_u16(buf, EXTENDED_AUTHENTICATION_TERM_VAL_LENGTH);
    Ok(())
}

fn put_u16<A>(buf: &mut SmallVec<A>, x: u16)
where
    A: smallvec::Array<Item = u8>,
{
    buf.extend_from_slice(&x.to_be_bytes())
}
