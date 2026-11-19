use super::link::Link;
use bytes::Bytes;
use http_body_util::Empty;
use hyper::{Method, Request, Response, Uri, body::Incoming, header};
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use rustls::ClientConfig;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use socket2::{Domain, Protocol, Socket, Type};
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::{TcpSocket, TcpStream, lookup_host};

pub type HttpClient = Client<HttpsConnector<BoundConnector>, Empty<Bytes>>;

#[derive(Clone)]
pub struct BoundConnector {
    link: Arc<Link>,
}

impl tower_service::Service<Uri> for BoundConnector {
    type Response = TokioIo<TcpStream>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let link = self.link.clone();
        Box::pin(async move {
            let host = dst
                .host()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no host"))?
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let port = dst
                .port_u16()
                .unwrap_or(if dst.scheme_str() == Some("https") { 443 } else { 80 });

            let addrs: Vec<SocketAddr> = lookup_host((host.as_str(), port))
                .await?
                .filter(|a| a.is_ipv4())
                .collect();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no IPv4 address for host",
                ));
            }
            let mut last = io::Error::other("connect failed");
            for addr in addrs.into_iter().take(3) {
                match connect_bound(&link, addr).await {
                    Ok(s) => return Ok(TokioIo::new(s)),
                    Err(e) => last = e,
                }
            }
            Err(last)
        })
    }
}

async fn connect_bound(link: &Link, addr: SocketAddr) -> io::Result<TcpStream> {
    let net = link.net();
    let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_nonblocking(true)?;
    let _ = sock.set_tcp_nodelay(true);
    #[cfg(windows)]
    if net.if_index != 0 {
        set_unicast_if(&sock, net.if_index)?;
    }
    sock.bind(&SocketAddr::new(IpAddr::V4(net.ip), 0).into())?;
    let sock = TcpSocket::from_std_stream(sock.into());
    tokio::time::timeout(Duration::from_secs(12), sock.connect(addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))?
}

#[cfg(windows)]
fn set_unicast_if(sock: &Socket, if_index: u32) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{IP_UNICAST_IF, IPPROTO_IP, SOCKET, setsockopt};
    let val: u32 = if_index.to_be();
    let r = unsafe {
        setsockopt(
            sock.as_raw_socket() as SOCKET,
            IPPROTO_IP as i32,
            IP_UNICAST_IF as i32,
            &val as *const u32 as *const u8,
            4,
        )
    };
    if r != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn tls_config() -> ClientConfig {
    static TLS: OnceLock<ClientConfig> = OnceLock::new();
    TLS.get_or_init(|| {
        let builder = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("tls protocol versions");
        let builder = builder.with_native_roots().unwrap_or_else(|_| {
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("tls protocol versions")
                .with_root_certificates(rustls::RootCertStore::empty())
        });
        builder.with_no_client_auth()
    })
    .clone()
}

pub fn make_client(link: Arc<Link>) -> HttpClient {
    let https = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config())
        .https_or_http()
        .enable_http1()
        .wrap_connector(BoundConnector { link });
    Client::builder(TokioExecutor::new())
        .pool_max_idle_per_host(1)
        .pool_idle_timeout(Duration::from_secs(20))
        .build(https)
}

const UA: &str = concat!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) LetsDownload/", env!("CARGO_PKG_VERSION"));

pub struct Fetched {
    pub resp: Response<Incoming>,
    pub final_url: String,
}

pub async fn get(
    client: &HttpClient,
    url: &str,
    range: Option<(u64, u64)>,
    if_range: Option<&str>,
) -> anyhow::Result<Fetched> {
    let mut cur = url.to_string();
    for _ in 0..8 {
        let uri: Uri = cur.parse()?;
        let mut rb = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::USER_AGENT, UA)
            .header(header::ACCEPT, "*/*")
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some((s, e)) = range {
            rb = rb.header(header::RANGE, format!("bytes={s}-{e}"));
            if let Some(v) = if_range {
                rb = rb.header(header::IF_RANGE, v);
            }
        }
        let resp = tokio::time::timeout(
            Duration::from_secs(30),
            client.request(rb.body(Empty::<Bytes>::new())?),
        )
        .await
        .map_err(|_| anyhow::anyhow!("sunucu yanıt vermedi (zaman aşımı)"))??;
        if resp.status().is_redirection() {
            if let Some(loc) = resp.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()) {
                let base = url::Url::parse(&cur)?;
                cur = base.join(loc)?.to_string();
                continue;
            }
        }
        return Ok(Fetched { resp, final_url: cur });
    }
    anyhow::bail!("çok fazla yönlendirme")
}
