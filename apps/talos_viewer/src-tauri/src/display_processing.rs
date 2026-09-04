use std::env;
use std::sync::OnceLock;

use tracing::warn;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayProcessingMode {
    Legacy,
    Auto,
    Gpu,
}

impl DisplayProcessingMode {
    pub(crate) fn prefers_gpu(self) -> bool {
        matches!(self, Self::Auto | Self::Gpu)
    }

    pub(crate) fn allows_cpu_fallback(self) -> bool {
        matches!(self, Self::Auto)
    }
}

static EFFECTIVE_MODE: OnceLock<DisplayProcessingMode> = OnceLock::new();

pub(crate) fn effective_display_processing_mode(context: &'static str) -> DisplayProcessingMode {
    *EFFECTIVE_MODE.get_or_init(|| resolve_display_processing_mode(context))
}

fn resolve_display_processing_mode(context: &'static str) -> DisplayProcessingMode {
    match env::var("RMM_DISPLAY_PROCESSING_MODE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "auto" => DisplayProcessingMode::Auto,
        "legacy" => DisplayProcessingMode::Legacy,
        "modern_gpu" => DisplayProcessingMode::Gpu,
        "experimental" => {
            warn!(
                context,
                env_var = "RMM_DISPLAY_PROCESSING_MODE",
                "experimental display processing mode is deprecated; using modern_gpu"
            );
            DisplayProcessingMode::Gpu
        }
        "modern_cpu" => {
            warn!(
                context,
                env_var = "RMM_DISPLAY_PROCESSING_MODE",
                "modern_cpu display processing mode is removed; using legacy CPU capture"
            );
            DisplayProcessingMode::Legacy
        }
        other => {
            warn!(
                context,
                env_var = "RMM_DISPLAY_PROCESSING_MODE",
                value = %other,
                "invalid display processing mode; falling back to auto mode"
            );
            DisplayProcessingMode::Auto
        }
    }
}
