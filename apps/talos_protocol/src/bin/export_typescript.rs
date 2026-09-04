use std::{env, error::Error, fs, path::PathBuf};

use talos_protocol::{
    AgentFeatureCapabilities, AgentPlatform, ChatSessionCapabilitiesHttpResponse,
    FileTransferSessionCapabilitiesHttpResponse, LocalAddr, ReflexAddress,
    RegistrySessionCapabilitiesHttpResponse, RemoteDesktopDisplayProfile,
    SessionCapabilitiesHttpResponse, ShellSessionCapabilitiesHttpResponse,
};
use ts_rs::{Config, TS};

fn exported<T: TS>(config: &Config) -> String {
    format!("export {}", T::decl(config))
}

fn generated_contents() -> String {
    let config = Config::default();
    let declarations = [
        exported::<AgentPlatform>(&config),
        exported::<AgentFeatureCapabilities>(&config),
        exported::<ReflexAddress>(&config),
        exported::<LocalAddr>(&config),
        exported::<RemoteDesktopDisplayProfile>(&config),
        exported::<SessionCapabilitiesHttpResponse>(&config),
        exported::<ShellSessionCapabilitiesHttpResponse>(&config),
        exported::<FileTransferSessionCapabilitiesHttpResponse>(&config),
        exported::<RegistrySessionCapabilitiesHttpResponse>(&config),
        exported::<ChatSessionCapabilitiesHttpResponse>(&config),
    ];

    format!(
        "// Generated from talos_protocol. Do not edit by hand.\n\
         // Regenerate with: cargo run --locked -p talos_protocol --features typescript --bin export-typescript\n\n\
         {}\n",
        declarations.join("\n\n")
    )
}

fn output_path() -> Result<PathBuf, Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    Ok(manifest_dir
        .parent()
        .ok_or("talos_protocol must be inside the apps workspace")?
        .join("talos_protocol_types")
        .join("src")
        .join("generated.ts"))
}

fn main() -> Result<(), Box<dyn Error>> {
    let expected = generated_contents();
    let output = output_path()?;
    let check_only = env::args().skip(1).any(|arg| arg == "--check");

    if check_only {
        let actual = fs::read_to_string(&output).map_err(|error| {
            format!(
                "generated contract {} is missing or unreadable: {error}",
                output.display()
            )
        })?;
        if actual != expected {
            return Err(format!(
                "generated contract {} is stale; run the export-typescript command",
                output.display()
            )
            .into());
        }
        println!("Generated TypeScript protocol contracts are current.");
        return Ok(());
    }

    let parent = output
        .parent()
        .ok_or("generated contract output must have a parent directory")?;
    fs::create_dir_all(parent)?;
    let temporary = output.with_extension("ts.tmp");
    fs::write(&temporary, expected)?;
    fs::rename(&temporary, &output)?;
    println!("Wrote {}", output.display());
    Ok(())
}
