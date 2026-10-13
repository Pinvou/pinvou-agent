use crate::FileHttpError;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;

pub(crate) const MAX_ADDRESSES: usize = 4096;

/// No path/query is exposed to approval callbacks or diagnostic formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct FileOrigin {
    host: String,
    port: u16,
}
impl FileOrigin {
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// The owner supplies an immutable, current approval snapshot. There is no
/// default allow rule; the module establishes no publication/authorization fact.
pub trait DestinationPolicy: Send + Sync {
    fn permits_origin(&self, origin: &FileOrigin) -> bool;
    fn permits_address(&self, origin: &FileOrigin, address: SocketAddr) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Failed,
    AboveLimit,
}

pub trait AddressResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        origin: &'a FileOrigin,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SocketAddr>, ResolveError>> + Send + 'a>>;
}

pub struct SystemResolver;
impl AddressResolver for SystemResolver {
    fn resolve<'a>(
        &'a self,
        origin: &'a FileOrigin,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SocketAddr>, ResolveError>> + Send + 'a>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((origin.host(), origin.port()))
                .await
                .map_err(|_| ResolveError::Failed)?;
            let addresses: Vec<_> = addresses.take(MAX_ADDRESSES + 1).collect();
            if addresses.len() > MAX_ADDRESSES {
                return Err(ResolveError::AboveLimit);
            }
            Ok(addresses)
        })
    }
}

pub(crate) struct FileDestination {
    pub(crate) url: reqwest::Url,
    pub(crate) origin: FileOrigin,
    pub(crate) literal: Option<IpAddr>,
}

impl FileDestination {
    pub(crate) fn parse(raw: &str) -> Result<Self, FileHttpError> {
        if raw.len() > 65_536
            || raw.chars().count() > 8192
            || !raw.starts_with("https://")
            || raw.bytes().any(|byte| byte <= 0x20 || byte == 0x7f)
            || raw.contains(['#', '\\'])
        {
            return Err(FileHttpError::InvalidUrl);
        }
        let authority = raw[8..]
            .split(['/', '?'])
            .next()
            .ok_or(FileHttpError::InvalidUrl)?;
        if authority.is_empty() || authority.contains('@') {
            return Err(FileHttpError::InvalidUrl);
        }
        let url = reqwest::Url::parse(raw).map_err(|_| FileHttpError::InvalidUrl)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(FileHttpError::InvalidUrl);
        }
        let host = url.host_str().ok_or(FileHttpError::InvalidUrl)?;
        let unbracketed = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        let literal = unbracketed.parse::<IpAddr>().ok();
        let host = literal.map_or_else(|| host.to_string(), |ip| ip.to_string());
        let port = url
            .port_or_known_default()
            .filter(|port| *port != 0)
            .ok_or(FileHttpError::InvalidUrl)?;
        Ok(Self {
            url,
            origin: FileOrigin { host, port },
            literal,
        })
    }
}

pub(crate) struct DenyFallbackResolver;
impl reqwest::dns::Resolve for DenyFallbackResolver {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async {
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "FILE_DNS_UNAPPROVED",
            )) as Box<dyn std::error::Error + Send + Sync>)
        })
    }
}
