// Generated from talos_protocol. Do not edit by hand.
// Regenerate with: cargo run --locked -p talos_protocol --features typescript --bin export-typescript

export type AgentPlatform = "windows" | "linux" | "macos" | "unknown";

export type AgentFeatureCapabilities = { remoteDesktop: boolean, systemShell: boolean, fileTransfer: boolean, remoteRegistry: boolean, chat: boolean, systemInfo: boolean, };

export type ReflexAddress = { ip: string, port: number, };

export type LocalAddr = { ip: string, prefix: number, };

export type RemoteDesktopDisplayProfile = { id: string, protocol: string, codec: string, compression: string, priority: number, };

export type SessionCapabilitiesHttpResponse = { codecs: Array<string>, encoding: string, transports: Array<string>, platform: AgentPlatform, features: AgentFeatureCapabilities, displayProfiles: Array<RemoteDesktopDisplayProfile>, selectedDisplayProfile?: string | null, agentReflex: ReflexAddress | null, agentHost: string | null, agentLocalAddrs: Array<LocalAddr>, pskCertPem: string, relayUrl: string | null, e2eKey: string | null, agentHostname?: string | null, agentOs?: string | null, agentVersion?: string | null, };

export type ShellSessionCapabilitiesHttpResponse = { transports: Array<string>, platform: AgentPlatform, features: AgentFeatureCapabilities, agentReflex?: ReflexAddress | null, agentHost?: string | null, agentLocalAddrs: Array<LocalAddr>, pskCertPem?: string | null, relayUrl?: string | null, e2eKey?: string | null, };

export type FileTransferSessionCapabilitiesHttpResponse = { transports: Array<string>, platform: AgentPlatform, features: AgentFeatureCapabilities, agentReflex: ReflexAddress | null, agentHost: string | null, agentLocalAddrs: Array<LocalAddr>, pskCertPem: string, relayUrl: string | null, e2eKey: string | null, zipThresholdFiles: number, zipThresholdBytes: number, maxChunkBytes: number, };

export type RegistrySessionCapabilitiesHttpResponse = { transports: Array<string>, platform: AgentPlatform, features: AgentFeatureCapabilities, agentReflex: ReflexAddress | null, agentHost: string | null, agentLocalAddrs: Array<LocalAddr>, pskCertPem: string, relayUrl: string | null, e2eKey: string | null, };

export type ChatSessionCapabilitiesHttpResponse = { transports: Array<string>, platform: AgentPlatform, features: AgentFeatureCapabilities, agentReflex: ReflexAddress | null, agentHost: string | null, agentLocalAddrs: Array<LocalAddr>, pskCertPem: string, relayUrl: string | null, e2eKey: string | null, };
