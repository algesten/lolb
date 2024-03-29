use regex::Regex;
use serde_derive::Deserialize;
use std::fs::File;
use std::io::{Error, ErrorKind};
use std::io::{Read, Result};
use std::net::{UdpSocket, SocketAddr};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::process::Command;
use toml::value::Table;
use std::sync::mpsc::channel;
use std::thread;
use std::sync::Arc;

pub struct BytePacketBuffer {
    pub buf: [u8; 512],
    pub pos: usize,
}

impl BytePacketBuffer {
    pub fn new() -> BytePacketBuffer {
        BytePacketBuffer {
            buf: [0; 512],
            pos: 0,
        }
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn step(&mut self, steps: usize) -> Result<()> {
        self.pos += steps;

        Ok(())
    }

    fn seek(&mut self, pos: usize) -> Result<()> {
        self.pos = pos;

        Ok(())
    }

    fn read(&mut self) -> Result<u8> {
        if self.pos >= 512 {
            return Err(Error::new(ErrorKind::InvalidInput, "End of buffer"));
        }
        let res = self.buf[self.pos];
        self.pos += 1;

        Ok(res)
    }

    fn get(&mut self, pos: usize) -> Result<u8> {
        if pos >= 512 {
            return Err(Error::new(ErrorKind::InvalidInput, "End of buffer"));
        }
        Ok(self.buf[pos])
    }

    fn get_range(&mut self, start: usize, len: usize) -> Result<&[u8]> {
        if start + len >= 512 {
            return Err(Error::new(ErrorKind::InvalidInput, "End of buffer"));
        }
        Ok(&self.buf[start..start + len as usize])
    }

    fn read_u16(&mut self) -> Result<u16> {
        let res = ((r#try!(self.read()) as u16) << 8) | (r#try!(self.read()) as u16);

        Ok(res)
    }

    fn read_u32(&mut self) -> Result<u32> {
        let res = ((r#try!(self.read()) as u32) << 24)
            | ((r#try!(self.read()) as u32) << 16)
            | ((r#try!(self.read()) as u32) << 8)
            | ((r#try!(self.read()) as u32) << 0);

        Ok(res)
    }

    fn read_qname(&mut self, outstr: &mut String) -> Result<()> {
        let mut pos = self.pos();
        let mut jumped = false;

        let mut delim = "";
        loop {
            let len = r#try!(self.get(pos));

            // A two byte sequence, where the two highest bits of the first byte is
            // set, represents a offset relative to the start of the buffer. We
            // handle this by jumping to the offset, setting a flag to indicate
            // that we shouldn't update the shared buffer position once done.
            if (len & 0xC0) == 0xC0 {
                // When a jump is performed, we only modify the shared buffer
                // position once, and avoid making the change later on.
                if !jumped {
                    r#try!(self.seek(pos + 2));
                }

                let b2 = r#try!(self.get(pos + 1)) as u16;
                let offset = (((len as u16) ^ 0xC0) << 8) | b2;
                pos = offset as usize;
                jumped = true;
                continue;
            }

            pos += 1;

            // Names are terminated by an empty label of length 0
            if len == 0 {
                break;
            }

            outstr.push_str(delim);

            let str_buffer = r#try!(self.get_range(pos, len as usize));
            outstr.push_str(&String::from_utf8_lossy(str_buffer).to_lowercase());

            delim = ".";

            pos += len as usize;
        }

        if !jumped {
            r#try!(self.seek(pos));
        }

        Ok(())
    }

    fn write(&mut self, val: u8) -> Result<()> {
        if self.pos >= 512 {
            return Err(Error::new(ErrorKind::InvalidInput, "End of buffer"));
        }
        self.buf[self.pos] = val;
        self.pos += 1;
        Ok(())
    }

    fn write_u8(&mut self, val: u8) -> Result<()> {
        r#try!(self.write(val));

        Ok(())
    }

    fn write_u16(&mut self, val: u16) -> Result<()> {
        r#try!(self.write((val >> 8) as u8));
        r#try!(self.write((val & 0xFF) as u8));

        Ok(())
    }

    fn write_u32(&mut self, val: u32) -> Result<()> {
        r#try!(self.write(((val >> 24) & 0xFF) as u8));
        r#try!(self.write(((val >> 16) & 0xFF) as u8));
        r#try!(self.write(((val >> 8) & 0xFF) as u8));
        r#try!(self.write(((val >> 0) & 0xFF) as u8));

        Ok(())
    }

    fn write_qname(&mut self, qname: &str) -> Result<()> {
        let split_str = qname.split('.').collect::<Vec<&str>>();

        for label in split_str {
            let len = label.len();
            if len > 0x34 {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Single label exceeds 63 characters of length",
                ));
            }

            r#try!(self.write_u8(len as u8));
            for b in label.as_bytes() {
                r#try!(self.write_u8(*b));
            }
        }

        r#try!(self.write_u8(0));

        Ok(())
    }

    fn set(&mut self, pos: usize, val: u8) -> Result<()> {
        self.buf[pos] = val;

        Ok(())
    }

    fn set_u16(&mut self, pos: usize, val: u16) -> Result<()> {
        r#try!(self.set(pos, (val >> 8) as u8));
        r#try!(self.set(pos + 1, (val & 0xFF) as u8));

        Ok(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResultCode {
    NOERROR = 0,
    FORMERR = 1,
    SERVFAIL = 2,
    NXDOMAIN = 3,
    NOTIMP = 4,
    REFUSED = 5,
}

impl ResultCode {
    pub fn from_num(num: u8) -> ResultCode {
        match num {
            1 => ResultCode::FORMERR,
            2 => ResultCode::SERVFAIL,
            3 => ResultCode::NXDOMAIN,
            4 => ResultCode::NOTIMP,
            5 => ResultCode::REFUSED,
            0 | _ => ResultCode::NOERROR,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DnsHeader {
    pub id: u16, // 16 bits

    pub recursion_desired: bool,
    // 1 bit
    pub truncated_message: bool,
    // 1 bit
    pub authoritative_answer: bool,
    // 1 bit
    pub opcode: u8,
    // 4 bits
    pub response: bool,             // 1 bit

    pub rescode: ResultCode,
    // 4 bits
    pub checking_disabled: bool,
    // 1 bit
    pub authed_data: bool,
    // 1 bit
    pub z: bool,
    // 1 bit
    pub recursion_available: bool, // 1 bit

    pub questions: u16,
    // 16 bits
    pub answers: u16,
    // 16 bits
    pub authoritative_entries: u16,
    // 16 bits
    pub resource_entries: u16,      // 16 bits
}

impl DnsHeader {
    pub fn new() -> DnsHeader {
        DnsHeader {
            id: 0,

            recursion_desired: false,
            truncated_message: false,
            authoritative_answer: false,
            opcode: 0,
            response: false,

            rescode: ResultCode::NOERROR,
            checking_disabled: false,
            authed_data: false,
            z: false,
            recursion_available: false,

            questions: 0,
            answers: 0,
            authoritative_entries: 0,
            resource_entries: 0,
        }
    }

    pub fn read(&mut self, buffer: &mut BytePacketBuffer) -> Result<()> {
        self.id = r#try!(buffer.read_u16());

        let flags = r#try!(buffer.read_u16());
        let a = (flags >> 8) as u8;
        let b = (flags & 0xFF) as u8;
        self.recursion_desired = (a & (1 << 0)) > 0;
        self.truncated_message = (a & (1 << 1)) > 0;
        self.authoritative_answer = (a & (1 << 2)) > 0;
        self.opcode = (a >> 3) & 0x0F;
        self.response = (a & (1 << 7)) > 0;

        self.rescode = ResultCode::from_num(b & 0x0F);
        self.checking_disabled = (b & (1 << 4)) > 0;
        self.authed_data = (b & (1 << 5)) > 0;
        self.z = (b & (1 << 6)) > 0;
        self.recursion_available = (b & (1 << 7)) > 0;

        self.questions = r#try!(buffer.read_u16());
        self.answers = r#try!(buffer.read_u16());
        self.authoritative_entries = r#try!(buffer.read_u16());
        self.resource_entries = r#try!(buffer.read_u16());

        // Return the constant header size
        Ok(())
    }

    pub fn write(&self, buffer: &mut BytePacketBuffer) -> Result<()> {
        r#try!(buffer.write_u16(self.id));

        r#try!(buffer.write_u8(
            (self.recursion_desired as u8)
                | ((self.truncated_message as u8) << 1)
                | ((self.authoritative_answer as u8) << 2)
                | (self.opcode << 3)
                | ((self.response as u8) << 7) as u8
        ));

        r#try!(buffer.write_u8(
            (self.rescode.clone() as u8)
                | ((self.checking_disabled as u8) << 4)
                | ((self.authed_data as u8) << 5)
                | ((self.z as u8) << 6)
                | ((self.recursion_available as u8) << 7)
        ));

        r#try!(buffer.write_u16(self.questions));
        r#try!(buffer.write_u16(self.answers));
        r#try!(buffer.write_u16(self.authoritative_entries));
        r#try!(buffer.write_u16(self.resource_entries));

        Ok(())
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Hash, Copy)]
pub enum QueryType {
    UNKNOWN(u16),
    A,
    // 1
    NS,
    // 2
    CNAME,
    // 5
    MX,
    // 15
    AAAA,  // 28
}

impl QueryType {
    pub fn to_num(&self) -> u16 {
        match *self {
            QueryType::UNKNOWN(x) => x,
            QueryType::A => 1,
            QueryType::NS => 2,
            QueryType::CNAME => 5,
            QueryType::MX => 15,
            QueryType::AAAA => 28,
        }
    }

    pub fn from_num(num: u16) -> QueryType {
        match num {
            1 => QueryType::A,
            2 => QueryType::NS,
            5 => QueryType::CNAME,
            15 => QueryType::MX,
            28 => QueryType::AAAA,
            _ => QueryType::UNKNOWN(num),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsQuestion {
    pub name: String,
    pub qtype: QueryType,
}

impl DnsQuestion {
    pub fn new(name: String, qtype: QueryType) -> DnsQuestion {
        DnsQuestion {
            name: name,
            qtype: qtype,
        }
    }

    pub fn read(&mut self, buffer: &mut BytePacketBuffer) -> Result<()> {
        r#try!(buffer.read_qname(&mut self.name));
        self.qtype = QueryType::from_num(r#try!(buffer.read_u16())); // qtype
        let _ = r#try!(buffer.read_u16()); // class

        Ok(())
    }

    pub fn write(&self, buffer: &mut BytePacketBuffer) -> Result<()> {
        r#try!(buffer.write_qname(&self.name));

        let typenum = self.qtype.to_num();
        r#try!(buffer.write_u16(typenum));
        r#try!(buffer.write_u16(1));

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum DnsRecord {
    UNKNOWN {
        domain: String,
        qtype: u16,
        data_len: u16,
        ttl: u32,
    },
    // 0
    A {
        domain: String,
        addr: Ipv4Addr,
        ttl: u32,
    },
    // 1
    NS {
        domain: String,
        host: String,
        ttl: u32,
    },
    // 2
    CNAME {
        domain: String,
        host: String,
        ttl: u32,
    },
    // 5
    MX {
        domain: String,
        priority: u16,
        host: String,
        ttl: u32,
    },
    // 15
    AAAA {
        domain: String,
        addr: Ipv6Addr,
        ttl: u32,
    }, // 28
}

impl DnsRecord {
    pub fn read(buffer: &mut BytePacketBuffer) -> Result<DnsRecord> {
        let mut domain = String::new();
        r#try!(buffer.read_qname(&mut domain));

        let qtype_num = r#try!(buffer.read_u16());
        let qtype = QueryType::from_num(qtype_num);
        let _ = r#try!(buffer.read_u16());
        let ttl = r#try!(buffer.read_u32());
        let data_len = r#try!(buffer.read_u16());

        match qtype {
            QueryType::A => {
                let raw_addr = r#try!(buffer.read_u32());
                let addr = Ipv4Addr::new(
                    ((raw_addr >> 24) & 0xFF) as u8,
                    ((raw_addr >> 16) & 0xFF) as u8,
                    ((raw_addr >> 8) & 0xFF) as u8,
                    ((raw_addr >> 0) & 0xFF) as u8,
                );

                Ok(DnsRecord::A {
                    domain: domain,
                    addr: addr,
                    ttl: ttl,
                })
            }
            QueryType::AAAA => {
                let raw_addr1 = r#try!(buffer.read_u32());
                let raw_addr2 = r#try!(buffer.read_u32());
                let raw_addr3 = r#try!(buffer.read_u32());
                let raw_addr4 = r#try!(buffer.read_u32());
                let addr = Ipv6Addr::new(
                    ((raw_addr1 >> 16) & 0xFFFF) as u16,
                    ((raw_addr1 >> 0) & 0xFFFF) as u16,
                    ((raw_addr2 >> 16) & 0xFFFF) as u16,
                    ((raw_addr2 >> 0) & 0xFFFF) as u16,
                    ((raw_addr3 >> 16) & 0xFFFF) as u16,
                    ((raw_addr3 >> 0) & 0xFFFF) as u16,
                    ((raw_addr4 >> 16) & 0xFFFF) as u16,
                    ((raw_addr4 >> 0) & 0xFFFF) as u16,
                );

                Ok(DnsRecord::AAAA {
                    domain: domain,
                    addr: addr,
                    ttl: ttl,
                })
            }
            QueryType::NS => {
                let mut ns = String::new();
                r#try!(buffer.read_qname(&mut ns));

                Ok(DnsRecord::NS {
                    domain: domain,
                    host: ns,
                    ttl: ttl,
                })
            }
            QueryType::CNAME => {
                let mut cname = String::new();
                r#try!(buffer.read_qname(&mut cname));

                Ok(DnsRecord::CNAME {
                    domain: domain,
                    host: cname,
                    ttl: ttl,
                })
            }
            QueryType::MX => {
                let priority = r#try!(buffer.read_u16());
                let mut mx = String::new();
                r#try!(buffer.read_qname(&mut mx));

                Ok(DnsRecord::MX {
                    domain: domain,
                    priority: priority,
                    host: mx,
                    ttl: ttl,
                })
            }
            QueryType::UNKNOWN(_) => {
                r#try!(buffer.step(data_len as usize));

                Ok(DnsRecord::UNKNOWN {
                    domain: domain,
                    qtype: qtype_num,
                    data_len: data_len,
                    ttl: ttl,
                })
            }
        }
    }

    pub fn write(&self, buffer: &mut BytePacketBuffer) -> Result<usize> {
        let start_pos = buffer.pos();

        match *self {
            DnsRecord::A {
                ref domain,
                ref addr,
                ttl,
            } => {
                r#try!(buffer.write_qname(domain));
                r#try!(buffer.write_u16(QueryType::A.to_num()));
                r#try!(buffer.write_u16(1));
                r#try!(buffer.write_u32(ttl));
                r#try!(buffer.write_u16(4));

                let octets = addr.octets();
                r#try!(buffer.write_u8(octets[0]));
                r#try!(buffer.write_u8(octets[1]));
                r#try!(buffer.write_u8(octets[2]));
                r#try!(buffer.write_u8(octets[3]));
            }
            DnsRecord::NS {
                ref domain,
                ref host,
                ttl,
            } => {
                r#try!(buffer.write_qname(domain));
                r#try!(buffer.write_u16(QueryType::NS.to_num()));
                r#try!(buffer.write_u16(1));
                r#try!(buffer.write_u32(ttl));

                let pos = buffer.pos();
                r#try!(buffer.write_u16(0));

                r#try!(buffer.write_qname(host));

                let size = buffer.pos() - (pos + 2);
                r#try!(buffer.set_u16(pos, size as u16));
            }
            DnsRecord::CNAME {
                ref domain,
                ref host,
                ttl,
            } => {
                r#try!(buffer.write_qname(domain));
                r#try!(buffer.write_u16(QueryType::CNAME.to_num()));
                r#try!(buffer.write_u16(1));
                r#try!(buffer.write_u32(ttl));

                let pos = buffer.pos();
                r#try!(buffer.write_u16(0));

                r#try!(buffer.write_qname(host));

                let size = buffer.pos() - (pos + 2);
                r#try!(buffer.set_u16(pos, size as u16));
            }
            DnsRecord::MX {
                ref domain,
                priority,
                ref host,
                ttl,
            } => {
                r#try!(buffer.write_qname(domain));
                r#try!(buffer.write_u16(QueryType::MX.to_num()));
                r#try!(buffer.write_u16(1));
                r#try!(buffer.write_u32(ttl));

                let pos = buffer.pos();
                r#try!(buffer.write_u16(0));

                r#try!(buffer.write_u16(priority));
                r#try!(buffer.write_qname(host));

                let size = buffer.pos() - (pos + 2);
                r#try!(buffer.set_u16(pos, size as u16));
            }
            DnsRecord::AAAA {
                ref domain,
                ref addr,
                ttl,
            } => {
                r#try!(buffer.write_qname(domain));
                r#try!(buffer.write_u16(QueryType::AAAA.to_num()));
                r#try!(buffer.write_u16(1));
                r#try!(buffer.write_u32(ttl));
                r#try!(buffer.write_u16(16));

                for octet in &addr.segments() {
                    r#try!(buffer.write_u16(*octet));
                }
            }
            DnsRecord::UNKNOWN { .. } => {
                println!("Skipping record: {:?}", self);
            }
        }

        Ok(buffer.pos() - start_pos)
    }
}

#[derive(Clone, Debug)]
pub struct DnsPacket {
    pub header: DnsHeader,
    pub questions: Vec<DnsQuestion>,
    pub answers: Vec<DnsRecord>,
    pub authorities: Vec<DnsRecord>,
    pub resources: Vec<DnsRecord>,
}

impl DnsPacket {
    pub fn new() -> DnsPacket {
        DnsPacket {
            header: DnsHeader::new(),
            questions: Vec::new(),
            answers: Vec::new(),
            authorities: Vec::new(),
            resources: Vec::new(),
        }
    }

    pub fn from_buffer(buffer: &mut BytePacketBuffer) -> Result<DnsPacket> {
        let mut result = DnsPacket::new();
        r#try!(result.header.read(buffer));

        for _ in 0..result.header.questions {
            let mut question = DnsQuestion::new("".to_string(), QueryType::UNKNOWN(0));
            r#try!(question.read(buffer));
            result.questions.push(question);
        }

        for _ in 0..result.header.answers {
            let rec = r#try!(DnsRecord::read(buffer));
            result.answers.push(rec);
        }
        for _ in 0..result.header.authoritative_entries {
            let rec = r#try!(DnsRecord::read(buffer));
            result.authorities.push(rec);
        }
        for _ in 0..result.header.resource_entries {
            let rec = r#try!(DnsRecord::read(buffer));
            result.resources.push(rec);
        }

        Ok(result)
    }

    pub fn write(&mut self, buffer: &mut BytePacketBuffer) -> Result<()> {
        self.header.questions = self.questions.len() as u16;
        self.header.answers = self.answers.len() as u16;
        self.header.authoritative_entries = self.authorities.len() as u16;
        self.header.resource_entries = self.resources.len() as u16;

        r#try!(self.header.write(buffer));

        for question in &self.questions {
            r#try!(question.write(buffer));
        }
        for rec in &self.answers {
            r#try!(rec.write(buffer));
        }
        for rec in &self.authorities {
            r#try!(rec.write(buffer));
        }
        for rec in &self.resources {
            r#try!(rec.write(buffer));
        }

        Ok(())
    }
}

#[allow(dead_code)]
fn lookup(qname: &str, qtype: QueryType, server: (&str, u16)) -> Result<DnsPacket> {
    let socket = r#try!(UdpSocket::bind(("0.0.0.0", 43210)));

    let mut packet = DnsPacket::new();

    packet.header.id = 6666;
    packet.header.questions = 1;
    packet.header.recursion_desired = true;
    packet
        .questions
        .push(DnsQuestion::new(qname.to_string(), qtype));

    let mut req_buffer = BytePacketBuffer::new();
    packet.write(&mut req_buffer).unwrap();
    r#try!(socket.send_to(&req_buffer.buf[0..req_buffer.pos], server));

    let mut res_buffer = BytePacketBuffer::new();
    socket.recv_from(&mut res_buffer.buf).unwrap();

    DnsPacket::from_buffer(&mut res_buffer)
}

#[derive(Debug, Deserialize)]
struct Config {
    address: Table,
}

enum AddressKey {
    Raw(String),
    Regex(Regex),
}

impl AddressKey {
    fn new(text: &str) -> std::result::Result<Self, regex::Error> {
        if text.starts_with("/") && text.ends_with("/") {
            let s = &text[1..text.len() - 1];
            Ok(AddressKey::Regex(Regex::new(s)?))
        } else {
            Ok(AddressKey::Raw(text.to_owned()))
        }
    }

    fn is_match(&self, addr: &str) -> bool {
        match self {
            AddressKey::Raw(text) => text == addr,
            AddressKey::Regex(regex) => regex.is_match(addr),
        }
    }
}

enum IpValue {
    Raw(String),
    Cmd(String),
}

impl IpValue {
    fn new(text: &str) -> Self {
        if text.starts_with("`") && text.ends_with("`") {
            let s = &text[1..text.len() - 1];
            IpValue::Cmd(s.to_owned())
        } else if text.starts_with("env(") && text.ends_with(")") {
            let env = &text[4..text.len() - 1];
            let arg = std::env::var(env).expect(&*format!("Environment argument `{}`", env));
            if arg.is_empty() {
                panic!("Environment argument `{}` can't be empty.", env);
            }
            println!("Environment argument load: {} = {}", env, arg);
            IpValue::Raw(arg)
        } else {
            IpValue::Raw(text.to_owned())
        }
    }

    fn value(&self) -> Result<String> {
        match self {
            IpValue::Raw(text) => Ok(text.to_owned()),
            IpValue::Cmd(cmd) => {
                let output = Command::new("bash").arg("-c").arg(cmd).output()?;
                if output.status.success() {
                    Ok(String::from_utf8(output.stdout).unwrap_or("".to_owned()))
                } else {
                    eprintln!("CmdError: {}", String::from_utf8(output.stderr).unwrap_or("".to_owned()).trim());
                    Ok("".to_owned())
                }
            }
        }
    }
}

struct AddressMatcher {
    inner: Vec<(AddressKey, IpValue)>,
}

impl AddressMatcher {
    fn new_from_config(config: &Config) -> std::result::Result<Self, regex::Error> {
        let mut vec = Vec::new();
        for (addr_text, ip_text) in &config.address {
            vec.push((AddressKey::new(addr_text)?, IpValue::new(ip_text.as_str().unwrap())));
        }
        Ok(Self { inner: vec })
    }

    fn matches(&self, qname: &str) -> Result<Option<String>> {
        for (address_key, ip_value) in &self.inner {
            if address_key.is_match(qname) {
                return Ok(Some(ip_value.value()?));
            }
        }
        Ok(None)
    }
}

fn lookup_regexp_hack(
    qname: &str,
    qtype: QueryType,
    matcher: &AddressMatcher,
) -> Result<DnsPacket> {
    let mut dns_packet = DnsPacket::new();
    dns_packet.header.rescode = ResultCode::NOERROR;

    if qtype != QueryType::A && qtype != QueryType::AAAA {
        eprintln!("Unsupported query type: {:?}", qtype);
        dns_packet.header.rescode = ResultCode::NOTIMP;
        return Ok(dns_packet);
    }

    dns_packet.answers.push(DnsRecord::A {
        domain: qname.to_owned(),
        addr: match matcher.matches(qname) {
            Ok(x) => match x {
                Some(x) => match x.trim().parse() {
                    Ok(x) => x,
                    Err(e) => {
                        eprintln!("Parse ip address error: {:?}", e);
                        dns_packet.header.rescode = ResultCode::SERVFAIL;
                        return Ok(dns_packet);
                    }
                },
                None => {
                    eprintln!("Not found url for query name");
                    dns_packet.header.rescode = ResultCode::SERVFAIL;
                    return Ok(dns_packet);
                }
            },
            Err(e) => {
                eprintln!("Match error: {:?}", e);
                dns_packet.header.rescode = ResultCode::SERVFAIL;
                return Ok(dns_packet);
            }
        },
        ttl: 0,
    });
    Ok(dns_packet)
}

struct IndexLoop<T> {
    current: T,
    max: T,
}

impl<T> IndexLoop<T> {
    fn new(max: T, current: T) -> Self {
        Self {
            current,
            max,
        }
    }
}

impl IndexLoop<usize> {
    fn next(&mut self) -> usize {
        let current = self.current;
        self.current = (self.current + 1) % self.max;
        current
    }
}

fn main() {
    let clap_app = clap::App::new("dns-regex")
        .version("0.1.0")
        .author("__JM_Joy__ <918734043@qq.com>")
        .about("A simple dns server support regexp server name.")
        .arg(
            clap::Arg::with_name("host")
                .short("h")
                .long("host")
                .value_name("HOST")
                .help("Which host the service to listen.")
                .takes_value(true),
        )
        .arg(
            clap::Arg::with_name("port")
                .short("p")
                .long("port")
                .value_name("PORT")
                .help("Which port the service to listen.")
                .takes_value(true),
        )
        .arg(
            clap::Arg::with_name("num-thread")
                .long("num-thread")
                .value_name("COUNT")
                .help("How many thread will spawn.")
                .takes_value(true),
        )
        .arg(
            clap::Arg::with_name("config-file")
                .long("config-file")
                .value_name("PATH")
                .help("Set config file.")
                .required(true)
                .takes_value(true),
        );

    let matches = clap_app.get_matches();

    let host = matches.value_of("host").unwrap_or("0.0.0.0");
    let port: u16 = matches
        .value_of("port")
        .unwrap_or("53")
        .parse()
        .expect("Must be a number between 1 ~ 65535.");
    let thread_num: usize = matches.value_of("num-thread")
        .map(|x| x.parse().expect("Must be a number."))
        .unwrap_or_else(|| num_cpus::get() - 1);

    let config_file = matches.value_of("config-file").unwrap_or("");

    let socket = UdpSocket::bind((host, port)).unwrap();

    let mut file = File::open(config_file).unwrap();
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();
    let config: Config = toml::from_str(&content).unwrap();

    println!("Load config: {:#?}\n", &config);

    let matcher = AddressMatcher::new_from_config(&config).unwrap();
    let matcher = Arc::new(matcher);
    let mut index_loop = IndexLoop::new(thread_num, 0);
    let mut senders = Vec::new();

    for _ in 0..thread_num {
        let matcher = matcher.clone();
        let socket = socket.try_clone().unwrap();
        let (sender, receiver) = channel::<(SocketAddr, BytePacketBuffer)>();
        senders.push(sender);

        thread::spawn(move || {
            loop {
                let (src, mut req_buffer) = receiver.recv().unwrap();

                let request = match DnsPacket::from_buffer(&mut req_buffer) {
                    Ok(x) => x,
                    Err(e) => {
                        println!("Failed to parse UDP query packet: {:?}", e);
                        continue;
                    }
                };

                let mut packet = DnsPacket::new();
                packet.header.id = request.header.id;
                packet.header.recursion_desired = true;
                packet.header.recursion_available = true;
                packet.header.response = true;

                if request.questions.is_empty() {
                    packet.header.rescode = ResultCode::FORMERR;
                } else {
                    let question = &request.questions[0];
                    println!("Received query: {:?}", question);

                    //            if let Ok(result) = lookup(&question.name, question.qtype, server) {
                    if let Ok(result) = lookup_regexp_hack(&question.name, question.qtype, &matcher) {
                        packet.questions.push(question.clone());
                        packet.header.rescode = result.header.rescode;
                        println!("Rescode: {:?}", &packet.header.rescode);

                        for rec in result.answers {
                            println!("Answer: {:?}", rec);
                            packet.answers.push(rec);
                        }
                        for rec in result.authorities {
                            println!("Authority: {:?}", rec);
                            packet.authorities.push(rec);
                        }
                        for rec in result.resources {
                            println!("Resource: {:?}", rec);
                            packet.resources.push(rec);
                        }
                    } else {
                        packet.header.rescode = ResultCode::SERVFAIL;
                    }
                }

                let mut res_buffer = BytePacketBuffer::new();
                match packet.write(&mut res_buffer) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("Failed to encode UDP response packet: {:?}", e);
                        continue;
                    }
                };

                let len = res_buffer.pos();
                let data = match res_buffer.get_range(0, len) {
                    Ok(x) => x,
                    Err(e) => {
                        println!("Failed to retrieve response buffer: {:?}", e);
                        continue;
                    }
                };

                match socket.send_to(data, src) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("Failed to send response buffer: {:?}", e);
                        continue;
                    }
                };
            }
        });
    }

    loop {
        let mut req_buffer = BytePacketBuffer::new();
        let (_, src) = match socket.recv_from(&mut req_buffer.buf) {
            Ok(x) => x,
            Err(e) => {
                println!("Failed to read from UDP socket: {:?}", e);
                continue;
            }
        };

        let index = index_loop.next();
        match senders.get(index as usize) {
            Some(sender) =>
                match sender.send((src, req_buffer)) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("Failed to send package to channel: {:?}", e);
                        continue;
                    }
                },
            None => {}
        };
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_index_loop() {
        let mut index_loop = IndexLoop::new(3, 0);
        assert_eq!(index_loop.next(), 0);
        assert_eq!(index_loop.next(), 1);
        assert_eq!(index_loop.next(), 2);
        assert_eq!(index_loop.next(), 0);
        assert_eq!(index_loop.next(), 1);
        assert_eq!(index_loop.next(), 2);
        assert_eq!(index_loop.next(), 0);
    }
}
