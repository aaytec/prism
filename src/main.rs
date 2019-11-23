use std::os::unix::io::AsRawFd;
use std::io::{BufRead};
use std::io::{Write};

// use std::io::{self, BufReader, BufRead};
// use std::net::{TcpStream, TcpListener, SocketAddr};
// use std::io::{Read, Write};

const MAX_POLLS: usize = 5;
const STD_IN: i32 = 0;
const _STD_OUT: i32 = 1;
const _STD_ERR: i32 = 2;

// NOTE: consider EPOLLIN | EPOLLPRI | EPOLLERR

struct DownStream(std::net::TcpStream, std::net::SocketAddr);

struct ChatNode {

    //self and children
    host_listener: std::net::TcpListener,
    host_port: u16,
    down_streams: Vec<DownStream>,

    //parent
    up_stream: Option<std::net::TcpStream>,
    up_stream_addr: Option<std::net::SocketAddr>,

    //failover
    failover_addr: Option<std::net::SocketAddr>,

    //successor
    successor_addr: Option<std::net::SocketAddr>,
}

impl ChatNode {
    fn new(addr: std::net::SocketAddr, port: u16) -> Self {
        ChatNode {
            host_listener: std::net::TcpListener::bind(addr).unwrap(),
            host_port: port,
            down_streams: Vec::new(),
            up_stream: None,
            up_stream_addr: None,
            failover_addr: None,
            successor_addr: None,
        }
    }

    fn broadcast(client: &mut DownStream) {

    }

    fn handle_cmd() {

    }
    
    fn is_down_stream(&self, fd: i32) -> bool {
        for client in &self.down_streams {
            if client.0.as_raw_fd() == fd {
                return true;
            }
        }

        return false;
    }

    fn get_stream(&self, fd: i32) -> std::option::Option<usize> {
        for index in 0..self.down_streams.len() {
             if self.down_streams[index].0.as_raw_fd() == fd {
                return Some(index);
            }
        }

        return None;
    }

    fn start_routine(&mut self) {
        let host_fd: i32 = self.host_listener.as_raw_fd();
        let up_fd: i32;
        
        //create epoll
        let fd_poller: i32 = match epoll::create(false) {
            Ok(fd) => fd,
            Err(error) => {
                println!("Epoll Create Failure: {:?}", error);
                std::process::exit(-1);
            },
        };

        //add tcp listener to read set
        match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, host_fd, 
                            epoll::Event::new(epoll::Events::EPOLLIN, host_fd as u64)){
            Ok(_) => {},
            Err(error) => {
                println!("Epoll Ctl Failure: {:?}", error);
                std::process::exit(-1);
            },
        }
        
        //add upstream to read set
        match &self.up_stream {
            Some(stream) => {
                up_fd = stream.as_raw_fd();
                match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, up_fd, 
                                    epoll::Event::new(epoll::Events::EPOLLIN, up_fd as u64)){
                    Ok(_) => {},
                    Err(error) => {
                        println!("Epoll Ctl Failure: {:?}", error);
                        std::process::exit(-1);
                    },
                };

                //STD_IN
                match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, STD_IN, 
                                    epoll::Event::new(epoll::Events::EPOLLIN, STD_IN as u64)){
                    Ok(_) => {},
                    Err(error) => {
                        println!("Epoll Ctl Failure: {:?}", error);
                        std::process::exit(-1);
                    },
                };
            },
            None => {
                up_fd = -1;
            }
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

                            println!("Got Connection From {}", down_stream_addr);
                            let client_fd = down_stream.as_raw_fd();
                            self.down_streams.push(DownStream(down_stream, down_stream_addr));

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
                else if self.is_down_stream(ready_fd) {
                    //got msg from downstream clients

                    let mut buf: String = String::new();
                    let index: usize = self.get_stream(ready_fd).expect("Finding Fcpstream: Got Invalid Epoll Request");
                    let raw_count = std::io::BufReader::new(&self.down_streams[index].0).read_line(&mut buf).unwrap();
                    match raw_count {
                        0 => {
                            println!("Client {} Closed Connection", index);
                            match self.down_streams[index].0.shutdown(std::net::Shutdown::Both){
                                Ok(_) => {},
                                Err(error) =>{
                                    println!("Socket Shutdown Failure: {:?}", error);
                                    std::process::exit(-1);
                                },
                            };
                            match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_DEL, ready_fd, 
                                                epoll::Event::new(epoll::Events::EPOLLERR, ready_fd as u64)){
                                Ok(_) => {},
                                Err(error) => {
                                    println!("Epoll Ctl Failure: {:?}", error);
                                    std::process::exit(-1);
                                },
                            };
                            continue;
                        },
                        _ => {
                            println!("Client {}, Got {} Bytes: {}", index, raw_count, buf);
                        }, 
                    };
                }
                else if ready_fd == up_fd {
                    //got msg from upstream


                }
                
                else if ready_fd == 0 {
                    //got msg from stdin
                    let mut buf: String = String::new();
                    match std::io::stdin().lock().read_line(&mut buf){
                        Ok(_count) => {
                            match self.up_stream.as_ref().unwrap().write(buf.as_bytes()){
                                Ok(_) => {},
                                Err(error) => {
                                    println!("Write Failure: {:?}:", error);
                                    std::process::exit(-1);
                                },
                            };
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
        node.up_stream = match std::net::TcpStream::connect(&upstream) {
            Ok(mut connection) => {
                println!("Connecting to {:?}", upstream);
                match connection.write(format!("@Port {}\r\n", node.host_port).as_bytes()){
                    Ok(_) => {},
                    Err(error) => {
                        println!("Write Failure: {:?}:", error);
                        std::process::exit(-1);
                    },
                };
                
                Some(connection)
            },
            Err(_) => {
                println!("Couldn't connect to {:?}", upstream);  
                None
            },
        };
    }
    node.start_routine();
    
}
