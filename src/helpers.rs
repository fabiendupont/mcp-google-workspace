use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
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

                requests.push(json!({
                    "insertTable": {
                        "rows": num_rows,
                        "columns": num_cols,
                        "location": { "index": current_index }
                    }
                }));

                for r in (0..num_rows).rev() {
                    let row = &rows[r as usize];
                    for c in (0..num_cols).rev() {
                        let cell_text = row.get(c as usize).map(|s| s.as_str()).unwrap_or("");
                        if cell_text.is_empty() {
                            continue;
                        }
                        let cell_idx = current_index + 3 + r * (2 * num_cols + 1) + c * 2;
                        requests.push(json!({
                            "insertText": {
                                "text": cell_text,
                                "location": { "index": cell_idx }
                            }
                        }));
                    }
                }

                if *header && num_rows > 0 {
                    let header_start = current_index + 3;
                    let header_end = current_index + 3 + 2 * num_cols;
                    requests.push(json!({
                        "updateTextStyle": {
                            "textStyle": { "bold": true },
                            "fields": "bold",
                            "range": {
                                "startIndex": header_start,
                                "endIndex": header_end
                            }
                        }
                    }));
                }

                let cell_text_len: i32 = rows.iter()
                    .flat_map(|r| r.iter())
                    .map(|s| s.chars().count() as i32)
                    .sum();
                // table structural footprint + cell text content
                current_index += 2 + num_rows * (2 * num_cols + 1) + cell_text_len;
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

pub fn markdown_tool_schema() -> Value {
    json!({
        "name": "gws_docs_import_markdown",
        "description": "Import Markdown content into a Google Doc with proper formatting. Converts headings, bold, italic, lists, links, code, and images to native Google Docs elements. Can insert at a position or replace a section.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Target Google Doc ID. Omit to create a new doc."
                },
                "markdown": {
                    "type": "string",
                    "description": "Markdown content to import"
                },
                "section": {
                    "type": "string",
                    "description": "Heading text to find and replace (e.g., 'Executive Summary'). Content from this heading to the next same-level heading is replaced."
                },
                "position": {
                    "type": "string",
                    "enum": ["start", "end"],
                    "description": "Where to insert (ignored if section is provided)"
                },
                "index": {
                    "type": "integer",
                    "description": "Specific character index (overrides position)"
                },
                "template_id": {
                    "type": "string",
                    "description": "Google Doc ID to copy named styles from"
                },
                "title": {
                    "type": "string",
                    "description": "Doc title (when creating new doc without document_id)"
                },
                "folder_id": {
                    "type": "string",
                    "description": "Drive folder ID (when creating new doc)"
                }
            },
            "required": ["markdown"]
        }
    })
}

pub fn helper_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "name": "gws_docs_insert_text",
            "description": "PURPOSE: Insert a single text block into a Google Doc with optional styling. \
WHEN TO USE: Only when you need precise control over position and styling for a single text block. \
For writing document content, prefer gws_docs_write which handles Markdown conversion automatically. \
HOW TO USE: Set 'paragraph_style' on EVERY call (TITLE, SUBTITLE, HEADING_1, NORMAL_TEXT). \
Without paragraph_style, text renders as unstyled default. Use 'sections' array to insert multiple \
styled blocks in one call. \
LIMITATIONS: Requires 'document_id'. Does not support Markdown — text is inserted literally.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "document_id": {
                        "type": "string",
                        "description": "The Google Docs document ID"
                    },
                    "text": {
                        "type": "string",
                        "description": "The text to insert"
                    },
                    "position": {
                        "type": "string",
                        "enum": ["end", "start"],
                        "description": "Where to insert: 'end' or 'start'. Use 'index' property for specific position."
                    },
                    "index": {
                        "type": "integer",
                        "description": "Specific character index to insert at (1-based). Overrides 'position'."
                    },
                    "bold": { "type": "boolean", "description": "Make text bold" },
                    "italic": { "type": "boolean", "description": "Make text italic" },
                    "font_size_pt": { "type": "number", "description": "Font size in points" },
                    "font_family": { "type": "string", "description": "Font family name" },
                    "foreground_color": { "type": "string", "description": "Text color as hex (e.g. '#CC0000')" },
                    "background_color": { "type": "string", "description": "Highlight color as hex" },
                    "paragraph_style": {
                        "type": "string",
                        "enum": ["NORMAL_TEXT", "HEADING_1", "HEADING_2", "HEADING_3", "HEADING_4", "HEADING_5", "HEADING_6", "TITLE", "SUBTITLE"],
                        "description": "Named paragraph style to apply"
                    },
                    "sections": {
                        "type": "array",
                        "description": "Multiple text blocks to insert in sequence (alternative to single text). Each can have its own style.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "paragraph_style": { "type": "string" },
                                "bold": { "type": "boolean" },
                                "foreground_color": { "type": "string" }
                            },
                            "required": ["text"]
                        }
                    }
                },
                "required": ["document_id"]
            }
        }),
        json!({
            "name": "gws_docs_insert_table",
            "description": "Insert a table into a Google Doc. Returns a batchUpdate request.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "document_id": {
                        "type": "string",
                        "description": "The Google Docs document ID"
                    },
                    "rows": {
                        "type": "integer",
                        "description": "Number of rows"
                    },
                    "columns": {
                        "type": "integer",
                        "description": "Number of columns"
                    },
                    "position": {
                        "type": "string",
                        "enum": ["end", "start"],
                        "description": "Where to insert: 'end' or 'start'"
                    },
                    "index": {
                        "type": "integer",
                        "description": "Specific character index (overrides position)"
                    }
                },
                "required": ["document_id", "rows", "columns"]
            }
        }),
        json!({
            "name": "gws_docs_insert_image",
            "description": "Insert an inline image into a Google Doc. Requires a publicly accessible image URL (Google Docs API limitation). Use gws_generate_image to create images — it uploads to Drive and returns a drive_file_id for manual insertion via the Docs UI.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "document_id": {
                        "type": "string",
                        "description": "The Google Docs document ID"
                    },
                    "image_url": {
                        "type": "string",
                        "description": "Public URL of the image to insert"
                    },
                    "drive_file_id": {
                        "type": "string",
                        "description": "Google Drive file ID of the image. The image is downloaded and embedded directly — no public sharing needed."
                    },
                    "image_data": {
                        "type": "string",
                        "description": "Base64-encoded image data to embed directly"
                    },
                    "image_content_type": {
                        "type": "string",
                        "description": "MIME type when using image_data (default: image/png)",
                        "enum": ["image/png", "image/jpeg", "image/gif"]
                    },
                    "position": {
                        "type": "string",
                        "enum": ["end", "start"],
                        "description": "Where to insert"
                    },
                    "index": {
                        "type": "integer",
                        "description": "Specific character index (overrides position)"
                    },
                    "width_pt": {
                        "type": "number",
                        "description": "Image width in points"
                    },
                    "height_pt": {
                        "type": "number",
                        "description": "Image height in points"
                    }
                },
                "required": ["document_id"]
            }
        }),
        json!({
            "name": "gws_docs_format_text",
            "description": "Apply text and paragraph styling to a range in a Google Doc. Use start_index/end_index OR text (finds the text automatically).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "document_id": {
                        "type": "string",
                        "description": "The Google Docs document ID"
                    },
                    "start_index": {
                        "type": "integer",
                        "description": "Start of the range (1-based, inclusive). Not needed if 'text' is provided."
                    },
                    "end_index": {
                        "type": "integer",
                        "description": "End of the range (exclusive). Not needed if 'text' is provided."
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to find and format (alternative to start_index/end_index)"
                    },
                    "occurrence": {
                        "type": "integer",
                        "description": "Which occurrence to format (1-based, default 1)"
                    },
                    "bold": { "type": "boolean" },
                    "italic": { "type": "boolean" },
                    "font_size_pt": { "type": "number" },
                    "font_family": { "type": "string" },
                    "foreground_color": { "type": "string", "description": "Hex color like '#CC0000'" },
                    "background_color": { "type": "string", "description": "Hex highlight color" },
                    "named_style": {
                        "type": "string",
                        "enum": ["NORMAL_TEXT", "HEADING_1", "HEADING_2", "HEADING_3", "HEADING_4", "HEADING_5", "HEADING_6", "TITLE", "SUBTITLE"],
                        "description": "Named paragraph style"
                    },
                    "alignment": {
                        "type": "string",
                        "enum": ["START", "CENTER", "END", "JUSTIFIED"],
                        "description": "Paragraph alignment"
                    }
                },
                "required": ["document_id"]
            }
        }),
        json!({
            "name": "gws_docs_add_bullets",
            "description": "Add bullet or numbered list formatting to a range of paragraphs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "document_id": {
                        "type": "string",
                        "description": "The Google Docs document ID"
                    },
                    "start_index": {
                        "type": "integer",
                        "description": "Start of the range"
                    },
                    "end_index": {
                        "type": "integer",
                        "description": "End of the range"
                    },
                    "preset": {
                        "type": "string",
                        "enum": [
                            "BULLET_DISC_CIRCLE_SQUARE",
                            "BULLET_DIAMONDX_ARROW3D_SQUARE",
                            "BULLET_CHECKBOX",
                            "BULLET_ARROW_DIAMOND_DISC",
                            "BULLET_STAR_CIRCLE_SQUARE",
                            "BULLET_ARROW3D_CIRCLE_SQUARE",
                            "BULLET_LEFTTRIANGLE_DIAMOND_DISC",
                            "NUMBERED_DECIMAL_ALPHA_ROMAN",
                            "NUMBERED_DECIMAL_ALPHA_ROMAN_PARENS",
                            "NUMBERED_DECIMAL_NESTED",
                            "NUMBERED_UPPERALPHA_ALPHA_ROMAN",
                            "NUMBERED_UPPERROMAN_UPPERALPHA_DECIMAL",
                            "NUMBERED_ZERODECIMAL_ALPHA_ROMAN"
                        ],
                        "description": "Bullet preset style"
                    }
                },
                "required": ["document_id", "start_index", "end_index", "preset"]
            }
        }),
    ]
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

            if let Some(inline_objs) = elem.get("paragraph").and_then(|p| p.get("elements")).and_then(|v| v.as_array()) {
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
            if let Some(paragraph) = elem.get("paragraph") {
                if let Some(elements) = paragraph.get("elements").and_then(|v| v.as_array()) {
                    for pe in elements {
                        if let Some(text_run) = pe.get("textRun") {
                            let text = text_run
                                .get("content")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let element_start = pe
                                .get("startIndex")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);

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

    if let Some(items) = items {
        if !items.is_empty() {
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
    }

    requests
}

pub fn structure_tool_schema() -> Value {
    json!({
        "name": "gws_docs_structure",
        "title": "Document Structure",
        "description": "Get a compact outline of a Google Doc: headings, tables, images with start/end indexes. Much smaller than fetching the full document.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": { "type": "string", "description": "Document ID" }
            },
            "required": ["document_id"]
        }
    })
}

pub fn find_text_tool_schema() -> Value {
    json!({
        "name": "gws_docs_find_text",
        "title": "Find Text in Document",
        "description": "Find text in a Google Doc and return its start/end indexes. Use before gws_docs_format_text or gws_docs_add_bullets.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": { "type": "string", "description": "Document ID" },
                "text": { "type": "string", "description": "Text to search for (exact substring match)" },
                "occurrence": { "type": "integer", "description": "Which occurrence (1-based, default 1)" }
            },
            "required": ["document_id", "text"]
        }
    })
}

pub fn append_section_tool_schema() -> Value {
    json!({
        "name": "gws_docs_append_section",
        "title": "Append Section",
        "description": "PREFERRED for document building. Append a complete section: heading + body text + optional bullet list. All in one call, no index math needed. Use this instead of multiple gws_docs_insert_text calls.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": { "type": "string", "description": "Document ID" },
                "heading": { "type": "string", "description": "Section heading text" },
                "heading_level": { "type": "integer", "description": "Heading level 1-6 (default 1)" },
                "body": { "type": "string", "description": "Body text (newlines become paragraphs)" },
                "items": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Bullet list items"
                },
                "bullet_preset": { "type": "string", "description": "Bullet style (default BULLET_DISC_CIRCLE_SQUARE)" }
            },
            "required": ["document_id"]
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
        let Some(table_row) = table_rows.get(row_idx) else { break };
        let Some(cells) = table_row.get("tableCells").and_then(|v| v.as_array()) else {
            continue;
        };
        for (col_idx, cell_text) in data_row.iter().enumerate() {
            let Some(cell) = cells.get(col_idx) else { break };
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

    inserts.sort_by(|a, b| b.index.cmp(&a.index));

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

    let tables: Vec<&Value> = content.iter().filter(|e| e.get("table").is_some()).collect();
    let Some(table_elem) = tables.get(table_index) else {
        return json!({ "error": format!("Table index {} not found ({} tables in doc)", table_index, tables.len()) });
    };

    let start_index = table_elem.get("startIndex").and_then(|v| v.as_i64()).unwrap_or(0);
    let end_index = table_elem.get("endIndex").and_then(|v| v.as_i64()).unwrap_or(0);

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
                                            e.pointer("/textRun/content")
                                                .and_then(|v| v.as_str())
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

pub fn insert_table_tool_schema() -> Value {
    json!({
        "name": "gws_docs_insert_table",
        "title": "Insert Table",
        "description": "PURPOSE: Insert a table into a Google Doc, optionally pre-filled with data. \
WHEN TO USE: Use when you need a table in the document. Pass headers and rows as JSON arrays — \
much easier than trying to write Markdown table syntax. \
HOW TO USE: Set rows/columns for an empty table, OR pass headers + rows arrays to create a \
pre-filled table with bold headers. \
LIMITATIONS: Requires document_id. Maximum ~20 rows recommended for performance.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Doc ID (accepts documentId too)"
                },
                "headers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Column header texts (creates a header row with bold text)"
                },
                "rows": {
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "description": "Data rows as array of arrays. Each inner array is one row."
                },
                "columns": {
                    "type": "integer",
                    "description": "Number of columns (only needed for empty table without headers)"
                },
                "position": {
                    "type": "string",
                    "enum": ["end", "start"],
                    "description": "Where to insert (default: end)"
                }
            },
            "required": ["document_id"]
        }
    })
}

pub fn read_table_tool_schema() -> Value {
    json!({
        "name": "gws_docs_read_table",
        "title": "Read Table from Google Doc",
        "description": "PURPOSE: Read a table's content from a Google Doc as a JSON array. \
WHEN TO USE: Use when you need to inspect or extract table data from a document. \
Returns rows as arrays of cell values, plus the table's character indexes. \
HOW TO USE: Pass document_id and optionally table_index (0-based, default 0 for first table).",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Doc ID (accepts documentId too)"
                },
                "table_index": {
                    "type": "integer",
                    "description": "Which table to read (0-based, default 0 = first table)"
                }
            },
            "required": ["document_id"]
        }
    })
}

pub fn docs_write_tool_schema() -> Value {
    json!({
        "name": "gws_docs_write",
        "title": "Write to Google Doc",
        "description": "PURPOSE: Write formatted content to a Google Doc. The server converts your content \
to native Google Docs elements (headings, bold, italic, bullets, numbered lists, tables). \
WHEN TO USE: Use this tool whenever you need to create or add content to a Google Doc. \
This is the primary document writing tool — prefer it over lower-level tools. \
HOW TO USE: Pass content as a Markdown string in the 'content' parameter. Use '\\n' for newlines. \
To create a new doc, provide 'title' (and optionally 'folder_id'). \
To write to an existing doc, provide 'document_id'. \
LIMITATIONS: The 'content' parameter must be a string, not an object or array. \
Requires either 'document_id' or 'title'. If neither is provided, the call will fail.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "ID of an existing Google Doc to write to. Get this from drive files.create response or gws_docs_read. Also accepts camelCase 'documentId'."
                },
                "title": {
                    "type": "string",
                    "description": "Title for a new document. When provided without document_id, creates a new Google Doc with this name."
                },
                "folder_id": {
                    "type": "string",
                    "description": "Google Drive folder ID where the new doc should be created. Only used when creating a new doc with 'title'."
                },
                "content": {
                    "type": "string",
                    "description": "Content to write as a string. Use Markdown syntax: # for title, ## for headings, **bold**, *italic*, - for bullets, 1. for numbered lists, | for tables. Use \\n for newlines within the string."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "plain"],
                    "description": "Content format. 'markdown' (default): full Markdown parsing with headings, lists, tables, inline formatting. 'plain': minimal parsing, # lines become headings."
                },
                "section": {
                    "type": "string",
                    "description": "Name of an existing heading to find and replace. The content from that heading to the next same-level heading is replaced with the new content."
                },
                "position": {
                    "type": "string",
                    "enum": ["end", "start"],
                    "description": "Where to insert content. 'end' (default) appends after existing content. 'start' inserts at the beginning."
                },
                "template_id": {
                    "type": "string",
                    "description": "Document ID of a template to copy named styles from (font families, sizes, colors for headings)."
                }
            },
            "required": ["content"]
        }
    })
}

pub fn docs_read_tool_schema() -> Value {
    json!({
        "name": "gws_docs_read",
        "title": "Read Google Doc",
        "description": "PURPOSE: Read and inspect a Google Doc's content and structure. Returns a compact \
representation that is much smaller than the raw Google Docs API response. \
WHEN TO USE: Use this tool to inspect a document's structure before writing, to verify content \
after writing, or to find text positions for formatting operations. \
HOW TO USE: Pass the document_id and choose an output format. Use 'structure' (default) for a \
compact outline with element types and indexes. Use 'markdown' to get the content as Markdown. \
Use 'search' to find specific text and get its start/end character indexes. \
LIMITATIONS: The 'full' output format returns the complete Google Docs JSON which can be very \
large (60KB+). Prefer 'structure' or 'markdown' for token efficiency.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "ID of the Google Doc to read. Also accepts camelCase 'documentId'."
                },
                "output": {
                    "type": "string",
                    "enum": ["structure", "markdown", "plain", "full"],
                    "description": "Output format. 'structure' (default): compact outline with element types (title, heading, paragraph, list_item, table, image) and character indexes. 'markdown': document content converted to Markdown. 'plain': raw text without formatting. 'full': complete Google Docs JSON (large)."
                },
                "search": {
                    "type": "string",
                    "description": "Search for this text in the document. Returns the startIndex and endIndex of the first match. Useful before calling formatting tools that need character positions."
                }
            },
            "required": ["document_id"]
        }
    })
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
    fn test_helper_tool_schemas_count() {
        let schemas = helper_tool_schemas();
        assert_eq!(schemas.len(), 5);
        let names: Vec<&str> = schemas.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"gws_docs_insert_text"));
        assert!(names.contains(&"gws_docs_insert_table"));
        assert!(names.contains(&"gws_docs_insert_image"));
        assert!(names.contains(&"gws_docs_format_text"));
        assert!(names.contains(&"gws_docs_add_bullets"));
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
    fn test_markdown_tool_schema() {
        let schema = markdown_tool_schema();
        assert_eq!(schema["name"], "gws_docs_import_markdown");
        assert!(schema["inputSchema"]["properties"]["markdown"].is_object());
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("markdown")));
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
}
