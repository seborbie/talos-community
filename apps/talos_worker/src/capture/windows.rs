//! DXGI Desktop Duplication capture backend (Windows only).

#![cfg(windows)]

use std::{ptr, slice};

use anyhow::Result;
use dxgi_capture_rs::{CaptureError, DXGIManager, OutputDuplicationError};

use crate::capture::{CaptureBackend, CaptureResult, Frame, PixelFormat};

/// DXGI-based screen capture using the Desktop Duplication API.
pub struct DxgiBackend {
    manager: DXGIManager,
}

impl DxgiBackend {
    /// Creates a new DXGI capture backend (first output, 100 ms timeout).
    /// A short timeout prevents blocking the capture loop for too long when
    /// the desktop is static (DXGI returns Timeout when nothing changed).
    pub fn new() -> std::result::Result<Self, OutputDuplicationError> {
        let manager = DXGIManager::new(100)?;
        Ok(Self { manager })
    }
}

/// Software GDI capture backend used when DXGI/D3D11 capture is unavailable.
pub struct GdiBackend {
    capture_source_index: usize,
}

impl GdiBackend {
    pub fn new() -> Result<Self> {
        let backend = Self {
            capture_source_index: 0,
        };
        backend.capture_bounds()?;
        Ok(backend)
    }

    pub fn capture_source_index(&self) -> usize {
        self.capture_source_index
    }

    fn capture_bounds(&self) -> Result<(i32, i32, u32, u32)> {
        if let Some((left, top, right, bottom)) =
            crate::capture::dxgi_output_desktop_rect_for_global_index(self.capture_source_index)
        {
            let width = right.saturating_sub(left) as u32;
            let height = bottom.saturating_sub(top) as u32;
            if width > 0 && height > 0 {
                return Ok((left, top, width, height));
            }
        }

        if self.capture_source_index == 0 {
            use winapi::um::winuser::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

            let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
            if width > 0 && height > 0 {
                return Ok((0, 0, width as u32, height as u32));
            }
        }

        anyhow::bail!(
            "no software capture output was found for index {}",
            self.capture_source_index
        )
    }

    fn capture_frame_gdi(&self) -> Result<Frame> {
        use winapi::shared::windef::HGDIOBJ;
        use winapi::um::wingdi::{
            BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject,
            BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
        };
        use winapi::um::winuser::{GetDC, ReleaseDC};

        let (origin_x, origin_y, width, height) = self.capture_bounds()?;
        let width_i32 =
            i32::try_from(width).map_err(|_| anyhow::anyhow!("capture width too large"))?;
        let height_i32 =
            i32::try_from(height).map_err(|_| anyhow::anyhow!("capture height too large"))?;
        let stride = width as usize * 4;
        let frame_len = stride
            .checked_mul(height as usize)
            .ok_or_else(|| anyhow::anyhow!("software capture frame size overflow"))?;

        unsafe {
            let screen_dc = GetDC(ptr::null_mut());
            if screen_dc.is_null() {
                anyhow::bail!("GetDC failed for software capture");
            }

            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_null() {
                let _ = ReleaseDC(ptr::null_mut(), screen_dc);
                anyhow::bail!("CreateCompatibleDC failed for software capture");
            }

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width_i32,
                    // Negative height requests a top-down DIB so we can copy rows directly.
                    biHeight: -height_i32,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [std::mem::zeroed()],
            };
            let mut dib_bits = ptr::null_mut();
            let bitmap = CreateDIBSection(
                mem_dc,
                &bmi,
                DIB_RGB_COLORS,
                &mut dib_bits,
                ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || dib_bits.is_null() {
                DeleteDC(mem_dc);
                let _ = ReleaseDC(ptr::null_mut(), screen_dc);
                anyhow::bail!("CreateDIBSection failed for software capture");
            }

            let old_bitmap = SelectObject(mem_dc, bitmap as HGDIOBJ);
            if old_bitmap.is_null() {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(mem_dc);
                let _ = ReleaseDC(ptr::null_mut(), screen_dc);
                anyhow::bail!("SelectObject failed for software capture");
            }

            let blit_ok = BitBlt(
                mem_dc,
                0,
                0,
                width_i32,
                height_i32,
                screen_dc,
                origin_x,
                origin_y,
                SRCCOPY | CAPTUREBLT,
            );
            let data = if blit_ok == 0 {
                Err(anyhow::anyhow!("BitBlt failed for software capture"))
            } else {
                Ok(slice::from_raw_parts(dib_bits as *const u8, frame_len).to_vec())
            };

            let _ = SelectObject(mem_dc, old_bitmap);
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(mem_dc);
            let _ = ReleaseDC(ptr::null_mut(), screen_dc);

            Ok(Frame {
                width,
                height,
                stride: (width * 4),
                format: PixelFormat::Bgra8,
                data: data?,
            })
        }
    }
}

fn frame_from_components(data: Vec<u8>, width: usize, height: usize) -> Frame {
    let stride = (width * 4) as u32;
    Frame {
        width: width as u32,
        height: height as u32,
        stride,
        format: PixelFormat::Bgra8,
        data,
    }
}

impl CaptureBackend for DxgiBackend {
    fn capture_frame(&mut self) -> Result<Frame> {
        let (data, (width, height)) = self
            .manager
            .capture_frame_components()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(frame_from_components(data, width, height))
    }

    fn try_capture_frame(&mut self) -> Result<CaptureResult> {
        match self.manager.capture_frame_components() {
            Ok((data, (width, height))) => Ok(CaptureResult::Frame(frame_from_components(
                data, width, height,
            ))),
            Err(CaptureError::Timeout) => Ok(CaptureResult::Timeout),
            Err(CaptureError::AccessLost) => Ok(CaptureResult::AccessLost),
            Err(e) => Err(anyhow::anyhow!("{}", e)),
        }
    }

    fn set_capture_source_index(&mut self, index: usize) -> Result<()> {
        self.manager.set_capture_source_index(index);
        Ok(())
    }
}

impl CaptureBackend for GdiBackend {
    fn capture_frame(&mut self) -> Result<Frame> {
        self.capture_frame_gdi()
    }

    fn try_capture_frame(&mut self) -> Result<CaptureResult> {
        self.capture_frame_gdi().map(CaptureResult::Frame)
    }

    fn set_capture_source_index(&mut self, index: usize) -> Result<()> {
        let previous = self.capture_source_index;
        self.capture_source_index = index;
        if let Err(err) = self.capture_bounds() {
            self.capture_source_index = previous;
            return Err(err);
        }
        Ok(())
    }
}
