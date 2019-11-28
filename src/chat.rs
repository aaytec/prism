extern crate console;
use console::style;

pub fn welcome(port: &str) {
    println!("\t\t\t\t\t{}\t\t\t\t", style("Welcome to Prism").blue());
    println!("\t\t\t\t\t\t\t\t\t\t\t");
    println!("\t{}\t", style("Prism is a multi-chat service provided via a shared network between your peers.").magenta());
    println!("\t{}\t", style("It allows you to create, join, leave, and change chat channels with ease!").magenta());
    println!("\t\t\t\t\t\t\t\t\t\t\t");
    println!("{}", style("\t\t\t\t\t\t\t\t\t\t\t"));
    chat::help();
    println!("\t\t\t\t\t\t\t\t\t\t\t");
    println!("\t\t\t\t\t\t\t\t\t\t\t");
    println!("\t{}\t\t\t\t\t\t\t", style(format!("You are Hosting Prism At Port {}", port)).yellow());
    println!("\t{}\t\t", style("**Reminder: Set your name first, this tells everyone who you are!").cyan());
    println!("\t{}\t\t", style("**Reminder: Share your connectivity information with discretion!").cyan());
    println!("\t{}\t\t\n\n", style("**Reminder: run $ifconfig for more information of your connection details.").cyan());
}


pub fn usage() {
    println!("Usage: ./chat <HOST-PORT>");
    println!("Usage: ./chat <HOST-PORT> <CONNECT-IP> <CONNECT-PORTNO>");
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() < 2 {
        usage();
        std::process::exit(0);
    }
    welcome(&argv[1]);
    
    let port: u16 = argv[1].trim().parse().unwrap();
    let mut node: chat::ChatNode = chat::ChatNode::new(std::net::SocketAddr::from(([0, 0, 0, 0], port)), port);
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
