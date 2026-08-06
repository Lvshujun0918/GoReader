//! GAP 130：正文图片代理 webp 转换（/assets/proxy?fmt=webp&q=80）
//!
//! 仅启用 jpeg/png/gif/webp 编解码（image crate default-features=false），
//! 把上游图片转码为 webp 输出（移动端流量友好）。
//! 说明：image 0.25 内置 webp 编码器为纯 Rust 无损实现（image-webp，无 libwebp C 依赖），
//! q 参数（质量）对无损编码不生效——保留参数以兼容未来有损编码器接入。
//! 解码/编码失败时返回 None——调用方回退原图透传（不中断阅读）。
//!
//! P1 解压炸弹防护：解码会完整展开像素（恶意超大尺寸图片可耗尽内存）——
//! 先读图片**头尺寸**（不解码像素）预检，单边超 8000px 或总像素超 40MP 拒绝转码。

use anyhow::{anyhow, Result};

/// 解码尺寸上限（单边，px）——超限拒绝转码（防解压炸弹）
pub const MAX_IMAGE_DIMENSION: u32 = 8000;
/// 解码总像素上限（8000x8000 = 64MP 也会超此限；40MP ≈ 160MB RGBA 展开）
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;

/// 图片头尺寸预检（只读头、不解码像素）：单边超 [`MAX_IMAGE_DIMENSION`] 或
/// 总像素超 [`MAX_IMAGE_PIXELS`] → Err；返回 (宽, 高)。
/// image crate 的 `into_dimensions` 仅解析文件头——恶意超大尺寸图片在解码前即被拒绝。
pub fn check_image_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow!("图片格式识别失败: {e}"))?
        .into_dimensions()
        .map_err(|e| anyhow!("图片头解析失败: {e}"))?;
    if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
        anyhow::bail!("图片尺寸超限（{w}x{h}，单边上限 {MAX_IMAGE_DIMENSION}px）");
    }
    if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
        anyhow::bail!(
            "图片像素超限（{} 像素，上限 {MAX_IMAGE_PIXELS}）",
            w as u64 * h as u64
        );
    }
    Ok((w, h))
}

/// 将图片字节转码为 webp（quality 1-100，无损编码下不生效；非 raster/解码失败返回 None）
pub fn to_webp(bytes: &[u8], quality: u8) -> Option<Vec<u8>> {
    if bytes.len() < 16 {
        return None;
    }
    // 已是 webp：无需转码（调用方直接透传——不涉及解压，无炸弹面）
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(bytes.to_vec());
    }
    // P1 解压炸弹：解码前先读头尺寸预检（超限拒绝，回退原图透传）
    check_image_dimensions(bytes).ok()?;
    let fmt = image::guess_format(bytes).ok()?;
    let img = image::load_from_memory_with_format(bytes, fmt).ok()?;
    let _ = quality; // 无损编码：质量参数不生效（保留以兼容未来有损编码器）
    let mut out = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    img.write_with_encoder(encoder).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成 4x4 PNG 测试图 → webp 转码：RIFF/WEBP 头 + 可解码回原尺寸
    #[test]
    fn test_to_webp_from_png() {
        // 4x4 纯色 RGBA
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
        let mut png_buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png_buf, image::ImageFormat::Png)
            .expect("PNG 编码应成功");
        let png = png_buf.into_inner();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));

        let webp = to_webp(&png, 80).expect("webp 转码应成功");
        assert_eq!(&webp[0..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");

        // 解码回原尺寸
        let decoded = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP).unwrap();
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }

    /// 已 webp 输入原样返回（不重复转码）
    #[test]
    fn test_to_webp_passthrough_webp() {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut buf);
        img.write_with_encoder(encoder).unwrap();
        let webp = buf.into_inner();
        let out = to_webp(&webp, 80).expect("webp 输入应通过");
        assert_eq!(out, webp, "webp 输入应原样返回");
    }

    /// 非图片字节（HTML/文本）→ None（调用方回退透传）
    #[test]
    fn test_to_webp_invalid_input() {
        assert!(to_webp(b"<html>not an image</html>", 80).is_none());
        assert!(to_webp(b"tiny", 80).is_none());
    }

    /// 构造 WxH PNG（编码 4x4 后改写 IHDR 宽高字段并重算 CRC——不解码像素即可预检）
    fn png_with_dims(w: u32, h: u32) -> Vec<u8> {
        // IEEE CRC-32（PNG chunk 校验）
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let mut png = buf.into_inner();
        // PNG IHDR chunk：length(8..12) + "IHDR"(12..16) + 宽(16..20) + 高(20..24)
        // + 其余头字段(24..29) + CRC(29..33)——CRC 覆盖 type+data（12..29）
        png[16..20].copy_from_slice(&w.to_be_bytes());
        png[20..24].copy_from_slice(&h.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    /// P1 解压炸弹：尺寸预检——正常尺寸通过；单边超限/总像素超限拒绝
    #[test]
    fn test_check_image_dimensions_limits() {
        // 正常尺寸（6000x6000 = 36MP < 40MP）
        let (w, h) = check_image_dimensions(&png_with_dims(6000, 6000)).unwrap();
        assert_eq!((w, h), (6000, 6000));
        // 单边超限（宽 9000）
        let err = check_image_dimensions(&png_with_dims(9000, 10)).unwrap_err();
        assert!(err.to_string().contains("尺寸超限"), "{err}");
        // 单边超限（高 9000）
        let err = check_image_dimensions(&png_with_dims(10, 9000)).unwrap_err();
        assert!(err.to_string().contains("尺寸超限"), "{err}");
        // 总像素超限（8000x8000 = 64MP > 40MP——单边恰在上限仍拒绝）
        let err = check_image_dimensions(&png_with_dims(8000, 8000)).unwrap_err();
        assert!(err.to_string().contains("像素超限"), "{err}");
        // 非图片字节 → Err（格式识别失败）
        assert!(check_image_dimensions(b"<html>not an image</html>").is_err());
    }

    /// P1 解压炸弹：to_webp 对超大尺寸图片拒绝转码（回退原图透传——不 OOM）
    #[test]
    fn test_to_webp_rejects_oversize_bomb() {
        let png = png_with_dims(9000, 9000);
        assert!(
            to_webp(&png, 80).is_none(),
            "超大尺寸图片应拒绝转码（防解压炸弹）"
        );
        // 总像素超限（8000x8000）同样拒绝
        let png2 = png_with_dims(8000, 8000);
        assert!(to_webp(&png2, 80).is_none(), "64MP 图片应拒绝转码");
    }
}
