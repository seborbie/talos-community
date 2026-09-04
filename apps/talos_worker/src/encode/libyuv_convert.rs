//! BGRA to I420 conversion via libyuv (SIMD-optimized).
//!
//! Output Y is converted from libyuv full range (0-255) to limited range (16-235)
//! so the RMM viewer's BT.601 limited-range I420→ARGB formula matches.

use std::os::raw::c_int;

use anyhow::{ensure, Result};

use crate::capture::Frame;

/// Convert Y plane from full range (0-255) to BT.601 limited range (16-235) in place.
/// Matches the RMM viewer's i420_to_argb which uses c = y_val - 16.
fn y_full_range_to_limited(y_plane: &mut [u8]) {
    for y in y_plane.iter_mut() {
        // Y_limited = (Y_full * 219) / 255 + 16, clamped to 16..235
        let v = (*y as u32 * 219) / 255 + 16;
        *y = v.clamp(16, 235) as u8;
    }
}

/// Converts BGRA to I420 (Y plane, then U, then V at half resolution).
/// If grayscale, U and V are 128. If crop is Some((w, h)), only top-left w×h is converted.
pub fn bgra_to_i420(frame: &Frame, grayscale: bool, crop: Option<(u32, u32)>) -> Result<Vec<u8>> {
    ensure!(
        matches!(frame.format, crate::capture::PixelFormat::Bgra8),
        "only Bgra8 format is supported"
    );
    let (w, h) = crop.unwrap_or((frame.width, frame.height));
    let w = w as usize;
    let h = h as usize;
    let stride = frame.stride as i32;
    ensure!(
        w.is_multiple_of(2) && h.is_multiple_of(2),
        "width and height must be even for I420"
    );
    ensure!(
        w <= frame.width as usize && h <= frame.height as usize,
        "crop size must not exceed frame size"
    );
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);
    let mut out = vec![0u8; y_size + uv_size * 2];
    let (y_plane, rest) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = rest.split_at_mut(uv_size);

    // Windows capture is BGRA in memory (B,G,R,A). Libyuv's ARGBToI420 expects
    // "ARGB little endian (bgra in memory)" per convert.h; BGRAToI420 expects
    // (argb in memory). Use ARGBToI420 so our buffer is interpreted correctly.
    let ret = unsafe {
        yuv_sys::rs_ARGBToI420(
            frame.data.as_ptr(),
            stride,
            y_plane.as_mut_ptr(),
            w as c_int,
            u_plane.as_mut_ptr(),
            (w / 2) as c_int,
            v_plane.as_mut_ptr(),
            (w / 2) as c_int,
            w as c_int,
            h as c_int,
        )
    };
    ensure!(ret == 0, "libyuv ARGBToI420 failed with code {}", ret);

    y_full_range_to_limited(y_plane);

    if grayscale {
        u_plane.fill(128);
        v_plane.fill(128);
    }

    Ok(out)
}

/// Converts BGRA bytes with an explicit stride to I420.
pub fn bgra_bytes_to_i420(
    data: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    grayscale: bool,
) -> Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    let stride = stride as usize;
    ensure!(
        w.is_multiple_of(2) && h.is_multiple_of(2),
        "width and height must be even for I420"
    );
    ensure!(
        stride >= w * 4,
        "stride must be at least width * 4 for BGRA input"
    );
    ensure!(
        data.len() >= stride * h,
        "input BGRA buffer shorter than declared stride*height"
    );

    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);
    let mut out = vec![0u8; y_size + uv_size * 2];
    let (y_plane, rest) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = rest.split_at_mut(uv_size);

    let ret = unsafe {
        yuv_sys::rs_ARGBToI420(
            data.as_ptr(),
            stride as c_int,
            y_plane.as_mut_ptr(),
            w as c_int,
            u_plane.as_mut_ptr(),
            (w / 2) as c_int,
            v_plane.as_mut_ptr(),
            (w / 2) as c_int,
            w as c_int,
            h as c_int,
        )
    };
    ensure!(ret == 0, "libyuv ARGBToI420 failed with code {}", ret);

    y_full_range_to_limited(y_plane);

    if grayscale {
        u_plane.fill(128);
        v_plane.fill(128);
    }

    Ok(out)
}

/// Converts I420 (Y, U, V planes) to NV12 (Y, interleaved UV planes).
pub fn i420_to_nv12(i420: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    ensure!(
        w.is_multiple_of(2) && h.is_multiple_of(2),
        "width and height must be even for NV12"
    );
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);
    let expected = y_size + uv_size * 2;
    ensure!(
        i420.len() == expected,
        "input I420 buffer length mismatch: expected {}, got {}",
        expected,
        i420.len()
    );

    let (src_y, rest) = i420.split_at(y_size);
    let (src_u, src_v) = rest.split_at(uv_size);
    let mut out = vec![0u8; expected];
    let (dst_y, dst_uv) = out.split_at_mut(y_size);
    dst_y.copy_from_slice(src_y);
    for index in 0..uv_size {
        let dst = index * 2;
        dst_uv[dst] = src_u[index];
        dst_uv[dst + 1] = src_v[index];
    }
    Ok(out)
}

/// Downscales frame to out_width×out_height and converts to I420.
/// Uses ARGBToI420 (our buffer is bgra in memory) then I420Scale (bilinear). out_width/out_height must be even.
pub fn bgra_to_i420_scaled(
    frame: &Frame,
    grayscale: bool,
    out_width: u32,
    out_height: u32,
) -> Result<Vec<u8>> {
    ensure!(
        matches!(frame.format, crate::capture::PixelFormat::Bgra8),
        "only Bgra8 format is supported"
    );
    let ow = out_width as usize;
    let oh = out_height as usize;
    let src_w = frame.width as usize;
    let src_h = frame.height as usize;
    let stride = frame.stride as i32;
    ensure!(
        ow.is_multiple_of(2) && oh.is_multiple_of(2),
        "out width and height must be even for I420"
    );
    ensure!(
        ow <= src_w && oh <= src_h,
        "output size must not exceed frame size"
    );

    let src_y_size = src_w * src_h;
    let src_uv_size = (src_w / 2) * (src_h / 2);
    let mut tmp = vec![0u8; src_y_size + src_uv_size * 2];
    let (tmp_y, tmp_rest) = tmp.split_at_mut(src_y_size);
    let (tmp_u, tmp_v) = tmp_rest.split_at_mut(src_uv_size);

    // Windows capture is BGRA in memory (B,G,R,A). Use ARGBToI420 ("bgra in memory").
    let ret = unsafe {
        yuv_sys::rs_ARGBToI420(
            frame.data.as_ptr(),
            stride,
            tmp_y.as_mut_ptr(),
            src_w as c_int,
            tmp_u.as_mut_ptr(),
            (src_w / 2) as c_int,
            tmp_v.as_mut_ptr(),
            (src_w / 2) as c_int,
            src_w as c_int,
            src_h as c_int,
        )
    };
    ensure!(ret == 0, "libyuv ARGBToI420 failed with code {}", ret);

    let dst_y_size = ow * oh;
    let dst_uv_size = (ow / 2) * (oh / 2);
    let mut out = vec![0u8; dst_y_size + dst_uv_size * 2];
    let (out_y, out_rest) = out.split_at_mut(dst_y_size);
    let (out_u, out_v) = out_rest.split_at_mut(dst_uv_size);

    // libyuv FilterMode: kFilterBilinear = 2.
    let filter_mode = 2 as yuv_sys::FilterMode;
    let scale_ret = unsafe {
        yuv_sys::rs_I420Scale(
            tmp_y.as_ptr(),
            src_w as c_int,
            tmp_u.as_ptr(),
            (src_w / 2) as c_int,
            tmp_v.as_ptr(),
            (src_w / 2) as c_int,
            src_w as c_int,
            src_h as c_int,
            out_y.as_mut_ptr(),
            ow as c_int,
            out_u.as_mut_ptr(),
            (ow / 2) as c_int,
            out_v.as_mut_ptr(),
            (ow / 2) as c_int,
            ow as c_int,
            oh as c_int,
            filter_mode,
        )
    };
    ensure!(
        scale_ret == 0,
        "libyuv I420Scale failed with code {}",
        scale_ret
    );

    y_full_range_to_limited(out_y);

    if grayscale {
        out_u.fill(128);
        out_v.fill(128);
    }

    Ok(out)
}
