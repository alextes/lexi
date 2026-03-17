use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// escape text for telegram HTML mode.
/// escapes <, >, and & characters.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// convert markdown to telegram-compatible HTML.
pub fn markdown_to_telegram_html(md: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(md, options);
    let mut out = String::with_capacity(md.len());
    let mut in_pre = false;
    let mut list_stack: Vec<ListKind> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => out.push_str("<b>"),
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(kind) => {
                    in_pre = true;
                    match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                            out.push_str(&format!(
                                "<pre><code class=\"language-{}\">",
                                escape_html(&lang)
                            ));
                        }
                        _ => out.push_str("<pre>"),
                    }
                }
                Tag::List(first_item) => {
                    let kind = match first_item {
                        Some(start) => ListKind::Ordered(start),
                        None => ListKind::Unordered,
                    };
                    list_stack.push(kind);
                }
                Tag::Item => {
                    if let Some(kind) = list_stack.last_mut() {
                        match kind {
                            ListKind::Unordered => out.push_str("• "),
                            ListKind::Ordered(n) => {
                                out.push_str(&format!("{}. ", n));
                                *n += 1;
                            }
                        }
                    }
                }
                Tag::Emphasis => out.push_str("<i>"),
                Tag::Strong => out.push_str("<b>"),
                Tag::Strikethrough => out.push_str("<s>"),
                Tag::Link { dest_url, .. } => {
                    out.push_str(&format!("<a href=\"{}\">", escape_html(&dest_url)));
                }
                Tag::Image { dest_url, .. } => {
                    out.push_str(&format!("<a href=\"{}\">", escape_html(&dest_url)));
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => out.push_str("\n\n"),
                TagEnd::Heading(_) => {
                    out.push_str("</b>\n\n");
                }
                TagEnd::BlockQuote(_) => {
                    let trimmed = out.trim_end().len();
                    out.truncate(trimmed);
                    out.push_str("</blockquote>\n\n");
                }
                TagEnd::CodeBlock => {
                    if out.contains("<code class=\"language-")
                        && in_pre
                        && !out.ends_with("</code></pre>")
                    {
                        if out.ends_with('\n') {
                            out.pop();
                        }
                        out.push_str("</code></pre>\n\n");
                    } else {
                        if out.ends_with('\n') {
                            out.pop();
                        }
                        out.push_str("</pre>\n\n");
                    }
                    in_pre = false;
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    if list_stack.is_empty() {
                        out.push('\n');
                    }
                }
                TagEnd::Item => {
                    while out.ends_with('\n') {
                        out.pop();
                    }
                    out.push('\n');
                }
                TagEnd::Emphasis => out.push_str("</i>"),
                TagEnd::Strong => out.push_str("</b>"),
                TagEnd::Strikethrough => out.push_str("</s>"),
                TagEnd::Link => out.push_str("</a>"),
                TagEnd::Image => out.push_str("</a>"),
                _ => {}
            },
            Event::Text(text) => {
                out.push_str(&escape_html(&text));
            }
            Event::Code(code) => {
                out.push_str("<code>");
                out.push_str(&escape_html(&code));
                out.push_str("</code>");
            }
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push('\n'),
            Event::Rule => out.push_str("———\n\n"),
            Event::Html(html) | Event::InlineHtml(html) => {
                out.push_str(&escape_html(&html));
            }
            _ => {}
        }
    }

    out.trim_end().to_string()
}

enum ListKind {
    Ordered(u64),
    Unordered,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold() {
        assert_eq!(markdown_to_telegram_html("**hello**"), "<b>hello</b>");
    }

    #[test]
    fn test_italic() {
        assert_eq!(markdown_to_telegram_html("*hello*"), "<i>hello</i>");
    }

    #[test]
    fn test_code_inline() {
        assert_eq!(markdown_to_telegram_html("`foo`"), "<code>foo</code>");
    }

    #[test]
    fn test_code_block() {
        let input = "```rust\nfn main() {}\n```";
        let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }

    #[test]
    fn test_code_block_no_lang() {
        let input = "```\nhello\n```";
        let expected = "<pre>hello</pre>";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }

    #[test]
    fn test_link() {
        assert_eq!(
            markdown_to_telegram_html("[text](https://example.com)"),
            "<a href=\"https://example.com\">text</a>"
        );
    }

    #[test]
    fn test_blockquote() {
        assert_eq!(
            markdown_to_telegram_html("> text"),
            "<blockquote>text</blockquote>"
        );
    }

    #[test]
    fn test_heading_as_bold() {
        assert_eq!(markdown_to_telegram_html("# title"), "<b>title</b>");
    }

    #[test]
    fn test_strikethrough() {
        assert_eq!(markdown_to_telegram_html("~~text~~"), "<s>text</s>");
    }

    #[test]
    fn test_unordered_list() {
        let input = "- a\n- b";
        let expected = "• a\n• b";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }

    #[test]
    fn test_ordered_list() {
        let input = "1. a\n2. b";
        let expected = "1. a\n2. b";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }

    #[test]
    fn test_escapes_html_entities() {
        assert_eq!(markdown_to_telegram_html("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn test_nested_formatting() {
        assert_eq!(
            markdown_to_telegram_html("**bold *and italic***"),
            "<b>bold <i>and italic</i></b>"
        );
    }

    #[test]
    fn test_plain_text_passthrough() {
        assert_eq!(markdown_to_telegram_html("hello world"), "hello world");
    }

    #[test]
    fn test_rule() {
        let input = "above\n\n---\n\nbelow";
        let expected = "above\n\n———\n\nbelow";
        assert_eq!(markdown_to_telegram_html(input), expected);
    }
}
