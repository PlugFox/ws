use maxminddb::Reader;
use serde::Deserialize;
use std::net::IpAddr;
use std::sync::Arc;
use tracing::{debug, warn};

#[derive(Deserialize)]
struct CountryRecord {
    country: Option<CountryField>,
}

#[derive(Deserialize)]
struct CountryField {
    iso_code: Option<String>,
}

/// Service for resolving client IP addresses to ISO 3166-1 alpha-2 country codes.
#[derive(Clone)]
pub struct GeolocationService {
    reader: Option<Arc<Reader<Vec<u8>>>>,
}

impl GeolocationService {
    pub fn new(db_path: Option<&str>) -> Self {
        match db_path {
            Some(path) => match Reader::open_readfile(path) {
                Ok(reader) => {
                    debug!("GeoIP database loaded successfully from {}", path);
                    Self {
                        reader: Some(Arc::new(reader)),
                    }
                }
                Err(err) => {
                    warn!(
                        "Failed to open GeoIP database at '{}': {}. Geolocation will return 'ZZ' or None.",
                        path, err
                    );
                    Self { reader: None }
                }
            },
            None => {
                debug!("No GeoIP database path provided. Geolocation will operate in fallback mode.");
                Self { reader: None }
            }
        }
    }

    /// Checks whether an IP address belongs to private/internal/loopback ranges.
    pub fn is_private_or_local(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_unspecified()
            }
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_multicast(),
        }
    }

    /// Resolves an IP address to an ISO 3166-1 alpha-2 country code (e.g., "US", "DE", "ZZ").
    pub fn resolve_country(&self, ip: IpAddr) -> Option<String> {
        if Self::is_private_or_local(ip) {
            return Some("ZZ".to_string());
        }

        let reader = self.reader.as_ref()?;
        match reader.lookup::<CountryRecord>(ip) {
            Ok(record) => record.country.and_then(|c| c.iso_code),
            Err(err) => {
                debug!("GeoIP lookup error for {}: {}", ip, err);
                None
            }
        }
    }
}

impl Default for GeolocationService {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_private_and_loopback_ips() {
        let service = GeolocationService::default();

        let localhost_v4 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(service.resolve_country(localhost_v4), Some("ZZ".to_string()));

        let private_v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(service.resolve_country(private_v4), Some("ZZ".to_string()));

        let localhost_v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(service.resolve_country(localhost_v6), Some("ZZ".to_string()));
    }
}
