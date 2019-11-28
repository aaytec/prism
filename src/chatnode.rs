extern crate regex;

use std::os::unix::io::AsRawFd;
use std::io::{BufRead};
use std::io::{Write, Read};
use std::cmp::Ordering;

const MAX_POLLS: usize = 5;
const STD_IN: i32 = 0;
const MAX_DOWNSTREAM: usize = 3;


mod chatlib;

pub struct ChatNode {

    //self and children
    host_listener: std::net::TcpListener,
    host_port: u16,
    down_streams: Vec<chatlib::InfoStream>,
    name: std::option::Option<String>,
    epoll_fd: i32,

    //parent
    pub up_stream: Option<std::net::TcpStream>,
    pub up_stream_port: u16,
    up_stream_info: Option<chatlib::Peer>,

    //failover
    failover: Option<chatlib::Peer>,

    //successor
    successor: Option<i32>,
}

#[warn(dead_code, unused_assignments)]
impl ChatNode {
    pub fn new(addr: std::net::SocketAddr, port: u16) -> Self {
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
        0
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

    fn set_stream_name(&mut self, fd: i32, name: &str) {
        for stream in self.down_streams.iter_mut() {
            if stream.0.as_raw_fd() == fd {
                stream.4 = name.to_string();
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

        if self.up_stream.is_some() && fd != self.up_stream.as_ref().unwrap().as_raw_fd(){
                match self.up_stream.as_ref().unwrap().write(buf) {
                    Ok(_) => {},
                    Err(error) => {
                        println!("In broadcast(), Write Failure: {:?}:", error);
                    },
                };
        }
    }
    
    fn handle_recv(&mut self, buf: &mut Vec<u8>, fd: i32) {
        match buf.len() {
            0 => { self.close_client(fd); },
            _ => {
                match chatlib::parse_raw(buf) {
                    (None, None) => { println!("error parsing"); },
                    (Some(hdr), payload) => {
                        match hdr.chat_t {
                            chatlib::ChatType::PORT => {
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
                            chatlib::ChatType::REGULAR => {
                                if let Some(load) = payload {
                                    println!("{}", String::from_utf8(load.to_vec()).unwrap());
                                    self.broadcast(buf, fd, false);
                                }
                            },
                            chatlib::ChatType::FAILOVER => {
                                self.failover = Some(hdr.peer.unwrap());
                            },
                            chatlib::ChatType::REBALANCE => {
                                self.reconnect(&hdr.peer.unwrap());
                            },
                            chatlib::ChatType::NAME => {
                                if let Some(load) = payload {
                                    let name: String = String::from_utf8(load.to_vec()).unwrap();
                                    self.set_stream_name(fd, &name);
                                    let msg = format!("{} has Joined the Chat Room", name);
                                    println!("{}", msg);
                                    self.send_msg(fd, msg.as_ref());
                                }
                            },
                        };
                    },
                    _ => { println!("invalid chat format"); },
                };
            },
        };
    }

    fn handle_send(&mut self, msg: &str, fd: i32) {
        let re = regex::Regex::new(r"^/(?P<cmd>[^\s\t\r\n]+)(?x)(?P<arg>[^\r\n]*)").unwrap();
        let cap = re.captures(msg);
        match cap {
            None => {
                match self.name {
                    Some(_) => {
                        let entire_msg: String = String::from(self.name.as_ref().unwrap()) + "> " + msg;
                        self.send_msg(fd, entire_msg.as_ref());
                    },
                    None => println!("Please Set Your Name First!\n/name <Name>"),
                };
            },
            Some(c) => {
                match c.name("cmd").unwrap().as_str() {
                    "name" => {
                        let name: &str = c.name("arg").unwrap().as_str().trim();
                        match name.len() {
                            0 => println!("Enter Valid Name!"),
                            _ => self.set_name(name.as_ref()), 
                        };
                    },
                    "exit" => {
                        std::process::exit(0);
                    },
                    _ => {/*  Ignore cmd */ },
                }
            },
        };
    }
    
    fn is_up_stream(&self, fd: i32) -> bool {
        if self.up_stream.is_some() && fd == self.up_stream.as_ref().unwrap().as_raw_fd(){
                return true;
        }

        false
    }

    fn is_stream(&self, fd: i32) -> bool {
        for client in &self.down_streams {
            if client.0.as_raw_fd() == fd {
                return true;
            }
        }

        self.is_up_stream(fd)
    }


    fn get_stream(&mut self, fd: i32) -> std::option::Option<&std::net::TcpStream> {
        for stream in &self.down_streams {
             if stream.0.as_raw_fd() == fd {
                return Some(&stream.0);
            }
        }

         if self.is_up_stream(fd) {
                return self.up_stream.as_ref();
        }

        None
    }

    fn get_stream_idx(&mut self, fd: i32) -> std::option::Option<usize> {
        for i in 0..self.down_streams.len() {
            if self.down_streams[i].0.as_raw_fd() == fd{
                return Some(i);
            }
        }
        None
    }

    fn _get_stream_info(&mut self, fd: i32) -> std::option::Option<&chatlib::InfoStream> {
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

        if self.is_up_stream(fd) {
            return String::from("Upstream");
        }

        String::from("UnKnown")
    }

    fn reconnect(&mut self, peer: &chatlib::Peer) {
        if self.up_stream.is_some() {
            println!("Closing connection with UpStream");
            match self.up_stream.as_ref().unwrap().shutdown(std::net::Shutdown::Both){
                Ok(_) => {

                    let fd: i32 = self.up_stream.as_ref().unwrap().as_raw_fd();
                    self.remove_poll(fd);
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

                self.add_poll(self.up_stream.as_ref().unwrap().as_raw_fd());

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

    fn close_client(&mut self, fd: i32) {
        println!("{} Closed Connection", self.get_name(fd));
        if self.up_stream.is_some() && self.is_up_stream(fd) {
            if let Some(peer) = self.failover {
                self.reconnect(&peer);
                return;
            }  
        }
        
        match self.get_stream(fd).unwrap().shutdown(std::net::Shutdown::Both){
            Ok(_) => {},
            Err(error) =>{
                println!("Socket Shutdown Failure: {:?}", error);
                std::process::exit(-1);
            },
        };

        self.remove_poll(fd);
    }

    fn send_msg(&mut self, fd: i32, msg: &[u8]) {
        let mut buf = chatlib::to_raw(&mut chatlib::ChatHeader::from_msg(), Some(msg));
        self.broadcast(&mut buf, fd, false);
    }

    fn send_name(&mut self) {
        if self.name.is_some() { 
            let mut buf: Vec<u8> = chatlib::to_raw(&mut chatlib::ChatHeader::from_name(), Some(self.name.as_ref().unwrap().as_ref()));
            self.broadcast(&mut buf, -1, true);
        }
    }

    fn send_peer(&mut self) {
        let send_port: u16 = self.host_port;
        let buf: Vec<u8> = chatlib::to_raw(&mut chatlib::ChatHeader::from_port(send_port), None);
        self.up_stream.as_ref().unwrap().write_all(&buf).expect("Write Failure send_peer()");
    }

    fn assign_successor(&mut self) -> std::option::Option<(Vec<u8>, i32)> {
        let fd: i32;
        if self.up_stream.as_ref().is_some() {
            fd = self.up_stream.as_ref().unwrap().as_raw_fd();
            self.successor = Some(fd);
            return Some((chatlib::to_raw(&mut chatlib::ChatHeader::from(chatlib::ChatType::FAILOVER, self.up_stream_info.unwrap()), None), fd));
        }
        match &self.successor {
            Some(fd_stream) => {
                fd = *fd_stream;
                return Some((chatlib::to_raw(&mut chatlib::ChatHeader::from_failover(self.get_stream(fd).unwrap().peer_addr().unwrap(), self.get_peer_port(fd)), None), fd));
            },
            None  => {
                for stream in &mut self.down_streams {
                    if stream.2 {
                        fd = stream.0.as_raw_fd();
                        self.successor = Some(fd);
                        let fail_ip: std::net::IpAddr = stream.0.peer_addr().unwrap().ip();
                        let fail_port: u16 = self.get_peer_port(fd);
                        return Some((chatlib::to_raw(&mut chatlib::ChatHeader::from_failover(std::net::SocketAddr::new(fail_ip, fail_port), fail_port), None), fd));
                    }
                }
            },
        };

        None
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
        let mut index: usize = 0;

        if self.down_streams.get(index).unwrap().0.as_raw_fd() == fd {
            index = (index + 1) % len;
        } 

        let addr: std::net::SocketAddr = self.down_streams.get(index).unwrap().1;
        let portno: u16 = self.down_streams.get(index).unwrap().3;

        (addr, portno)
    }

    fn send_rebalance(&mut self, fd: i32) {
        let (addr, portno) = self.pick_down_stream(fd);
        match self.get_stream(fd).unwrap().write(chatlib::to_raw(&mut chatlib::ChatHeader::from_rebalance(addr, portno), None).as_ref()){
            Ok(_) => {},
            Err(error) => {
                println!("In broadcast(), Write Failure: {:?}:", error);
            },
        };    
    }

    fn add_poll(&self, fd: i32) {
        match epoll::ctl(   self.epoll_fd, epoll::ControlOptions::EPOLL_CTL_ADD, fd, 
                            epoll::Event::new(epoll::Events::EPOLLIN, fd as u64)){
            Ok(_) => {},
            Err(error) => {
                println!("Epoll Ctl Failure: {:?}", error);
                std::process::exit(-1);
            },
        }
    }

    fn remove_poll(&mut self, fd: i32) {
        match epoll::ctl(   self.epoll_fd, epoll::ControlOptions::EPOLL_CTL_DEL, fd, 
                            epoll::Event::new(epoll::Events::EPOLLERR, fd as u64)){
            Ok(_) => {},
            Err(error) => {
                println!("Epoll Ctl Failure: {:?}", error);
                std::process::exit(-1);
            },
        };

        if let Some(index) = self.get_stream_idx(fd) {
            self.down_streams.remove(index);
        };

        if let Some(successor_fd) = self.successor {
            if successor_fd == fd {
                self.successor = None;
            }
        };

        if self.is_up_stream(fd) {
            self.up_stream = None;
            self.up_stream_info = None;
            self.up_stream_port = 0;
        }
    }

    pub fn start_routine(&mut self) {
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
        self.add_poll(host_fd);
        self.add_poll(STD_IN);

        //add upstream to read set
        if let Some(stream) = &self.up_stream {
                self.add_poll(stream.as_raw_fd());
                self.up_stream_info = Some(chatlib::Peer::new(Some(self.up_stream.as_ref().unwrap().peer_addr().unwrap()), self.up_stream_port));
                self.up_stream.as_ref().unwrap().set_nonblocking(true).expect("Error in SetNonBlocking(true)");
                self.up_stream.as_ref().unwrap().set_nodelay(true).expect("set_nodelay failure");
                self.send_peer();
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

            for event in all_events.iter().take(num_events) {
                let ready_fd: i32 = event.data as i32;
                let _event = match epoll::Events::from_bits(event.events){
                    Some(ev) => ev,
                    _ => {
                        println!("Error in from_bits()...");
                        continue
                    },
                };

                match ready_fd {
                    _ if ready_fd == host_fd => {
                         match self.host_listener.accept() {
                            Ok((down_stream, down_stream_addr)) => {

                                let client_fd = down_stream.as_raw_fd();
                                down_stream.set_nonblocking(true).expect("Error in SetNonBlocking(true)");
                                down_stream.set_nodelay(true).expect("set_nodelay failure");

                                println!("Got Connection From {}", down_stream_addr);
                                self.down_streams.push(chatlib::InfoStream(down_stream, down_stream_addr, false, 0, format!("Client {}", client_fd)));
                                self.add_poll(client_fd);
                            },
                            Err(error) => {
                                println!("Couldn't Accept New Connection: {:?}", error);
                                continue;
                            },
                        };
                    },
                    _ if ready_fd == STD_IN => {
                        let mut msg: String = String::new();
                        if let Ok(_count) = std::io::stdin().lock().read_line(&mut msg) {
                                self.handle_send(msg.trim(), ready_fd);
                        };
                    },
                    _ if self.is_stream(ready_fd) => {
                        let mut vecbuf: Vec<u8> = Vec::new();
                        loop {
                            let mut peakbuf = [0u8; 1400];
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
                                    vecbuf.extend_from_slice(&buf);
                                },
                                Err(_) => {
                                    println!("BufRead Err");
                                    self.close_client(ready_fd);
                                    break;
                                }
                            };
                        }

                        self.handle_recv(&mut vecbuf, ready_fd);
                    },
                    _ => { unreachable!("Epoll FD Picked Up FD That Is Not In Our Interest List"); },
                };
            }
        }
    }
}