use std::{net::Ipv4Addr, path::Path, str::FromStr, time::Duration};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::process::{output_text, require_success, CommandExecutor, CommandSpec};

const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
const TALOS_EDGE_NETWORK_NAME: &str = "talos-community_talos_edge";

pub fn validate_docker_network_availability(
    executor: &dyn CommandExecutor,
    docker: &Path,
    requested_subnet: &str,
) -> Result<()> {
    let requested = ipv4_range(requested_subnet)?;
    let list = executor.execute(
        &CommandSpec::new(docker).args(["network", "ls", "--quiet"]),
        NETWORK_TIMEOUT,
    )?;
    require_success("Docker network inventory", &list, &[])?;
    for id in output_text(&list.stdout)
        .lines()
        .filter(|line| !line.is_empty())
    {
        if id.len() > 64 || id.len() < 12 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Docker returned a malformed network identifier");
        }
        let inspect = executor.execute(
            &CommandSpec::new(docker).args(["network", "inspect", id, "--format", "{{json .}}"]),
            NETWORK_TIMEOUT,
        )?;
        require_success("Docker network inspection", &inspect, &[])?;
        let document: Value = serde_json::from_str(&output_text(&inspect.stdout))
            .context("Docker returned malformed network metadata")?;
        let name = document
            .get("Name")
            .and_then(Value::as_str)
            .context("Docker network metadata has no name")?;
        if name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            bail!("Docker network metadata contains an unsafe name");
        }
        let subnets = document
            .get("IPAM")
            .and_then(|ipam| ipam.get("Config"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|config| config.get("Subnet").and_then(Value::as_str));
        for existing_subnet in subnets {
            let Ok(existing) = ipv4_range(existing_subnet) else {
                continue;
            };
            if ranges_overlap(requested, existing)
                && !(name == TALOS_EDGE_NETWORK_NAME && requested == existing)
            {
                bail!(
                    "requested Talos edge subnet overlaps existing Docker network {name} ({existing_subnet}); select a non-overlapping subnet and proxy address"
                );
            }
        }
    }
    Ok(())
}

fn ipv4_range(value: &str) -> Result<(u32, u32)> {
    let (address, prefix) = value
        .split_once('/')
        .context("IPv4 network must use CIDR notation")?;
    let address = u32::from(Ipv4Addr::from_str(address).context("invalid IPv4 network")?);
    let prefix = prefix.parse::<u8>().context("invalid IPv4 prefix")?;
    if prefix > 32 {
        bail!("invalid IPv4 prefix");
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let start = address & mask;
    Ok((start, start | !mask))
}

fn ranges_overlap(left: (u32, u32), right: (u32, u32)) -> bool {
    left.0 <= right.1 && right.0 <= left.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_contained_partial_and_disjoint_ipv4_ranges() {
        let requested = ipv4_range("172.31.240.0/24").expect("requested");
        assert!(ranges_overlap(
            requested,
            ipv4_range("172.31.0.0/16").expect("parent")
        ));
        assert!(ranges_overlap(
            requested,
            ipv4_range("172.31.240.128/25").expect("child")
        ));
        assert!(!ranges_overlap(
            requested,
            ipv4_range("172.31.241.0/24").expect("disjoint")
        ));
    }
}
