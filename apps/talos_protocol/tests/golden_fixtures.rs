use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

use talos_protocol::{
    build_control_frame, build_file_transfer_frame, build_shell_frame, parse_control_frame,
    parse_file_transfer_frame, AgentFeatureCapabilities, AgentHello, AgentPlatform,
    FileTransferConflictMode, FileTransferRequest, FileTransferResponse, FullSnapshotUpdate,
    IncomingEnvelope, LocalAddr, OperationErrorCode, OutgoingEnvelope, RegistryHive,
    RegistryRequest, RegistryValueData, RemoteDesktopCapabilities, RemoteDesktopDisplayProfile,
    RequestFullSnapshotPayload, SessionCapabilitiesResponse, SessionTransportMode,
    ShellCommandPayload, ShellOutputPayload, ShellRunAs, ShellStartPayload, TelemetryEventsUpdate,
    TunnelPreparePayload, CONTROL_PAYLOAD_STREAM_BITRATE_LEN, CONTROL_TYPE_STREAM_BITRATE,
    FILE_TRANSFER_MSG_JSON, REMOTE_DESKTOP_PROFILE_MODERN_GPU, SHELL_MSG_INPUT,
};

fn assert_json_fixture<T: Serialize>(actual: &T, fixture: &str) {
    let actual = serde_json::to_value(actual).expect("serialize current value");
    let expected: Value = serde_json::from_str(fixture).expect("parse fixture json");
    assert_eq!(actual, expected);
}

fn parse_envelope_payload<T: DeserializeOwned>(fixture: &str) -> (String, T) {
    let envelope: IncomingEnvelope = serde_json::from_str(fixture).expect("parse envelope");
    let payload = serde_json::from_value(envelope.data).expect("parse envelope payload");
    (envelope.message_type, payload)
}

fn hex_fixture(fixture: &str) -> Vec<u8> {
    fixture
        .split_whitespace()
        .map(|token| u8::from_str_radix(token, 16).expect("parse hex byte"))
        .collect()
}

fn synthetic_private_key_pem() -> String {
    [
        "-----BEGIN PRIVATE ",
        "KEY-----\\nMIIB\\n-----END PRIVATE ",
        "KEY-----\\n",
    ]
    .concat()
}

fn current_agent_hello() -> AgentHello {
    AgentHello {
        agent_id: "agent-win11-001".to_string(),
        hostname: "talos-win11".to_string(),
        os: "windows 11 pro".to_string(),
        ip: "10.0.0.41".to_string(),
        local_addrs: Some(vec![
            LocalAddr {
                ip: "10.0.0.41".to_string(),
                prefix: 24,
            },
            LocalAddr {
                ip: "fe80::1".to_string(),
                prefix: 64,
            },
        ]),
        version: Some("0.6.13".to_string()),
        is_admin: true,
        platform: AgentPlatform::Windows,
        features: AgentFeatureCapabilities::windows(),
    }
}

fn current_remote_desktop_capabilities() -> RemoteDesktopCapabilities {
    RemoteDesktopCapabilities {
        codecs: vec!["h264".to_string(), "vp8".to_string()],
        encoding: "software".to_string(),
        transports: vec!["quic".to_string(), "relay".to_string()],
        platform: AgentPlatform::Windows,
        features: AgentFeatureCapabilities::windows(),
        display_profiles: vec![
            RemoteDesktopDisplayProfile::modern_gpu(),
            RemoteDesktopDisplayProfile::legacy(),
        ],
        selected_display_profile: Some(REMOTE_DESKTOP_PROFILE_MODERN_GPU.to_string()),
    }
}

#[test]
fn agent_check_in_serializes_to_current_fixture() {
    let envelope = OutgoingEnvelope {
        message_type: "agent_hello",
        data: current_agent_hello(),
    };

    assert_json_fixture(
        &envelope,
        include_str!("../fixtures/current/agent_check_in.json"),
    );

    let (message_type, parsed): (String, AgentHello) =
        parse_envelope_payload(include_str!("../fixtures/current/agent_check_in.json"));
    assert_eq!(message_type, "agent_hello");
    assert_eq!(parsed, current_agent_hello());
}

#[test]
fn capability_report_serializes_to_current_fixture() {
    let envelope = OutgoingEnvelope {
        message_type: "session_capabilities_response",
        data: SessionCapabilitiesResponse {
            request_id: "cap-001".to_string(),
            capabilities: current_remote_desktop_capabilities(),
        },
    };

    assert_json_fixture(
        &envelope,
        include_str!("../fixtures/current/capability_report.json"),
    );
}

#[test]
fn command_execution_fixtures_roundtrip() {
    let request = OutgoingEnvelope {
        message_type: "shell_command",
        data: ShellCommandPayload {
            request_id: "cmd-001".to_string(),
            command: "Get-Process -Id $PID".to_string(),
        },
    };
    assert_json_fixture(
        &request,
        include_str!("../fixtures/current/command_execution_request.json"),
    );

    let response = OutgoingEnvelope {
        message_type: "shell_output",
        data: ShellOutputPayload {
            request_id: "cmd-001".to_string(),
            output: "ok\n".to_string(),
            exit_code: Some(0),
        },
    };
    assert_json_fixture(
        &response,
        include_str!("../fixtures/current/command_execution_response.json"),
    );
}

#[test]
fn shell_session_json_and_frame_fixtures_roundtrip() {
    let start = OutgoingEnvelope {
        message_type: "shell_start",
        data: ShellStartPayload {
            session_id: "shell-001".to_string(),
            token: "shell-token-001".to_string(),
            run_as: ShellRunAs::User,
            target_session_id: Some(2),
            relay_url: Some("https://relay.example.test:17443".to_string()),
            e2e_key: Some("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=".to_string()),
            psk_cert_pem: Some(
                "-----BEGIN CERTIFICATE-----\\nMIIB\\n-----END CERTIFICATE-----\\n".to_string(),
            ),
            psk_key_pem: Some(synthetic_private_key_pem()),
        },
    };
    assert_json_fixture(
        &start,
        include_str!("../fixtures/current/shell_session_start.json"),
    );

    let frame = build_shell_frame(SHELL_MSG_INPUT, b"whoami\r").expect("build shell frame");
    assert_eq!(
        frame,
        hex_fixture(include_str!("../fixtures/current/shell_frame_input.hex"))
    );
}

#[test]
fn file_transfer_request_and_frame_fixtures_roundtrip() {
    let upload = FileTransferRequest::Upload {
        transfer_id: "xfer-001".to_string(),
        destination_path: "C:\\Temp".to_string(),
        file_name: "talos-demo.zip".to_string(),
        is_archive: true,
        extract_archive: true,
        conflict_mode: FileTransferConflictMode::Overwrite,
        expected_size_bytes: 4096,
        resume_offset: 0,
    };
    assert_json_fixture(
        &upload,
        include_str!("../fixtures/current/file_transfer_upload_request.json"),
    );

    let list_dir = FileTransferRequest::ListDir {
        path: "C:\\Temp".to_string(),
    };
    let payload = serde_json::to_vec(&list_dir).expect("serialize list dir request");
    let frame =
        build_file_transfer_frame(FILE_TRANSFER_MSG_JSON, &payload).expect("build file frame");
    assert_eq!(
        frame,
        hex_fixture(include_str!(
            "../fixtures/current/file_transfer_json_frame.hex"
        ))
    );

    let parsed = parse_file_transfer_frame(&frame).expect("parse file frame");
    assert_eq!(parsed.message_type, FILE_TRANSFER_MSG_JSON);
    let decoded: FileTransferRequest =
        serde_json::from_slice(parsed.payload).expect("decode file transfer payload");
    assert_eq!(decoded, list_dir);
}

#[test]
fn registry_operation_serializes_to_current_fixture() {
    let request = RegistryRequest::SetValue {
        request_id: "reg-001".to_string(),
        session_id: "sess-reg-001".to_string(),
        hive: RegistryHive::HKLM,
        path: "SOFTWARE\\Talos".to_string(),
        name: "Enabled".to_string(),
        data: RegistryValueData::Dword { data: 1 },
    };

    assert_json_fixture(
        &request,
        include_str!("../fixtures/current/registry_set_value_request.json"),
    );
}

#[test]
fn remote_desktop_negotiation_and_quality_update_fixtures_roundtrip() {
    let negotiation = OutgoingEnvelope {
        message_type: "tunnel_prepare",
        data: TunnelPreparePayload {
            session_id: "rdp-001".to_string(),
            psk_cert_pem: "-----BEGIN CERTIFICATE-----\\nMIIB\\n-----END CERTIFICATE-----\\n"
                .to_string(),
            psk_key_pem: synthetic_private_key_pem(),
            relay_url: Some("https://relay.example.test:17443".to_string()),
            e2e_key: Some("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=".to_string()),
            mode: SessionTransportMode::RemoteDesktop,
            selected_display_profile: Some("modern_gpu".to_string()),
            hide_cursor: false,
            viewer_session_token: Some("viewer-token-001".to_string()),
            parent_desktop_session_id: Some("desktop-parent-001".to_string()),
        },
    };
    assert_json_fixture(
        &negotiation,
        include_str!("../fixtures/current/remote_desktop_negotiation.json"),
    );

    let frame = build_control_frame(CONTROL_TYPE_STREAM_BITRATE, &20_000u32.to_be_bytes())
        .expect("build stream bitrate frame");
    assert_eq!(
        frame,
        hex_fixture(include_str!(
            "../fixtures/current/quality_update_control_frame.hex"
        ))
    );
    let parsed = parse_control_frame(&frame).expect("parse quality update frame");
    assert_eq!(parsed.message_type, CONTROL_TYPE_STREAM_BITRATE);
    assert_eq!(parsed.payload.len(), CONTROL_PAYLOAD_STREAM_BITRATE_LEN);
    assert_eq!(
        u32::from_be_bytes(parsed.payload.try_into().unwrap()),
        20_000
    );
}

#[test]
fn telemetry_snapshot_serializes_to_current_fixture() {
    let snapshot = OutgoingEnvelope {
        message_type: "full_snapshot",
        data: FullSnapshotUpdate {
            agent_id: "agent-win11-001".to_string(),
            collected_at: "2026-05-11T01:15:00Z".to_string(),
            snapshot: json!({
                "metadata": {
                    "agentId": "agent-win11-001",
                    "deviceName": "talos-win11",
                    "agentVersion": "0.6.13",
                    "collectionProfile": "full"
                },
                "collection": {
                    "cpu": {
                        "brand": "12th Gen Intel(R) Core(TM)",
                        "cores": 8
                    },
                    "memory": {
                        "totalBytes": 17179869184u64
                    }
                }
            }),
            snapshot_request_id: Some("snap-001".to_string()),
        },
    };

    assert_json_fixture(
        &snapshot,
        include_str!("../fixtures/current/telemetry_snapshot.json"),
    );
}

#[test]
fn snapshot_request_serializes_to_current_fixture_and_accepts_legacy_spelling() {
    let request = OutgoingEnvelope {
        message_type: "request_full_snapshot",
        data: RequestFullSnapshotPayload {
            snapshot_request_id: Some("snap-001".to_string()),
        },
    };
    assert_json_fixture(
        &request,
        include_str!("../fixtures/current/request_full_snapshot.json"),
    );

    let (_, current): (String, RequestFullSnapshotPayload) = parse_envelope_payload(include_str!(
        "../fixtures/current/request_full_snapshot.json"
    ));
    assert_eq!(current.snapshot_request_id.as_deref(), Some("snap-001"));

    let (_, legacy): (String, RequestFullSnapshotPayload) = parse_envelope_payload(include_str!(
        "../fixtures/old/request_full_snapshot_snake_case_v0.json"
    ));
    assert_eq!(
        legacy.snapshot_request_id.as_deref(),
        Some("snap-legacy-001")
    );
}

#[test]
fn old_fixtures_parse_with_defaults_and_ignored_fields() {
    let (message_type, hello): (String, AgentHello) =
        parse_envelope_payload(include_str!("../fixtures/old/agent_check_in_v0.json"));
    assert_eq!(message_type, "agent_hello");
    assert_eq!(hello.local_addrs, None);
    assert_eq!(hello.version, None);
    assert!(!hello.is_admin);
    assert_eq!(hello.platform, AgentPlatform::Unknown);
    assert_eq!(hello.features, AgentFeatureCapabilities::windows());

    let (_, capabilities): (String, SessionCapabilitiesResponse) =
        parse_envelope_payload(include_str!("../fixtures/old/capability_report_v0.json"));
    assert_eq!(capabilities.capabilities.platform, AgentPlatform::Unknown);
    assert_eq!(
        capabilities.capabilities.features,
        AgentFeatureCapabilities::windows()
    );
    assert!(capabilities.capabilities.display_profiles.is_empty());
    assert_eq!(capabilities.capabilities.selected_display_profile, None);

    let upload: FileTransferRequest =
        serde_json::from_str(include_str!("../fixtures/old/file_transfer_upload_v0.json"))
            .expect("parse legacy file transfer upload");
    match upload {
        FileTransferRequest::Upload { resume_offset, .. } => assert_eq!(resume_offset, 0),
        other => panic!("unexpected upload fixture variant: {other:?}"),
    }

    let (_, snapshot): (String, FullSnapshotUpdate) =
        parse_envelope_payload(include_str!("../fixtures/old/telemetry_snapshot_v0.json"));
    assert_eq!(snapshot.snapshot_request_id, None);
}

#[test]
fn unknown_operation_error_codes_map_to_unknown_variant() {
    let response: FileTransferResponse = serde_json::from_str(include_str!(
        "../fixtures/old/file_transfer_error_unknown_code.json"
    ))
    .expect("parse future operation error code");

    match response {
        FileTransferResponse::Error {
            code,
            message,
            retryable,
        } => {
            assert_eq!(code, OperationErrorCode::Unknown);
            assert!(message.contains("newer error code"));
            assert!(retryable);
        }
        other => panic!("unexpected file transfer response variant: {other:?}"),
    }
}

#[test]
fn telemetry_events_default_to_empty_for_forward_compatibility() {
    let events: TelemetryEventsUpdate =
        serde_json::from_value(json!({ "agent_id": "agent-win11-001" }))
            .expect("parse empty telemetry event batch");
    assert!(events.events.is_empty());
}
