use bytes::{Buf, BufMut, Bytes, BytesMut};
use ring::rand::SecureRandom;
use crate::{downstream, forwarder, http_datagram_codec, icmp_utils, net_utils};


/// Outgoing ICMP packet format (sent from us to client)
///
/// +----------+----------------+--------+--------+-----------------+
/// |  ID      | Source address | Type   | Code   | Sequence number |
/// | 2 bytes  |  16 bytes      | 1 byte | 1 byte | 2 bytes         |
/// +----------+----------------+--------+--------+-----------------+
///
/// Incoming ICMP packet format (sent from client to us)
///
/// +----------+---------------------+-----------------+---------------+-----------+
/// |  ID      | Destination address | Sequence number | TTL/Hop limit | Data size |
/// | 2 bytes  |  16 bytes           | 2 bytes         | 1 byte        | 2 bytes   |
/// +----------+---------------------+-----------------+---------------+-----------+


const ICMPPKT_ID_SIZE: usize = 2;
const ICMPPKT_ADDR_SIZE: usize = 16;
const ICMPPKT_SEQNO_SIZE: usize = 2;
const ICMPPKT_TTL_SIZE: usize = 1;
const ICMPPKT_DATA_SIZE: usize = 2;
const ICMPPKT_TYPE_SIZE: usize = 1;
const ICMPPKT_CODE_SIZE: usize = 1;
const ICMPPKT_REQ_SIZE: usize = ICMPPKT_ID_SIZE + ICMPPKT_ADDR_SIZE
    + ICMPPKT_SEQNO_SIZE + ICMPPKT_TTL_SIZE + ICMPPKT_DATA_SIZE;
const ICMPPKT_REPLY_SIZE: usize = ICMPPKT_ID_SIZE + ICMPPKT_ADDR_SIZE
    + ICMPPKT_TYPE_SIZE + ICMPPKT_CODE_SIZE + ICMPPKT_SEQNO_SIZE;


pub(crate) struct Decoder {
    buffer: BytesMut,
}

#[derive(Default)]
pub(crate) struct Encoder {
}


impl http_datagram_codec::Decoder for Decoder {
    type Datagram = downstream::IcmpDatagram;

    fn decode_chunk(&mut self, data: Bytes) -> http_datagram_codec::DecodeResult<Self::Datagram> {
        match self.on_message_chunk(data) {
            None => http_datagram_codec::DecodeResult::WantMore,
            Some((mut raw, tail)) => {
                let identifier = raw.get_u16();
                let destination = net_utils::get_fixed_size_ip(&mut raw);
                let sequence_number = raw.get_u16();

                let ttl = raw.get_u8();

                let data_size = raw.get_u16() as usize;
                let mut data = vec![0_u8; data_size];
                ring::rand::SystemRandom::new().fill(&mut data).unwrap();

                let echo = icmp_utils::Echo {
                    code: 0,
                    identifier,
                    sequence_number,
                    data: Bytes::from(data),
                };

                http_datagram_codec::DecodeResult::Complete(
                    downstream::IcmpDatagram {
                        meta: downstream::IcmpDatagramMeta {
                            peer: destination,
                        },
                        message: if destination.is_ipv4() {
                            icmp_utils::Message::V4(icmp_utils::v4::Message::Echo(echo))
                        } else {
                            icmp_utils::Message::V6(icmp_utils::v6::Message::EchoRequest(echo))
                        },
                        ttl,
                    },
                    tail,
                )
            }
        }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::with_capacity(ICMPPKT_REQ_SIZE),
        }
    }

    fn on_message_chunk(&mut self, mut chunk: Bytes) -> Option<(Bytes, Bytes)> {
        if !self.buffer.is_empty() || self.buffer.len() + chunk.len() < ICMPPKT_REQ_SIZE {
            self.buffer.extend(
                chunk.split_to(std::cmp::min(chunk.len(), ICMPPKT_REQ_SIZE - self.buffer.len()))
            );
            assert!(self.buffer.len() <= ICMPPKT_REQ_SIZE);
            if self.buffer.len() < ICMPPKT_REQ_SIZE {
                assert!(chunk.is_empty());
                None
            } else {
                Some((
                    std::mem::replace(
                        &mut self.buffer, BytesMut::with_capacity(ICMPPKT_REQ_SIZE)
                    ).freeze(),
                    chunk,
                ))
            }
        } else {
            Some((chunk.split_to(ICMPPKT_REQ_SIZE), chunk))
        }
    }
}

impl http_datagram_codec::Encoder for Encoder {
    type Datagram = forwarder::IcmpDatagram;

    fn encode_packet(&self, datagram: &forwarder::IcmpDatagram) -> Option<Bytes> {
        let echo = datagram.message.responded_echo_request()?;

        let mut encoded = BytesMut::with_capacity(ICMPPKT_REPLY_SIZE);

        encoded.put_u16(echo.identifier);
        net_utils::put_fixed_size_ip(&mut encoded, &datagram.meta.peer);
        encoded.put_u8(datagram.message.type_id());
        encoded.put_u8(datagram.message.code());
        encoded.put_u16(echo.sequence_number);

        Some(encoded.freeze())
    }
}
