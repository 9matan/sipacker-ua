use std::net::Ipv4Addr;

use clap::{self, Parser};

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(long, help = "Ip address to listen")]
    pub ip_addr: Ipv4Addr,
    #[arg(long, help = "Port to listen", default_value = "5060")]
    pub port: u16,
    #[arg(long, help = "Concurrent jobs", default_value = "4")]
    pub jobs: usize,
}
