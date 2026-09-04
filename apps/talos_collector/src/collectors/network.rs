use crate::collectors::Collector;
use crate::models::{
    ConnectionSummary, IpConfig, NetworkAdapterConfig, NetworkInfo, NetworkShare, ProxyConfig,
    RouteEntry,
};
use crate::windows_utils::{registry::RegistryHelper, wmi::WmiHelper};
use anyhow::Result;
use async_trait::async_trait;
use local_ip_address::list_afinet_netifas;
use serde_json::{json, Value};
use tracing::debug;

pub struct NetworkCollector;

#[async_trait]
impl Collector for NetworkCollector {
    fn name(&self) -> &'static str {
        "Network"
    }

    fn data_type(&self) -> &'static str {
        "network"
    }

    fn estimated_duration_ms(&self) -> u64 {
        2000
    }

    fn requires_admin(&self) -> bool {
        false
    }

    async fn collect(&self) -> Result<Value> {
        debug!("Starting Network collection");

        let adapters = self.collect_adapters().await?;
        let routes = self.collect_routes().await?;
        let connections = self.collect_connections().await?;
        let shares = self.collect_shares().await?;
        let proxy = self.collect_proxy().await?;
        let dns_cache_entries = self.collect_dns_cache_entries().await;
        let firewall_rules_count = self.collect_firewall_rules_count().await;

        let network = NetworkInfo {
            adapters,
            routing_table: routes,
            dns_cache_entries,
            active_connections: connections,
            shares,
            proxy,
            firewall_rules_count,
            todo_data_collection: Vec::new(),
        };

        debug!("Network collection completed");

        Ok(json!(network))
    }
}

impl NetworkCollector {
    async fn collect_dns_cache_entries(&self) -> u32 {
        WmiHelper::get_dns_cache()
            .await
            .map(|entries| entries.len() as u32)
            .unwrap_or(0)
    }

    async fn collect_firewall_rules_count(&self) -> Option<u32> {
        WmiHelper::get_firewall_rules_standard_cim()
            .await
            .ok()
            .map(|rules| rules.len() as u32)
    }

    async fn collect_adapters(&self) -> Result<Vec<NetworkAdapterConfig>> {
        let mut adapters = Vec::new();

        let wmi_adapters = WmiHelper::get_network_adapter_config().await?;
        let wmi_nics = WmiHelper::get_network_adapters().await?;

        let _local_ips = list_afinet_netifas().unwrap_or_default();

        for adapter in wmi_adapters.iter() {
            let index = adapter.get("Index").and_then(|v| v.as_u64()).unwrap_or(0);

            // Find matching physical adapter
            let nic_info = wmi_nics.iter().find(|n| {
                n.get("Index")
                    .and_then(|v| v.as_u64())
                    .map(|i| i == index)
                    .unwrap_or(false)
            });

            let description = adapter
                .get("Description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    nic_info
                        .and_then(|n| n.get("Name").and_then(|v| v.as_str()))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                });

            let mac_address = nic_info
                .and_then(|n| n.get("MACAddress").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .unwrap_or_default();

            // Build IP configs
            let mut ips = Vec::new();

            if let Some(ip_addresses) = adapter.get("IPAddress").and_then(|v| v.as_array()) {
                if let Some(subnets) = adapter.get("IPSubnet").and_then(|v| v.as_array()) {
                    for (i, ip) in ip_addresses.iter().enumerate() {
                        if let Some(ip_str) = ip.as_str() {
                            let prefix = subnets
                                .get(i)
                                .and_then(|s| s.as_str())
                                .and_then(|s| s.parse::<u8>().ok())
                                .unwrap_or(if ip_str.contains(':') { 64 } else { 24 });

                            let is_dhcp = adapter
                                .get("DHCPEnabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            ips.push(IpConfig {
                                address: ip_str.to_string(),
                                family: if ip_str.contains(':') {
                                    "IPv6".to_string()
                                } else {
                                    "IPv4".to_string()
                                },
                                prefix,
                                is_dhcp,
                                dhcp_server: adapter
                                    .get("DHCPServer")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                lease_obtained: adapter
                                    .get("DHCPLeaseObtained")
                                    .and_then(|v| v.as_str())
                                    .and_then(WmiHelper::parse_wmi_datetime_str),
                                lease_expires: adapter
                                    .get("DHCPLeaseExpires")
                                    .and_then(|v| v.as_str())
                                    .and_then(WmiHelper::parse_wmi_datetime_str),
                            });
                        }
                    }
                }
            }

            // Get gateways
            let gateways: Vec<String> = adapter
                .get("DefaultIPGateway")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            // Get DNS servers
            let dns_servers: Vec<String> = adapter
                .get("DNSServerSearchOrder")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let status = nic_info
                .and_then(|n| n.get("NetConnectionStatus").and_then(|v| v.as_u64()))
                .map(|s| {
                    match s {
                        0 => "Disconnected",
                        1 => "Connecting",
                        2 => "Connected",
                        3 => "Disconnecting",
                        4 => "Hardware not present",
                        5 => "Hardware disabled",
                        6 => "Hardware malfunction",
                        7 => "Media disconnected",
                        8 => "Authenticating",
                        9 => "Authentication succeeded",
                        10 => "Authentication failed",
                        11 => "Invalid address",
                        12 => "Credentials required",
                        _ => "Unknown",
                    }
                    .to_string()
                })
                .unwrap_or_else(|| "Unknown".to_string());

            adapters.push(NetworkAdapterConfig {
                name: description.clone(),
                description,
                mac_address,
                ips,
                gateways,
                dns_servers,
                dns_suffix: adapter
                    .get("DNSDomainSuffixSearchOrder")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                status,
                speed_mbps: nic_info
                    .and_then(|n| n.get("Speed").and_then(|v| v.as_u64()))
                    .map(|s| s / 1_000_000),
                mtu: adapter
                    .get("MTU")
                    .and_then(|v| v.as_u64())
                    .map(|m| m as u32),
            });
        }

        Ok(adapters)
    }

    async fn collect_routes(&self) -> Result<Vec<RouteEntry>> {
        let mut routes = Vec::new();

        let wmi_routes = WmiHelper::get_ip4_route_table().await?;

        for route in wmi_routes {
            let destination = route
                .get("Destination")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let mask = route
                .get("Mask")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            // Skip loopback and multicast routes
            if destination.starts_with("127.") || destination.starts_with("224.") {
                continue;
            }

            routes.push(RouteEntry {
                destination,
                mask,
                gateway: route
                    .get("NextHop")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                interface: route
                    .get("InterfaceIndex")
                    .and_then(|v| v.as_u64())
                    .map(|i| i.to_string())
                    .unwrap_or_default(),
                metric: route.get("Metric1").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                is_persistent: route
                    .get("Type")
                    .and_then(|v| v.as_u64())
                    .map(|t| t == 4) // 4 = Persistent
                    .unwrap_or(false),
            });
        }

        // Sort by metric
        routes.sort_by_key(|r| r.metric);

        Ok(routes)
    }

    async fn collect_connections(&self) -> Result<ConnectionSummary> {
        let mut summary = ConnectionSummary::default();

        use crate::windows_utils::winapi_helpers::{get_tcp_connections, get_udp_endpoints};

        if let Ok(tcp_conns) = get_tcp_connections() {
            for conn in tcp_conns {
                match conn.state.as_str() {
                    "Established" => summary.tcp_established += 1,
                    "TimeWait" => summary.tcp_time_wait += 1,
                    "CloseWait" => summary.tcp_close_wait += 1,
                    _ => summary.tcp_other += 1,
                }
            }
        }

        if let Ok(udp_eps) = get_udp_endpoints() {
            summary.udp_listeners = udp_eps.len() as u32
        }

        Ok(summary)
    }

    async fn collect_shares(&self) -> Result<Vec<NetworkShare>> {
        let mut shares = Vec::new();

        let wmi_shares = WmiHelper::get_shares().await?;

        for share in wmi_shares {
            let share_type = share
                .get("Type")
                .and_then(|v| v.as_u64())
                .map(|t| {
                    match t {
                        0 => "Disk Drive",
                        1 => "Print Queue",
                        2 => "Device",
                        3 => "IPC",
                        2147483648 => "Disk Drive Admin",
                        2147483649 => "Print Queue Admin",
                        2147483650 => "Device Admin",
                        2147483651 => "IPC Admin",
                        _ => "Unknown",
                    }
                    .to_string()
                })
                .unwrap_or_else(|| "Unknown".to_string());

            shares.push(NetworkShare {
                name: share
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                path: share
                    .get("Path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                share_type,
                description: share
                    .get("Description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                connections: share
                    .get("MaximumAllowed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
            });
        }

        Ok(shares)
    }

    async fn collect_proxy(&self) -> Result<ProxyConfig> {
        let mut proxy = ProxyConfig::default();

        if let Ok(Some(auto_detect)) = RegistryHelper::read_dword(
            "HKCU",
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "AutoDetect",
        ) {
            proxy.auto_detect = auto_detect != 0;
        }

        if let Ok(Some(proxy_server)) = RegistryHelper::read_string(
            "HKCU",
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "ProxyServer",
        ) {
            proxy.enabled = !proxy_server.is_empty();
            proxy.proxy_server = Some(proxy_server);
        }

        if let Ok(Some(pac_url)) = RegistryHelper::read_string(
            "HKCU",
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "AutoConfigURL",
        ) {
            proxy.pac_url = Some(pac_url);
            proxy.auto_detect = true;
        }

        if let Ok(Some(bypass)) = RegistryHelper::read_string(
            "HKCU",
            r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "ProxyOverride",
        ) {
            proxy.bypass_list = bypass.split(';').map(|s| s.to_string()).collect();
        }

        Ok(proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_collector_name() {
        let collector = NetworkCollector;
        assert_eq!(collector.name(), "Network");
    }
}
