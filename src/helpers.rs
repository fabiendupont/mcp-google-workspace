use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::server::ServerState;
use crate::tools;
use google_workspace::error::GwsError;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub enum Position {
    End,
    Start,
    Index(i32),
}

#[derive(Debug, Clone, Default)]
pub struct TextStyle {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub font_size_pt: Option<f64>,
    pub font_family: Option<String>,
    pub foreground_color: Option<String>,
    pub background_color: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ParagraphStyle {
    pub named_style: Option<String>,
    pub alignment: Option<String>,
}

pub fn hex_to_rgb_color(hex: &str) -> Value {
    let hex = hex.trim_start_matches('#');
    let (r, g, b) = if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        (r, g, b)
    } else {
        (0, 0, 0)
    };
    json!({
        "color": {
            "rgbColor": {
                "red": r as f64 / 255.0,
                "green": g as f64 / 255.0,
                "blue": b as f64 / 255.0
            }
        }
    })
}

enum LocationField {
    EndOfSegment,
    Index(Value),
}

fn position_to_location_field(position: &Position) -> LocationField {
    match position {
        Position::End => LocationField::EndOfSegment,
        Position::Start => LocationField::Index(json!({ "index": 1 })),
        Position::Index(i) => LocationField::Index(json!({ "index": i })),
    }
}

fn position_to_index(position: &Position) -> i32 {
    match position {
        Position::End => -1,
        Position::Start => 1,
        Position::Index(i) => *i,
    }
}

fn build_text_style_value(style: &TextStyle) -> (Value, String) {
    let mut obj = serde_json::Map::new();
    let mut fields = Vec::new();

    if let Some(bold) = style.bold {
        obj.insert("bold".to_string(), json!(bold));
        fields.push("bold");
    }
    if let Some(italic) = style.italic {
        obj.insert("italic".to_string(), json!(italic));
        fields.push("italic");
    }
    if let Some(size) = style.font_size_pt {
        obj.insert(
            "fontSize".to_string(),
            json!({ "magnitude": size, "unit": "PT" }),
        );
        fields.push("fontSize");
    }
    if let Some(ref family) = style.font_family {
        obj.insert(
            "weightedFontFamily".to_string(),
            json!({ "fontFamily": family }),
        );
        fields.push("weightedFontFamily");
    }
    if let Some(ref fg) = style.foreground_color {
        obj.insert("foregroundColor".to_string(), hex_to_rgb_color(fg));
        fields.push("foregroundColor");
    }
    if let Some(ref bg) = style.background_color {
        obj.insert("backgroundColor".to_string(), hex_to_rgb_color(bg));
        fields.push("backgroundColor");
    }

    (Value::Object(obj), fields.join(","))
}

fn build_paragraph_style_value(style: &ParagraphStyle) -> (Value, String) {
    let mut obj = serde_json::Map::new();
    let mut fields = Vec::new();

    if let Some(ref named) = style.named_style {
        obj.insert("namedStyleType".to_string(), json!(named));
        fields.push("namedStyleType");
    }
    if let Some(ref align) = style.alignment {
        obj.insert("alignment".to_string(), json!(align));
        fields.push("alignment");
    }

    (Value::Object(obj), fields.join(","))
}

pub fn build_insert_text_requests(
    text: &str,
    position: Position,
    style: Option<TextStyle>,
    paragraph_style: Option<&str>,
) -> Vec<Value> {
    let mut requests = Vec::new();

    let loc_field = position_to_location_field(&position);
    let insert_index = position_to_index(&position);

    requests.push(match &loc_field {
        LocationField::EndOfSegment => json!({
            "insertText": {
                "text": text,
                "endOfSegmentLocation": { "segmentId": "" }
            }
        }),
        LocationField::Index(loc) => json!({
            "insertText": {
                "text": text,
                "location": loc
            }
        }),
    });

    if let Some(ref ts) = style {
        let (style_val, fields_mask) = build_text_style_value(ts);
        if !fields_mask.is_empty() {
            let end_index = if insert_index == -1 {
                json!(null)
            } else {
                json!(insert_index + text.len() as i32)
            };
            let start_index = if insert_index == -1 {
                json!(null)
            } else {
                json!(insert_index)
            };
            requests.push(json!({
                "updateTextStyle": {
                    "textStyle": style_val,
                    "fields": fields_mask,
                    "range": {
                        "startIndex": start_index,
                        "endIndex": end_index
                    }
                }
            }));
        }
    }

    if let Some(named_style) = paragraph_style {
        let end_index = if insert_index == -1 {
            json!(null)
        } else {
            json!(insert_index + text.len() as i32)
        };
        let start_index = if insert_index == -1 {
            json!(null)
        } else {
            json!(insert_index)
        };
        requests.push(json!({
            "updateParagraphStyle": {
                "paragraphStyle": { "namedStyleType": named_style },
                "fields": "namedStyleType",
                "range": {
                    "startIndex": start_index,
                    "endIndex": end_index
                }
            }
        }));
    }

    requests
}

pub fn build_insert_table_request(rows: u32, columns: u32, position: Position) -> Value {
    match position_to_location_field(&position) {
        LocationField::EndOfSegment => json!({
            "insertTable": {
                "rows": rows,
                "columns": columns,
                "endOfSegmentLocation": { "segmentId": "" }
            }
        }),
        LocationField::Index(loc) => json!({
            "insertTable": {
                "rows": rows,
                "columns": columns,
                "location": loc
            }
        }),
    }
}

pub fn build_insert_image_request(
    image_url: &str,
    position: Position,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
) -> Value {
    let mut req = match position_to_location_field(&position) {
        LocationField::EndOfSegment => json!({
            "insertInlineImage": {
                "uri": image_url,
                "endOfSegmentLocation": { "segmentId": "" }
            }
        }),
        LocationField::Index(loc) => json!({
            "insertInlineImage": {
                "uri": image_url,
                "location": loc
            }
        }),
    };

    if width_pt.is_some() || height_pt.is_some() {
        let mut size = serde_json::Map::new();
        if let Some(w) = width_pt {
            size.insert("width".to_string(), json!({ "magnitude": w, "unit": "PT" }));
        }
        if let Some(h) = height_pt {
            size.insert(
                "height".to_string(),
                json!({ "magnitude": h, "unit": "PT" }),
            );
        }
        req["insertInlineImage"]["objectSize"] = Value::Object(size);
    }

    req
}

pub fn build_format_text_requests(
    start_index: i32,
    end_index: i32,
    style: TextStyle,
    paragraph_style: Option<ParagraphStyle>,
) -> Vec<Value> {
    let mut requests = Vec::new();

    let (style_val, fields_mask) = build_text_style_value(&style);
    if !fields_mask.is_empty() {
        requests.push(json!({
            "updateTextStyle": {
                "textStyle": style_val,
                "fields": fields_mask,
                "range": {
                    "startIndex": start_index,
                    "endIndex": end_index
                }
            }
        }));
    }

    if let Some(ps) = paragraph_style {
        let (ps_val, ps_fields) = build_paragraph_style_value(&ps);
        if !ps_fields.is_empty() {
            requests.push(json!({
                "updateParagraphStyle": {
                    "paragraphStyle": ps_val,
                    "fields": ps_fields,
                    "range": {
                        "startIndex": start_index,
                        "endIndex": end_index
                    }
                }
            }));
        }
    }

    requests
}

pub fn build_add_bullets_request(start_index: i32, end_index: i32, preset: &str) -> Value {
    json!({
        "createParagraphBullets": {
            "range": {
                "startIndex": start_index,
                "endIndex": end_index
            },
            "bulletPreset": preset
        }
    })
}

fn heading_level_to_style(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "HEADING_1",
        HeadingLevel::H2 => "HEADING_2",
        HeadingLevel::H3 => "HEADING_3",
        HeadingLevel::H4 => "HEADING_4",
        HeadingLevel::H5 => "HEADING_5",
        HeadingLevel::H6 => "HEADING_6",
    }
}

#[derive(Debug, Clone)]
struct InlineStyle {
    start: i32,
    end: i32,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
    link_url: Option<String>,
}

#[derive(Debug, Clone)]
enum Block {
    Paragraph {
        text: String,
        styles: Vec<InlineStyle>,
        heading: Option<String>,
        is_blockquote: bool,
    },
    ListItem {
        text: String,
        styles: Vec<InlineStyle>,
        ordered: bool,
    },
    Table {
        rows: Vec<Vec<String>>,
        header: bool,
    },
    Image {
        url: String,
    },
    HorizontalRule,
    FencedCode {
        text: String,
    },
}

pub fn markdown_to_batch_requests(markdown: &str, start_index: i32) -> Vec<Value> {
    let blocks = parse_markdown_to_blocks(markdown);
    generate_requests_from_blocks(&blocks, start_index)
}

fn parse_markdown_to_blocks(markdown: &str) -> Vec<Block> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(markdown, options);

    let mut blocks: Vec<Block> = Vec::new();

    let mut para_text = String::new();
    let mut para_styles: Vec<InlineStyle> = Vec::new();
    let mut para_char_count: i32 = 0;

    let mut bold_depth = 0u32;
    let mut italic_depth = 0u32;
    let mut strikethrough_depth = 0u32;
    let mut code_block = false;
    let mut code_block_text = String::new();
    let mut in_blockquote = false;
    let mut list_stack: Vec<bool> = Vec::new();
    let mut link_url_stack: Vec<String> = Vec::new();
    let mut in_image = false;
    let mut image_url: Option<String> = None;
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_row: Vec<String> = Vec::new();
    let mut table_cell_buf = String::new();

    let mut current_heading: Option<String> = None;
    let mut in_list_item = false;
    let mut seen_first_h1 = false;
    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let style = if level == HeadingLevel::H1 && !seen_first_h1 {
                    seen_first_h1 = true;
                    "TITLE"
                } else {
                    heading_level_to_style(level)
                };
                current_heading = Some(style.to_string());
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::End(TagEnd::Heading(_)) => {
                if !para_text.ends_with('\n') {
                    para_text.push('\n');
                }
                blocks.push(Block::Paragraph {
                    text: para_text.clone(),
                    styles: para_styles.clone(),
                    heading: current_heading.take(),
                    is_blockquote: false,
                });
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::Start(Tag::Paragraph) => {
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::End(TagEnd::Paragraph) => {
                if in_list_item {
                    continue;
                }
                if !para_text.ends_with('\n') {
                    para_text.push('\n');
                }
                if in_image {
                    continue;
                }
                blocks.push(Block::Paragraph {
                    text: para_text.clone(),
                    styles: para_styles.clone(),
                    heading: Some("NORMAL_TEXT".to_string()),
                    is_blockquote: in_blockquote,
                });
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                in_blockquote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                in_blockquote = false;
            }
            Event::Start(Tag::List(first_num)) => {
                list_stack.push(first_num.is_some());
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                in_list_item = true;
                para_text.clear();
                para_styles.clear();
                para_char_count = 0;
            }
            Event::End(TagEnd::Item) => {
                in_list_item = false;
                if !para_text.ends_with('\n') {
                    para_text.push('\n');
                }
                let ordered = list_stack.last().copied().unwrap_or(false);
                blocks.push(Block::ListItem {
                    text: para_text.clone(),
                    styles: para_styles.clone(),
                    ordered,
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
            Event::Start(Tag::Strikethrough) => {
                strikethrough_depth += 1;
            }
            Event::End(TagEnd::Strikethrough) => {
                strikethrough_depth = strikethrough_depth.saturating_sub(1);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url_stack.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                link_url_stack.pop();
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                image_url = Some(dest_url.to_string());
                in_image = true;
            }
            Event::End(TagEnd::Image) => {
                if let Some(url) = image_url.take() {
                    blocks.push(Block::Image { url });
                }
                in_image = false;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                code_block = true;
                code_block_text.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                code_block = false;
                if !code_block_text.ends_with('\n') {
                    code_block_text.push('\n');
                }
                blocks.push(Block::FencedCode {
                    text: code_block_text.clone(),
                });
                code_block_text.clear();
            }
            Event::Text(t) => {
                if in_image {
                    continue;
                }
                if in_table {
                    table_cell_buf.push_str(t.as_ref());
                    continue;
                }
                if code_block {
                    code_block_text.push_str(t.as_ref());
                    continue;
                }
                let s = t.as_ref();
                let range_start = para_char_count;
                para_text.push_str(s);
                para_char_count += s.chars().count() as i32;
                let range_end = para_char_count;

                let has_style = bold_depth > 0
                    || italic_depth > 0
                    || strikethrough_depth > 0
                    || !link_url_stack.is_empty();

                if has_style && range_start < range_end {
                    para_styles.push(InlineStyle {
                        start: range_start,
                        end: range_end,
                        bold: bold_depth > 0,
                        italic: italic_depth > 0,
                        strikethrough: strikethrough_depth > 0,
                        code: false,
                        link_url: link_url_stack.last().cloned(),
                    });
                }
            }
            Event::Code(t) => {
                let s = t.as_ref();
                let range_start = para_char_count;
                para_text.push_str(s);
                para_char_count += s.chars().count() as i32;
                let range_end = para_char_count;

                if range_start < range_end {
                    para_styles.push(InlineStyle {
                        start: range_start,
                        end: range_end,
                        bold: bold_depth > 0,
                        italic: italic_depth > 0,
                        strikethrough: strikethrough_depth > 0,
                        code: true,
                        link_url: link_url_stack.last().cloned(),
                    });
                }
            }
            Event::SoftBreak => {
                para_text.push(' ');
                para_char_count += 1;
            }
            Event::HardBreak => {
                para_text.push('\n');
                para_char_count += 1;
            }
            Event::Rule => {
                blocks.push(Block::HorizontalRule);
            }
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                if !table_rows.is_empty() {
                    blocks.push(Block::Table {
                        rows: table_rows.clone(),
                        header: true,
                    });
                }
                in_table = false;
                table_rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                table_rows.push(table_row.clone());
                table_row.clear();
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
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
            _ => {}
        }
    }

    blocks
}

fn emit_text_style_request(style: &InlineStyle, base_index: i32) -> Option<Value> {
    let mut ts = serde_json::Map::new();
    let mut fields = Vec::new();

    if style.bold {
        ts.insert("bold".to_string(), json!(true));
        fields.push("bold");
    }
    if style.italic {
        ts.insert("italic".to_string(), json!(true));
        fields.push("italic");
    }
    if style.strikethrough {
        ts.insert("strikethrough".to_string(), json!(true));
        fields.push("strikethrough");
    }
    if style.code {
        ts.insert(
            "weightedFontFamily".to_string(),
            json!({ "fontFamily": "Courier New" }),
        );
        fields.push("weightedFontFamily");
    }
    if let Some(ref url) = style.link_url {
        ts.insert("link".to_string(), json!({ "url": url }));
        fields.push("link");
    }

    if fields.is_empty() {
        return None;
    }

    Some(json!({
        "updateTextStyle": {
            "textStyle": Value::Object(ts),
            "fields": fields.join(","),
            "range": {
                "startIndex": base_index + style.start,
                "endIndex": base_index + style.end
            }
        }
    }))
}

fn generate_requests_from_blocks(blocks: &[Block], start_index: i32) -> Vec<Value> {
    let mut requests: Vec<Value> = Vec::new();
    let mut current_index = start_index;

    let mut pending_bullet_start: Option<i32> = None;
    let mut pending_bullet_end: Option<i32> = None;
    let mut pending_bullet_ordered: Option<bool> = None;

    for (i, block) in blocks.iter().enumerate() {
        let next_is_same_list = match block {
            Block::ListItem { ordered, .. } => {
                if let Some(Block::ListItem {
                    ordered: next_ord, ..
                }) = blocks.get(i + 1)
                {
                    *ordered == *next_ord
                } else {
                    false
                }
            }
            _ => false,
        };

        match block {
            Block::Paragraph {
                text,
                styles,
                heading,
                is_blockquote,
            } => {
                flush_bullets(
                    &mut requests,
                    &mut pending_bullet_start,
                    &mut pending_bullet_end,
                    &mut pending_bullet_ordered,
                );

                let text_chars = text.chars().count() as i32;
                requests.push(json!({
                    "insertText": {
                        "text": text,
                        "location": { "index": current_index }
                    }
                }));

                if let Some(h) = heading {
                    requests.push(json!({
                        "updateParagraphStyle": {
                            "paragraphStyle": { "namedStyleType": h },
                            "fields": "namedStyleType",
                            "range": {
                                "startIndex": current_index,
                                "endIndex": current_index + text_chars
                            }
                        }
                    }));
                }

                if *is_blockquote {
                    requests.push(json!({
                        "updateParagraphStyle": {
                            "paragraphStyle": {
                                "indentStart": { "magnitude": 36, "unit": "PT" }
                            },
                            "fields": "indentStart",
                            "range": {
                                "startIndex": current_index,
                                "endIndex": current_index + text_chars
                            }
                        }
                    }));
                }

                for sr in styles {
                    if let Some(req) = emit_text_style_request(sr, current_index) {
                        requests.push(req);
                    }
                }

                current_index += text_chars;
            }
            Block::ListItem {
                text,
                styles,
                ordered,
            } => {
                let text_chars = text.chars().count() as i32;
                requests.push(json!({
                    "insertText": {
                        "text": text,
                        "location": { "index": current_index }
                    }
                }));

                requests.push(json!({
                    "updateParagraphStyle": {
                        "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                        "fields": "namedStyleType",
                        "range": {
                            "startIndex": current_index,
                            "endIndex": current_index + text_chars
                        }
                    }
                }));

                for sr in styles {
                    if let Some(req) = emit_text_style_request(sr, current_index) {
                        requests.push(req);
                    }
                }

                if pending_bullet_start.is_none() {
                    pending_bullet_start = Some(current_index);
                    pending_bullet_ordered = Some(*ordered);
                }
                pending_bullet_end = Some(current_index + text_chars);

                current_index += text_chars;

                if !next_is_same_list {
                    flush_bullets(
                        &mut requests,
                        &mut pending_bullet_start,
                        &mut pending_bullet_end,
                        &mut pending_bullet_ordered,
                    );
                }
            }
            Block::Table { rows, header } => {
                flush_bullets(
                    &mut requests,
                    &mut pending_bullet_start,
                    &mut pending_bullet_end,
                    &mut pending_bullet_ordered,
                );

                let num_rows = rows.len() as i32;
                let num_cols = rows.first().map(|r| r.len()).unwrap_or(0) as i32;
                if num_cols == 0 || num_rows == 0 {
                    continue;
                }

                // Only emit insertTable — cell population happens in a
                // separate batchUpdate after fetching the doc to get real
                // cell indexes (Google Docs internal table structure can't
                // be reliably pre-calculated).
                requests.push(json!({
                    "insertTable": {
                        "rows": num_rows,
                        "columns": num_cols,
                        "location": { "index": current_index },
                        "_tableData": {
                            "rows": rows,
                            "header": header
                        }
                    }
                }));

                // Empty table: 1 (table) + N*(1 (row) + M*2 (cell + newline)) + 1 (trailing paragraph)
                current_index += 2 + num_rows * (2 * num_cols + 1);
            }
            Block::Image { url } => {
                flush_bullets(
                    &mut requests,
                    &mut pending_bullet_start,
                    &mut pending_bullet_end,
                    &mut pending_bullet_ordered,
                );

                requests.push(json!({
                    "insertInlineImage": {
                        "uri": url,
                        "location": { "index": current_index }
                    }
                }));
                current_index += 1;
            }
            Block::HorizontalRule => {
                flush_bullets(
                    &mut requests,
                    &mut pending_bullet_start,
                    &mut pending_bullet_end,
                    &mut pending_bullet_ordered,
                );

                let rule_text = "\u{2014}\u{2014}\u{2014}\n";
                requests.push(json!({
                    "insertText": {
                        "text": rule_text,
                        "location": { "index": current_index }
                    }
                }));
                current_index += 4;
            }
            Block::FencedCode { text } => {
                flush_bullets(
                    &mut requests,
                    &mut pending_bullet_start,
                    &mut pending_bullet_end,
                    &mut pending_bullet_ordered,
                );

                let text_chars = text.chars().count() as i32;
                requests.push(json!({
                    "insertText": {
                        "text": text,
                        "location": { "index": current_index }
                    }
                }));

                if text_chars > 0 {
                    requests.push(json!({
                        "updateTextStyle": {
                            "textStyle": {
                                "weightedFontFamily": { "fontFamily": "Courier New" }
                            },
                            "fields": "weightedFontFamily",
                            "range": {
                                "startIndex": current_index,
                                "endIndex": current_index + text_chars
                            }
                        }
                    }));
                }

                current_index += text_chars;
            }
        }
    }

    flush_bullets(
        &mut requests,
        &mut pending_bullet_start,
        &mut pending_bullet_end,
        &mut pending_bullet_ordered,
    );

    requests
}

fn flush_bullets(
    requests: &mut Vec<Value>,
    start: &mut Option<i32>,
    end: &mut Option<i32>,
    ordered: &mut Option<bool>,
) {
    if let (Some(s), Some(e), Some(o)) = (*start, *end, *ordered) {
        let preset = if o {
            "NUMBERED_DECIMAL_NESTED"
        } else {
            "BULLET_DISC_CIRCLE_SQUARE"
        };
        requests.push(json!({
            "createParagraphBullets": {
                "range": {
                    "startIndex": s,
                    "endIndex": e
                },
                "bulletPreset": preset
            }
        }));
    }
    *start = None;
    *end = None;
    *ordered = None;
}

pub fn insert_image_tool_schema() -> Value {
    json!({
        "name": "gws_docs_insert_image",
        "title": "Insert Image in Doc",
        "description": "Insert an image into a Google Doc from Drive, URL, or base64 data.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID"
                },
                "drive_file_id": {
                    "type": "string",
                    "description": "Drive file ID of the image (downloaded and embedded, no sharing needed)"
                },
                "image_url": {
                    "type": "string",
                    "description": "Public URL of the image"
                },
                "image_data": {
                    "type": "string",
                    "description": "Base64-encoded image data"
                },
                "image_content_type": {
                    "type": "string",
                    "enum": ["image/png", "image/jpeg", "image/gif"],
                    "default": "image/png"
                },
                "position": {
                    "type": "string",
                    "enum": ["end", "start"],
                    "default": "end"
                },
                "index": {
                    "type": "integer",
                    "description": "Character index (overrides position)"
                },
                "width_pt": { "type": "number", "description": "Width in points" },
                "height_pt": { "type": "number", "description": "Height in points" }
            },
            "required": ["document_id"]
        }
    })
}

pub fn format_tool_schema() -> Value {
    json!({
        "name": "gws_docs_format",
        "title": "Format Text in Doc",
        "description": "Apply bold, italic, color, or font styling to text in a document.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID"
                },
                "text": {
                    "type": "string",
                    "description": "Text to find and format (alternative to index range)"
                },
                "start_index": { "type": "integer", "description": "Range start (1-based, inclusive)" },
                "end_index": { "type": "integer", "description": "Range end (exclusive)" },
                "occurrence": { "type": "integer", "description": "Which occurrence of text (1-based)", "default": 1 },
                "bold": { "type": "boolean" },
                "italic": { "type": "boolean" },
                "font_size_pt": { "type": "number" },
                "font_family": { "type": "string" },
                "foreground_color": { "type": "string", "description": "Hex color, e.g. '#CC0000'" },
                "background_color": { "type": "string", "description": "Hex highlight color" },
                "named_style": {
                    "type": "string",
                    "enum": ["NORMAL_TEXT", "HEADING_1", "HEADING_2", "HEADING_3", "HEADING_4", "HEADING_5", "HEADING_6", "TITLE", "SUBTITLE"]
                },
                "alignment": {
                    "type": "string",
                    "enum": ["START", "CENTER", "END", "JUSTIFIED"]
                }
            },
            "required": ["document_id"]
        }
    })
}

pub fn heading_level(style: &str) -> Option<u32> {
    match style {
        "HEADING_1" => Some(1),
        "HEADING_2" => Some(2),
        "HEADING_3" => Some(3),
        "HEADING_4" => Some(4),
        "HEADING_5" => Some(5),
        "HEADING_6" => Some(6),
        _ => None,
    }
}

pub fn parse_doc_structure(doc: &Value) -> Value {
    let title = doc
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut elements = Vec::new();
    let mut last_end = 0i64;

    if let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) {
        for elem in content {
            let start = elem.get("startIndex").and_then(|v| v.as_i64()).unwrap_or(0);
            let end = elem.get("endIndex").and_then(|v| v.as_i64()).unwrap_or(0);
            last_end = end;

            if let Some(paragraph) = elem.get("paragraph") {
                let style = paragraph
                    .pointer("/paragraphStyle/namedStyleType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("NORMAL_TEXT");

                let text: String = paragraph
                    .get("elements")
                    .and_then(|v| v.as_array())
                    .map(|elems| {
                        elems
                            .iter()
                            .filter_map(|e| e.pointer("/textRun/content").and_then(|v| v.as_str()))
                            .collect::<String>()
                    })
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                if style == "TITLE" {
                    elements.push(json!({
                        "type": "title",
                        "text": text,
                        "startIndex": start,
                        "endIndex": end
                    }));
                } else if style == "SUBTITLE" {
                    elements.push(json!({
                        "type": "subtitle",
                        "text": text,
                        "startIndex": start,
                        "endIndex": end
                    }));
                } else if let Some(level) = heading_level(style) {
                    elements.push(json!({
                        "type": "heading",
                        "level": level,
                        "text": text,
                        "startIndex": start,
                        "endIndex": end
                    }));
                } else if paragraph.get("bullet").is_some() {
                    elements.push(json!({
                        "type": "list_item",
                        "text": text,
                        "startIndex": start,
                        "endIndex": end
                    }));
                } else if !text.is_empty() {
                    elements.push(json!({
                        "type": "paragraph",
                        "preview": if text.len() > 80 { format!("{}...", &text[..77]) } else { text },
                        "startIndex": start,
                        "endIndex": end
                    }));
                }
            } else if let Some(table) = elem.get("table") {
                let rows = table.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
                let columns = table.get("columns").and_then(|v| v.as_u64()).unwrap_or(0);
                elements.push(json!({
                    "type": "table",
                    "rows": rows,
                    "columns": columns,
                    "startIndex": start,
                    "endIndex": end
                }));
            }

            if let Some(inline_objs) = elem
                .get("paragraph")
                .and_then(|p| p.get("elements"))
                .and_then(|v| v.as_array())
            {
                for ie in inline_objs {
                    if ie.get("inlineObjectElement").is_some() {
                        elements.push(json!({
                            "type": "image",
                            "startIndex": ie.get("startIndex").and_then(|v| v.as_i64()).unwrap_or(start),
                            "endIndex": ie.get("endIndex").and_then(|v| v.as_i64()).unwrap_or(end)
                        }));
                    }
                }
            }
        }
    }

    json!({
        "title": title,
        "elements": elements,
        "endIndex": last_end
    })
}

pub fn find_text_in_doc(doc: &Value, needle: &str, occurrence: usize) -> Value {
    let mut found_count = 0usize;

    if let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) {
        for elem in content {
            if let Some(paragraph) = elem.get("paragraph")
                && let Some(elements) = paragraph.get("elements").and_then(|v| v.as_array())
            {
                for pe in elements {
                    if let Some(text_run) = pe.get("textRun") {
                        let text = text_run
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let element_start =
                            pe.get("startIndex").and_then(|v| v.as_i64()).unwrap_or(0);

                        let mut search_from = 0;
                        while let Some(pos) = text[search_from..].find(needle) {
                            found_count += 1;
                            if found_count == occurrence {
                                let abs_start = element_start + (search_from + pos) as i64;
                                let abs_end = abs_start + needle.len() as i64;
                                return json!({
                                    "found": true,
                                    "startIndex": abs_start,
                                    "endIndex": abs_end,
                                    "occurrence": found_count
                                });
                            }
                            search_from += pos + 1;
                        }
                    }
                }
            }
        }
    }

    json!({ "found": false, "occurrences_found": found_count })
}

pub fn build_append_section_requests(
    heading: Option<&str>,
    heading_level: u32,
    body: Option<&str>,
    items: Option<&[String]>,
    bullet_preset: &str,
) -> Vec<Value> {
    let mut requests = Vec::new();
    let named_style = format!("HEADING_{}", heading_level.clamp(1, 6));

    if let Some(h) = heading {
        let text = format!("{h}\n");
        requests.push(json!({
            "insertText": {
                "text": text,
                "endOfSegmentLocation": { "segmentId": "" }
            }
        }));
        requests.push(json!({
            "updateParagraphStyle": {
                "paragraphStyle": { "namedStyleType": named_style },
                "fields": "namedStyleType",
                "range": { "startIndex": null, "endIndex": null }
            }
        }));
    }

    if let Some(b) = body {
        let text = if b.ends_with('\n') {
            b.to_string()
        } else {
            format!("{b}\n")
        };
        requests.push(json!({
            "insertText": {
                "text": text,
                "endOfSegmentLocation": { "segmentId": "" }
            }
        }));
    }

    if let Some(items) = items
        && !items.is_empty()
    {
        let bullet_text: String = items.iter().map(|i| format!("{i}\n")).collect();
        requests.push(json!({
            "insertText": {
                "text": bullet_text,
                "endOfSegmentLocation": { "segmentId": "" }
            }
        }));
        requests.push(json!({
            "createParagraphBullets": {
                "range": { "startIndex": null, "endIndex": null },
                "bulletPreset": bullet_preset
            }
        }));
    }

    requests
}

pub fn outline_tool_schema() -> Value {
    json!({
        "name": "gws_docs_outline",
        "title": "Doc Outline",
        "description": "Get doc structure: headings, sections, tables, images, with character indexes.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": { "type": "string", "description": "Google Docs document ID" }
            },
            "required": ["document_id"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "elements": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["title", "subtitle", "heading", "paragraph", "table", "image", "list_item"] },
                            "text": { "type": "string" },
                            "preview": { "type": "string" },
                            "level": { "type": "integer" },
                            "rows": { "type": "integer" },
                            "columns": { "type": "integer" },
                            "startIndex": { "type": "integer" },
                            "endIndex": { "type": "integer" }
                        },
                        "required": ["type", "startIndex", "endIndex"]
                    }
                },
                "endIndex": { "type": "integer" }
            },
            "required": ["title", "elements", "endIndex"]
        }
    })
}

pub fn find_tool_schema() -> Value {
    json!({
        "name": "gws_docs_find",
        "title": "Find Text in Doc",
        "description": "Find text in a Google Doc, returns start/end character indexes for formatting.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": { "type": "string", "description": "Google Docs document ID" },
                "text": { "type": "string", "description": "Exact substring to search for" },
                "occurrence": { "type": "integer", "description": "Which occurrence (1-based)", "default": 1 }
            },
            "required": ["document_id", "text"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "found": { "type": "boolean" },
                "startIndex": { "type": "integer" },
                "endIndex": { "type": "integer" },
                "occurrence": { "type": "integer" },
                "occurrences_found": { "type": "integer" }
            },
            "required": ["found"]
        }
    })
}

pub fn build_table_populate_requests(
    doc: &Value,
    headers: Option<&[String]>,
    rows: &[Vec<String>],
) -> Vec<Value> {
    let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let table_elem = content.iter().rev().find(|e| e.get("table").is_some());
    let Some(table) = table_elem.and_then(|e| e.get("table")) else {
        return Vec::new();
    };
    let Some(table_rows) = table.get("tableRows").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let all_rows: Vec<&[String]> = if let Some(h) = headers {
        let mut v: Vec<&[String]> = vec![h];
        v.extend(rows.iter().map(|r| r.as_slice()));
        v
    } else {
        rows.iter().map(|r| r.as_slice()).collect()
    };

    struct CellInsert {
        index: i32,
        text: String,
        bold: bool,
    }

    let mut inserts: Vec<CellInsert> = Vec::new();

    for (row_idx, data_row) in all_rows.iter().enumerate() {
        let Some(table_row) = table_rows.get(row_idx) else {
            break;
        };
        let Some(cells) = table_row.get("tableCells").and_then(|v| v.as_array()) else {
            continue;
        };
        for (col_idx, cell_text) in data_row.iter().enumerate() {
            let Some(cell) = cells.get(col_idx) else {
                break;
            };
            let cell_start = cell
                .pointer("/content/0/startIndex")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            if cell_start > 0 && !cell_text.is_empty() {
                inserts.push(CellInsert {
                    index: cell_start,
                    text: cell_text.clone(),
                    bold: headers.is_some() && row_idx == 0,
                });
            }
        }
    }

    inserts.sort_by_key(|a| std::cmp::Reverse(a.index));

    let mut requests = Vec::new();
    for insert in &inserts {
        requests.push(json!({
            "insertText": {
                "text": &insert.text,
                "location": { "index": insert.index }
            }
        }));
        if insert.bold {
            requests.push(json!({
                "updateTextStyle": {
                    "textStyle": { "bold": true },
                    "fields": "bold",
                    "range": {
                        "startIndex": insert.index,
                        "endIndex": insert.index + insert.text.len() as i32
                    }
                }
            }));
        }
    }

    requests
}

pub fn read_table_from_doc(doc: &Value, table_index: usize) -> Value {
    let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) else {
        return json!({ "error": "No document body" });
    };

    let tables: Vec<&Value> = content
        .iter()
        .filter(|e| e.get("table").is_some())
        .collect();
    let Some(table_elem) = tables.get(table_index) else {
        return json!({ "error": format!("Table index {} not found ({} tables in doc)", table_index, tables.len()) });
    };

    let start_index = table_elem
        .get("startIndex")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let end_index = table_elem
        .get("endIndex")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let Some(table) = table_elem.get("table") else {
        return json!({ "error": "Not a table element" });
    };
    let Some(table_rows) = table.get("tableRows").and_then(|v| v.as_array()) else {
        return json!({ "rows": [], "startIndex": start_index, "endIndex": end_index });
    };

    let mut result_rows: Vec<Vec<String>> = Vec::new();
    for row in table_rows {
        let Some(cells) = row.get("tableCells").and_then(|v| v.as_array()) else {
            continue;
        };
        let mut row_data: Vec<String> = Vec::new();
        for cell in cells {
            let text: String = cell
                .get("content")
                .and_then(|v| v.as_array())
                .map(|paras| {
                    paras
                        .iter()
                        .filter_map(|p| {
                            p.pointer("/paragraph/elements")
                                .and_then(|v| v.as_array())
                                .map(|elems| {
                                    elems
                                        .iter()
                                        .filter_map(|e| {
                                            e.pointer("/textRun/content").and_then(|v| v.as_str())
                                        })
                                        .collect::<String>()
                                })
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
                .trim()
                .to_string();
            row_data.push(text);
        }
        result_rows.push(row_data);
    }

    json!({
        "rows": result_rows,
        "startIndex": start_index,
        "endIndex": end_index,
        "row_count": result_rows.len(),
        "column_count": result_rows.first().map(|r| r.len()).unwrap_or(0)
    })
}

pub fn extract_all_tables(doc: &Value) -> Vec<Value> {
    let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let table_count = content.iter().filter(|e| e.get("table").is_some()).count();
    (0..table_count)
        .map(|i| {
            let mut table = read_table_from_doc(doc, i);
            if let Some(obj) = table.as_object_mut() {
                obj.insert("index".to_string(), json!(i));
            }
            table
        })
        .filter(|t| t.get("error").is_none())
        .collect()
}

pub fn insert_table_tool_schema() -> Value {
    json!({
        "name": "gws_docs_insert_table",
        "title": "Insert Table in Doc",
        "description": "Insert a table into a Google Doc from headers and row arrays.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID"
                },
                "headers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column headers (bold row)"
                },
                "rows": {
                    "type": "array",
                    "items": { "type": "array", "items": { "type": "string" } },
                    "description": "Data rows as array of arrays"
                },
                "columns": {
                    "type": "integer",
                    "description": "Column count (only for empty table without headers)"
                },
                "position": {
                    "type": "string",
                    "enum": ["end", "start"],
                    "default": "end"
                }
            },
            "required": ["document_id"]
        }
    })
}

pub fn read_table_tool_schema() -> Value {
    json!({
        "name": "gws_docs_read_table",
        "title": "Read Table from Doc",
        "description": "Read a table from a Google Doc as a JSON array of rows.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID"
                },
                "table_index": {
                    "type": "integer",
                    "description": "Which table (0-based)",
                    "default": 0
                }
            },
            "required": ["document_id"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "rows": {
                    "type": "array",
                    "items": { "type": "array", "items": { "type": "string" } }
                },
                "startIndex": { "type": "integer" },
                "endIndex": { "type": "integer" },
                "row_count": { "type": "integer" },
                "column_count": { "type": "integer" }
            },
            "required": ["rows", "startIndex", "endIndex", "row_count", "column_count"]
        }
    })
}

pub fn docs_write_tool_schema() -> Value {
    json!({
        "name": "gws_docs_write",
        "title": "Write to Google Doc",
        "description": "Write to a document. Omit document_id with title to create new.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Markdown string. # headings, **bold**, *italic*, - bullets, 1. numbered, | tables."
                },
                "document_id": {
                    "type": "string",
                    "description": "Existing doc ID. Omit with title to create new."
                },
                "title": {
                    "type": "string",
                    "description": "New doc title when creating without document_id."
                },
                "folder_id": {
                    "type": "string",
                    "description": "Drive folder for new doc creation."
                },
                "section": {
                    "type": "string",
                    "description": "Heading text to find and replace content under."
                },
                "position": {
                    "type": "string",
                    "enum": ["end", "start"],
                    "default": "end"
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "plain"],
                    "default": "markdown"
                }
            },
            "required": ["content"]
        }
    })
}

pub fn docs_replace_section_tool_schema() -> Value {
    json!({
        "name": "gws_docs_replace_section",
        "title": "Replace Section in Doc",
        "description": "Replace a section in a Google Doc by heading name. Deletes old content, writes new.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID"
                },
                "section": {
                    "type": "string",
                    "description": "Heading text of the section to replace"
                },
                "content": {
                    "type": "string",
                    "description": "New Markdown content (include the heading itself)"
                }
            },
            "required": ["document_id", "section", "content"]
        }
    })
}

pub fn docs_read_tool_schema() -> Value {
    json!({
        "name": "gws_docs_read",
        "title": "Read Google Doc",
        "description": "Read a document as Markdown. Use section= to read one section.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID"
                },
                "section": {
                    "type": "string",
                    "description": "Heading text — returns only that section's content."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "plain"],
                    "default": "markdown"
                }
            },
            "required": ["document_id"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "tables": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "index": { "type": "integer" },
                            "rows": {
                                "type": "array",
                                "items": { "type": "array", "items": { "type": "string" } }
                            },
                            "startIndex": { "type": "integer" },
                            "endIndex": { "type": "integer" },
                            "row_count": { "type": "integer" },
                            "column_count": { "type": "integer" }
                        },
                        "required": ["index", "rows", "row_count", "column_count"]
                    }
                }
            },
            "required": ["text"]
        }
    })
}

fn parse_text_style(arguments: &Value) -> TextStyle {
    TextStyle {
        bold: arguments.get("bold").and_then(|v| v.as_bool()),
        italic: arguments.get("italic").and_then(|v| v.as_bool()),
        font_size_pt: arguments.get("font_size_pt").and_then(|v| v.as_f64()),
        font_family: arguments
            .get("font_family")
            .and_then(|v| v.as_str())
            .map(String::from),
        foreground_color: arguments
            .get("foreground_color")
            .and_then(|v| v.as_str())
            .map(String::from),
        background_color: arguments
            .get("background_color")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

async fn docs_batch_update(
    doc_id: &str,
    requests: Vec<Value>,
    tool_name: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let batch_args = json!({
        "params": { "documentId": doc_id },
        "body": { "requests": requests }
    });
    let doc = state.get_doc("docs").await?;
    let resource = tools::find_resource(&doc.resources, "documents")
        .ok_or_else(|| GwsError::Validation("documents resource not found in docs API".into()))?;
    let method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;
    let result = crate::execute::execute_tool(
        &doc,
        method,
        "documents",
        "batchUpdate",
        &batch_args,
        "docs",
        policy,
        meta,
        None,
        None,
        dry_run,
        &mut state.token_cache,
    )
    .await
    .map_err(|e| {
        GwsError::Other(anyhow::anyhow!(
            "{tool_name}: batchUpdate failed for document '{doc_id}': {e}"
        ))
    })?;
    crate::server::check_api_result(&result).map_err(|e| {
        GwsError::Other(anyhow::anyhow!(
            "{tool_name}: Google Docs API error on document '{doc_id}': {e}"
        ))
    })?;
    Ok(result)
}

async fn execute_docs_read_table(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let doc_ref = state.get_doc("docs").await?;
    let resource = tools::find_resource(&doc_ref.resources, "documents")
        .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
    let get_args = json!({"params": {"documentId": doc_id}});
    let doc_content = crate::execute::execute_tool(
        &doc_ref,
        get_method,
        "documents",
        "get",
        &get_args,
        "docs",
        policy,
        meta,
        None,
        None,
        false,
        &mut state.token_cache,
    )
    .await?;
    let table_index = arguments
        .get("table_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let result = read_table_from_doc(&doc_content, table_index);
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_docs_read_block(
    tool_name: &str,
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let doc_ref = state.get_doc("docs").await?;
    let resource = tools::find_resource(&doc_ref.resources, "documents")
        .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
    let get_args = json!({"params": {"documentId": doc_id}});
    let doc_content = crate::execute::execute_tool(
        &doc_ref,
        get_method,
        "documents",
        "get",
        &get_args,
        "docs",
        policy,
        meta,
        None,
        None,
        false,
        &mut state.token_cache,
    )
    .await?;

    if tool_name == "gws_docs_find" {
        let needle = arguments
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GwsError::Validation("Missing 'text'".into()))?;
        let occurrence = arguments
            .get("occurrence")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;
        let result = find_text_in_doc(&doc_content, needle, occurrence);
        return Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
            "structuredContent": result,
            "isError": false
        }));
    }

    if tool_name == "gws_docs_outline" {
        let structure = parse_doc_structure(&doc_content);
        return Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&structure).unwrap_or_default() }],
            "structuredContent": structure,
            "isError": false
        }));
    }

    // gws_docs_read: section-level or full doc
    let section = arguments.get("section").and_then(|v| v.as_str());
    let format = arguments
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("markdown");

    let content_to_convert = if let Some(heading) = section {
        if let Some((start, end)) = find_section_range(&doc_content, heading) {
            extract_section_doc(&doc_content, start, end)
        } else {
            return Err(GwsError::Validation(format!(
                "Section '{heading}' not found. Use gws_docs_outline to see available headings."
            )));
        }
    } else {
        doc_content.clone()
    };

    let tables = extract_all_tables(&content_to_convert);
    let has_tables = !tables.is_empty();

    match format {
        "plain" => {
            let plain = crate::format::doc_to_plain(&content_to_convert);
            if has_tables {
                Ok(json!({
                    "content": [{ "type": "text", "text": plain }],
                    "structuredContent": { "text": plain, "tables": tables },
                    "isError": false
                }))
            } else {
                Ok(json!({
                    "content": [{ "type": "text", "text": plain }],
                    "isError": false
                }))
            }
        }
        _ => {
            let md = crate::format::doc_to_markdown(&content_to_convert);
            if has_tables {
                Ok(json!({
                    "content": [{ "type": "text", "text": md }],
                    "structuredContent": { "text": md, "tables": tables },
                    "isError": false
                }))
            } else {
                Ok(json!({
                    "content": [{ "type": "text", "text": md }],
                    "isError": false
                }))
            }
        }
    }
}

async fn execute_docs_insert_text(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let needs_end_index = arguments.get("index").is_none()
        && arguments.get("position").and_then(|v| v.as_str()) != Some("start")
        && (arguments.get("bold").is_some()
            || arguments.get("italic").is_some()
            || arguments.get("font_size_pt").is_some()
            || arguments.get("font_family").is_some()
            || arguments.get("foreground_color").is_some()
            || arguments.get("background_color").is_some()
            || arguments.get("paragraph_style").is_some());

    let end_index = if needs_end_index && !dry_run {
        let doc_ref = state.get_doc("docs").await?;
        let resource = tools::find_resource(&doc_ref.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": doc_id}});
        let doc_content = crate::execute::execute_tool(
            &doc_ref,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;
        doc_content["body"]["content"]
            .as_array()
            .and_then(|arr| arr.last())
            .and_then(|el| el["endIndex"].as_i64())
            .map(|idx| (idx - 1) as i32)
    } else {
        None
    };

    let requests = if let Some(sections) = arguments.get("sections").and_then(|v| v.as_array()) {
        let mut all_requests = Vec::new();
        for section in sections {
            let text = section.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                continue;
            }
            let style = parse_text_style(section);
            let has_style = style.bold.is_some()
                || style.foreground_color.is_some()
                || style.italic.is_some()
                || style.font_size_pt.is_some();
            let para = section.get("paragraph_style").and_then(|v| v.as_str());
            all_requests.extend(build_insert_text_requests(
                text,
                Position::End,
                if has_style { Some(style) } else { None },
                para,
            ));
        }
        all_requests
    } else {
        let text = arguments
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GwsError::Validation("Missing 'text' or 'sections'".into()))?;
        let position = match (crate::server::parse_position(arguments), end_index) {
            (Position::End, Some(idx)) => Position::Index(idx),
            (pos, _) => pos,
        };
        let style = parse_text_style(arguments);
        let has_style = style.bold.is_some()
            || style.italic.is_some()
            || style.font_size_pt.is_some()
            || style.font_family.is_some()
            || style.foreground_color.is_some()
            || style.background_color.is_some();
        let para = arguments
            .get("paragraph_style")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        build_insert_text_requests(
            text,
            position,
            if has_style { Some(style) } else { None },
            para.as_deref(),
        )
    };

    docs_batch_update(
        doc_id,
        requests,
        "gws_docs_insert_text",
        policy,
        meta,
        state,
        dry_run,
    )
    .await
}

async fn execute_docs_insert_table(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let headers: Option<Vec<String>> =
        arguments
            .get("headers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
    let data_rows: Option<Vec<Vec<String>>> =
        arguments.get("rows").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|row| {
                    row.as_array().map(|cells| {
                        cells
                            .iter()
                            .filter_map(|c| c.as_str().map(String::from))
                            .collect()
                    })
                })
                .collect()
        });

    if headers.is_some() || data_rows.is_some() {
        let num_cols = headers
            .as_ref()
            .map(|h| h.len())
            .or_else(|| {
                data_rows
                    .as_ref()
                    .and_then(|r| r.first().map(|row| row.len()))
            })
            .unwrap_or(1) as u32;
        let num_rows = (if headers.is_some() { 1 } else { 0 }
            + data_rows.as_ref().map(|r| r.len()).unwrap_or(0)) as u32;

        let position = crate::server::parse_position(arguments);
        let insert_req = build_insert_table_request(num_rows, num_cols, position);

        let doc_ref = state.get_doc("docs").await?;
        let resource = tools::find_resource(&doc_ref.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let batch_method = resource
            .methods
            .get("batchUpdate")
            .ok_or_else(|| GwsError::Validation("batchUpdate not found".into()))?;

        let create_args = json!({
            "params": { "documentId": doc_id },
            "body": { "requests": [insert_req] }
        });
        crate::execute::execute_tool(
            &doc_ref,
            batch_method,
            "documents",
            "batchUpdate",
            &create_args,
            "docs",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await?;

        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": doc_id}});
        let doc_content = crate::execute::execute_tool(
            &doc_ref,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;

        let empty_rows: Vec<Vec<String>> = Vec::new();
        let populate_reqs = build_table_populate_requests(
            &doc_content,
            headers.as_deref(),
            data_rows.as_ref().unwrap_or(&empty_rows),
        );

        if populate_reqs.is_empty() {
            return Ok(json!({
                "content": [{ "type": "text", "text": "Table created (no data to populate)" }],
                "isError": false
            }));
        }

        let populate_args = json!({
            "params": { "documentId": doc_id },
            "body": { "requests": populate_reqs }
        });
        let result = crate::execute::execute_tool(
            &doc_ref,
            batch_method,
            "documents",
            "batchUpdate",
            &populate_args,
            "docs",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await?;
        return Ok(json!({
            "content": [{ "type": "text", "text": format!("Table created and populated ({} rows, {} columns)", num_rows, num_cols) }],
            "structuredContent": result,
            "isError": false
        }));
    }

    let rows = arguments
        .get("rows")
        .and_then(|v| v.as_u64())
        .or_else(|| arguments.get("row_count").and_then(|v| v.as_u64()))
        .ok_or_else(|| GwsError::Validation("Missing 'rows' or 'headers'".into()))?
        as u32;
    let columns = arguments
        .get("columns")
        .and_then(|v| v.as_u64())
        .or_else(|| arguments.get("column_count").and_then(|v| v.as_u64()))
        .ok_or_else(|| GwsError::Validation("Missing 'columns' or 'headers'".into()))?
        as u32;
    let position = crate::server::parse_position(arguments);
    let requests = vec![build_insert_table_request(rows, columns, position)];

    docs_batch_update(
        doc_id,
        requests,
        "gws_docs_insert_table",
        policy,
        meta,
        state,
        dry_run,
    )
    .await
}

async fn execute_docs_insert_image(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let image_url = arguments.get("image_url").and_then(|v| v.as_str());
    let drive_file_id = arguments.get("drive_file_id").and_then(|v| v.as_str());
    let image_data = arguments.get("image_data").and_then(|v| v.as_str());

    let content_type = arguments
        .get("image_content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("image/png");
    let uri = if let Some(url) = image_url {
        url.to_string()
    } else if let Some(fid) = drive_file_id {
        let (url, _perm_id) =
            crate::server::make_image_insertable(fid, policy, meta, state).await?;
        url
    } else if let Some(data) = image_data {
        format!("data:{content_type};base64,{data}")
    } else {
        return Err(GwsError::Validation(
            "One of 'image_url', 'drive_file_id', or 'image_data' is required".into(),
        ));
    };

    let position = crate::server::parse_position(arguments);
    let w = arguments.get("width_pt").and_then(|v| v.as_f64());
    let h = arguments.get("height_pt").and_then(|v| v.as_f64());
    let mut requests = vec![build_insert_image_request(&uri, position, w, h)];
    requests.push(json!({
        "insertText": {
            "text": "\n",
            "endOfSegmentLocation": { "segmentId": "" }
        }
    }));

    docs_batch_update(
        doc_id,
        requests,
        "gws_docs_insert_image",
        policy,
        meta,
        state,
        dry_run,
    )
    .await
}

async fn execute_docs_format(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let (start, end) = if let Some(text_match) = arguments.get("text").and_then(|v| v.as_str()) {
        let doc_ref = state.get_doc("docs").await?;
        let resource = tools::find_resource(&doc_ref.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": doc_id}});
        let doc_content = crate::execute::execute_tool(
            &doc_ref,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;
        let occurrence = arguments
            .get("occurrence")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;
        let result = find_text_in_doc(&doc_content, text_match, occurrence);
        if result.get("found") != Some(&json!(true)) {
            return Err(GwsError::Validation(format!(
                "Text '{}' not found in document",
                text_match
            )));
        }
        let s = result["startIndex"].as_i64().unwrap() as i32;
        let e = result["endIndex"].as_i64().unwrap() as i32;
        (s, e)
    } else {
        let s = arguments
            .get("start_index")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| GwsError::Validation("Missing 'start_index' or 'text'".into()))?
            as i32;
        let e = arguments
            .get("end_index")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| GwsError::Validation("Missing 'end_index'".into()))?
            as i32;
        (s, e)
    };
    let style = parse_text_style(arguments);
    let para = if arguments.get("named_style").is_some() || arguments.get("alignment").is_some() {
        Some(ParagraphStyle {
            named_style: arguments
                .get("named_style")
                .and_then(|v| v.as_str())
                .map(String::from),
            alignment: arguments
                .get("alignment")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    } else {
        None
    };
    let requests = build_format_text_requests(start, end, style, para);

    docs_batch_update(
        doc_id,
        requests,
        "gws_docs_format",
        policy,
        meta,
        state,
        dry_run,
    )
    .await
}

async fn execute_docs_add_bullets(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let start = arguments
        .get("start_index")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| GwsError::Validation("Missing 'start_index'".into()))?
        as i32;
    let end = arguments
        .get("end_index")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| GwsError::Validation("Missing 'end_index'".into()))? as i32;
    let preset = arguments
        .get("bullet_preset")
        .and_then(|v| v.as_str())
        .unwrap_or("BULLET_DISC_CIRCLE_SQUARE");
    let requests = vec![build_add_bullets_request(start, end, preset)];

    docs_batch_update(
        doc_id,
        requests,
        "gws_docs_add_bullets",
        policy,
        meta,
        state,
        dry_run,
    )
    .await
}

async fn execute_docs_append_section(
    doc_id: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let heading = arguments.get("heading").and_then(|v| v.as_str());
    let level = arguments
        .get("heading_level")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as u32;
    let body = arguments.get("body").and_then(|v| v.as_str());
    let items: Option<Vec<String>> = arguments
        .get("items")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });
    let preset = arguments
        .get("bullet_preset")
        .and_then(|v| v.as_str())
        .unwrap_or("BULLET_DISC_CIRCLE_SQUARE");
    let requests = build_append_section_requests(heading, level, body, items.as_deref(), preset);

    docs_batch_update(
        doc_id,
        requests,
        "gws_docs_append_section",
        policy,
        meta,
        state,
        dry_run,
    )
    .await
}

pub(crate) async fn execute_docs_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    if tool_name == "gws_docs_write" || tool_name == "gws_docs_replace_section" {
        tracing::info!(
            tool = tool_name,
            has_content = arguments.get("content").is_some(),
            has_document_id = arguments.get("document_id").is_some() || arguments.get("documentId").is_some(),
            has_title = arguments.get("title").is_some(),
            content_type = ?arguments.get("content").map(|v| v.is_string()),
            arg_keys = ?arguments.as_object().map(|m| m.keys().collect::<Vec<_>>()),
            "docs_write dispatch"
        );
        if tool_name == "gws_docs_replace_section" {
            if arguments.get("section").and_then(|v| v.as_str()).is_none() {
                return Err(GwsError::Validation(
                    "Missing 'section' — specify the heading text of the section to replace."
                        .into(),
                ));
            }
            if arguments
                .get("document_id")
                .and_then(|v| v.as_str())
                .is_none()
                && arguments
                    .get("documentId")
                    .and_then(|v| v.as_str())
                    .is_none()
            {
                return Err(GwsError::Validation(
                    "Missing 'document_id' — specify the doc to update.".into(),
                ));
            }
        }
        let format = crate::format::parse_format(arguments.get("format").and_then(|v| v.as_str()));
        return execute_docs_write(arguments, policy, meta, state, dry_run, format).await;
    }

    let doc_id = arguments
        .get("document_id")
        .or_else(|| arguments.get("documentId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GwsError::Validation(format!(
                "Missing 'document_id' in {tool_name}. Pass the Google Docs document ID."
            ))
        })?;

    match tool_name {
        "gws_docs_read_table" => {
            execute_docs_read_table(doc_id, arguments, policy, meta, state).await
        }
        "gws_docs_read" | "gws_docs_outline" | "gws_docs_find" => {
            execute_docs_read_block(tool_name, doc_id, arguments, policy, meta, state).await
        }
        "gws_docs_insert_text" => {
            execute_docs_insert_text(doc_id, arguments, policy, meta, state, dry_run).await
        }
        "gws_docs_insert_table" => {
            execute_docs_insert_table(doc_id, arguments, policy, meta, state, dry_run).await
        }
        "gws_docs_insert_image" => {
            execute_docs_insert_image(doc_id, arguments, policy, meta, state, dry_run).await
        }
        "gws_docs_format" | "gws_docs_format_text" => {
            execute_docs_format(doc_id, arguments, policy, meta, state, dry_run).await
        }
        "gws_docs_add_bullets" => {
            execute_docs_add_bullets(doc_id, arguments, policy, meta, state, dry_run).await
        }
        "gws_docs_append_section" => {
            execute_docs_append_section(doc_id, arguments, policy, meta, state, dry_run).await
        }
        _ => Err(GwsError::Validation(format!(
            "Unknown helper tool: {tool_name}"
        ))),
    }
}

fn find_section_range(doc: &Value, section: &str) -> Option<(i32, i32)> {
    let content = doc["body"]["content"].as_array()?;
    let mut section_start = None;
    let mut section_level = None;

    for element in content {
        if let Some(para) = element.get("paragraph") {
            let style_type = para["paragraphStyle"]["namedStyleType"]
                .as_str()
                .unwrap_or("");
            let text: String = para["elements"]
                .as_array()
                .map(|els| {
                    els.iter()
                        .filter_map(|e| e["textRun"]["content"].as_str())
                        .collect::<String>()
                })
                .unwrap_or_default();
            let text_trimmed = text.trim();

            if let Some(level) = heading_level(style_type) {
                if let Some(start_level) = section_level
                    && level <= start_level
                {
                    let start = section_start.unwrap();
                    let end = element["startIndex"].as_i64().unwrap() as i32;
                    return Some((start, end));
                }
                if text_trimmed == section {
                    section_start = Some(element["startIndex"].as_i64().unwrap() as i32);
                    section_level = Some(level);
                }
            }
        }
    }

    if let Some(start) = section_start {
        let last = content.last()?;
        let end = last["endIndex"].as_i64().unwrap_or(start as i64) as i32;
        return Some((start, end - 1));
    }
    None
}

fn shift_request_indexes(requests: &[Value], shift: i32) -> Vec<Value> {
    if shift == 0 {
        return requests.to_vec();
    }
    requests
        .iter()
        .map(|req| {
            let mut r = req.clone();
            for path in &[
                "/insertText/location/index",
                "/insertTable/location/index",
                "/updateParagraphStyle/range/startIndex",
                "/updateParagraphStyle/range/endIndex",
                "/updateTextStyle/range/startIndex",
                "/updateTextStyle/range/endIndex",
                "/createParagraphBullets/range/startIndex",
                "/createParagraphBullets/range/endIndex",
            ] {
                if let Some(idx) = r.pointer_mut(path)
                    && let Some(v) = idx.as_i64()
                {
                    *idx = json!(v + shift as i64);
                }
            }
            r
        })
        .collect()
}

fn extract_section_doc(doc: &Value, start: i32, end: i32) -> Value {
    let mut section_doc = doc.clone();
    if let Some(content) = doc["body"]["content"].as_array() {
        let filtered: Vec<Value> = content
            .iter()
            .filter(|elem| {
                let elem_start = elem["startIndex"].as_i64().unwrap_or(0) as i32;
                let elem_end = elem["endIndex"].as_i64().unwrap_or(0) as i32;
                elem_start >= start && elem_end <= end
            })
            .cloned()
            .collect();
        section_doc["body"]["content"] = json!(filtered);
    }
    section_doc
}

pub(crate) async fn execute_docs_write(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
    format: crate::format::ContentFormat,
) -> Result<Value, GwsError> {
    let content = arguments
        .get("content")
        .or_else(|| arguments.get("markdown"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GwsError::Validation(
                "Missing 'content' parameter (must be a string). Pass the content to write.".into(),
            )
        })?;

    let doc_id_arg = arguments
        .get("document_id")
        .or_else(|| arguments.get("documentId"))
        .and_then(|v| v.as_str());
    let title = arguments.get("title").and_then(|v| v.as_str());
    let mut folder_id = arguments
        .get("folder_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let section = arguments.get("section").and_then(|v| v.as_str());
    let template_id = arguments.get("template_id").and_then(|v| v.as_str());

    // If document_id looks like a folder (title also provided), treat it as folder_id.
    let doc_id_arg = if let (Some(id), Some(_)) = (doc_id_arg, title) {
        if folder_id.is_none() {
            if let Ok(drive_doc) = state.get_doc("drive").await {
                if let Some(resource) = tools::find_resource(&drive_doc.resources, "files") {
                    if let Some(gm) = resource.methods.get("get") {
                        let args = json!({"params": {"fileId": id}, "fields": "mimeType"});
                        if let Ok(file_meta) = crate::execute::execute_tool(
                            &drive_doc,
                            gm,
                            "files",
                            "get",
                            &args,
                            "drive",
                            policy,
                            meta,
                            None,
                            None,
                            false,
                            &mut state.token_cache,
                        )
                        .await
                        {
                            if file_meta["mimeType"].as_str()
                                == Some("application/vnd.google-apps.folder")
                            {
                                tracing::info!(
                                    provided_id = id,
                                    "document_id is a folder — treating as folder_id"
                                );
                                folder_id = Some(id.to_string());
                                None
                            } else {
                                Some(id)
                            }
                        } else {
                            Some(id)
                        }
                    } else {
                        Some(id)
                    }
                } else {
                    Some(id)
                }
            } else {
                Some(id)
            }
        } else {
            Some(id)
        }
    } else {
        doc_id_arg
    };

    let folder_id = folder_id.as_deref();

    // Step A: resolve, find existing, or create the document
    let (doc_id, created_new_doc) = if let Some(id) = doc_id_arg {
        (id.to_string(), false)
    } else if title.is_some() || folder_id.is_some() {
        let doc_title = title.unwrap_or("Untitled");
        let (effective_policy, resolved_folder) =
            crate::server::policy_for_folder(folder_id, policy, meta, state).await?;
        let folder_id = resolved_folder.as_deref().or(folder_id);
        let drive_doc = state.get_doc("drive").await.map_err(|e| {
            GwsError::Other(anyhow::anyhow!(
                "gws_docs_import_markdown: failed to load Drive API: {e}"
            ))
        })?;
        let drive_resource =
            tools::find_resource(&drive_doc.resources, "files").ok_or_else(|| {
                GwsError::Validation(
                    "gws_docs_import_markdown: files resource not found in drive API".into(),
                )
            })?;

        {
            let mut body = json!({
                "name": doc_title,
                "mimeType": "application/vnd.google-apps.document"
            });
            if let Some(fid) = folder_id {
                body["parents"] = json!([fid]);
            }
            let create_args = json!({"body": body});
            let create_method = drive_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
            let result = crate::execute::execute_tool(
                &drive_doc,
                create_method,
                "files",
                "create",
                &create_args,
                "drive",
                &effective_policy,
                meta,
                None,
                None,
                dry_run,
                &mut state.token_cache,
            )
            .await?;
            crate::server::check_api_result(&result)?;
            let new_id = result["id"]
                .as_str()
                .ok_or_else(|| {
                    GwsError::Other(anyhow::anyhow!("No 'id' in drive.files.create response"))
                })?
                .to_string();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            (new_id, true)
        }
    } else {
        return Err(GwsError::Validation(
            "Either 'document_id' (existing doc) or 'title' (create new doc) is required. \
             Pass document_id to import into an existing document, or title to create a new one."
                .into(),
        ));
    };

    // Step B: handle template (apply named styles from another doc)
    let template_requests = if let Some(tmpl_id) = template_id {
        let docs_doc = state.get_doc("docs").await?;
        let resource = tools::find_resource(&docs_doc.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": tmpl_id}});
        let tmpl_result = crate::execute::execute_tool(
            &docs_doc,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;

        let mut style_reqs = Vec::new();
        if let Some(styles) = tmpl_result["namedStyles"]["styles"].as_array() {
            for style in styles {
                if let (Some(props), Some(style_type)) = (
                    style.get("textStyle"),
                    style.get("namedStyleType").and_then(|v| v.as_str()),
                ) {
                    let mut ns_props = serde_json::Map::new();
                    ns_props.insert("namedStyleType".to_string(), json!(style_type));
                    ns_props.insert("textStyle".to_string(), props.clone());
                    if let Some(para) = style.get("paragraphStyle") {
                        ns_props.insert("paragraphStyle".to_string(), para.clone());
                    }
                    style_reqs.push(json!({
                        "updateNamedStyle": {
                            "namedStyle": Value::Object(ns_props),
                            "fields": "*"
                        }
                    }));
                }
            }
        }
        if style_reqs.is_empty() {
            None
        } else {
            Some(style_reqs)
        }
    } else {
        None
    };

    // Step C: handle section replacement
    let (section_delete, insert_index) = if let Some(section_text) = section {
        let docs_doc = state.get_doc("docs").await?;
        let resource = tools::find_resource(&docs_doc.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": doc_id}});
        let doc_content = crate::execute::execute_tool(
            &docs_doc,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;

        match find_section_range(&doc_content, section_text) {
            Some((start, end)) => (
                Some(json!({
                    "deleteContentRange": {
                        "range": { "startIndex": start, "endIndex": end }
                    }
                })),
                start,
            ),
            None => {
                return Err(GwsError::Validation(format!(
                    "Section '{}' not found in document",
                    section_text
                )));
            }
        }
    } else {
        let idx = if created_new_doc {
            1
        } else if let Some(i) = arguments.get("index").and_then(|v| v.as_i64()) {
            i as i32
        } else {
            match arguments.get("position").and_then(|v| v.as_str()) {
                Some("start") => 1,
                _ => {
                    // Fetch document to find end index
                    let docs_doc = state.get_doc("docs").await?;
                    let resource = tools::find_resource(&docs_doc.resources, "documents")
                        .ok_or_else(|| {
                            GwsError::Validation("documents resource not found".into())
                        })?;
                    let get_method = resource
                        .methods
                        .get("get")
                        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                    let get_args = json!({"params": {"documentId": doc_id}});
                    let doc_content = crate::execute::execute_tool(
                        &docs_doc,
                        get_method,
                        "documents",
                        "get",
                        &get_args,
                        "docs",
                        policy,
                        meta,
                        None,
                        None,
                        false,
                        &mut state.token_cache,
                    )
                    .await?;
                    doc_content["body"]["content"]
                        .as_array()
                        .and_then(|arr| arr.last())
                        .and_then(|el| el["endIndex"].as_i64())
                        .map(|idx| (idx - 1) as i32)
                        .unwrap_or(1)
                }
            }
        };
        (None, idx)
    };

    // Step D: execute batchUpdate(s)
    let docs_doc = state.get_doc("docs").await?;
    let resource = tools::find_resource(&docs_doc.resources, "documents")
        .ok_or_else(|| GwsError::Validation("documents resource not found in docs API".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
    let batch_method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;

    if let Some(style_reqs) = template_requests {
        let style_args = json!({
            "params": { "documentId": doc_id },
            "body": { "requests": style_reqs }
        });
        let style_result = crate::execute::execute_tool(
            &docs_doc,
            batch_method,
            "documents",
            "batchUpdate",
            &style_args,
            "docs",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await?;
        crate::server::check_api_result(&style_result)?;
    }

    let mut content_requests: Vec<Value> = Vec::new();
    if let Some(delete_req) = section_delete {
        content_requests.push(delete_req);
    }
    content_requests.extend(crate::format::content_to_batch_requests(
        content,
        format,
        insert_index,
    ));

    // Split at table boundaries: insertTable changes the doc's index space,
    // so subsequent inserts at pre-calculated offsets fail. Split into
    // separate batches, re-derive indexes from the doc after each table.
    let mut batches: Vec<(Vec<Value>, Option<Value>)> = Vec::new();
    let mut current_batch: Vec<Value> = Vec::new();
    for req in &content_requests {
        if let Some(mut it) = req.get("insertTable").cloned() {
            let table_data = it.as_object_mut().and_then(|m| m.remove("_tableData"));
            current_batch.push(json!({ "insertTable": it }));
            batches.push((current_batch, table_data));
            current_batch = Vec::new();
        } else {
            current_batch.push(req.clone());
        }
    }
    if !current_batch.is_empty() {
        batches.push((current_batch, None));
    }

    let mut result: Result<Value, GwsError> = Ok(json!({}));
    for (batch_idx, (batch_reqs, table_data)) in batches.iter().enumerate() {
        let final_reqs = if batch_idx > 0 && !batch_reqs.is_empty() {
            let doc_now = crate::execute::execute_tool(
                &docs_doc,
                get_method,
                "documents",
                "get",
                &json!({"params": {"documentId": &doc_id}}),
                "docs",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let end_index = doc_now
                .pointer("/body/content")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.last())
                .and_then(|el| el["endIndex"].as_i64())
                .unwrap_or(1) as i32;
            let first_idx = batch_reqs
                .iter()
                .find_map(|r| {
                    r.pointer("/insertText/location/index")
                        .and_then(|v| v.as_i64())
                        .map(|i| i as i32)
                })
                .unwrap_or(end_index);
            let shift = (end_index - 1) - first_idx;
            shift_request_indexes(batch_reqs, shift)
        } else {
            batch_reqs.clone()
        };

        if !final_reqs.is_empty() {
            let batch_args = json!({
                "params": { "documentId": doc_id },
                "body": { "requests": final_reqs }
            });
            result = crate::execute::execute_tool(
                &docs_doc,
                batch_method,
                "documents",
                "batchUpdate",
                &batch_args,
                "docs",
                policy,
                meta,
                None,
                None,
                dry_run,
                &mut state.token_cache,
            )
            .await;

            // If batchUpdate fails with too many requests, retry in smaller chunks
            let should_chunk = match &result {
                Err(_) => final_reqs.len() > 10,
                Ok(r) => crate::server::check_api_result(r).is_err() && final_reqs.len() > 10,
            };
            if should_chunk {
                tracing::info!(
                    total_requests = final_reqs.len(),
                    "batchUpdate failed, retrying in chunks of 50"
                );
                result = Ok(json!({}));
                for chunk in final_reqs.chunks(50) {
                    let chunk_args = json!({
                        "params": { "documentId": doc_id },
                        "body": { "requests": chunk }
                    });
                    result = crate::execute::execute_tool(
                        &docs_doc,
                        batch_method,
                        "documents",
                        "batchUpdate",
                        &chunk_args,
                        "docs",
                        policy,
                        meta,
                        None,
                        None,
                        dry_run,
                        &mut state.token_cache,
                    )
                    .await;
                    match &result {
                        Ok(r) if crate::server::check_api_result(r).is_err() => break,
                        Err(_) => break,
                        _ => {}
                    }
                }
            } else if let Ok(ref r) = result {
                if crate::server::check_api_result(r).is_err() {
                    break;
                }
            } else {
                break;
            }
        }

        // Populate table cells after inserting the table
        if let Some(data) = table_data
            && !dry_run
        {
            let doc_now = crate::execute::execute_tool(
                &docs_doc,
                get_method,
                "documents",
                "get",
                &json!({"params": {"documentId": &doc_id}}),
                "docs",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let rows: Vec<Vec<String>> = data
                .get("rows")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let is_header = data
                .get("header")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !rows.is_empty() {
                let (headers, data_rows) = if is_header && rows.len() > 1 {
                    (Some(rows[0].clone()), rows[1..].to_vec())
                } else {
                    (None, rows)
                };
                let populate_reqs =
                    build_table_populate_requests(&doc_now, headers.as_deref(), &data_rows);
                if !populate_reqs.is_empty() {
                    let _ = crate::execute::execute_tool(
                            &docs_doc, batch_method, "documents", "batchUpdate",
                            &json!({"params": {"documentId": doc_id}, "body": {"requests": populate_reqs}}),
                            "docs", policy, meta, None, None, false, &mut state.token_cache,
                        ).await;
                }
            }
        }
    }

    let failed = match &result {
        Ok(r) => crate::server::check_api_result(r).is_err(),
        Err(_) => true,
    };

    if failed
        && created_new_doc
        && let Ok(drive_doc) = state.get_doc("drive").await
        && let Some(resource) = tools::find_resource(&drive_doc.resources, "files")
        && let Some(delete_method) = resource.methods.get("delete")
    {
        let args = json!({"params": {"fileId": &doc_id}});
        let _ = crate::execute::execute_tool(
            &drive_doc,
            delete_method,
            "files",
            "delete",
            &args,
            "drive",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await;
        tracing::info!(doc_id = %doc_id, "Cleaned up empty doc after failed write");
    }

    let result = match result {
        Ok(mut r) => {
            if let Err(e) = crate::server::check_api_result(&r) {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!(
                        "gws_docs_write: content insertion failed: {e}. \
                         Try with simpler content or split into smaller sections."
                    )}],
                    "isError": true
                }));
            }
            r["document_id"] = json!(doc_id);
            r
        }
        Err(e) => {
            return Ok(json!({
                "content": [{ "type": "text", "text": format!(
                    "gws_docs_write: failed: {e}. \
                     Try with simpler content or split into smaller sections."
                )}],
                "isError": true
            }));
        }
    };

    let text = if created_new_doc {
        format!("Content written to new document.\ndocument_id: {doc_id}")
    } else {
        format!("Content written to document.\ndocument_id: {doc_id}")
    };

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": result,
        "isError": false
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_insert_text_simple() {
        let requests = build_insert_text_requests("Hello", Position::End, None, None);
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req["insertText"]["text"], "Hello");
        assert!(req["insertText"]["endOfSegmentLocation"].is_object());
    }

    #[test]
    fn test_build_insert_text_with_style() {
        let style = TextStyle {
            bold: Some(true),
            ..Default::default()
        };
        let requests = build_insert_text_requests(
            "Title\n",
            Position::Index(1),
            Some(style),
            Some("HEADING_1"),
        );
        assert_eq!(requests.len(), 3);
        assert!(requests[0].get("insertText").is_some());
        assert!(requests[1].get("updateTextStyle").is_some());
        assert!(requests[2].get("updateParagraphStyle").is_some());

        let style_req = &requests[1]["updateTextStyle"];
        assert_eq!(style_req["textStyle"]["bold"], true);
        assert_eq!(style_req["fields"], "bold");
        assert_eq!(style_req["range"]["startIndex"], 1);
        assert_eq!(style_req["range"]["endIndex"], 7);

        let para_req = &requests[2]["updateParagraphStyle"];
        assert_eq!(para_req["paragraphStyle"]["namedStyleType"], "HEADING_1");
    }

    #[test]
    fn test_build_insert_table() {
        let req = build_insert_table_request(3, 4, Position::End);
        assert_eq!(req["insertTable"]["rows"], 3);
        assert_eq!(req["insertTable"]["columns"], 4);
        assert!(req["insertTable"]["endOfSegmentLocation"].is_object());
    }

    #[test]
    fn test_build_insert_image() {
        let req = build_insert_image_request(
            "https://example.com/image.png",
            Position::Index(5),
            Some(300.0),
            Some(200.0),
        );
        assert_eq!(
            req["insertInlineImage"]["uri"],
            "https://example.com/image.png"
        );
        assert_eq!(req["insertInlineImage"]["location"]["index"], 5);
        assert_eq!(
            req["insertInlineImage"]["objectSize"]["width"]["magnitude"],
            300.0
        );
        assert_eq!(
            req["insertInlineImage"]["objectSize"]["height"]["magnitude"],
            200.0
        );
    }

    #[test]
    fn test_build_format_text() {
        let style = TextStyle {
            bold: Some(true),
            italic: Some(true),
            font_size_pt: Some(14.0),
            ..Default::default()
        };
        let requests = build_format_text_requests(1, 10, style, None);
        assert_eq!(requests.len(), 1);
        let req = &requests[0]["updateTextStyle"];
        assert_eq!(req["textStyle"]["bold"], true);
        assert_eq!(req["textStyle"]["italic"], true);
        assert_eq!(req["textStyle"]["fontSize"]["magnitude"], 14.0);
        let fields = req["fields"].as_str().unwrap();
        assert!(fields.contains("bold"));
        assert!(fields.contains("italic"));
        assert!(fields.contains("fontSize"));
        assert_eq!(req["range"]["startIndex"], 1);
        assert_eq!(req["range"]["endIndex"], 10);
    }

    #[test]
    fn test_build_format_text_with_paragraph() {
        let style = TextStyle {
            bold: Some(true),
            ..Default::default()
        };
        let ps = ParagraphStyle {
            named_style: Some("HEADING_2".to_string()),
            alignment: Some("CENTER".to_string()),
        };
        let requests = build_format_text_requests(1, 20, style, Some(ps));
        assert_eq!(requests.len(), 2);
        assert!(requests[0].get("updateTextStyle").is_some());
        let para = &requests[1]["updateParagraphStyle"];
        assert_eq!(para["paragraphStyle"]["namedStyleType"], "HEADING_2");
        assert_eq!(para["paragraphStyle"]["alignment"], "CENTER");
        let fields = para["fields"].as_str().unwrap();
        assert!(fields.contains("namedStyleType"));
        assert!(fields.contains("alignment"));
    }

    #[test]
    fn test_hex_to_rgb() {
        let result = hex_to_rgb_color("#CC0000");
        let rgb = &result["color"]["rgbColor"];
        let r = rgb["red"].as_f64().unwrap();
        let g = rgb["green"].as_f64().unwrap();
        let b = rgb["blue"].as_f64().unwrap();
        assert!((r - 0.8).abs() < 0.01);
        assert!(g.abs() < 0.001);
        assert!(b.abs() < 0.001);
    }

    #[test]
    fn test_hex_to_rgb_white() {
        let result = hex_to_rgb_color("#FFFFFF");
        let rgb = &result["color"]["rgbColor"];
        assert!((rgb["red"].as_f64().unwrap() - 1.0).abs() < 0.01);
        assert!((rgb["green"].as_f64().unwrap() - 1.0).abs() < 0.01);
        assert!((rgb["blue"].as_f64().unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_hex_to_rgb_no_hash() {
        let result = hex_to_rgb_color("00FF00");
        let rgb = &result["color"]["rgbColor"];
        assert!(rgb["red"].as_f64().unwrap().abs() < 0.001);
        assert!((rgb["green"].as_f64().unwrap() - 1.0).abs() < 0.01);
        assert!(rgb["blue"].as_f64().unwrap().abs() < 0.001);
    }

    #[test]
    fn test_add_bullets() {
        let req = build_add_bullets_request(5, 25, "BULLET_DISC_CIRCLE_SQUARE");
        assert_eq!(req["createParagraphBullets"]["range"]["startIndex"], 5);
        assert_eq!(req["createParagraphBullets"]["range"]["endIndex"], 25);
        assert_eq!(
            req["createParagraphBullets"]["bulletPreset"],
            "BULLET_DISC_CIRCLE_SQUARE"
        );
    }

    #[test]
    fn test_insert_text_at_start() {
        let requests = build_insert_text_requests("Start", Position::Start, None, None);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["insertText"]["location"]["index"], 1);
    }

    #[test]
    fn test_insert_image_no_size() {
        let req =
            build_insert_image_request("https://example.com/img.png", Position::End, None, None);
        assert!(req["insertInlineImage"].get("objectSize").is_none());
    }

    #[test]
    fn test_insert_image_partial_size() {
        let req = build_insert_image_request(
            "https://example.com/img.png",
            Position::Start,
            Some(400.0),
            None,
        );
        assert_eq!(
            req["insertInlineImage"]["objectSize"]["width"]["magnitude"],
            400.0
        );
        assert!(
            req["insertInlineImage"]["objectSize"]
                .get("height")
                .is_none()
        );
    }

    #[test]
    fn test_tool_schemas_have_short_descriptions() {
        let schemas = vec![
            docs_write_tool_schema(),
            docs_read_tool_schema(),
            outline_tool_schema(),
            find_tool_schema(),
            insert_table_tool_schema(),
            read_table_tool_schema(),
            insert_image_tool_schema(),
            format_tool_schema(),
        ];
        for schema in &schemas {
            let name = schema["name"].as_str().unwrap();
            let desc = schema["description"].as_str().unwrap();
            assert!(
                desc.len() < 100,
                "Tool {name} description too long ({} chars): {desc}",
                desc.len()
            );
            assert!(
                schema["inputSchema"]["type"].as_str() == Some("object"),
                "Tool {name} missing inputSchema type"
            );
        }
        assert_eq!(schemas.len(), 8);
    }

    #[test]
    fn test_format_text_empty_style() {
        let style = TextStyle::default();
        let requests = build_format_text_requests(1, 10, style, None);
        assert!(requests.is_empty());
    }

    #[test]
    fn test_markdown_heading() {
        let requests = markdown_to_batch_requests("# Title\n", 1);
        assert!(requests.len() >= 2);
        assert_eq!(requests[0]["insertText"]["text"], "Title\n");
        assert_eq!(requests[0]["insertText"]["location"]["index"], 1);

        let para = requests
            .iter()
            .find(|r| r.get("updateParagraphStyle").is_some())
            .unwrap();
        assert_eq!(
            para["updateParagraphStyle"]["paragraphStyle"]["namedStyleType"],
            "TITLE"
        );
        assert_eq!(para["updateParagraphStyle"]["range"]["startIndex"], 1);
        assert_eq!(para["updateParagraphStyle"]["range"]["endIndex"], 7);
    }

    #[test]
    fn test_markdown_bold_italic() {
        let requests = markdown_to_batch_requests("**bold** and *italic*\n", 1);
        assert!(!requests.is_empty());

        let styles: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("updateTextStyle").is_some())
            .collect();
        assert_eq!(styles.len(), 2);

        let bold_req = &styles[0]["updateTextStyle"];
        assert_eq!(bold_req["textStyle"]["bold"], true);
        assert_eq!(bold_req["range"]["startIndex"], 1);
        assert_eq!(bold_req["range"]["endIndex"], 5);

        let italic_req = &styles[1]["updateTextStyle"];
        assert_eq!(italic_req["textStyle"]["italic"], true);
        assert_eq!(italic_req["range"]["startIndex"], 10);
        assert_eq!(italic_req["range"]["endIndex"], 16);
    }

    #[test]
    fn test_markdown_list() {
        let requests = markdown_to_batch_requests("- item1\n- item2\n", 1);
        assert!(!requests.is_empty());

        let bullets: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("createParagraphBullets").is_some())
            .collect();
        assert_eq!(bullets.len(), 1);
        assert_eq!(
            bullets[0]["createParagraphBullets"]["bulletPreset"],
            "BULLET_DISC_CIRCLE_SQUARE"
        );
    }

    #[test]
    fn test_markdown_link() {
        let requests = markdown_to_batch_requests("[click here](https://example.com)\n", 1);
        assert!(!requests.is_empty());

        let styles: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("updateTextStyle").is_some())
            .collect();
        assert_eq!(styles.len(), 1);
        let link_style = &styles[0]["updateTextStyle"];
        assert_eq!(
            link_style["textStyle"]["link"]["url"],
            "https://example.com"
        );
        let fields = link_style["fields"].as_str().unwrap();
        assert!(fields.contains("link"));
    }

    #[test]
    fn test_markdown_code() {
        let requests = markdown_to_batch_requests("use `code` here\n", 1);
        assert!(!requests.is_empty());

        let styles: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("updateTextStyle").is_some())
            .collect();
        assert_eq!(styles.len(), 1);
        let code_style = &styles[0]["updateTextStyle"];
        assert_eq!(
            code_style["textStyle"]["weightedFontFamily"]["fontFamily"],
            "Courier New"
        );
    }

    #[test]
    fn test_markdown_mixed() {
        let md =
            "# Welcome\n\nThis is **bold** and *italic* text.\n\n- first\n- second\n\n> a quote\n";
        let requests = markdown_to_batch_requests(md, 1);
        assert!(!requests.is_empty());

        assert!(requests[0].get("insertText").is_some());

        let has_title = requests.iter().any(|r| {
            r.get("updateParagraphStyle")
                .and_then(|u| u.get("paragraphStyle"))
                .and_then(|p| p.get("namedStyleType"))
                .and_then(|n| n.as_str())
                == Some("TITLE")
        });
        assert!(has_title);

        let has_bold = requests.iter().any(|r| {
            r.get("updateTextStyle")
                .and_then(|u| u.get("textStyle"))
                .and_then(|t| t.get("bold"))
                .and_then(|b| b.as_bool())
                == Some(true)
        });
        assert!(has_bold);

        let has_italic = requests.iter().any(|r| {
            r.get("updateTextStyle")
                .and_then(|u| u.get("textStyle"))
                .and_then(|t| t.get("italic"))
                .and_then(|b| b.as_bool())
                == Some(true)
        });
        assert!(has_italic);

        let has_bullets = requests
            .iter()
            .any(|r| r.get("createParagraphBullets").is_some());
        assert!(has_bullets);

        let has_indent = requests.iter().any(|r| {
            r.get("updateParagraphStyle")
                .and_then(|u| u.get("paragraphStyle"))
                .and_then(|p| p.get("indentStart"))
                .is_some()
        });
        assert!(has_indent);
    }

    #[test]
    fn test_markdown_ordered_list() {
        let requests = markdown_to_batch_requests("1. first\n2. second\n", 1);
        let bullets: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("createParagraphBullets").is_some())
            .collect();
        assert!(!bullets.is_empty());
        assert_eq!(
            bullets[0]["createParagraphBullets"]["bulletPreset"],
            "NUMBERED_DECIMAL_NESTED"
        );
    }

    #[test]
    fn test_markdown_horizontal_rule() {
        let requests = markdown_to_batch_requests("---\n", 1);
        assert!(!requests.is_empty());
        let text = requests[0]["insertText"]["text"].as_str().unwrap();
        assert!(text.contains('\u{2014}'));
    }

    #[test]
    fn test_markdown_image() {
        let requests = markdown_to_batch_requests("![alt](https://example.com/img.png)\n", 1);
        let imgs: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("insertInlineImage").is_some())
            .collect();
        assert_eq!(imgs.len(), 1);
        assert_eq!(
            imgs[0]["insertInlineImage"]["uri"],
            "https://example.com/img.png"
        );
    }

    #[test]
    fn test_markdown_start_index_offset() {
        let requests = markdown_to_batch_requests("**bold**\n", 50);
        assert_eq!(requests[0]["insertText"]["location"]["index"], 50);

        let style = requests
            .iter()
            .find(|r| r.get("updateTextStyle").is_some())
            .unwrap();
        assert_eq!(style["updateTextStyle"]["range"]["startIndex"], 50);
        assert_eq!(style["updateTextStyle"]["range"]["endIndex"], 54);
    }

    #[test]
    fn test_markdown_strikethrough() {
        let requests = markdown_to_batch_requests("~~removed~~\n", 1);
        let styles: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("updateTextStyle").is_some())
            .collect();
        assert_eq!(styles.len(), 1);
        assert_eq!(
            styles[0]["updateTextStyle"]["textStyle"]["strikethrough"],
            true
        );
    }

    #[test]
    fn test_markdown_code_block() {
        let requests = markdown_to_batch_requests("```\nlet x = 1;\n```\n", 1);
        let styles: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("updateTextStyle").is_some())
            .collect();
        assert!(!styles.is_empty());
        assert_eq!(
            styles[0]["updateTextStyle"]["textStyle"]["weightedFontFamily"]["fontFamily"],
            "Courier New"
        );
    }

    #[test]
    fn test_markdown_empty() {
        let requests = markdown_to_batch_requests("", 1);
        assert!(requests.is_empty());
    }

    #[test]
    fn test_markdown_table() {
        let md = "| Name | Value |\n|------|-------|\n| Alpha | 100 |\n| Beta | 200 |\n";
        let requests = markdown_to_batch_requests(md, 1);
        let tables: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("insertTable").is_some())
            .collect();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0]["insertTable"]["rows"], 3);
        assert_eq!(tables[0]["insertTable"]["columns"], 2);
    }

    #[test]
    fn test_markdown_table_no_cell_inserts() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let requests = markdown_to_batch_requests(md, 1);
        for (i, r) in requests.iter().enumerate() {
            eprintln!("req[{i}]: {}", serde_json::to_string(r).unwrap());
        }
        // Should only have insertTable with _tableData, no cell insertText
        assert_eq!(
            requests.len(),
            1,
            "Expected only insertTable, got {} requests",
            requests.len()
        );
        assert!(requests[0].get("insertTable").is_some());
        assert!(requests[0].pointer("/insertTable/_tableData").is_some());
    }

    #[test]
    fn test_markdown_table_with_text() {
        let md = "# Title\n\nSome text.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nMore text.\n";
        let requests = markdown_to_batch_requests(md, 1);
        let has_insert = requests.iter().any(|r| r.get("insertText").is_some());
        let has_table = requests.iter().any(|r| r.get("insertTable").is_some());
        let has_heading = requests
            .iter()
            .any(|r| r.get("updateParagraphStyle").is_some());
        assert!(has_insert);
        assert!(has_table);
        assert!(has_heading);
    }

    #[test]
    fn test_output_schema_on_outline_tool() {
        let schema = outline_tool_schema();
        let os = &schema["outputSchema"];
        assert_eq!(os["type"], "object");
        assert!(os["properties"]["title"].is_object());
        assert!(os["properties"]["elements"].is_object());
        assert!(os["properties"]["endIndex"].is_object());
        let items = &os["properties"]["elements"]["items"]["properties"];
        assert!(items["type"].is_object());
        assert!(items["startIndex"].is_object());
    }

    #[test]
    fn test_output_schema_on_find_tool() {
        let schema = find_tool_schema();
        let os = &schema["outputSchema"];
        assert_eq!(os["type"], "object");
        assert!(os["properties"]["found"].is_object());
        assert!(os["properties"]["startIndex"].is_object());
        assert!(os["properties"]["endIndex"].is_object());
        assert_eq!(os["required"][0], "found");
    }

    #[test]
    fn test_output_schema_on_read_table_tool() {
        let schema = read_table_tool_schema();
        let os = &schema["outputSchema"];
        assert_eq!(os["type"], "object");
        assert!(os["properties"]["rows"].is_object());
        assert!(os["properties"]["row_count"].is_object());
        assert!(os["properties"]["column_count"].is_object());
    }

    #[test]
    fn test_output_schema_on_docs_read_tool() {
        let schema = docs_read_tool_schema();
        let os = &schema["outputSchema"];
        assert_eq!(os["type"], "object");
        assert!(os["properties"]["text"].is_object());
        assert!(os["properties"]["tables"].is_object());
        assert_eq!(os["required"][0], "text");
    }

    #[test]
    fn test_extract_all_tables_empty_doc() {
        let doc = json!({ "body": { "content": [] } });
        let tables = extract_all_tables(&doc);
        assert!(tables.is_empty());
    }

    #[test]
    fn test_extract_all_tables_no_body() {
        let doc = json!({});
        let tables = extract_all_tables(&doc);
        assert!(tables.is_empty());
    }

    #[test]
    fn test_extract_all_tables_with_tables() {
        let doc = json!({
            "body": {
                "content": [
                    {
                        "startIndex": 1,
                        "endIndex": 50,
                        "table": {
                            "rows": 2,
                            "columns": 2,
                            "tableRows": [
                                {
                                    "tableCells": [
                                        { "content": [{ "paragraph": { "elements": [{ "textRun": { "content": "A1" } }] } }] },
                                        { "content": [{ "paragraph": { "elements": [{ "textRun": { "content": "B1" } }] } }] }
                                    ]
                                },
                                {
                                    "tableCells": [
                                        { "content": [{ "paragraph": { "elements": [{ "textRun": { "content": "A2" } }] } }] },
                                        { "content": [{ "paragraph": { "elements": [{ "textRun": { "content": "B2" } }] } }] }
                                    ]
                                }
                            ]
                        }
                    },
                    {
                        "startIndex": 51,
                        "endIndex": 100,
                        "table": {
                            "rows": 1,
                            "columns": 1,
                            "tableRows": [
                                {
                                    "tableCells": [
                                        { "content": [{ "paragraph": { "elements": [{ "textRun": { "content": "Only" } }] } }] }
                                    ]
                                }
                            ]
                        }
                    }
                ]
            }
        });
        let tables = extract_all_tables(&doc);
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0]["index"], 0);
        assert_eq!(tables[0]["rows"][0][0], "A1");
        assert_eq!(tables[0]["rows"][1][1], "B2");
        assert_eq!(tables[0]["row_count"], 2);
        assert_eq!(tables[0]["column_count"], 2);
        assert_eq!(tables[1]["index"], 1);
        assert_eq!(tables[1]["rows"][0][0], "Only");
        assert_eq!(tables[1]["row_count"], 1);
    }
}
#[test]
fn test_heading_level_known() {
    assert_eq!(heading_level("HEADING_1"), Some(1));
    assert_eq!(heading_level("HEADING_3"), Some(3));
    assert_eq!(heading_level("HEADING_6"), Some(6));
}

#[test]
fn test_heading_level_unknown() {
    assert_eq!(heading_level("NORMAL_TEXT"), None);
    assert_eq!(heading_level("TITLE"), None);
}

#[test]
fn test_find_section_range_basic() {
    let doc = json!({
        "body": {
            "content": [
                { "startIndex": 1, "endIndex": 10, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_1" },
                    "elements": [{ "textRun": { "content": "Introduction\n" } }]
                }},
                { "startIndex": 10, "endIndex": 30, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                    "elements": [{ "textRun": { "content": "Some body text\n" } }]
                }},
                { "startIndex": 30, "endIndex": 45, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_1" },
                    "elements": [{ "textRun": { "content": "Next Section\n" } }]
                }}
            ]
        }
    });
    let range = find_section_range(&doc, "Introduction");
    assert_eq!(range, Some((1, 30)));
}

#[test]
fn test_find_section_range_to_end() {
    let doc = json!({
        "body": {
            "content": [
                { "startIndex": 1, "endIndex": 10, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_2" },
                    "elements": [{ "textRun": { "content": "Only Section\n" } }]
                }},
                { "startIndex": 10, "endIndex": 50, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                    "elements": [{ "textRun": { "content": "Content goes here\n" } }]
                }}
            ]
        }
    });
    let range = find_section_range(&doc, "Only Section");
    assert_eq!(range, Some((1, 49)));
}

#[test]
fn test_find_section_range_not_found() {
    let doc = json!({
        "body": {
            "content": [
                { "startIndex": 1, "endIndex": 10, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_1" },
                    "elements": [{ "textRun": { "content": "Existing\n" } }]
                }}
            ]
        }
    });
    assert!(find_section_range(&doc, "Missing").is_none());
}

#[test]
fn test_find_section_range_subsection_not_terminated_by_lower() {
    let doc = json!({
        "body": {
            "content": [
                { "startIndex": 1, "endIndex": 10, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_2" },
                    "elements": [{ "textRun": { "content": "Parent\n" } }]
                }},
                { "startIndex": 10, "endIndex": 20, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_3" },
                    "elements": [{ "textRun": { "content": "Child\n" } }]
                }},
                { "startIndex": 20, "endIndex": 30, "paragraph": {
                    "paragraphStyle": { "namedStyleType": "HEADING_2" },
                    "elements": [{ "textRun": { "content": "Sibling\n" } }]
                }}
            ]
        }
    });
    // H2 "Parent" should include the H3 child, stopping at the next H2
    let range = find_section_range(&doc, "Parent");
    assert_eq!(range, Some((1, 20)));
    #[test]
    fn test_heading_level_known() {
        assert_eq!(heading_level("HEADING_1"), Some(1));
        assert_eq!(heading_level("HEADING_3"), Some(3));
        assert_eq!(heading_level("HEADING_6"), Some(6));
    }

    #[test]
    fn test_heading_level_unknown() {
        assert_eq!(heading_level("NORMAL_TEXT"), None);
        assert_eq!(heading_level("TITLE"), None);
    }

    #[test]
    fn test_find_section_range_basic() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "Introduction\n" } }]
                    }},
                    { "startIndex": 10, "endIndex": 30, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                        "elements": [{ "textRun": { "content": "Some body text\n" } }]
                    }},
                    { "startIndex": 30, "endIndex": 45, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "Next Section\n" } }]
                    }}
                ]
            }
        });
        let range = find_section_range(&doc, "Introduction");
        assert_eq!(range, Some((1, 30)));
    }

    #[test]
    fn test_find_section_range_to_end() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_2" },
                        "elements": [{ "textRun": { "content": "Only Section\n" } }]
                    }},
                    { "startIndex": 10, "endIndex": 50, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                        "elements": [{ "textRun": { "content": "Content goes here\n" } }]
                    }}
                ]
            }
        });
        let range = find_section_range(&doc, "Only Section");
        assert_eq!(range, Some((1, 49)));
    }

    #[test]
    fn test_find_section_range_not_found() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "Existing\n" } }]
                    }}
                ]
            }
        });
        assert!(find_section_range(&doc, "Missing").is_none());
    }

    #[test]
    fn test_find_section_range_subsection_not_terminated_by_lower() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_2" },
                        "elements": [{ "textRun": { "content": "Parent\n" } }]
                    }},
                    { "startIndex": 10, "endIndex": 20, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_3" },
                        "elements": [{ "textRun": { "content": "Child\n" } }]
                    }},
                    { "startIndex": 20, "endIndex": 30, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_2" },
                        "elements": [{ "textRun": { "content": "Sibling\n" } }]
                    }}
                ]
            }
        });
        // H2 "Parent" should include the H3 child, stopping at the next H2
        let range = find_section_range(&doc, "Parent");
        assert_eq!(range, Some((1, 20)));
    }
}
