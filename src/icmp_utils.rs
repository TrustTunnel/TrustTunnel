use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use crate::net_utils;


const TYPE_SIZE: usize = 1;
const CODE_SIZE: usize = 1;
const CHECKSUM_SIZE: usize = 2;
const ICMP_ID_SIZE: usize = 2;
const ICMP_SEQNO_SIZE: usize = 2;
/// The meaning of the last 4 bytes depends on the type, but they are always present
/// in a message
const ICMP_MIN_COMMON_HEADER_SIZE: usize = TYPE_SIZE + CODE_SIZE + CHECKSUM_SIZE + 4;
const ICMP_V4_MIN_MATCHING_DATA_SIZE: usize = net_utils::MIN_IPV4_HEADER_SIZE + ICMP_MIN_COMMON_HEADER_SIZE;
const ECHO_HEADER_SIZE: usize = TYPE_SIZE + CODE_SIZE + CHECKSUM_SIZE + ICMP_ID_SIZE + ICMP_SEQNO_SIZE;


#[derive(Debug, Clone)]
pub(crate) enum Message {
    V4(v4::Message),
    V6(v6::Message),
}

#[derive(Debug, Clone)]
pub(crate) struct Echo {
    pub code: u8,
    /// If code = 0, an identifier to aid in matching echos and replies, may be zero.
    pub identifier: u16,
    /// If code = 0, a sequence number to aid in matching echos and replies, may be zero.
    pub sequence_number: u16,
    /// The data received in the echo message must be returned in the echo reply message.
    pub data: Bytes,
}

#[derive(Debug)]
pub(crate) enum DeserializeError {
    InvalidLength(String),
    MessageType(u8),
    DestinationUnreachableCode(u8),
    TimeExceededCode(u8),
}

pub(crate) type DeserializeResult<M> = std::result::Result<M, DeserializeError>;

impl Message {
    /// Serialize the message into the wire format
    pub fn serialize(&self) -> Bytes {
        match self {
            Message::V4(x) => x.serialize(),
            Message::V6(x) => x.serialize(),
        }
    }

    /// Unwrap an [`Echo`] message in case the message contains it
    pub fn to_echo(&self) -> Option<&Echo> {
        match self {
            Message::V4(v4::Message::Echo(x))
            | Message::V4(v4::Message::EchoReply(x))
            | Message::V6(v6::Message::EchoRequest(x))
            | Message::V6(v6::Message::EchoReply(x))
            => Some(x),
            _ => None,
        }
    }

    /// Get the echo request that is responded responded by this message
    pub fn responded_echo_request(&self) -> Option<Echo> {
        match self {
            Message::V4(x) => x.responded_echo_request(),
            Message::V6(x) => x.responded_echo_request(),
        }
    }

    /// Get the message type identifier
    pub fn type_id(&self) -> u8 {
        match self {
            Message::V4(x) => x.type_id().0,
            Message::V6(x) => x.type_id().0,
        }
    }

    /// Get the message code identifier
    pub fn code(&self) -> u8 {
        match self {
            Message::V4(x) => x.code(),
            Message::V6(x) => x.code(),
        }
    }
}

impl Hash for Echo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identifier.hash(state);
        self.sequence_number.hash(state);
    }
}

impl Eq for Echo {}

impl PartialEq for Echo {
    fn eq(&self, other: &Self) -> bool {
        if self.identifier != other.identifier
            || self.sequence_number != other.sequence_number
        {
            return false;
        }

        if self.data.len() >= other.data.len() {
            self.data.starts_with(&other.data)
        } else {
            other.data.starts_with(&self.data)
        }
    }
}

impl Echo {
    fn serialize(&self, type_id: u8) -> Bytes {
        let mut packet = BytesMut::new();
        packet.reserve(ECHO_HEADER_SIZE + self.data.len());

        packet.put_u8(type_id);
        packet.put_u8(0);
        packet.put_u16(0);
        packet.put_u16(self.identifier);
        packet.put_u16(self.sequence_number);
        packet.put_slice(&self.data);

        let checksum = net_utils::rfc1071_checksum(&packet).to_be_bytes();
        packet[2..4].copy_from_slice(&checksum);

        packet.freeze()
    }
}

impl From<v4::Message> for Message {
    fn from(x: v4::Message) -> Self {
        Self::V4(x)
    }
}

impl From<v6::Message> for Message {
    fn from(x: v6::Message) -> Self {
        Self::V6(x)
    }
}

enum LengthCheck {
    Exact(usize),
    LowerBound(usize),
}

macro_rules! deserialize_packet {
    ($func:ident, $packet:ident, $length_check:expr, $out_type:ident) => {
        match ($length_check, 1 + $packet.len()) { // 1 is for message type extracted earlier
            (LengthCheck::Exact(n), m) if n != m => Err(super::DeserializeError::InvalidLength(
                format!("Expected length: {}, packet length: {}", n, m)
            )),
            (LengthCheck::LowerBound(n), m) if m < n => Err(super::DeserializeError::InvalidLength(
                format!("Expected length at least: {}, packet length: {}", n, m)
            )),
            _ => {
                let code = $packet.get_u8();
                // todo: verify checksum
                Self::$func(code, $packet.split_off(super::CHECKSUM_SIZE)).map(Message::$out_type)
            }
        }
    }
}

pub(crate) mod v4 {
    use bytes::{Buf, Bytes};
    use crate::net_utils;
    use super::LengthCheck;

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub(crate) struct TypeId(pub u8);

    impl TypeId {
        pub const ECHO_REPLY: TypeId = TypeId(0);
        pub const DESTINATION_UNREACHABLE: TypeId = TypeId(3);
        pub const SOURCE_QUENCH: TypeId = TypeId(4);
        pub const REDIRECT: TypeId = TypeId(5);
        pub const ECHO: TypeId = TypeId(8);
        pub const TIME_EXCEEDED: TypeId = TypeId(11);
        pub const PARAMETER_PROBLEM: TypeId = TypeId(12);
        pub const TIMESTAMP: TypeId = TypeId(13);
        pub const TIMESTAMP_REPLY: TypeId = TypeId(14);
        pub const INFORMATION_REQUEST: TypeId = TypeId(15);
        pub const INFORMATION_REPLY: TypeId = TypeId(16);
    }

    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        DestinationUnreachable(DestinationUnreachable),
        TimeExceeded(TimeExceeded),
        ParameterProblem(ParameterProblem),
        SourceQuench(SourceQuench),
        Redirect(Redirect),
        Echo(super::Echo),
        EchoReply(super::Echo),
        Timestamp(Timestamp),
        TimestampReply(Timestamp),
        InformationRequest(Information),
        InformationReply(Information),
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub(crate) struct DestinationUnreachableCode(u8);

    impl DestinationUnreachableCode {
        pub const NET_UNREACHABLE: DestinationUnreachableCode = DestinationUnreachableCode(0);
        pub const HOST_UNREACHABLE: DestinationUnreachableCode = DestinationUnreachableCode(1);
        pub const PROTOCOL_UNREACHABLE: DestinationUnreachableCode = DestinationUnreachableCode(2);
        pub const PORT_UNREACHABLE: DestinationUnreachableCode = DestinationUnreachableCode(3);
        pub const FRAGMENTATION_NEEDED: DestinationUnreachableCode = DestinationUnreachableCode(4);
        pub const ROUTE_FAILED: DestinationUnreachableCode = DestinationUnreachableCode(5);
    }

    #[derive(Debug, Clone)]
    pub(crate) struct DestinationUnreachable {
        pub code: DestinationUnreachableCode,
        /// The internet header plus the first 64 bits of the original
        /// datagram's data.  This data is used by the host to match the
        /// message to the appropriate process.  If a higher level protocol
        /// uses port numbers, they are assumed to be in the first 64 data
        /// bits of the original datagram's data.
        pub data: Bytes,
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub(crate) struct TimeExceededCode(u8);

    impl TimeExceededCode {
        /// Time to live exceeded in transit
        pub const TTL_EXCEEDED: TimeExceededCode = TimeExceededCode(0);
        /// Fragment reassembly time exceeded
        pub const FRAGMENT_REASSEMBLY: TimeExceededCode = TimeExceededCode(1);
    }

    #[derive(Debug, Clone)]
    pub(crate) struct TimeExceeded {
        pub code: TimeExceededCode,
        /// The internet header plus the first 64 bits of the original
        /// datagram's data.  This data is used by the host to match the
        /// message to the appropriate process.  If a higher level protocol
        /// uses port numbers, they are assumed to be in the first 64 data
        /// bits of the original datagram's data.
        pub data: Bytes,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct ParameterProblem {
        pub code: u8,
        /// If code = 0, identifies the octet where an error was detected.
        #[allow(unused)]
        pub pointer: u8,
        /// The internet header plus the first 64 bits of the original
        /// datagram's data.  This data is used by the host to match the
        /// message to the appropriate process.  If a higher level protocol
        /// uses port numbers, they are assumed to be in the first 64 data
        /// bits of the original datagram's data.
        pub data: Bytes,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct SourceQuench {
        pub code: u8,
        /// The internet header plus the first 64 bits of the original
        /// datagram's data.  This data is used by the host to match the
        /// message to the appropriate process.  If a higher level protocol
        /// uses port numbers, they are assumed to be in the first 64 data
        /// bits of the original datagram's data.
        pub data: Bytes,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct Redirect {
        /// 0 = Redirect datagrams for the Network.
        /// 1 = Redirect datagrams for the Host.
        /// 2 = Redirect datagrams for the Type of Service and Network.
        /// 3 = Redirect datagrams for the Type of Service and Host.
        pub code: u8,
        /// Address of the gateway to which traffic for the network specified
        /// in the internet destination network field of the original
        /// datagram's data should be sent.
        #[allow(unused)]
        pub gateway_redirect_address: u32,
        /// The internet header plus the first 64 bits of the original
        /// datagram's data.  This data is used by the host to match the
        /// message to the appropriate process.  If a higher level protocol
        /// uses port numbers, they are assumed to be in the first 64 data
        /// bits of the original datagram's data.
        pub data: Bytes,
    }

    #[derive(Debug, Clone)]
    #[allow(unused)]
    pub(crate) struct Timestamp {
        pub code: u8,
        /// If code = 0, an identifier to aid in matching timestamp and replies, may be zero.
        pub identifier: u16,
        /// If code = 0, a sequence number to aid in matching timestamp and replies, may be zero.
        pub sequence_number: u16,
        /// The time the sender last touched the message before sending it.
        pub originate_timestamp: u32,
        /// The time the echoer first touched it on receipt.
        pub receive_timestamp: u32,
        /// The time the echoer last touched the message on sending it.
        pub transmit_timestamp: u32,
    }

    #[derive(Debug, Clone)]
    #[allow(unused)]
    pub(crate) struct Information {
        pub code: u8,
        /// If code = 0, an identifier to aid in matching request and replies, may be zero.
        pub identifier: u16,
        /// If code = 0, a sequence number to aid in matching request and replies, may be zero.
        pub sequence_number: u16,
    }

    impl Message {
        pub fn type_id(&self) -> TypeId {
            match self {
                Message::DestinationUnreachable(_) => TypeId::DESTINATION_UNREACHABLE,
                Message::TimeExceeded(_) => TypeId::TIME_EXCEEDED,
                Message::ParameterProblem(_) => TypeId::PARAMETER_PROBLEM,
                Message::SourceQuench(_) => TypeId::SOURCE_QUENCH,
                Message::Redirect(_) => TypeId::REDIRECT,
                Message::Echo(_) => TypeId::ECHO,
                Message::EchoReply(_) => TypeId::ECHO_REPLY,
                Message::Timestamp(_) => TypeId::TIMESTAMP,
                Message::TimestampReply(_) => TypeId::TIMESTAMP_REPLY,
                Message::InformationRequest(_) => TypeId::INFORMATION_REQUEST,
                Message::InformationReply(_) => TypeId::INFORMATION_REPLY,
            }
        }

        pub fn code(&self) -> u8 {
            match self {
                Message::DestinationUnreachable(x) => x.code.0,
                Message::TimeExceeded(x) => x.code.0,
                Message::ParameterProblem(x) => x.code,
                Message::SourceQuench(x) => x.code,
                Message::Redirect(x) => x.code,
                Message::Echo(x) | Message::EchoReply(x) => x.code,
                Message::Timestamp(x) => x.code,
                Message::TimestampReply(x) => x.code,
                Message::InformationRequest(x) | Message::InformationReply(x) => x.code,
            }
        }

        pub fn responded_echo_request(&self) -> Option<super::Echo> {
            let icmp_data = match self {
                Message::DestinationUnreachable(x) => Some(&x.data),
                Message::TimeExceeded(x) => Some(&x.data),
                Message::ParameterProblem(x) => Some(&x.data),
                Message::SourceQuench(x) => Some(&x.data),
                Message::Redirect(x) => Some(&x.data),
                Message::EchoReply(x) => return Some(x.clone()),
                _ => None,
            }?;

            let (proto, mut payload) = net_utils::skip_ipv4_header(icmp_data.clone())?;
            if proto != libc::IPPROTO_ICMP
                || payload.is_empty()
                || TypeId(payload.get_u8()) != TypeId::ECHO
            {
                return None;
            }
            match deserialize_packet!(parse_echo, payload, LengthCheck::LowerBound(super::ECHO_HEADER_SIZE), Echo) {
                Ok(Message::Echo(x)) => Some(x),
                _ => None,
            }
        }

        pub fn serialize(&self) -> Bytes {
            match self {
                Message::Echo(x) => x.serialize(self.type_id().0),
                x => unreachable!("{:?}", x),
            }
        }

        pub fn deserialize(mut packet: Bytes) -> super::DeserializeResult<Self> {
            if packet.is_empty() {
                return Err(super::DeserializeError::InvalidLength("Empty packet".to_string()));
            }

            match TypeId(packet.get_u8()) {
                TypeId::ECHO_REPLY =>
                    deserialize_packet!(parse_echo, packet, LengthCheck::LowerBound(super::ECHO_HEADER_SIZE), EchoReply),
                TypeId::DESTINATION_UNREACHABLE =>
                    deserialize_packet!(parse_destination_unreachable, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + super::ICMP_V4_MIN_MATCHING_DATA_SIZE), DestinationUnreachable),
                TypeId::SOURCE_QUENCH =>
                    deserialize_packet!(parse_source_quench, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + super::ICMP_V4_MIN_MATCHING_DATA_SIZE), SourceQuench),
                TypeId::REDIRECT =>
                    deserialize_packet!(parse_redirect, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + super::ICMP_V4_MIN_MATCHING_DATA_SIZE), Redirect),
                TypeId::ECHO =>
                    deserialize_packet!(parse_echo, packet, LengthCheck::LowerBound(super::ECHO_HEADER_SIZE), Echo),
                TypeId::TIME_EXCEEDED =>
                    deserialize_packet!(parse_time_exceeded, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + super::ICMP_V4_MIN_MATCHING_DATA_SIZE), TimeExceeded),
                TypeId::PARAMETER_PROBLEM =>
                    deserialize_packet!(parse_parameter_problem, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + super::ICMP_V4_MIN_MATCHING_DATA_SIZE), ParameterProblem),
                TypeId::TIMESTAMP =>
                    deserialize_packet!(parse_timestamp, packet, LengthCheck::Exact(20), Timestamp),
                TypeId::TIMESTAMP_REPLY =>
                    deserialize_packet!(parse_timestamp, packet, LengthCheck::Exact(20), TimestampReply),
                TypeId::INFORMATION_REQUEST =>
                    deserialize_packet!(parse_information, packet, LengthCheck::Exact(20), InformationRequest),
                TypeId::INFORMATION_REPLY =>
                    deserialize_packet!(parse_information, packet, LengthCheck::Exact(20), InformationReply),
                x => Err(super::DeserializeError::MessageType(x.0)),
            }
        }

        fn parse_echo(code: u8, packet: Bytes) -> super::DeserializeResult<super::Echo> {
            super::parse_echo(code, packet)
        }

        fn parse_destination_unreachable(code: u8, mut packet: Bytes)
            -> super::DeserializeResult<DestinationUnreachable>
        {
            Ok(DestinationUnreachable {
                code: match DestinationUnreachableCode(code) {
                    DestinationUnreachableCode::NET_UNREACHABLE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::HOST_UNREACHABLE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::PROTOCOL_UNREACHABLE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::PORT_UNREACHABLE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::FRAGMENTATION_NEEDED => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::ROUTE_FAILED => DestinationUnreachableCode(code),
                    _ => return Err(super::DeserializeError::DestinationUnreachableCode(code)),
                },
                data: packet.split_off(4),
            })
        }

        fn parse_source_quench(code: u8, mut packet: Bytes) -> super::DeserializeResult<SourceQuench> {
            Ok(SourceQuench {
                code,
                data: packet.split_off(4),
            })
        }

        fn parse_redirect(code: u8, mut packet: Bytes) -> super::DeserializeResult<Redirect> {
            Ok(Redirect {
                code,
                gateway_redirect_address: packet.get_u32(),
                data: packet,
            })
        }

        fn parse_time_exceeded(code: u8, mut packet: Bytes) -> super::DeserializeResult<TimeExceeded> {
            Ok(TimeExceeded {
                code: match TimeExceededCode(code) {
                    TimeExceededCode::TTL_EXCEEDED => TimeExceededCode(code),
                    TimeExceededCode::FRAGMENT_REASSEMBLY => TimeExceededCode(code),
                    _ => return Err(super::DeserializeError::TimeExceededCode(code)),
                },
                data: packet.split_off(4),
            })
        }

        fn parse_parameter_problem(code: u8, mut packet: Bytes) -> super::DeserializeResult<ParameterProblem> {
            Ok(ParameterProblem {
                code,
                pointer: packet.get_u8(),
                data: packet.split_off(3),
            })
        }

        fn parse_timestamp(code: u8, mut packet: Bytes) -> super::DeserializeResult<Timestamp> {
            Ok(Timestamp {
                code,
                identifier: packet.get_u16(),
                sequence_number: packet.get_u16(),
                originate_timestamp: packet.get_u32(),
                receive_timestamp: packet.get_u32(),
                transmit_timestamp: packet.get_u32(),
            })
        }

        fn parse_information(code: u8, mut packet: Bytes) -> super::DeserializeResult<Information> {
            Ok(Information {
                code,
                identifier: packet.get_u16(),
                sequence_number: packet.get_u16(),
            })
        }
    }
}

pub(crate) mod v6 {
    use bytes::{Buf, Bytes};
    use crate::net_utils;
    use super::LengthCheck;

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub(crate) struct TypeId(pub u8);

    impl TypeId {
        pub const DESTINATION_UNREACHABLE: TypeId = TypeId(1);
        pub const PACKET_TOO_BIG: TypeId = TypeId(2);
        pub const TIME_EXCEEDED: TypeId = TypeId(3);
        pub const PARAMETER_PROBLEM: TypeId = TypeId(4);
        pub const ECHO_REQUEST: TypeId = TypeId(128);
        pub const ECHO_REPLY: TypeId = TypeId(129);
    }

    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        DestinationUnreachable(DestinationUnreachable),
        PacketTooBig(PacketTooBig),
        TimeExceeded(TimeExceeded),
        ParameterProblem(ParameterProblem),
        EchoRequest(super::Echo),
        EchoReply(super::Echo),
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub(crate) struct DestinationUnreachableCode(u8);

    impl DestinationUnreachableCode {
        /// No route to destination
        pub const NO_ROUTE: DestinationUnreachableCode = DestinationUnreachableCode(0);
        /// Communication with destination administratively prohibited
        pub const ADMINISTRATIVELY_PROHIBITED: DestinationUnreachableCode = DestinationUnreachableCode(1);
        /// Beyond scope of source address
        pub const BEYOND_SCOPE: DestinationUnreachableCode = DestinationUnreachableCode(2);
        /// Address unreachable
        pub const ADDRESS_UNREACHABLE: DestinationUnreachableCode = DestinationUnreachableCode(3);
        /// Port unreachable
        pub const PORT_UNREACHABLE: DestinationUnreachableCode = DestinationUnreachableCode(4);
        /// Source address failed ingress/egress policy
        pub const SOURCE_FAILED_POLICY: DestinationUnreachableCode = DestinationUnreachableCode(5);
        /// Reject route to destination
        pub const REJECT_ROUTE: DestinationUnreachableCode = DestinationUnreachableCode(6);
    }

    #[derive(Debug, Clone)]
    pub(crate) struct DestinationUnreachable {
        pub code: DestinationUnreachableCode,
        /// As much of invoking packet as possible without the ICMPv6 packet
        /// exceeding the minimum IPv6 MTU.
        pub data: Bytes,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct PacketTooBig {
        pub code: u8,
        /// The Maximum Transmission Unit of the next-hop link.
        #[allow(unused)]
        pub mtu: u32,
        /// As much of invoking packet as possible without the ICMPv6 packet
        /// exceeding the minimum IPv6 MTU.
        pub data: Bytes,
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub(crate) struct TimeExceededCode(u8);

    impl TimeExceededCode {
        /// Hop limit exceeded in transit
        pub const HOP_LIMIT: TimeExceededCode = TimeExceededCode(0);
        /// Fragment reassembly time exceeded
        pub const FRAGMENT_REASSEMBLY: TimeExceededCode = TimeExceededCode(1);
    }

    #[derive(Debug, Clone)]
    pub(crate) struct TimeExceeded {
        pub code: TimeExceededCode,
        /// As much of invoking packet as possible without the ICMPv6 packet
        /// exceeding the minimum IPv6 MTU.
        pub data: Bytes,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct ParameterProblem {
        /// 0 - Erroneous header field encountered
        /// 1 - Unrecognized Next Header type encountered
        /// 2 - Unrecognized IPv6 option encountered
        pub code: u8,
        /// Identifies the octet offset within the
        /// invoking packet where the error was detected.
        ///
        /// The pointer will point beyond the end of the ICMPv6
        /// packet if the field in error is beyond what can fit
        /// in the maximum size of an ICMPv6 error message.
        #[allow(unused)]
        pub pointer: u32,
        /// As much of invoking packet as possible without the ICMPv6 packet
        /// exceeding the minimum IPv6 MTU.
        pub data: Bytes,
    }

    impl Message {
        pub fn type_id(&self) -> TypeId {
            match self {
                Message::DestinationUnreachable(_) => TypeId::DESTINATION_UNREACHABLE,
                Message::PacketTooBig(_) => TypeId::PACKET_TOO_BIG,
                Message::TimeExceeded(_) => TypeId::TIME_EXCEEDED,
                Message::ParameterProblem(_) => TypeId::PARAMETER_PROBLEM,
                Message::EchoRequest(_) => TypeId::ECHO_REQUEST,
                Message::EchoReply(_) => TypeId::ECHO_REPLY,
            }
        }

        pub fn code(&self) -> u8 {
            match self {
                Message::DestinationUnreachable(x) => x.code.0,
                Message::PacketTooBig(x) => x.code,
                Message::TimeExceeded(x) => x.code.0,
                Message::ParameterProblem(x) => x.code,
                Message::EchoRequest(x) | Message::EchoReply(x) => x.code,
            }
        }

        pub fn responded_echo_request(&self) -> Option<super::Echo> {
            let icmp_data = match self {
                Message::DestinationUnreachable(x) => Some(&x.data),
                Message::PacketTooBig(x) => Some(&x.data),
                Message::TimeExceeded(x) => Some(&x.data),
                Message::ParameterProblem(x) => Some(&x.data),
                Message::EchoRequest(_) => None,
                Message::EchoReply(x) => return Some(x.clone()),
            }?;

            let (proto, mut payload) = net_utils::skip_ipv6_header(icmp_data.clone())?;
            if proto != libc::IPPROTO_ICMPV6
                || payload.is_empty()
                || TypeId(payload.get_u8()) != TypeId::ECHO_REQUEST
            {
                return None;
            }
            match deserialize_packet!(parse_echo, payload, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE), EchoRequest) {
                Ok(Message::EchoRequest(x)) => Some(x),
                _ => None,
            }
        }

        pub fn serialize(&self) -> Bytes {
            match self {
                Message::EchoRequest(x) => x.serialize(self.type_id().0),
                x => unreachable!("{:?}", x),
            }
        }

        pub fn deserialize(mut packet: Bytes) -> super::DeserializeResult<Self> {
            if packet.is_empty() {
                return Err(super::DeserializeError::InvalidLength("Empty packet".to_string()));
            }

            match TypeId(packet.get_u8()) {
                TypeId::DESTINATION_UNREACHABLE =>
                    deserialize_packet!(parse_destination_unreachable, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + net_utils::MIN_IPV6_HEADER_SIZE), DestinationUnreachable),
                TypeId::PACKET_TOO_BIG =>
                    deserialize_packet!(parse_packet_too_big, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + net_utils::MIN_IPV6_HEADER_SIZE), PacketTooBig),
                TypeId::TIME_EXCEEDED =>
                    deserialize_packet!(parse_time_exceeded, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + net_utils::MIN_IPV6_HEADER_SIZE), TimeExceeded),
                TypeId::PARAMETER_PROBLEM =>
                    deserialize_packet!(parse_parameter_problem, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE + net_utils::MIN_IPV6_HEADER_SIZE), ParameterProblem),
                TypeId::ECHO_REQUEST =>
                    deserialize_packet!(parse_echo, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE), EchoRequest),
                TypeId::ECHO_REPLY =>
                    deserialize_packet!(parse_echo, packet, LengthCheck::LowerBound(super::ICMP_MIN_COMMON_HEADER_SIZE), EchoReply),
                x => Err(super::DeserializeError::MessageType(x.0)),
            }
        }

        fn parse_echo(code: u8, packet: Bytes) -> super::DeserializeResult<super::Echo> {
            super::parse_echo(code, packet)
        }

        fn parse_destination_unreachable(code: u8, mut packet: Bytes) -> super::DeserializeResult<DestinationUnreachable> {
            Ok(DestinationUnreachable {
                code: match DestinationUnreachableCode(code) {
                    DestinationUnreachableCode::NO_ROUTE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::ADMINISTRATIVELY_PROHIBITED => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::BEYOND_SCOPE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::ADDRESS_UNREACHABLE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::PORT_UNREACHABLE => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::SOURCE_FAILED_POLICY => DestinationUnreachableCode(code),
                    DestinationUnreachableCode::REJECT_ROUTE => DestinationUnreachableCode(code),
                    _ => return Err(super::DeserializeError::DestinationUnreachableCode(code)),
                },
                data: packet.split_off(4),
            })
        }

        fn parse_packet_too_big(code: u8, mut packet: Bytes) -> super::DeserializeResult<PacketTooBig> {
            Ok(PacketTooBig {
                code,
                mtu: packet.get_u32(),
                data: packet,
            })
        }

        fn parse_time_exceeded(code: u8, mut packet: Bytes) -> super::DeserializeResult<TimeExceeded> {
            Ok(TimeExceeded {
                code: match TimeExceededCode(code) {
                    TimeExceededCode::HOP_LIMIT => TimeExceededCode(code),
                    TimeExceededCode::FRAGMENT_REASSEMBLY => TimeExceededCode(code),
                    _ => return Err(super::DeserializeError::TimeExceededCode(code)),
                },
                data: packet.split_off(4),
            })
        }

        fn parse_parameter_problem(code: u8, mut packet: Bytes) -> super::DeserializeResult<ParameterProblem> {
            Ok(ParameterProblem {
                code,
                pointer: packet.get_u32(),
                data: packet,
            })
        }
    }
}


fn parse_echo(code: u8, mut packet: Bytes) -> DeserializeResult<Echo> {
    Ok(Echo {
        code,
        identifier: packet.get_u16(),
        sequence_number: packet.get_u16(),
        data: packet,
    })
}
