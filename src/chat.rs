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
    let mut node: chat::ChatNode = chat::ChatNode::new(std::net::SocketAddr::from(([127, 0, 0, 1], port)), port);
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
