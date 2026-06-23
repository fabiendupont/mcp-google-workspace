use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Default)]
pub struct MarpPresentation {
    pub frontmatter: MarpFrontmatter,
    pub slides: Vec<MarpSlide>,
}

#[derive(Debug, Clone, Default)]
pub struct MarpFrontmatter {
    pub theme: Option<String>,
    pub paginate: Option<bool>,
    pub background_color: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MarpSlide {
    pub title: Option<String>,
    pub body_blocks: Vec<SlideBlock>,
    pub speaker_notes: Option<String>,
    pub directives: SlideDirectives,
}

#[derive(Debug, Clone, Default)]
pub struct SlideDirectives {
    pub background_color: Option<String>,
    pub background_image: Option<String>,
    pub color: Option<String>,
    pub class: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SlideBlock {
    Text {
        text: String,
        styles: Vec<MarpInlineStyle>,
    },
    BulletList {
        items: Vec<ListItem>,
        ordered: bool,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Image {
        url: String,
        width: Option<f64>,
        height: Option<f64>,
        is_background: bool,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
}

#[derive(Debug, Clone)]
pub struct ListItem {
    pub text: String,
    pub styles: Vec<MarpInlineStyle>,
}

#[derive(Debug, Clone)]
pub struct MarpInlineStyle {
    pub start: usize,
    pub end: usize,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link_url: Option<String>,
}

pub fn parse_marp(input: &str) -> Result<MarpPresentation, String> {
    let (frontmatter, remainder) = extract_frontmatter(input);
    let raw_sections = split_slides(remainder);

    let mut slides = Vec::new();
    for section in &raw_sections {
        if !section.trim().is_empty() {
            slides.push(parse_slide(section, &frontmatter));
        }
    }

    if slides.is_empty() {
        slides.push(MarpSlide::default());
    }

    Ok(MarpPresentation {
        frontmatter,
        slides,
    })
}

fn split_slides(input: &str) -> Vec<&str> {
    let mut sections = Vec::new();
    let mut start = 0;
    let mut in_code_block = false;
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if i == 0 || bytes[i - 1] == b'\n' {
            if bytes[i] == b'`' && i + 2 < len && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
                in_code_block = !in_code_block;
                i += 3;
                continue;
            }
            if !in_code_block
                && bytes[i] == b'-'
                && i + 2 < len
                && bytes[i + 1] == b'-'
                && bytes[i + 2] == b'-'
            {
                let sep_end = i + 3;
                let is_sep = sep_end >= len
                    || bytes[sep_end] == b'\n'
                    || (bytes[sep_end] == b'\r' && sep_end + 1 < len && bytes[sep_end + 1] == b'\n');
                if is_sep {
                    let section_end = if i > 0 { i - 1 } else { i };
                    sections.push(&input[start..section_end]);
                    start = if sep_end < len {
                        let mut s = sep_end;
                        if s < len && bytes[s] == b'\n' {
                            s += 1;
                        } else if s + 1 < len && bytes[s] == b'\r' && bytes[s + 1] == b'\n' {
                            s += 2;
                        }
                        s
                    } else {
                        sep_end
                    };
                    i = start;
                    continue;
                }
            }
        }
        i += 1;
    }

    if start <= len {
        sections.push(&input[start..]);
    }

    sections
}

fn extract_frontmatter(first_section: &str) -> (MarpFrontmatter, &str) {
    let trimmed = first_section.trim_start();

    if !trimmed.starts_with("---") {
        return (MarpFrontmatter::default(), first_section);
    }

    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(
        after_first.strip_prefix("\r\n").unwrap_or(after_first),
    );

    if let Some(end_pos) = after_first.find("\n---") {
        let yaml = &after_first[..end_pos];
        let rest_start = end_pos + 4;
        let rest = if rest_start < after_first.len() {
            let r = &after_first[rest_start..];
            r.strip_prefix('\n')
                .unwrap_or(r.strip_prefix("\r\n").unwrap_or(r))
        } else {
            ""
        };

        let fm = parse_frontmatter_yaml(yaml);
        (fm, rest)
    } else {
        (MarpFrontmatter::default(), first_section)
    }
}

fn parse_frontmatter_yaml(yaml: &str) -> MarpFrontmatter {
    let mut fm = MarpFrontmatter::default();
    for line in yaml.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            match key {
                "theme" => fm.theme = Some(value.to_string()),
                "paginate" => fm.paginate = Some(value == "true"),
                "backgroundColor" => fm.background_color = Some(value.to_string()),
                "color" => fm.color = Some(value.to_string()),
                _ => {}
            }
        }
    }
    fm
}

fn parse_slide(content: &str, global: &MarpFrontmatter) -> MarpSlide {
    let (directives, speaker_notes, body) = extract_slide_metadata(content, global);
    let (title, body_blocks) = parse_slide_body(body);

    MarpSlide {
        title,
        body_blocks,
        speaker_notes,
        directives,
    }
}

fn extract_slide_metadata<'a>(
    content: &'a str,
    global: &MarpFrontmatter,
) -> (SlideDirectives, Option<String>, &'a str) {
    let mut directives = SlideDirectives {
        background_color: global.background_color.clone(),
        background_image: None,
        color: global.color.clone(),
        class: None,
    };
    let mut notes: Option<String> = None;
    let body_start = 0;
    let mut body_end = content.len();

    let mut remaining = content;
    let mut processed_len = 0;

    while let Some(start) = remaining.find("<!--") {
        if let Some(end) = remaining[start..].find("-->") {
            let comment_inner = remaining[start + 4..start + end].trim();
            let comment_end = start + end + 3;

            if comment_inner.eq_ignore_ascii_case("notes") {
                let notes_start = comment_end;
                let notes_content = remaining[notes_start..].trim();
                notes = if notes_content.is_empty() {
                    None
                } else {
                    Some(notes_content.to_string())
                };
                body_end = processed_len + start;
                break;
            }

            if let Some((key, value)) = comment_inner.split_once(':') {
                let key = key.trim();
                let value = value.trim();
                let is_scoped = key.starts_with('_');
                let key = key.strip_prefix('_').unwrap_or(key);

                match key {
                    "backgroundColor" => {
                        if is_scoped || directives.background_color.is_none() {
                            directives.background_color = Some(value.to_string());
                        }
                    }
                    "color" => {
                        if is_scoped || directives.color.is_none() {
                            directives.color = Some(value.to_string());
                        }
                    }
                    "class" => {
                        directives.class = Some(value.to_string());
                    }
                    _ => {}
                }
            }

            processed_len += comment_end;
            remaining = &remaining[comment_end..];
        } else {
            break;
        }
    }

    let body = &content[body_start..body_end];
    let body = strip_html_comments(body);
    let body_ref = body.as_str();

    // We need to return owned data for the body since we stripped comments.
    // But the caller expects &str. We'll handle this by returning the cleaned body
    // as part of the slide parsing instead.
    // For now, return the raw body range and strip comments in parse_slide_body.
    let _ = body_ref;

    (directives, notes, &content[body_start..body_end])
}

fn strip_html_comments(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find("<!--") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find("-->") {
            remaining = &remaining[start + end + 3..];
        } else {
            remaining = &remaining[start..];
            break;
        }
    }
    result.push_str(remaining);
    result
}

fn parse_slide_body(content: &str) -> (Option<String>, Vec<SlideBlock>) {
    let clean = strip_html_comments(content);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(&clean, options);

    let mut title: Option<String> = None;
    let mut blocks: Vec<SlideBlock> = Vec::new();

    let mut para_text = String::new();
    let mut para_styles: Vec<MarpInlineStyle> = Vec::new();
    let mut para_char_count: usize = 0;

    let mut bold_depth = 0u32;
    let mut italic_depth = 0u32;
    let mut link_url_stack: Vec<String> = Vec::new();

    let mut in_heading = false;
    let mut heading_text = String::new();

    let mut code_block = false;
    let mut code_block_text = String::new();
    let mut code_block_lang: Option<String> = None;

    let mut list_stack: Vec<bool> = Vec::new();
    let mut in_list_item = false;
    let mut list_items: Vec<ListItem> = Vec::new();

    let mut in_image = false;
    let mut image_url: Option<String> = None;
    let mut image_alt: String = String::new();

    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_row: Vec<String> = Vec::new();
    let mut table_cell_buf = String::new();

    let flush_list = |items: &mut Vec<ListItem>, blocks: &mut Vec<SlideBlock>, ordered: bool| {
        if !items.is_empty() {
            blocks.push(SlideBlock::BulletList {
                items: items.drain(..).collect(),
                ordered,
            });
        }
    };

    let mut last_list_ordered = false;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                in_heading = true;
                heading_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                if title.is_none() {
                    title = Some(heading_text.clone());
                } else {
                    if !para_text.is_empty() {
                        blocks.push(SlideBlock::Text {
                            text: para_text.clone(),
                            styles: para_styles.clone(),
                        });
                        para_text.clear();
                        para_styles.clear();
                        para_char_count = 0;
                    }
                    blocks.push(SlideBlock::Text {
                        text: heading_text.clone(),
                        styles: vec![MarpInlineStyle {
                            start: 0,
                            end: heading_text.len(),
                            bold: true,
                            italic: false,
                            code: false,
                            link_url: None,
                        }],
                    });
                }
                heading_text.clear();
            }
            Event::Start(Tag::Paragraph) => {
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::End(TagEnd::Paragraph) => {
                if in_list_item || in_image {
                    continue;
                }
                if !para_text.is_empty() {
                    blocks.push(SlideBlock::Text {
                        text: para_text.clone(),
                        styles: para_styles.clone(),
                    });
                }
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::Start(Tag::List(first_num)) => {
                let ordered = first_num.is_some();
                last_list_ordered = ordered;
                list_stack.push(ordered);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                if list_stack.is_empty() {
                    flush_list(&mut list_items, &mut blocks, last_list_ordered);
                }
            }
            Event::Start(Tag::Item) => {
                in_list_item = true;
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::End(TagEnd::Item) => {
                in_list_item = false;
                list_items.push(ListItem {
                    text: para_text.clone(),
                    styles: para_styles.clone(),
                });
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::Start(Tag::Strong) => {
                bold_depth += 1;
            }
            Event::End(TagEnd::Strong) => {
                bold_depth = bold_depth.saturating_sub(1);
            }
            Event::Start(Tag::Emphasis) => {
                italic_depth += 1;
            }
            Event::End(TagEnd::Emphasis) => {
                italic_depth = italic_depth.saturating_sub(1);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url_stack.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                link_url_stack.pop();
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                image_url = Some(dest_url.to_string());
                image_alt.clear();
                in_image = true;
            }
            Event::End(TagEnd::Image) => {
                if let Some(url) = image_url.take() {
                    let (is_bg, width, height) = parse_image_alt(&image_alt);
                    blocks.push(SlideBlock::Image {
                        url,
                        width,
                        height,
                        is_background: is_bg,
                    });
                }
                in_image = false;
                image_alt.clear();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                code_block = true;
                code_block_text.clear();
                code_block_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                        let lang = lang.as_ref().trim();
                        if lang.is_empty() {
                            None
                        } else {
                            Some(lang.to_string())
                        }
                    }
                    _ => None,
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                code_block = false;
                blocks.push(SlideBlock::CodeBlock {
                    language: code_block_lang.take(),
                    code: code_block_text.clone(),
                });
                code_block_text.clear();
            }
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                if !table_rows.is_empty() {
                    blocks.push(SlideBlock::Table {
                        rows: table_rows.clone(),
                    });
                }
                in_table = false;
                table_rows.clear();
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                table_rows.push(table_row.clone());
                table_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                table_cell_buf.clear();
            }
            Event::End(TagEnd::TableCell) => {
                table_row.push(table_cell_buf.trim().to_string());
                table_cell_buf.clear();
            }
            Event::Text(t) => {
                if in_table {
                    table_cell_buf.push_str(t.as_ref());
                    continue;
                }
                if in_image {
                    image_alt.push_str(t.as_ref());
                    continue;
                }
                if code_block {
                    code_block_text.push_str(t.as_ref());
                    continue;
                }
                if in_heading {
                    heading_text.push_str(t.as_ref());
                    continue;
                }
                let s = t.as_ref();
                let range_start = para_char_count;
                para_text.push_str(s);
                para_char_count += s.chars().count();
                let range_end = para_char_count;

                let has_style =
                    bold_depth > 0 || italic_depth > 0 || !link_url_stack.is_empty();

                if has_style && range_start < range_end {
                    para_styles.push(MarpInlineStyle {
                        start: range_start,
                        end: range_end,
                        bold: bold_depth > 0,
                        italic: italic_depth > 0,
                        code: false,
                        link_url: link_url_stack.last().cloned(),
                    });
                }
            }
            Event::Code(t) => {
                if in_heading {
                    heading_text.push_str(t.as_ref());
                    continue;
                }
                let s = t.as_ref();
                let range_start = para_char_count;
                para_text.push_str(s);
                para_char_count += s.chars().count();
                let range_end = para_char_count;

                if range_start < range_end {
                    para_styles.push(MarpInlineStyle {
                        start: range_start,
                        end: range_end,
                        bold: bold_depth > 0,
                        italic: italic_depth > 0,
                        code: true,
                        link_url: link_url_stack.last().cloned(),
                    });
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_heading {
                    heading_text.push(' ');
                } else {
                    para_text.push('\n');
                    para_char_count += 1;
                }
            }
            _ => {}
        }
    }

    if !para_text.is_empty() {
        blocks.push(SlideBlock::Text {
            text: para_text,
            styles: para_styles,
        });
    }

    (title, blocks)
}

fn parse_image_alt(alt: &str) -> (bool, Option<f64>, Option<f64>) {
    let is_bg = alt.contains("bg");
    let mut width = None;
    let mut height = None;

    for token in alt.split_whitespace() {
        if let Some(w) = token
            .strip_prefix("w:")
            .or_else(|| token.strip_prefix("width:"))
        {
            width = parse_dimension(w);
        }
        if let Some(h) = token
            .strip_prefix("h:")
            .or_else(|| token.strip_prefix("height:"))
        {
            height = parse_dimension(h);
        }
    }

    (is_bg, width, height)
}

fn parse_dimension(s: &str) -> Option<f64> {
    let s = s.trim_end_matches("px").trim_end_matches("pt");
    s.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let pres = parse_marp("").unwrap();
        assert_eq!(pres.slides.len(), 1);
    }

    #[test]
    fn test_frontmatter_extraction() {
        let input = "---\nmarp: true\ntheme: default\npaginate: true\nbackgroundColor: \"#fff\"\ncolor: '#333'\n---\n# Hello";
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.frontmatter.theme.as_deref(), Some("default"));
        assert_eq!(pres.frontmatter.paginate, Some(true));
        assert_eq!(pres.frontmatter.background_color.as_deref(), Some("#fff"));
        assert_eq!(pres.frontmatter.color.as_deref(), Some("#333"));
        assert_eq!(pres.slides.len(), 1);
        assert_eq!(pres.slides[0].title.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_slide_separator() {
        let input = "# Slide 1\n\nContent\n\n---\n\n# Slide 2\n\nMore content";
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.slides.len(), 2);
        assert_eq!(pres.slides[0].title.as_deref(), Some("Slide 1"));
        assert_eq!(pres.slides[1].title.as_deref(), Some("Slide 2"));
    }

    #[test]
    fn test_no_split_inside_code_block() {
        let input = "# Slide\n\n```\ncode\n---\nmore code\n```\n\n---\n\n# Next";
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.slides.len(), 2);
        assert_eq!(pres.slides[0].title.as_deref(), Some("Slide"));
    }

    #[test]
    fn test_heading_as_title() {
        let input = "# My Title\n\nSome body text";
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.slides[0].title.as_deref(), Some("My Title"));
        assert!(!pres.slides[0].body_blocks.is_empty());
    }

    #[test]
    fn test_background_image() {
        let input = "![bg](https://example.com/bg.jpg)";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            SlideBlock::Image {
                url,
                is_background,
                ..
            } => {
                assert_eq!(url, "https://example.com/bg.jpg");
                assert!(is_background);
            }
            _ => panic!("Expected Image block"),
        }
    }

    #[test]
    fn test_inline_image_with_size() {
        let input = "![w:200 h:150](https://example.com/img.png)";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        match &blocks[0] {
            SlideBlock::Image {
                url,
                width,
                height,
                is_background,
            } => {
                assert_eq!(url, "https://example.com/img.png");
                assert_eq!(*width, Some(200.0));
                assert_eq!(*height, Some(150.0));
                assert!(!is_background);
            }
            _ => panic!("Expected Image block"),
        }
    }

    #[test]
    fn test_speaker_notes() {
        let input = "# Title\n\nContent\n\n<!-- notes -->\nThese are my notes";
        let pres = parse_marp(input).unwrap();
        assert!(pres.slides[0].speaker_notes.is_some());
        let notes = pres.slides[0].speaker_notes.as_deref().unwrap();
        assert!(notes.contains("These are my notes"));
    }

    #[test]
    fn test_directives_scoped() {
        let input = "<!-- _backgroundColor: #000 -->\n# Dark Slide";
        let pres = parse_marp(input).unwrap();
        assert_eq!(
            pres.slides[0].directives.background_color.as_deref(),
            Some("#000")
        );
    }

    #[test]
    fn test_global_color_inherited() {
        let input = "---\ncolor: '#red'\n---\n# Slide 1\n\n---\n\n# Slide 2";
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.slides[0].directives.color.as_deref(), Some("#red"));
        assert_eq!(pres.slides[1].directives.color.as_deref(), Some("#red"));
    }

    #[test]
    fn test_bullet_list() {
        let input = "- Item 1\n- Item 2\n- Item 3";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        match &blocks[0] {
            SlideBlock::BulletList { items, ordered } => {
                assert_eq!(items.len(), 3);
                assert!(!ordered);
                assert_eq!(items[0].text, "Item 1");
            }
            _ => panic!("Expected BulletList block"),
        }
    }

    #[test]
    fn test_ordered_list() {
        let input = "1. First\n2. Second";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        match &blocks[0] {
            SlideBlock::BulletList { items, ordered } => {
                assert_eq!(items.len(), 2);
                assert!(ordered);
            }
            _ => panic!("Expected BulletList block"),
        }
    }

    #[test]
    fn test_code_block() {
        let input = "```rust\nfn main() {}\n```";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        match &blocks[0] {
            SlideBlock::CodeBlock { language, code } => {
                assert_eq!(language.as_deref(), Some("rust"));
                assert!(code.contains("fn main()"));
            }
            _ => panic!("Expected CodeBlock"),
        }
    }

    #[test]
    fn test_inline_styles() {
        let input = "This is **bold** and *italic* text";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        match &blocks[0] {
            SlideBlock::Text { styles, .. } => {
                let bold_styles: Vec<_> = styles.iter().filter(|s| s.bold).collect();
                let italic_styles: Vec<_> = styles.iter().filter(|s| s.italic).collect();
                assert!(!bold_styles.is_empty());
                assert!(!italic_styles.is_empty());
            }
            _ => panic!("Expected Text block"),
        }
    }

    #[test]
    fn test_inline_code() {
        let input = "Use `println!` to print";
        let pres = parse_marp(input).unwrap();
        let blocks = &pres.slides[0].body_blocks;
        match &blocks[0] {
            SlideBlock::Text { styles, .. } => {
                let code_styles: Vec<_> = styles.iter().filter(|s| s.code).collect();
                assert!(!code_styles.is_empty());
            }
            _ => panic!("Expected Text block"),
        }
    }

    #[test]
    fn test_multi_slide_full() {
        let input = r#"---
marp: true
theme: gaia
paginate: true
---

# Welcome

Intro text with **bold**

- Point 1
- Point 2

---

<!-- _backgroundColor: #1a1a2e -->

# Architecture

![bg](https://example.com/arch.png)

```rust
fn main() {}
```

<!-- notes -->
Remember to explain the architecture diagram
"#;
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.frontmatter.theme.as_deref(), Some("gaia"));
        assert_eq!(pres.frontmatter.paginate, Some(true));
        assert_eq!(pres.slides.len(), 2);

        assert_eq!(pres.slides[0].title.as_deref(), Some("Welcome"));
        assert!(pres.slides[0]
            .body_blocks
            .iter()
            .any(|b| matches!(b, SlideBlock::BulletList { .. })));

        assert_eq!(pres.slides[1].title.as_deref(), Some("Architecture"));
        assert_eq!(
            pres.slides[1].directives.background_color.as_deref(),
            Some("#1a1a2e")
        );
        assert!(pres.slides[1].speaker_notes.is_some());
    }

    #[test]
    fn test_frontmatter_only() {
        let input = "---\nmarp: true\ntheme: default\n---\n";
        let pres = parse_marp(input).unwrap();
        assert_eq!(pres.frontmatter.theme.as_deref(), Some("default"));
        assert_eq!(pres.slides.len(), 1);
    }

    #[test]
    fn test_image_alt_parsing() {
        assert_eq!(parse_image_alt("bg"), (true, None, None));
        assert_eq!(parse_image_alt("bg left"), (true, None, None));
        assert_eq!(
            parse_image_alt("w:200px h:100px"),
            (false, Some(200.0), Some(100.0))
        );
        assert_eq!(
            parse_image_alt("width:300 height:200"),
            (false, Some(300.0), Some(200.0))
        );
        assert_eq!(parse_image_alt(""), (false, None, None));
    }
}
