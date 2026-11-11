//! Text for the bar: Pango markup escaping.

/// Escapes text for Pango markup, which waybar renders.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
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

#[cfg(test)]
mod tests {
    #[test]
    fn escapes() {
        assert_eq!(super::escape("a<b>&'\""), "a&lt;b&gt;&amp;&#39;&quot;");
    }
}
