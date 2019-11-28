pub enum ChatType {
    REGULAR,
    PORT,
    REBALANCE,
    FAILOVER,
    NAME,
}

#[derive(Copy, Clone)]
pub struct Peer{
    pub addr: std::option::Option<std::net::SocketAddr>,
    pub port: u16,
}

impl Peer {
     pub fn new(a: std::option::Option<std::net::SocketAddr>, p: u16) -> Self {
        Peer{
            addr: a,
            port: p,
        }
    }
}

pub struct ChatHeader {
    pub chat_t: ChatType,
    pub peer: std::option::Option<Peer>,
}

impl ChatHeader {
    pub fn from_port(portno: u16) -> Self {
        ChatHeader {
            chat_t: ChatType::PORT, 
            peer: Some(Peer::new(None, portno)),
        }
    }

    pub fn from_msg() -> Self {
        ChatHeader {
            chat_t: ChatType::REGULAR,
            peer: None,
        }
    }

    pub fn from_rebalance(addr: std::net::SocketAddr, portno: u16) -> Self {
        ChatHeader {
            chat_t: ChatType::REBALANCE,
            peer: Some(Peer::new(Some(addr), portno)),
        }
    }

    pub fn from_failover(addr: std::net::SocketAddr, portno: u16) -> Self {
        ChatHeader {
            chat_t: ChatType::FAILOVER,
            peer: Some(Peer::new(Some(addr), portno)),
        }
    }
    
    pub fn from_name() -> Self {
        ChatHeader {
            chat_t: ChatType::NAME,
            peer: None,
        }
    }

    pub fn from(t: ChatType, p: Peer) -> Self {
        ChatHeader {
            chat_t: t,
            peer: Some(p),
        }
    }
}

pub fn to_raw(head: &mut ChatHeader, buffer: std::option::Option<&[u8]>) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    unsafe{
        let head_slice: &[u8; std::mem::size_of::<ChatHeader>()] = std::mem::transmute(head);
        buf.extend_from_slice(head_slice);
    }

    if let Some(bytes) = buffer {
        buf.extend_from_slice(bytes);
    };
    
    buf
}

pub fn parse_raw(buffer: &mut [u8]) -> (std::option::Option<&ChatHeader>, std::option::Option<&[u8]>) {
    let hdr_size: usize = std::mem::size_of::<ChatHeader>();
    if buffer.len() < hdr_size {
        (None, None)
    }
    else{
        let hdr: &mut ChatHeader;
        unsafe{
            hdr = &mut *(buffer as *mut _ as *mut ChatHeader);
        }
        if buffer.len() > hdr_size {
            (Some(hdr), Some(&buffer[hdr_size..]))
        }
        else {
            (Some(hdr), None)
        }
    }
}

pub struct InfoStream(pub std::net::TcpStream, pub std::net::SocketAddr, pub bool, pub u16, pub String);
