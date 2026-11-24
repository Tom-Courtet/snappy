//! Texte enrichi des descriptions : le « HTML simple » de Screenpresso (§ 7.3).
//!
//! Seules les balises `<b>` (`<strong>`), `<i>` (`<em>`), `<u>` et `<br>` sont
//! comprises ; toute autre balise reste du texte, affiché tel quel (et échappé en
//! HTML). Un retour à la ligne tapé dans le champ vaut un `<br>`.

/// Un morceau de texte et son style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Élément d'un paragraphe : du texte, ou un passage à la ligne.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Run(Run),
    Break,
}

/// Découpe une description en morceaux stylés.
pub fn parse(source: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let (mut bold, mut italic, mut underline) = (0u32, 0u32, 0u32);
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut Vec<Piece>, b: u32, i: u32, u: u32| {
        if !text.is_empty() {
            out.push(Piece::Run(Run {
                text: decode_entities(text),
                bold: b > 0,
                italic: i > 0,
                underline: u > 0,
            }));
            text.clear();
        }
    };
    let mut rest = source;
    while let Some(c) = rest.chars().next() {
        if c == '\n' {
            flush(&mut text, &mut out, bold, italic, underline);
            out.push(Piece::Break);
            rest = &rest[1..];
            continue;
        }
        if c == '\r' {
            rest = &rest[1..];
            continue;
        }
        if c == '<'
            && let Some(end) = rest.find('>')
            && let Some(tag) = Tag::parse(&rest[1..end])
        {
            flush(&mut text, &mut out, bold, italic, underline);
            match tag {
                Tag::Break => out.push(Piece::Break),
                Tag::Style(counter, open) => {
                    let n = match counter {
                        Style::Bold => &mut bold,
                        Style::Italic => &mut italic,
                        Style::Underline => &mut underline,
                    };
                    *n = if open { *n + 1 } else { n.saturating_sub(1) };
                }
            }
            rest = &rest[end + 1..];
            continue;
        }
        text.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut text, &mut out, bold, italic, underline);
    out
}

/// Texte sans style (titre de secours, texte alternatif).
pub fn plain(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Run(r) => r.text.as_str(),
            Piece::Break => " ",
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Les morceaux réécrits en HTML sûr : texte échappé, balises connues seulement.
pub fn to_html(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Break => out.push_str("<br>"),
            Piece::Run(r) => {
                let (open, close) = wrap(r);
                out.push_str(&open);
                out.push_str(&escape(&r.text));
                out.push_str(&close);
            }
        }
    }
    out
}

fn wrap(r: &Run) -> (String, String) {
    let tags: Vec<&str> = [(r.bold, "b"), (r.italic, "i"), (r.underline, "u")]
        .into_iter()
        .filter_map(|(on, t)| on.then_some(t))
        .collect();
    let open = tags.iter().map(|t| format!("<{t}>")).collect();
    let close = tags.iter().rev().map(|t| format!("</{t}>")).collect();
    (open, close)
}

/// Échappe un texte pour le HTML (contenu et valeurs d'attributs).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Style {
    Bold,
    Italic,
    Underline,
}

enum Tag {
    Break,
    /// Style, et `true` pour une balise ouvrante.
    Style(Style, bool),
}

impl Tag {
    /// `inner` : ce qui est entre `<` et `>`.
    fn parse(inner: &str) -> Option<Self> {
        let inner = inner.trim().to_ascii_lowercase();
        let (closing, name) = match inner.strip_prefix('/') {
            Some(rest) => (true, rest.trim()),
            None => (false, inner.as_str()),
        };
        let name = name.trim_end_matches('/').trim();
        let style = match name {
            "br" => return Some(Tag::Break),
            "b" | "strong" => Style::Bold,
            "i" | "em" => Style::Italic,
            "u" => Style::Underline,
            _ => return None,
        };
        Some(Tag::Style(style, !closing))
    }
}

/// Les entités les plus courantes ; les autres restent telles quelles.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", "\u{a0}")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, bold: bool, italic: bool, underline: bool) -> Piece {
        Piece::Run(Run {
            text: text.into(),
            bold,
            italic,
            underline,
        })
    }

    #[test]
    fn known_tags_become_styles() {
        assert_eq!(
            parse("a <b>gras <i>et</i></b><br/><U>souligné</u>"),
            vec![
                run("a ", false, false, false),
                run("gras ", true, false, false),
                run("et", true, true, false),
                Piece::Break,
                run("souligné", false, false, true),
            ]
        );
    }

    #[test]
    fn unknown_tags_and_newlines() {
        assert_eq!(
            parse("<script>x</script>\n1 < 2 &amp; 3"),
            vec![
                run("<script>x</script>", false, false, false),
                Piece::Break,
                run("1 < 2 & 3", false, false, false),
            ]
        );
    }

    #[test]
    fn html_output_is_escaped() {
        let html = to_html(&parse("<b>a</b> <img src=x onerror=alert(1)>"));
        assert_eq!(html, "<b>a</b> &lt;img src=x onerror=alert(1)&gt;");
        assert_eq!(plain(&parse("<b>Titre</b><br>suite")), "Titre suite");
    }

    #[test]
    fn unbalanced_closing_tags_are_harmless() {
        assert_eq!(parse("</b>x"), vec![run("x", false, false, false)]);
    }
}
