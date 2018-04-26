use std::io::Read;
use std::io::Write;
use std::iter;

pub type LightweightConnectionId = u32;

pub const LIGHT_ID_MIN : u32 = 1024;

pub struct EndPoint(Vec<u8>);
impl AsRef<[u8]> for EndPoint {
    fn as_ref(&self) -> &[u8] { &self.0 }
}

impl EndPoint {
    pub fn unaddressable() -> Self {
        EndPoint(vec![])
    }
}

pub struct Connection<W: Sized> {
    stream: W,
    drg: u64,
}

impl<W: Sized+Write+Read> Connection<W> {
    pub fn handshake(drg_seed: u64, stream: W) -> Result<Self,&'static str> {
        let mut conn = Connection { stream: stream, drg: drg_seed };
        let mut buf = vec![];
        protocol::handshake(&mut buf);
        conn.emit("handshake", &buf);
        match conn.recv_u32() {
            Ok(0xffffffff) => Err("unsupported version"),
            Ok(0x00000001) => Err("request invalid"),
            Ok(0x00000002) => Err("request crossed"),
            Ok(0x00000000) => { println!("HANDSHAKE OK"); Ok(conn) },
            Ok(_)          => Err("unknown code"),
            Err(_)         => Err("random io error"),
        }
    }

    pub fn get_nonce(&mut self) -> protocol::Nonce {
        let v = self.drg;
        self.drg += 1;
        v
    }

    pub fn create_light(&mut self, cid: LightweightConnectionId) {
        assert!(cid >= LIGHT_ID_MIN);
        let mut buf = vec![];
        protocol::create_conn(cid, &mut buf);
        self.emit("create-connection", &buf);
    }

    pub fn close_light(&mut self, cid: LightweightConnectionId) {
        assert!(cid >= LIGHT_ID_MIN);
        let mut buf = vec![];
        protocol::delete_conn(cid, &mut buf);
        self.emit("close-connection", &buf);
    }

    pub fn send_endpoint(&mut self, endpoint: &EndPoint) {
        let mut buf = vec![];
        protocol::append_with_length(endpoint.as_ref(), &mut buf);
        self.emit("send endpoint", &buf);
    }

    pub fn light_send_data(&mut self, lwc: LightweightConnectionId, dat: &[u8]) {
        let mut buf = vec![];
        protocol::append_lightweight_data(lwc, dat.len() as u32, &mut buf);
        self.emit("send lightcon data header", &buf);
        self.emit("send lightcon data",  &dat);
    }

    // emit utility
    fn emit(&mut self, step: &str, dat: &[u8]) {
        println!("sending {} {:?}", step, dat);
        self.stream.write_all(dat).unwrap();
    }

    // TODO some kind of error
    fn recv_u32(&mut self) -> Result<u32, &str> {
        let mut buf = [0u8; 4];
        match self.stream.read_exact(&mut buf) {
            Ok(_) => {
                let v = ((buf[0] as u32) << 24) |
                        ((buf[1] as u32) << 16) |
                        ((buf[2] as u32) << 8) |
                        (buf[3] as u32);
                Ok(v)
            },
            Err(_) => Err("recvword32: io error"),
        }
    }

    pub fn recv(&mut self) -> Result<protocol::Command, &str>  {
        let hdr = self.recv_u32().unwrap();
        if hdr < LIGHT_ID_MIN {
            match protocol::control_header_from_u32(hdr) {
                Ok(c)  => {
                    let r = self.recv_u32().unwrap();
                    Ok(protocol::Command::Control(c, r))
                },
                Err(_) => Err("recv command failed")
            }

        } else {
            let len = self.recv_u32().unwrap();
            Ok(protocol::Command::Data(hdr, len))
        }
    }

    pub fn recv_cmd(&mut self) -> Result<(), &str> {
        let lwc = self.recv_u32().unwrap();
        assert!(lwc < 0x400);
        let len = self.recv_u32().unwrap();
        println!("received lwc {} and len {}", lwc, len);
        Ok(())
    }

    pub fn recv_data(&mut self) -> Result<(LightweightConnectionId, Vec<u8>), &str> {
        let lwc = self.recv_u32().unwrap();
        println!("received lwc {}", lwc);
        let len = self.recv_u32().unwrap();
        let mut buf : Vec<u8> = iter::repeat(0).take(len as usize).collect();
        self.stream.read_exact(&mut buf[..]).unwrap();
        Ok((lwc,buf))
    }

    pub fn recv_len(&mut self, len: u32) -> Result<Vec<u8>, &str> {
        let mut buf : Vec<u8> = iter::repeat(0).take(len as usize).collect();
        self.stream.read_exact(&mut buf[..]).unwrap();
        Ok(buf)
    }
}

pub mod protocol {
    const PROTOCOL_VERSION : u32 = 0x00000000;

    #[derive(Debug)]
    pub enum ControlHeader {
        CreatedNewConnection,
        CloseConnection,
        CloseSocket,
        CloseEndPoint,
        ProbeSocket,
        ProbeSocketAck,
    }

    #[derive(Debug)]
    pub enum Command {
        Control(ControlHeader, super::LightweightConnectionId),
        Data(super::LightweightConnectionId, u32),
    }

    pub type Nonce = u64;
    pub enum NodeControlHeader {
        Syn,
        Ack,
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
    pub struct NodeId([u8;9]);

    impl AsRef<[u8]> for NodeId {
        fn as_ref(&self) -> &[u8] { &self.0 }
    }

    // use make_syn_nodeid or make_ack_nodeid
    fn make_nodeid(header: NodeControlHeader, nonce: Nonce) -> NodeId {
        let mut v = [0;9];
        v[0] = match header {
                    NodeControlHeader::Syn => 0x53, // 'S'
                    NodeControlHeader::Ack => 0x41  // 'A'
        };
        v[1] = (nonce >> 56) as u8;
        v[2] = (nonce >> 48) as u8;
        v[3] = (nonce >> 40) as u8;
        v[4] = (nonce >> 32) as u8;
        v[5] = (nonce >> 24) as u8;
        v[6] = (nonce >> 16) as u8;
        v[7] = (nonce >> 8) as u8;
        v[8] = nonce as u8;
        NodeId(v)
    }

    impl NodeId {
        pub fn make_syn(nonce: Nonce) -> Self {
            make_nodeid(NodeControlHeader::Syn, nonce)
        }

        pub fn make_ack(nonce: Nonce) -> Self {
            make_nodeid(NodeControlHeader::Ack, nonce)
        }
        pub fn get_control_header(&self) -> NodeControlHeader {
            if self.0[0] == 0x53 { NodeControlHeader::Syn } else { NodeControlHeader::Ack }
        }
    }

    pub fn handshake(buf: &mut Vec<u8>) {
        let handshake_length = 0;
        append_u32(PROTOCOL_VERSION, buf);
        append_u32(handshake_length, buf);
        append_u32(0, buf); // ourEndPointId
        append_u32(0, buf); // send length 0
        //append_u32(0, buf); // ignored but should be handshake length
        //append_u32(0, buf); // ignored but should be handshake length
    }

    /// encode an int32
    /*
    fn append_i32(v: i32, buf: &mut Vec<u8>) {
        buf.push((v >> 24) as u8);
        buf.push((v >> 16) as u8);
        buf.push((v >> 8) as u8);
        buf.push(v as u8);
    }
    */
    
    fn append_u32(v: u32, buf: &mut Vec<u8>) {
        buf.push((v >> 24) as u8);
        buf.push((v >> 16) as u8);
        buf.push((v >> 8) as u8);
        buf.push(v as u8);
    }

    pub fn append_lightweight_data(cid: super::LightweightConnectionId, len: u32, buf: &mut Vec<u8>) {
        assert!(cid >= 1024);
        append_u32(cid, buf);
        append_u32(len, buf);
    }

    pub fn create_conn(cid: super::LightweightConnectionId, buf: &mut Vec<u8>) {
        append_u32(control_header_to_u32(ControlHeader::CreatedNewConnection), buf);
        append_u32(cid, buf);
    }

    pub fn delete_conn(cid: super::LightweightConnectionId, buf: &mut Vec<u8>) {
        append_u32(control_header_to_u32(ControlHeader::CloseConnection), buf);
        append_u32(cid, buf);
    }


    pub fn append_with_length(dat: &[u8], buf: &mut Vec<u8>) {
        append_u32(dat.len() as u32, buf);
        buf.extend_from_slice(dat);
    }

    pub fn control_header_to_u32(h: ControlHeader) -> u32 {
        match h {
            ControlHeader::CreatedNewConnection => 0,
            ControlHeader::CloseConnection      => 1,
            ControlHeader::CloseSocket          => 2,
            ControlHeader::CloseEndPoint        => 3,
            ControlHeader::ProbeSocket          => 4,
            ControlHeader::ProbeSocketAck       => 5,
        }
    }

    pub fn control_header_from_u32(v: u32) -> Result<ControlHeader, ()> {
        match v {
            0 => Ok(ControlHeader::CreatedNewConnection),
            1 => Ok(ControlHeader::CloseConnection),
            2 => Ok(ControlHeader::CloseSocket),
            3 => Ok(ControlHeader::CloseEndPoint),
            4 => Ok(ControlHeader::ProbeSocket),
            5 => Ok(ControlHeader::ProbeSocketAck),
            _ => Err(()),
        }
    }
}
