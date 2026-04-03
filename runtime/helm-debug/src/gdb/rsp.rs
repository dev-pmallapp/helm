//! GDB Remote Serial Protocol implementation.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};

use super::target::{GdbTarget, StopReason};

/// GDB RSP server over TCP.
pub struct RspServer {
    port: u16,
}

impl RspServer {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Start listening. Blocks until a client connects, then enters the packet loop.
    pub fn listen(&self, target: &mut dyn GdbTarget) -> io::Result<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port))?;
        log::info!("GDB RSP server listening on port {}", self.port);
        let (stream, addr) = listener.accept()?;
        log::info!("GDB client connected from {addr}");
        let mut session = RspSession::new(stream)?;
        session.run(target)
    }
}

struct RspSession {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    no_ack: bool,
}

impl RspSession {
    fn new(stream: TcpStream) -> io::Result<Self> {
        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);
        Ok(Self {
            reader,
            writer,
            no_ack: false,
        })
    }

    fn run(&mut self, target: &mut dyn GdbTarget) -> io::Result<()> {
        loop {
            let packet = match self.read_packet()? {
                Some(p) => p,
                None => return Ok(()),
            };
            if !self.no_ack {
                self.send_ack()?;
            }
            match self.handle(&packet, target) {
                Resp::Reply(d) => self.send_packet(&d)?,
                Resp::Empty => self.send_packet("")?,
                Resp::Disconnect => return Ok(()),
            }
        }
    }

    fn read_packet(&mut self) -> io::Result<Option<String>> {
        const MAX_RETRIES: usize = 5;
        let mut b = [0u8; 1];
        for _ in 0..=MAX_RETRIES {
            // Wait for packet start '$'
            loop {
                match self.reader.read_exact(&mut b) {
                    Ok(()) if b[0] == b'$' => break,
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                    Err(e) => return Err(e),
                }
            }
            let mut data = Vec::new();
            let mut computed_cksum: u8 = 0;
            loop {
                self.reader.read_exact(&mut b)?;
                if b[0] == b'#' {
                    break;
                }
                computed_cksum = computed_cksum.wrapping_add(b[0]);
                data.push(b[0]);
            }
            let mut cksum_hex = [0u8; 2];
            self.reader.read_exact(&mut cksum_hex)?;
            let expected = u8::from_str_radix(
                std::str::from_utf8(&cksum_hex).unwrap_or("00"),
                16,
            )
            .unwrap_or(0);
            if computed_cksum != expected && !self.no_ack {
                // NAK — request retransmission
                self.writer.write_all(b"-")?;
                self.writer.flush()?;
                continue;
            }
            return Ok(Some(String::from_utf8_lossy(&data).to_string()));
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RSP checksum validation failed after max retries",
        ))
    }

    fn send_ack(&mut self) -> io::Result<()> {
        self.writer.write_all(b"+")?;
        self.writer.flush()
    }

    fn send_packet(&mut self, data: &str) -> io::Result<()> {
        let cksum: u8 = data.bytes().fold(0u8, |a, b| a.wrapping_add(b));
        write!(self.writer, "${}#{:02x}", data, cksum)?;
        self.writer.flush()
    }

    fn handle(&mut self, pkt: &str, t: &mut dyn GdbTarget) -> Resp {
        if pkt.is_empty() {
            return Resp::Empty;
        }
        match pkt.as_bytes()[0] {
            b'?' => Resp::Reply("S05".into()),
            b'g' => {
                let mut hex = String::new();
                for i in 0..t.num_registers() {
                    let v = t.read_register(i).unwrap_or(0);
                    for bi in 0..8 {
                        hex.push_str(&format!("{:02x}", (v >> (bi * 8)) & 0xFF));
                    }
                }
                Resp::Reply(hex)
            }
            b'G' => {
                let hd = &pkt[1..];
                for i in 0..t.num_registers() {
                    let off = i * 16;
                    if off + 16 > hd.len() {
                        break;
                    }
                    if let Ok(v) = parse_le_hex(&hd[off..off + 16]) {
                        t.write_register(i, v);
                    }
                }
                Resp::Reply("OK".into())
            }
            b'm' => {
                let parts: Vec<&str> = pkt[1..].split(',').collect();
                if parts.len() != 2 {
                    return Resp::Reply("E01".into());
                }
                let addr = u64::from_str_radix(parts[0], 16).unwrap_or(0);
                let len = usize::from_str_radix(parts[1], 16).unwrap_or(0);
                match t.read_memory(addr, len) {
                    Some(d) => Resp::Reply(d.iter().map(|b| format!("{b:02x}")).collect()),
                    None => Resp::Reply("E14".into()),
                }
            }
            b'M' => {
                let cp = pkt.find(':').unwrap_or(1);
                let parts: Vec<&str> = pkt[1..cp].split(',').collect();
                if parts.len() != 2 {
                    return Resp::Reply("E01".into());
                }
                let addr = u64::from_str_radix(parts[0], 16).unwrap_or(0);
                let hd = &pkt[cp + 1..];
                let data: Vec<u8> = (0..hd.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(&hd[i..i + 2], 16).ok())
                    .collect();
                if t.write_memory(addr, &data) {
                    Resp::Reply("OK".into())
                } else {
                    Resp::Reply("E14".into())
                }
            }
            b's' => {
                t.step();
                Resp::Reply("S05".into())
            }
            b'c' => match t.continue_exec() {
                StopReason::Breakpoint(_) | StopReason::Step => Resp::Reply("S05".into()),
                StopReason::Exited(c) => Resp::Reply(format!("W{c:02x}")),
                StopReason::Signal(s) => Resp::Reply(format!("S{s:02x}")),
            },
            b'Z' | b'z' => {
                let set = pkt.as_bytes()[0] == b'Z';
                let parts: Vec<&str> = pkt[1..].split(',').collect();
                if parts.len() < 2 {
                    return Resp::Reply("E01".into());
                }
                if parts[0] != "0" {
                    return Resp::Empty;
                }
                let addr = u64::from_str_radix(parts[1], 16).unwrap_or(0);
                let ok = if set {
                    t.set_breakpoint(addr)
                } else {
                    t.remove_breakpoint(addr)
                };
                if ok {
                    Resp::Reply("OK".into())
                } else {
                    Resp::Reply("E01".into())
                }
            }
            b'D' => {
                let _ = self.send_packet("OK");
                Resp::Disconnect
            }
            b'k' => Resp::Disconnect,
            b'q' => {
                if pkt.starts_with("qSupported") {
                    Resp::Reply("PacketSize=4096".into())
                } else if pkt == "qAttached" {
                    Resp::Reply("1".into())
                } else if pkt.starts_with("qC") {
                    Resp::Reply("QC1".into())
                } else {
                    Resp::Empty
                }
            }
            b'Q' => {
                if pkt == "QStartNoAckMode" {
                    self.no_ack = true;
                    Resp::Reply("OK".into())
                } else {
                    Resp::Empty
                }
            }
            _ => Resp::Empty,
        }
    }
}

enum Resp {
    Reply(String),
    Empty,
    Disconnect,
}

fn parse_le_hex(hex: &str) -> Result<u64, ()> {
    let mut val: u64 = 0;
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|_| ())?;
        let byte = u8::from_str_radix(s, 16).map_err(|_| ())?;
        val |= (byte as u64) << (i * 8);
    }
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_le() {
        assert_eq!(parse_le_hex("00000080"), Ok(0x80000000));
    }
}
