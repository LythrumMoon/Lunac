// ── Lunac 媒体代理 helper（独立进程，2026-10-20）─────────────────────
//
// 为什么单独一个进程：`lunac.exe` 亲自用 reqwest 去取任意远程图片时，
// Windows Defender 的 LummaStealer 行为模型会把宿主判定为「任意 URL 取回代理」
// 并终止进程（见 docs/testing.md §测试 12.1）。把取回动作挪到这个 helper 后，
// 宿主只与 127.0.0.1 通信，不再亲自扮演代理。
//
// 入口由宿主拉起：`lunac-media-proxy.exe --port <n> --token <t>`。
// 只接受 `GET|HEAD /fetch?token=…&url=<encoded>&r=<referer>&u=<ua>&c=<cookie>`；
// 私网 / 本机 / 链路本地目标一律拒绝（重定向逐跳复检）。
//
// 这个进程不是安全边界：token 只用于阻止同机其它进程随手借用，
// 真正的能力门仍在宿主 `plugin_media::handle`（清单 `media.proxy` + URL 校验）。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use reqwest::Url;

const WORKERS: usize = 6;
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECTS: usize = 3;
const MAX_MEDIA_BYTES: u64 = 32 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

fn validate_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("URL 不合法：{error}"))?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("只允许 http/https：{scheme}"));
    }
    let host = url.host_str().ok_or_else(|| "URL 缺少主机名".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("DNS 解析失败：{error}"))?;
    let mut any = false;
    for addr in addrs {
        any = true;
        if is_private_ip(addr.ip()) {
            return Err("目标地址解析到私网 / 本机，已拒绝".into());
        }
    }
    if !any {
        return Err("DNS 没有返回地址".into());
    }
    Ok(url)
}

fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    return attempt.stop();
                }
                match validate_url(attempt.url().as_str()) {
                    Ok(_) => attempt.follow(),
                    Err(error) => attempt.error(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        error,
                    )),
                }
            }))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "OK",
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    extra: &[(String, String)],
    body: &[u8],
) {
    let cache = if content_type.starts_with("image/") {
        "private, max-age=86400"
    } else {
        "private, max-age=300"
    };
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n",
        reason(status),
        body.len()
    );
    for (name, value) in extra {
        if name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn fail(stream: &mut TcpStream, status: u16, message: &str) {
    write_response(
        stream,
        status,
        "text/plain; charset=utf-8",
        &[],
        message.as_bytes(),
    );
}

fn safe_header(value: &str) -> bool {
    !value.contains('\r') && !value.contains('\n')
}

fn handle(mut stream: TcpStream, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let reader_stream = match stream.try_clone() {
        Ok(value) => value,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if !matches!(method, "GET" | "HEAD") {
        fail(&mut stream, 405, "method not allowed");
        return;
    }
    if !target.starts_with('/') {
        fail(&mut stream, 400, "bad request target");
        return;
    }

    let mut range: Option<String> = None;
    let mut header_bytes = request_line.len();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        header_bytes += line.len();
        if header_bytes > MAX_HEADER_BYTES {
            fail(&mut stream, 431, "headers too large");
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("range") {
                let value = value.trim();
                if safe_header(value) {
                    range = Some(value.to_string());
                }
            }
        }
    }

    let url = match Url::parse(&format!("http://127.0.0.1{target}")) {
        Ok(value) => value,
        Err(_) => {
            fail(&mut stream, 400, "bad request");
            return;
        }
    };
    if url.path() != "/fetch" {
        fail(&mut stream, 404, "not found");
        return;
    }
    let params: HashMap<String, String> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if params.get("token").map(String::as_str) != Some(token) {
        fail(&mut stream, 403, "bad token");
        return;
    }
    let Some(target_url) = params.get("url") else {
        fail(&mut stream, 400, "missing url");
        return;
    };
    let target_url = match validate_url(target_url) {
        Ok(value) => value,
        Err(error) => {
            fail(&mut stream, 403, &error);
            return;
        }
    };

    let client = client();
    let mut request = if method == "HEAD" {
        client.head(target_url)
    } else {
        client.get(target_url)
    };
    if let Some(range) = range {
        request = request.header(reqwest::header::RANGE, range);
    }
    if let Some(cookie) = params.get("c").filter(|value| safe_header(value)) {
        request = request.header(reqwest::header::COOKIE, cookie);
    }
    if let Some(referer) = params.get("r").filter(|value| safe_header(value)) {
        request = request.header(reqwest::header::REFERER, referer);
    }
    let user_agent = params
        .get("u")
        .filter(|value| safe_header(value))
        .map(String::as_str)
        .unwrap_or("Lunac-MediaProxy/1.0");
    request = request.header(reqwest::header::USER_AGENT, user_agent);

    let response = match request.send() {
        Ok(value) => value,
        Err(_) => {
            fail(&mut stream, 502, "upstream fetch failed");
            return;
        }
    };
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let mut extra: Vec<(String, String)> = Vec::new();
    for name in [
        reqwest::header::CONTENT_RANGE,
        reqwest::header::ACCEPT_RANGES,
        reqwest::header::CONTENT_LENGTH,
    ] {
        if let Some(value) = response.headers().get(&name).and_then(|value| value.to_str().ok()) {
            extra.push((name.as_str().to_string(), value.to_string()));
        }
    }

    if method == "HEAD" {
        write_response(&mut stream, status, &content_type, &extra, &[]);
        return;
    }

    let mut body: Vec<u8> = Vec::new();
    if response.take(MAX_MEDIA_BYTES).read_to_end(&mut body).is_err() {
        fail(&mut stream, 502, "upstream read failed");
        return;
    }
    let expected = extra
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<u64>().ok());
    if expected.map(|size| size > body.len() as u64).unwrap_or(false) {
        extra.push(("X-Lunac-Truncated".into(), "1".into()));
    }
    write_response(&mut stream, status, &content_type, &extra, &body);
}

fn main() {
    let mut port: u16 = 0;
    let mut token = String::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                if let Some(value) = args.next() {
                    port = value.parse().unwrap_or(0);
                }
            }
            "--token" => {
                if let Some(value) = args.next() {
                    token = value;
                }
            }
            _ => {}
        }
    }
    if port == 0 || token.is_empty() {
        eprintln!("usage: lunac-media-proxy --port <n> --token <t>");
        std::process::exit(2);
    }

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("bind 127.0.0.1:{port} failed: {error}");
            std::process::exit(3);
        }
    };
    println!("READY {port}");
    let _ = std::io::stdout().flush();

    let (tx, rx) = mpsc::channel::<TcpStream>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..WORKERS {
        let rx = Arc::clone(&rx);
        let token = token.clone();
        std::thread::Builder::new()
            .name("lunac-media-proxy".into())
            .spawn(move || loop {
                let job = match rx.lock() {
                    Ok(guard) => guard.recv(),
                    Err(_) => return,
                };
                match job {
                    Ok(stream) => handle(stream, &token),
                    Err(_) => return,
                }
            })
            .ok();
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if tx.send(stream).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
