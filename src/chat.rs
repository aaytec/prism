use std::os::unix::io::AsRawFd;
use std::io::{BufRead};
use std::io::{Write};

// use std::io::{self, BufReader, BufRead};
// use std::net::{TcpStream, TcpListener, SocketAddr};
// use std::io::{Read, Write};

const MAX_POLLS: usize = 5;
const CMD_SYM: char = '~';
const STD_IN: i32 = 0;
const _STD_OUT: i32 = 1;
const _STD_ERR: i32 = 2;


// NOTE: consider EPOLLIN | EPOLLPRI | EPOLLERR

struct InfoStream(std::net::TcpStream, std::net::SocketAddr, bool, i32);

enum CommandType {
    PORT,
    REBALANCE,
    FAILOVER,
    NAMECHANGE,

}

struct ChatCommand {
    tpye: CommandType,
    buf: String,
}

struct ChatNode {

    //self and children
    host_listener: std::net::TcpListener,
    host_port: u16,
    down_streams: Vec<InfoStream>,

    //parent
    up_stream_fd: Option<i32>,

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
            up_stream_fd: None,
            failover_addr: None,
            successor_addr: None,
        }
    }

    fn broadcast(&mut self, buf: &str, fd: i32) {
        println!("BROADCASTING");
        for stream in &mut self.down_streams {
            let send_fd: i32 = stream.0.as_raw_fd();
            if send_fd != fd {
                println!("\t| Sending Client {}: {:?}", send_fd, buf);
                match stream.0.write(buf.as_bytes()){
                    Ok(_) => {},
                    Err(error) => {
                        println!("In broadcast(), Write Failure: {:?}:", error);
                        std::process::exit(-1);
                    },
                };
            }
        }
    }

    fn handle_cmd(&self, buf: &str, fd: i32){
        println!("Got Command: {:?}", buf);
        let mut iter = buf.split_whitespace();
    }

    fn handle_recv(&mut self, buf: &str, fd: i32) {
        match buf.len() {
            0 => panic!("shouldn't recv 0 in handle_recv(): checked already"),
            _ => {
                match buf.chars().nth(0).unwrap(){
                    CMD_SYM => {
                        self.handle_cmd(buf, fd);
                    },
                    _ => {
                        self.broadcast(buf, fd);
                    },
                };
            },
        };
    }
    
    fn is_down_stream(&self, fd: i32) -> bool {
        for client in &self.down_streams {
            if client.0.as_raw_fd() == fd {
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

    fn close_client(&mut self, efd: i32, fd: i32) {
        let index: usize = self.get_stream_idx(fd).expect("Finding Fcpstream: Got Invalid Epoll Request");
        println!("Client {} Closed Connection", index);
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
        match &self.up_stream_fd {
            Some(stream_fd) => {
                match epoll::ctl(   fd_poller, epoll::ControlOptions::EPOLL_CTL_ADD, *stream_fd, 
                                    epoll::Event::new(epoll::Events::EPOLLIN, *stream_fd as u64)){
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

                            println!("Got Connection From {}", down_stream_addr);
                            let client_fd = down_stream.as_raw_fd();
                            self.down_streams.push(InfoStream(down_stream, down_stream_addr, false, -1));

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
                    //got msg from connections

                    let mut buf: String = String::new();
                    let index: usize = self.get_stream_idx(ready_fd).expect("Finding Tcpstream: Got Invalid Epoll Request");
                    match std::io::BufReader::new(self.get_stream(ready_fd).unwrap()).read_line(&mut buf){
                        Ok(raw_count) => {
                            match raw_count {
                                0 => self.close_client(fd_poller, ready_fd),
                                _ => {
                                    println!("Client {}, Got {} Bytes: {}", index, raw_count, buf.trim());
                                    self.handle_recv(buf.trim(), ready_fd);
                                }, 
                            };
                        },
                        Err(_) => {
                            self.close_client(fd_poller, ready_fd);
                        }
                    };
                }
                else if ready_fd == 0 {
                    //got msg from stdin
                    let mut buf: String = String::new();
                    match std::io::stdin().lock().read_line(&mut buf){
                        Ok(_count) => {
                            self.broadcast(buf.as_mut_str(), ready_fd);
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
        node.up_stream_fd = match std::net::TcpStream::connect(&upstream) {
            Ok(mut connection) => {
                println!("Connecting to {:?}", upstream);
                match connection.write(format!("~Port {}\r\n", node.host_port).as_bytes()){
                    Ok(_) => {},
                    Err(error) => {
                        println!("Write Failure: {:?}:", error);
                        std::process::exit(-1);
                    },
                };
                let raw_fd: i32 = connection.as_raw_fd();
                node.down_streams.push(InfoStream(connection, upstream.parse().unwrap(), true, argv[3].trim().parse().unwrap()));
                Some(raw_fd)
            },
            Err(_) => {
                println!("Couldn't connect to {:?}", upstream);  
                None
            },
        };
    }
    node.start_routine();
    
}
