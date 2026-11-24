//! Page HTML autonome : un seul fichier, images incluses en base64.

use std::fmt::Write as _;

use anyhow::Result;
use image::RgbaImage;

use super::rich::{self, escape};
use super::{DocItem, DocOptions, png_bytes};

const STYLE: &str = "\
body{margin:0;background:#f4f4f5;color:#1c1c21;font:16px/1.5 -apple-system,\
BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif}\
main{max-width:960px;margin:0 auto;padding:32px 16px}\
h1{font-size:28px;margin:0 0 24px}\
section{background:#fff;border-radius:12px;padding:24px;margin:0 0 24px;\
box-shadow:0 1px 3px rgba(0,0,0,.08)}\
h2{color:#7f3aed;font-size:20px;margin:0 0 12px}\
p{margin:0 0 12px}ol{margin:0 0 12px;padding-left:24px}\
figure{margin:0 0 12px}img{max-width:100%;height:auto;border:1px solid #e4e4e7;\
border-radius:4px}\
.break{break-before:page;page-break-before:always}\
@media print{body{background:#fff}section{box-shadow:none;padding:0}}";

pub fn write(
    items: &[DocItem],
    images: &[RgbaImage],
    options: &DocOptions,
    title: &str,
) -> Result<String> {
    let mut out = String::new();
    let title = escape(title);
    write!(
        out,
        "<!doctype html>\n<html lang=\"fr\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"generator\" content=\"Snappy\">\n<title>{title}</title>\n\
         <style>{STYLE}</style>\n</head>\n<body>\n<main>\n<h1>{title}</h1>\n"
    )?;
    for (item, img) in items.iter().zip(images) {
        out.push_str("<section>\n");
        let heading = item.display_title(options);
        if !heading.is_empty() {
            writeln!(out, "<h2>{}</h2>", escape(&heading))?;
        }
        let mut text = String::new();
        if !item.description.trim().is_empty() {
            writeln!(
                text,
                "<p>{}</p>",
                rich::to_html(&rich::parse(&item.description))
            )?;
        }
        let steps: Vec<_> = item.steps().collect();
        if !steps.is_empty() {
            text.push_str("<ol>\n");
            for (number, step) in steps {
                writeln!(
                    text,
                    "<li value=\"{number}\">{}</li>",
                    rich::to_html(&rich::parse(step))
                )?;
            }
            text.push_str("</ol>\n");
        }
        let alt = if heading.is_empty() {
            rich::plain(&rich::parse(&item.description))
        } else {
            heading
        };
        let figure = format!(
            "<figure><img src=\"data:image/png;base64,{}\" width=\"{}\" \
             alt=\"{}\"></figure>\n",
            base64(&png_bytes(img)?),
            img.width(),
            escape(&alt),
        );
        let text = if text.is_empty() {
            text
        } else {
            format!("<div>\n{text}</div>\n")
        };
        let has_text = !text.is_empty();
        let (first, second) = if options.image_before_text {
            (figure, text)
        } else {
            (text, figure)
        };
        out.push_str(&first);
        // Saut de page à l'impression : la classe va sur le second bloc.
        if options.page_break && has_text {
            out.push_str(&second.replacen('>', " class=\"break\">", 1));
        } else {
            out.push_str(&second);
        }
        out.push_str("</section>\n");
    }
    out.push_str("</main>\n</body>\n</html>\n");
    Ok(out)
}

/// Base64 standard (RFC 4648), avec remplissage.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_rfc_examples() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }
}
