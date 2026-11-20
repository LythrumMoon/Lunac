//! UoT（UDP over TCP）v2 的 **connect 模式**编解码 —— AnyTLS 这类只能跑 TCP 的载体用它转发
//! UDP（重建 v1 `native_proxy/anytls_udp.rs` 的做法）。
//!
//! 协议依据（2026-11-15 核对 sing 的 `common/uot/protocol.go` / `conn.go` / `client.go` 与
//! <https://sing-box.sagernet.org/configuration/shared/udp-over-tcp/>）：
//!   1. 先按上层代理协议连到魔法域名 [`MAGIC_ADDRESS`]（`sp.v2.udp-over-tcp.arpa`，端口 0）；
//!   2. 写请求：`isConnect u8=1 | ATYP addr | port u16 BE`（**SOCKS 地址格式**：
//!      1=IPv4 / 3=域名 / 4=IPv6）；`isConnect=0`（多目标）本引擎不用 —— 会话本就按目标建；
//!   3. 之后每个数据报：`length u16 BE | payload`（connect 模式不带地址）。
//!
//! 注意：非 connect 模式的**包内地址**用的是另一套类型码（0x00=IPv4 / 0x01=IPv6 /
//! 0x02=域名，见 sing 的 `AddrParser`），不要与请求里的 SOCKS 编码混用。

use std::io::{self, Read, Write};

use super::transport::BoxStream;

/// UoT v2 的魔法域名（`RequestDestination(2)`）。
pub const MAGIC_ADDRESS: &str = "sp.v2.udp-over-tcp.arpa";

/// 一条 UoT（connect 模式）会话：目标固定，数据报按 `[len u16 BE][payload]` 收发。
pub struct UotStream {
    inner: BoxStream,
    pending: Vec<u8>,
}

impl UotStream {
    /// 写 UoT 请求（`isConnect=1` + 目标），返回会话。
    pub fn connect(mut stream: BoxStream, host: &str, port: u16) -> Result<Self, String> {
        let mut request = Vec::with_capacity(24);
        request.push(0x01); // isConnect
        request.extend_from_slice(&super::encode_target(host, port));
        stream
            .write_all(&request)
            .map_err(|e| format!("UoT 请求发送失败：{e}"))?;
        Ok(Self {
            inner: stream,
            pending: Vec::new(),
        })
    }

    fn pump(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 8192];
        match self.inner.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "UoT 流被对端关闭",
            )),
            Ok(read) => {
                self.pending.extend_from_slice(&chunk[..read]);
                Ok(true)
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// 发一个数据报：`[length u16 BE][payload]`。
    pub fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "UoT 单包超过 65535 字节",
            ));
        }
        let mut frame = Vec::with_capacity(2 + payload.len());
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(payload);
        self.inner.write_all(&frame)
    }

    /// 收一个数据报；`Ok(None)` = 暂时没有。
    pub fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if self.pending.len() >= 2 {
                let length = u16::from_be_bytes([self.pending[0], self.pending[1]]) as usize;
                if self.pending.len() >= 2 + length {
                    self.pending.drain(..2);
                    let payload: Vec<u8> = self.pending.drain(..length).collect();
                    return Ok(Some(payload));
                }
            }
            if !self.pump()? {
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for Duplex {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.input.read(out)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl super::super::transport::Stream for Duplex {}

    /// 请求 = `isConnect=1` + SOCKS 地址；数据报 = `[len u16][payload]`。
    #[test]
    fn connect_request_and_datagram_framing() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut response = 5u16.to_be_bytes().to_vec();
        response.extend_from_slice(b"reply");
        let stream: BoxStream = Box::new(Duplex {
            input: Cursor::new(response),
            output: written.clone(),
        });
        let mut session = UotStream::connect(stream, "dns.example.com", 53).unwrap();
        session.send(b"query").unwrap();

        let bytes = written.lock().unwrap().clone();
        assert_eq!(bytes[0], 0x01, "isConnect");
        assert_eq!(bytes[1], 0x03, "SOCKS 的域名类型码是 3");
        assert_eq!(bytes[2] as usize, "dns.example.com".len());
        assert_eq!(&bytes[3..18], b"dns.example.com");
        assert_eq!(&bytes[18..20], &53u16.to_be_bytes());
        assert_eq!(&bytes[20..27], b"\x00\x05query");

        assert_eq!(session.recv().unwrap().unwrap(), b"reply");
    }

    /// `isConnect` 之后必须是 SOCKS 地址（不是 UoT 包内那套 0x00/0x01/0x02）。
    #[test]
    fn request_uses_socks_address_types() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let stream: BoxStream = Box::new(Duplex {
            input: Cursor::new(Vec::new()),
            output: written.clone(),
        });
        let _ = UotStream::connect(stream, "1.2.3.4", 80).unwrap();
        let bytes = written.lock().unwrap().clone();
        assert_eq!(bytes[0], 0x01);
        assert_eq!(bytes[1], 0x01, "IPv4 是 1");
        assert_eq!(&bytes[2..6], &[1, 2, 3, 4]);
        assert_eq!(&bytes[6..8], &80u16.to_be_bytes());
    }

    /// 半包不能当空数据报返回（读超时 ⇒ `None`，不是空包）。
    #[test]
    fn partial_datagram_returns_none() {
        struct Stalled {
            data: Cursor<Vec<u8>>,
            stalled: bool,
        }

        impl Read for Stalled {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                if self.stalled {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "stall"));
                }
                let read = self.data.read(out)?;
                if read == 0 {
                    self.stalled = true;
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "stall"));
                }
                Ok(read)
            }
        }

        impl Write for Stalled {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        impl super::super::transport::Stream for Stalled {}

        let stream: BoxStream = Box::new(Stalled {
            data: Cursor::new(vec![0u8, 9, b'a']),
            stalled: false,
        });
        let mut session = UotStream::connect(stream, "a.test", 53).unwrap();
        assert!(session.recv().unwrap().is_none());
    }
}
