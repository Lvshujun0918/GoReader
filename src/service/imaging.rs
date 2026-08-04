//! GAP 130：正文图片代理 webp 转换（/assets/proxy?fmt=webp&q=80）
//!
//! 仅启用 jpeg/png/gif/webp 编解码（image crate default-features=false），
//! 把上游图片转码为 webp 输出（移动端流量友好）。
//! 说明：image 0.25 内置 webp 编码器为纯 Rust 无损实现（image-webp，无 libwebp C 依赖），
//! q 参数（质量）对无损编码不生效——保留参数以兼容未来有损编码器接入。
//! 解码/编码失败时返回 None——调用方回退原图透传（不中断阅读）。

/// 将图片字节转码为 webp（quality 1-100，无损编码下不生效；非 raster/解码失败返回 None）
pub fn to_webp(bytes: &[u8], quality: u8) -> Option<Vec<u8>> {
    if bytes.len() < 16 {
        return None;
    }
    // 已是 webp：无需转码（调用方直接透传）
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(bytes.to_vec());
    }
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
}
