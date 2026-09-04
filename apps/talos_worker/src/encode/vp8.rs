use std::os::raw::c_int;

use anyhow::Result;
use vpx_encode::{Config, Encoder, VideoCodecId};

use super::Preset;

fn preset_bitrate_kbps(preset: Preset) -> u32 {
    match preset {
        Preset::Grayscale => 200,
        Preset::Low => 500,
        Preset::Medium => 1500,
        Preset::High => 4000,
        Preset::Maximum => 16_000,
    }
}

/// Encode tuning: preset drives defaults; bitrate and cpu_used can override.
#[derive(Clone, Copy, Debug)]
pub struct EncodeTuning {
    pub preset: Preset,
    pub bitrate_override_kbps: Option<u32>,
    pub cpu_used: Option<i32>,
}

impl EncodeTuning {
    pub fn bitrate_kbps(self) -> u32 {
        self.bitrate_override_kbps
            .unwrap_or_else(|| preset_bitrate_kbps(self.preset))
    }
}

pub struct Vp8Encoder {
    encoder: Encoder,
}

impl Vp8Encoder {
    pub fn new(width: u32, height: u32, fps: u32, tuning: EncodeTuning) -> Result<Self> {
        let bitrate = tuning
            .bitrate_override_kbps
            .unwrap_or_else(|| preset_bitrate_kbps(tuning.preset));
        let vp8_cpu_used = tuning.cpu_used.map(|v| v as c_int);
        let config = Config {
            width: width as std::os::raw::c_uint,
            height: height as std::os::raw::c_uint,
            timebase: [1, fps as c_int],
            bitrate: bitrate as std::os::raw::c_uint,
            codec: VideoCodecId::VP8,
            vp8_cpu_used,
        };
        let encoder = Encoder::new(config).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(Self { encoder })
    }

    pub fn encode(&mut self, i420: &[u8], pts: i64) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut packets = self
            .encoder
            .encode(pts, i420)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for pkt in packets.by_ref() {
            out.extend_from_slice(pkt.data);
        }
        Ok(out)
    }
}
