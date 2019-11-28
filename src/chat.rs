extern crate regex;

use std::os::unix::io::AsRawFd;
use std::io::{BufRead};
use std::io::{Write, Read};
use std::cmp::Ordering;
use rand::Rng;

const MAX_POLLS: usize = 5;
const STD_IN: i32 = 0;
const MAX_DOWNSTREAM: usize = 3;

enum ChatType {
    REGULAR,
    PORT,
    REBALANCE,
    FAILOVER,
    NAME,
}

#[derive(Copy, Clone)]
struct Peer{
    addr: std::option::Option<std::net::SocketAddr>,
    port: u16,
}

impl Peer {
    fn new(a: std::option::Option<std::net::SocketAddr>, p: u16) -> Self {
        Peer{
            addr: a,
            port: p,
        }
    }
}

struct ChatHeader {
    chat_t: ChatType,
    peer: std::option::Option<Peer>,
}

impl ChatHeader {
    fn from_port(portno: u16) -> Self {
        ChatHeader {
            chat_t: ChatType::PORT, 
            peer: Some(Peer::new(None, portno)),
        }
    }

    fn from_msg() -> Self {
        ChatHeader {
            chat_t: ChatType::REGULAR,
            peer: None,
        }
    }

    fn from_rebalance(addr: std::net::SocketAddr, portno: u16) -> Self {
        ChatHeader {
            chat_t: ChatType::REBALANCE,
            peer: Some(Peer::new(Some(addr), portno)),
        }
    }

    fn from_failover(addr: std::net::SocketAddr, portno: u16) -> Self {
        ChatHeader {
            chat_t: ChatType::FAILOVER,
            peer: Some(Peer::new(Some(addr), portno)),
        }
    }
    
    fn from_name() -> Self {
        ChatHeader {
            chat_t: ChatType::NAME,
            peer: None,
        }
    }

    fn from(t: ChatType, p: Peer) -> Self {
        ChatHeader {
            chat_t: t,
            peer: Some(p),
        }
    }
}

fn to_raw(head: &mut ChatHeader, buffer: std::option::Option<&[u8]>) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    unsafe{
        let head_slice: &[u8; std::mem::size_of::<ChatHeader>()] = std::mem::transmute(head);
        buf.extend_from_slice(head_slice);
    }

    match buffer {
        Some(bytes) => {
            buf.extend_from_slice(bytes);
        },
        _ => {},
    };
    
    buf
}

fn parse_raw(buffer: &mut [u8]) -> (std::option::Option<&ChatHeader>, std::option::Option<&[u8]>) {
    let hdr_size: usize = std::mem::size_of::<ChatHeader>();
    if buffer.len() < hdr_size {
        return (None, None);
    }
    else{
        let hdr: &mut ChatHeader;
        unsafe{
            hdr = &mut *(buffer as *mut _ as *mut ChatHeader);
        }
        if buffer.len() > hdr_size {
            return (Some(hdr), Some(&buffer[hdr_size..]));
        }
        else {
            return (Some(hdr), None);
        }
    }
}

struct InfoStream(std::net::TcpStream, std::net::SocketAddr, bool, u16, String);

struct ChatNode {

    //self and children
    host_listener: std::net::TcpListener,
    host_port: u16,
    down_streams: Vec<InfoStream>,
    name: std::option::Option<String>,
    epoll_fd: i32,

    //parent
    up_stream: Option<std::net::TcpStream>,
    up_stream_port: u16,
    up_stream_info: Option<Peer>,

    //failover
    failover: Option<Peer>,

    //successor
    successor: Option<i32>,
}

#[warn(dead_code, unused_assignments)]
impl ChatNode {
    fn new(addr: std::net::SocketAddr, port: u16) -> Self {
        ChatNode {
            host_listener: std::net::TcpListener::bind(addr).unwrap(),
            host_port: port,
            down_streams: Vec::new(),
            name: None,
            epoll_fd: -1,
            up_stream: None,
            up_stream_port: 0,
            up_stream_info: None,
            failover: None,
            successor: None,
        }
    }

    fn set_peer(&mut self, fd: i32, portno: u16) {
        for mut stream in &mut self.down_streams {
            if stream.0.as_raw_fd() == fd {
                stream.2 = true;
                stream.3 = portno;
            }
        }
    }

    fn _is_peer(&self, fd: i32) -> bool {
        for stream in &self.down_streams {
            if stream.0.as_raw_fd() == fd && stream.2{
                return true;
            }
        }
        false
    }
    
    fn get_peer_port(&self, fd: i32) -> u16 {
        for stream in &self.down_streams {
            if stream.0.as_raw_fd() == fd {
                return stream.3;
            }
        }
        println!("couldn't find port");
        return 0;
    }

    fn set_name(&mut self, name: &str) {
        if self.name == None {
            self.name = Some(String::from(name));
            println!("Welcome {}!", name);
            self.send_name();
        }
        else{
            println!("Setting Name Again Not Allowed!");
        }
    }

    fn set_stream_name(&mut self, fd: i32, name: &String) {
        for stream in self.down_streams.iter_mut() {
            if stream.0.as_raw_fd() == fd {
                stream.4 = name.clone();
            }
        }
    }

    fn broadcast(&mut self, buf: &mut Vec<u8>, fd: i32, only_peer: bool) {
        for i in 0..self.down_streams.len() {
            if only_peer && !self.down_streams[i].2 { continue; }
            else if self.down_streams[i].0.as_raw_fd() != fd {
                match self.down_streams[i].0.write(buf){
                    Ok(_) => {},
                    Err(error) => {
                        println!("In broadcast(), Write Failure: {:?}:", error);
                    },
                };
            }
        }

        if self.up_stream.is_some() {
            if fd != self.up_stream.as_ref().unwrap().as_raw_fd() {
                match self.up_stream.as_ref().unwrap().write(buf) {
                    Ok(_) => {},
                    Err(error) => {
                        println!("In broadcast(), Write Failure: {:?}:", error);
                    },
                };
            }
        }
    }
    
    fn handle_recv(&mut self, buf: &mut Vec<u8>, epd: i32, fd: i32) {
        match buf.len() {
            0 => { self.close_client(epd, fd); },
            _ => {
                match parse_raw(buf) {
                    (None, None) => { println!("error parsing"); },
                    (Some(hdr), payload) => {
                        match hdr.chat_t {
                            ChatType::PORT => {
                                let limit: usize = MAX_DOWNSTREAM;
                                match self.down_streams.len().cmp(&limit) {
                                    Ordering::Equal | Ordering::Greater => {
                                        self.send_rebalance(fd);
                                    },
                                     _ => {
                                        let portno: u16 = hdr.peer.as_ref().unwrap().port;
                                        self.set_peer(fd, portno);
                                        self.send_failover();
                                    },
                                };
                                
                            },
                            ChatType::REGULAR => {
                                if payload.is_some() {
                                    println!("{}", String::from_utf8(payload.unwrap().to_vec()).unwrap());
                                    self.broadcast(buf, fd, false);
                                }
                            },
                            ChatType::FAILOVER => {
                                self.failover = Some(hdr.peer.unwrap());
                            },
                            ChatType::REBALANCE => {
                                self.reconnect(&hdr.peer.unwrap());
                            },
                            ChatType::NAME => {
                                if payload.is_some() {
                                    let name: String = String::from_utf8(payload.unwrap().to_vec()).unwrap();
                                    self.set_stream_name(fd, &name);
                                    let msg = String::from(format!("{} has Joined the Chat Room", name));
                                    println!("{}", msg);
                                    self.send_msg(fd, msg.as_ref());
                                }
                            }
                        };
                    },
                    _ => { println!("invalid chat format"); },
                };
            },
        };
    }

    fn handle_send(&mut self, msg: &str, fd: i32) {
        let re = regex::Regex::new(r"^/(?P<cmd>[^\s\t\r\n]+)(?x)(?P<arg>[^\r\n]+)").unwrap();
        let cap = re.captures(msg);
        match cap {
            None => {
                match self.name {
                    Some(_) => {
                        let entire_msg: String = String::from(self.name.as_ref().unwrap()) + "> " + msg.as_ref();
                        self.send_msg(fd, entire_msg.as_ref());
                    },
                    None => println!("Please Set Your Name First!\n/name <Name>"),
                };
            },
            Some(c) => {
                match c.name("cmd").unwrap().as_str() {
                    "name" => {
                        let name: &str = c.name("arg").unwrap().as_str().trim();
                        self.set_name(name.as_ref());
                    }
                    "exit" => {
                        std::process::exit(0);
                    }
                    _ => {/*  Ignore cmd */ },
                }
            },
        };
    }
    
    fn is_stream(&self, fd: i32) -> bool {
        for client in &self.down_streams {
            if client.0.as_raw_fd() == fd {
                return true;
            }
        }

        if self.up_stream.is_some() {
            if fd == self.up_stream.as_ref().unwrap().as_raw_fd() {
                return true;
            }
        }

        return false;
    }

    fn _is_up_stream(&self, fd: i32) -> bool {
        if self.up_stream.is_some() {
            if fd == self.up_stream.as_ref().unwrap().as_raw_fd() {
                return true;
            }
        }

        return false;
    }

    fn get_stream(&mut self, fd: i32) -> std::option::Option<&mut std::net::TcpStream> {
        for stream in &mut self.down_streams {
             if stream.0.as_raw_fd() == fd {
                return Some(&mut stream.0);
            }
        }

         if self.up_stream.is_some() {
            if fd == self.up_stream.as_ref().unwrap().as_raw_fd() {
                return self.up_stream.as_mut();
            }
        }

        return None;
    }

    fn get_stream_idx(&mut self, fd: i32) -> std::option::Option<usize> {
        for i in 0..self.down_streams.len() {
            if self.down_streams[i].0.as_raw_fd() == fd{
                return Some(i);
            }
        }
        None
    }

    fn _get_stream_info(&mut self, fd: i32) -> std::option::Option<&InfoStream> {
        for stream in &self.down_streams {
            if stream.0.as_raw_fd() == fd {
                return Some(&stream);
            }
        }

        None
    }

    fn get_name(&self, fd: i32) -> String {
        for stream in &self.down_streams {
            if stream.0.as_raw_fd() == fd {
                return String::from(&stream.4);
            }
        }

         if self.up_stream.is_some() {
            if fd == self.up_stream.as_ref().unwrap().as_raw_fd() {
                return String::from("Upstream");
            }
        }

        String::from("UnKnown")
    }

    fn reconnect(&mut self, peer: &Peer) {
        if self.up_stream.is_some() {
            println!("Closing connection with UpStream");
            match self.up_stream.as_ref().unwrap().shutdown(std::net::Shutdown::Both){
                Ok(_) => {

                    let fd: i32 = self.up_stream.as_ref().unwrap().as_raw_fd();
                    match epoll::ctl(   self.epoll_fd, epoll::ControlOptions::EPOLL_CTL_DEL, fd, 
                            epoll::Event::new(epoll::Events::EPOLLERR, fd as u64)){
                        Ok(_) => {},
                        Err(error) => {
                            println!("Epoll Ctl Failure: {:?}", error);
                            std::process::exit(-1);
                        },
                    };
                    
                    match self.successor {
                        Some(successor_fd) => {
                            if successor_fd == fd {
                                self.successor = None;
                            }
                        },
                        None => {},
                    };
                    
                    self.up_stream = None;
                    self.up_stream_info = None;
                    self.up_stream_port = 0;
                },
                Err(error) =>{
                    println!("Socket Shutdown Failure: {:?}", error);
                    std::process::exit(-1);
                },
            };
        }

        let addr: std::net::SocketAddr = std::net::SocketAddr::new(peer.addr.unwrap().ip(), peer.port);
        match std::net::TcpStream::connect(addr) {
            Ok(connection) => {
                println!("Connected to {}", addr);
                self.up_stream = Some(connection).take();
                self.up_stream_port = peer.port;
                self.up_stream_info = Some(*peer);

                match epoll::ctl(   self.epoll_fd, epoll::ControlOptions::EPOLL_CTL_ADD, self.up_stream.as_ref().unwrap().as_raw_fd(), 
                                    epoll::Event::new(epoll::Events::EPOLLIN, self.up_stream.as_ref().unwrap().as_raw_fd() as u64)){
                    Ok(_) => {},
                    Err(error) => {
                        println!("Epoll Ctl Failure: {:?}", error);
                        std::process::exit(-1);
                    },
                };

                self.up_stream.as_ref().unwrap().set_nonblocking(true).expect("Error in SetNonBlocking(true)");
                self.up_stream.as_ref().unwrap().set_nodelay(true).expect("set_nodelay failure");
                self.send_peer();
                self.send_name();
            },
            Err(_) => {
                println!("Couldn't connect to {:?}", addr);  
            },
        };
    }

    fn close_client(&mut self, efd: i32, fd: i32) {
        println!("{} Closed Connection", self.get_name(fd));
        let mut up_stream_fd: i32 = -1;
        if self.up_stream.is_some() {
            up_stream_fd = self.up_stream.as_ref().unwrap().as_raw_fd();
            if up_stream_fd == fd {
                match self.failover {
                    Some(peer) => {
                        self.reconnect(&peer);
                        return;
                    },
                    None => {},
                };   
            }
        }
        
        match self.get_stream(fd).unwrap().shutdown(std::net::Shutdown::Both){
            Ok(_) => {},
            Err(error) =>{
                println!("Socket Shutdown Failure: {:?}", error);
                std::process::exit(-1);
            },
        };
        match epoll::ctl(   efd, epoll::ControlOptions::EPOLL_CTL_DEL, fd, 
                            epoll::Event::new(epoll::Events::EPOLLERR, fd as u64)){
            Ok(_) => {},
            Err(error) => {
                println!("Epoll Ctl Failure: {:?}", error);
                std::process::exit(-1);
            },
        };

        match self.get_stream_idx(fd) {
            Some(index) => {
                self.down_streams.remove(index);
            },
            None => {},
        };

        if up_stream_fd == fd {
            self.up_stream = None;
            self.up_stream_info = None;
            self.up_stream_port = 0;
        }

        match self.successor {
            Some(successor_fd) => {
                if successor_fd == fd {
                    self.successor = None;
                }
            },
            None => {},
        };
    }

    fn send_msg(&mut self, fd: i32, msg: &[u8]) {
        let mut buf = to_raw(&mut ChatHeader::from_msg(), Some(msg));
        self.broadcast(&mut buf, fd, false);
    }

    fn send_name(&mut self) {
        if self.name.is_some() { 
            let mut buf: Vec<u8> = to_raw(&mut ChatHeader::from_name(), Some(self.name.as_ref().unwrap().as_ref()));
            self.broadcast(&mut buf, -1, true);
        }
    }

    fn send_peer(&mut self) {
        let send_port: u16 = self.host_port;
        let buf: Vec<u8> = to_raw(&mut ChatHeader::from_port(send_port), None);
        self.up_stream.as_ref().unwrap().write(&buf).expect("Write Failure send_peer()");
    }

    fn assign_successor(&mut self) -> std::option::Option<(Vec<u8>, i32)> {
        let fd: i32;
        if self.up_stream.as_ref().is_some() {
            fd = self.up_stream.as_ref().unwrap().as_raw_fd();
            self.successor = Some(fd);
            return Some((to_raw(&mut ChatHeader::from(ChatType::FAILOVER, self.up_stream_info.unwrap()), None), fd));
        }
        match &self.successor {
            Some(fd_stream) => {
                fd = *fd_stream;
                return Some((to_raw(&mut ChatHeader::from_failover(self.get_stream(fd).unwrap().peer_addr().unwrap(), self.get_peer_port(fd)), None), fd));
            },
            None  => {
                for stream in &mut self.down_streams {
                    if stream.2 {
                        fd = stream.0.as_raw_fd();
                        self.successor = Some(fd);
                        let fail_ip: std::net::IpAddr = stream.0.peer_addr().unwrap().ip();
                        let fail_port: u16 = self.get_peer_port(fd);
                        return Some((to_raw(&mut ChatHeader::from_failover(std::net::SocketAddr::new(fail_ip, fail_port), fail_port), None), fd));
                    }
                }
            },
        };
        return None;
    }

    fn send_failover(&mut self) {    
        match self.assign_successor() {
            None => {},
            Some((mut buf, fd)) => {
                self.broadcast(&mut buf, fd, true);
            },
        }
    }

    fn pick_down_stream(&mut self, fd: i32) -> (std::net::SocketAddr, u16) {
        let len: usize = self.down_streams.len();
        let mut index: usize = rand::thread_rng().gen_range(0, len);

        if self.down_streams.get(index).unwrap().0.as_raw_fd() == fd {
            index = (index + 1) % len;
        } 

        let addr: std::net::SocketAddr = self.down_streams.get(index).unwrap().1;
        let portno: u16 = self.down_streams.get(index).unwrap().3;

        (addr, portno)
    }

    fn send_rebalance(&mut self, fd: i32) {
        let (addr, portno) = self.pick_down_stream(fd);
        match self.get_stream(fd).unwrap().write(to_raw(&mut ChatHeader::from_rebalance(addr, portno), None).as_ref()){
            Ok(_) => {},
            Err(error) => {
                println!("In broadcast(), Write Failure: {:?}:", error);
            },
        };    
    }

    fn start_routine(&mut self) {
        let host_fd: i32 = self.host_listener.as_raw_fd();
        
        //create epoll
        let fd_poller: i32 = match epoll::create(false) {
            Ok(fd) => fd,
            Err(error) => {
                println!("Epoll Create Failure: {:?}", error);
                std::process::exit(-1);
            },
        };
        self.epoll_fd = fd_poller;

        //add tcp listener to read set
        match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, host_fd, 
                            epoll::Event::new(epoll::Events::EPOLLIN, host_fd as u64)){
            Ok(_) => {},
            Err(error) => {
                println!("Epoll Ctl Failure: {:?}", error);
                std::process::exit(-1);
            },
        }
        
        //STD_IN
        match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, STD_IN, 
                            epoll::Event::new(epoll::Events::EPOLLIN, STD_IN as u64)){
            Ok(_) => {},
            Err(error) => {
                println!("Epoll Ctl Failure: {:?}", error);
                std::process::exit(-1);
            },
        };

        //add upstream to read set
        match &self.up_stream {
            Some(stream) => {
                match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, stream.as_raw_fd(), 
                                    epoll::Event::new(epoll::Events::EPOLLIN, stream.as_raw_fd() as u64)){
                    Ok(_) => {},
                    Err(error) => {
                        println!("Epoll Ctl Failure: {:?}", error);
                        std::process::exit(-1);
                    },
                };

                self.up_stream_info = Some(Peer::new(Some(self.up_stream.as_ref().unwrap().peer_addr().unwrap()), self.up_stream_port));
                self.up_stream.as_ref().unwrap().set_nonblocking(true).expect("Error in SetNonBlocking(true)");
                self.up_stream.as_ref().unwrap().set_nodelay(true).expect("set_nodelay failure");
                self.send_peer();
            },
            None => {},
        };

        loop{
            let mut all_events: [epoll::Event; MAX_POLLS] = [epoll::Event::new(epoll::Events::EPOLLIN, 0); MAX_POLLS];
            let num_events = match epoll::wait(fd_poller, -2, &mut all_events){
                Ok(num) => num,
                Err(error) => {
                    println!("Epoll Wait Failure: {:?}", error);
                    continue
                }
            };

            for i in 0..num_events {
                let ready_fd: i32 = all_events[i].data as i32;
                let _event = match epoll::Events::from_bits(all_events[i].events){
                    Some(ev) => ev,
                    _ => {
                        println!("Error in from_bits()...");
                        continue
                    },
                };

                if ready_fd == host_fd {
                    //got incoming connection

                    match self.host_listener.accept() {
                        Ok((down_stream, down_stream_addr)) => {

                            let client_fd = down_stream.as_raw_fd();
                            down_stream.set_nonblocking(true).expect("Error in SetNonBlocking(true)");
                            down_stream.set_nodelay(true).expect("set_nodelay failure");

                            println!("Got Connection From {}", down_stream_addr);
                            self.down_streams.push(InfoStream(down_stream, down_stream_addr, false, 0, format!("Client {}", client_fd)));

                            match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, client_fd, 
                                                epoll::Event::new(epoll::Events::EPOLLIN, client_fd as u64)){
                                Ok(_) => {},
                                Err(error) => {
                                    println!("Epoll Ctl Failure: {:?}", error);
                                    std::process::exit(-1);
                                },
                            };
                        },
                        Err(error) => {
                            println!("Couldn't Accept New Connection: {:?}", error);
                            continue;
                        },
                    }
                }
                else if self.is_stream(ready_fd) {
                    //got msg from connections
                    let mut vecbuf: Vec<u8> = Vec::new();
                    loop {
                        let mut peakbuf = [0u8; 1400]; // <--------------------- buffered fixed, fix later by looping TcpStream.peek() and stop at 0
                        let mut _peak_count: usize = 0;
                        match self.get_stream(ready_fd).unwrap().peek(&mut peakbuf) {
                            Ok(count) => { _peak_count = count; },
                            Err(_) => { break; },
                        };

                        if _peak_count == 0 {
                            break;
                        }
                        
                        let mut buf = vec![0u8; _peak_count];
                        match self.get_stream(ready_fd).unwrap().read_exact(&mut buf) {
                            Ok(()) => {
                                // println!("From {}, Got {} Bytes", self.get_name(ready_fd), peak_count);
                                // println!("recved {:?}", buf);
                                vecbuf.extend_from_slice(&buf);
                            },
                            Err(_) => {
                                println!("BufRead Err");
                                self.close_client(fd_poller, ready_fd);
                                break;
                            }
                        };
                    }

                    self.handle_recv(&mut vecbuf, fd_poller, ready_fd);
                }
                else if ready_fd == 0 {
                    //got msg from stdin
                    let mut msg: String = String::new();
                    match std::io::stdin().lock().read_line(&mut msg){
                        Ok(_count) => {
                            self.handle_send(msg.trim(), ready_fd);                            
                        },
                        Err(_) => {},
                    };
                }
            }
        }
    }
}


fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() < 2 {
        println!("Usage: ./chat <host-port>");
        println!("Usage: ./chat <host-port> <connect-ip> <connect-port>");
        std::process::exit(0);
    }
    

    let s: String = String::from("127.0.0.1:") + &argv[1];
    println!("Hosting Chat At {:?}", s);

    
    let port: u16 = argv[1].trim().parse().unwrap();
    let mut node: ChatNode = ChatNode::new(std::net::SocketAddr::from(([127, 0, 0, 1], port)), port);
    if argv.len() >= 4 {
        let upstream: String = String::from(&argv[2]) + ":" + &argv[3];
        match std::net::TcpStream::connect(&upstream) {
            Ok(connection) => {
                println!("Connected to {:?}", upstream);
                node.up_stream = Some(connection).take();
                node.up_stream_port = argv[3].trim().parse().unwrap();
            },
            Err(_) => {
                println!("Couldn't connect to {:?}", upstream);  
            },
        };
    }
    node.start_routine();
}
