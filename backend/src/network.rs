use std::time::Duration;

/// Source traffic can use a physical interface independently of the model API.
/// An explicitly configured interface must fail closed when unavailable.
pub fn source_client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent("RealtyAggregator/0.1");
    match std::env::var("SOURCE_NETWORK_INTERFACE")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(interface) => bind_interface(builder.no_proxy(), &interface),
        None => builder,
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "fuchsia"))]
fn bind_interface(builder: reqwest::ClientBuilder, interface: &str) -> reqwest::ClientBuilder {
    builder.interface(interface)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "fuchsia")))]
fn bind_interface(_: reqwest::ClientBuilder, _: &str) -> reqwest::ClientBuilder {
    panic!("SOURCE_NETWORK_INTERFACE requires a platform supporting interface binding")
}
