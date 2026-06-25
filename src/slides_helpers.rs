use serde_json::{Value, json};

use crate::marp::{MarpFrontmatter, MarpInlineStyle, MarpPresentation, MarpSlide, SlideBlock};

fn hex_to_raw_rgb(hex: &str) -> Value {
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
        "red": r as f64 / 255.0,
        "green": g as f64 / 255.0,
        "blue": b as f64 / 255.0
    })
}

#[derive(Debug, Clone)]
pub struct PlaceholderInfo {
    pub ph_type: String,
    pub index: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TemplateLayout {
    pub object_id: String,
    pub display_name: String,
    pub master_object_id: String,
    pub has_title: bool,
    pub has_body: bool,
    pub has_subtitle: bool,
    pub placeholders: Vec<PlaceholderInfo>,
}

pub fn extract_layouts(presentation: &Value) -> Vec<TemplateLayout> {
    let layouts = presentation
        .get("layouts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Use the last slide's master as the active master — that's what the Slides API enforces
    let active_master = presentation
        .get("slides")
        .and_then(|v| v.as_array())
        .and_then(|slides| slides.last())
        .and_then(|s| s.get("slideProperties"))
        .and_then(|sp| sp.get("masterObjectId"))
        .and_then(|m| m.as_str())
        .unwrap_or("");

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for layout in &layouts {
        let props = layout.get("layoutProperties").unwrap_or(layout);
        let display_name = props
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let object_id = layout
            .get("objectId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let master_id = props
            .get("masterObjectId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if display_name.is_empty() {
            continue;
        }

        // Filter to active master only — Slides API won't allow cross-master layouts
        if !active_master.is_empty() && master_id != active_master {
            continue;
        }

        if !seen.insert(display_name.clone()) {
            continue;
        }

        let mut has_title = false;
        let mut has_body = false;
        let mut has_subtitle = false;
        let mut placeholders = Vec::new();

        if let Some(elements) = layout.get("pageElements").and_then(|v| v.as_array()) {
            for elem in elements {
                if let Some(ph) = elem.get("shape").and_then(|s| s.get("placeholder")) {
                    let ph_type = ph.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    let ph_index = ph.get("index").and_then(|i| i.as_i64());
                    match ph_type {
                        "TITLE" => has_title = true,
                        "BODY" => has_body = true,
                        "SUBTITLE" => has_subtitle = true,
                        _ => {}
                    }
                    placeholders.push(PlaceholderInfo {
                        ph_type: ph_type.to_string(),
                        index: ph_index,
                    });
                }
            }
        }

        result.push(TemplateLayout {
            object_id,
            display_name,
            master_object_id: master_id,
            has_title,
            has_body,
            has_subtitle,
            placeholders,
        });
    }

    result
}

fn select_layout<'a>(
    slide: &MarpSlide,
    idx: usize,
    total: usize,
    layouts: &'a [TemplateLayout],
) -> Option<&'a TemplateLayout> {
    if layouts.is_empty() {
        return None;
    }

    let class = slide.directives.class.as_deref().unwrap_or("");
    let has_title = slide.title.is_some();
    let has_body = !slide.body_blocks.is_empty();
    let is_first = idx == 0;
    let is_last = idx == total - 1;

    let find = |name: &str| layouts.iter().find(|l| l.display_name == name);

    if class == "title" || class == "lead" {
        return find("Title");
    }
    if class == "closing" {
        return find("Closing");
    }
    if class == "section-divider" || class == "invert" {
        return find("Divider with title");
    }
    if class == "split" || class == "two-column" {
        return find("Interior title and two column body")
            .or_else(|| find("Interior title and body"));
    }

    if is_first && has_title && !has_body {
        return find("Title");
    }
    if is_last && has_title && !has_body {
        return find("Closing");
    }

    if has_title && has_body {
        return find("Interior title and body");
    }

    if has_title && !has_body {
        return find("Interior title").or_else(|| find("Divider with title"));
    }

    if !has_title && has_body {
        return find("Interior body").or_else(|| find("Interior blank"));
    }

    find("Interior blank").or_else(|| layouts.first())
}

pub fn marp_to_slide_requests(
    pres: &MarpPresentation,
    notes_object_ids: Option<&[String]>,
    layouts: Option<&[TemplateLayout]>,
) -> (Vec<Value>, Vec<Value>) {
    let mut create_requests = Vec::new();
    let mut content_requests = Vec::new();
    let total = pres.slides.len();

    for (idx, slide) in pres.slides.iter().enumerate() {
        let slide_id = format!("slide_{idx}");
        let title_id = format!("title_{idx}");
        let body_id = format!("body_{idx}");

        let has_title = slide.title.is_some();
        let has_body = !slide.body_blocks.is_empty();

        let selected_layout = layouts.and_then(|ls| select_layout(slide, idx, total, ls));

        if let Some(layout) = selected_layout {
            let mut mappings = Vec::new();
            if has_title && layout.has_title {
                let title_ph = layout.placeholders.iter().find(|p| p.ph_type == "TITLE");
                let mut lp = json!({ "type": "TITLE" });
                if let Some(ph) = title_ph {
                    if let Some(idx) = ph.index {
                        lp["index"] = json!(idx);
                    }
                }
                mappings.push(json!({
                    "layoutPlaceholder": lp,
                    "objectId": title_id
                }));
            }
            if has_body && layout.has_body {
                let body_ph = layout.placeholders.iter().find(|p| p.ph_type == "BODY");
                let mut lp = json!({ "type": "BODY" });
                if let Some(ph) = body_ph {
                    if let Some(idx) = ph.index {
                        lp["index"] = json!(idx);
                    }
                }
                mappings.push(json!({
                    "layoutPlaceholder": lp,
                    "objectId": body_id
                }));
            }

            create_requests.push(json!({
                "createSlide": {
                    "objectId": slide_id,
                    "slideLayoutReference": {
                        "layoutId": layout.object_id
                    },
                    "placeholderIdMappings": mappings
                }
            }));

            if let Some(ref title_text) = slide.title {
                if layout.has_title {
                    content_requests.push(json!({
                        "insertText": {
                            "objectId": title_id,
                            "text": title_text
                        }
                    }));
                } else {
                    emit_manual_title(
                        &title_id,
                        &slide_id,
                        title_text,
                        has_body,
                        &mut content_requests,
                    );
                }
            }

            if has_body {
                let target = if layout.has_body {
                    body_id.clone()
                } else {
                    emit_manual_body_shape(&body_id, &slide_id, has_title, &mut content_requests);
                    body_id.clone()
                };
                emit_body_content(&slide.body_blocks, &target, &mut content_requests);
            }
        } else {
            create_requests.push(json!({
                "createSlide": {
                    "objectId": slide_id
                }
            }));

            if let Some(ref title_text) = slide.title {
                emit_manual_title(
                    &title_id,
                    &slide_id,
                    title_text,
                    has_body,
                    &mut content_requests,
                );
            }

            if has_body {
                emit_manual_body_shape(&body_id, &slide_id, has_title, &mut content_requests);
                emit_body_content(&slide.body_blocks, &body_id, &mut content_requests);
            }
        }

        emit_images(slide, &slide_id, &mut content_requests);
        emit_tables(slide, &slide_id, idx, &mut content_requests);

        if needs_light_override(slide, layouts.is_some()) {
            content_requests.push(json!({
                "updatePageProperties": {
                    "objectId": slide_id,
                    "pageProperties": {
                        "pageBackgroundFill": {
                            "solidFill": { "color": { "rgbColor": hex_to_raw_rgb("#FFFFFF") } }
                        }
                    },
                    "fields": "pageBackgroundFill"
                }
            }));
            let dark_text = json!({ "opaqueColor": { "rgbColor": hex_to_raw_rgb("#151515") } });
            for target in [&title_id, &body_id] {
                content_requests.push(json!({
                    "updateTextStyle": {
                        "objectId": target,
                        "textRange": { "type": "ALL" },
                        "style": { "foregroundColor": dark_text },
                        "fields": "foregroundColor"
                    }
                }));
            }
        }

        emit_backgrounds(
            slide,
            &slide_id,
            pres,
            &title_id,
            &body_id,
            has_title,
            has_body,
            &mut content_requests,
        );

        if let Some(ref notes_text) = slide.speaker_notes {
            if let Some(ids) = notes_object_ids {
                if let Some(notes_id) = ids.get(idx) {
                    content_requests.push(json!({
                        "insertText": {
                            "objectId": notes_id,
                            "text": notes_text
                        }
                    }));
                }
            }
        }
    }

    (create_requests, content_requests)
}

fn emit_manual_title(
    title_id: &str,
    slide_id: &str,
    title_text: &str,
    has_body: bool,
    requests: &mut Vec<Value>,
) {
    let title_h = if has_body { 60.0 } else { 200.0 };
    requests.push(json!({
        "createShape": {
            "objectId": title_id,
            "shapeType": "TEXT_BOX",
            "elementProperties": {
                "pageObjectId": slide_id,
                "size": {
                    "width": { "magnitude": 620, "unit": "PT" },
                    "height": { "magnitude": title_h, "unit": "PT" }
                },
                "transform": {
                    "scaleX": 1.0,
                    "scaleY": 1.0,
                    "translateX": 40.0 * 12700.0,
                    "translateY": 30.0 * 12700.0,
                    "unit": "EMU"
                }
            }
        }
    }));
    requests.push(json!({
        "insertText": {
            "objectId": title_id,
            "text": title_text
        }
    }));
    requests.push(json!({
        "updateTextStyle": {
            "objectId": title_id,
            "textRange": { "type": "ALL" },
            "style": {
                "bold": true,
                "fontSize": { "magnitude": 28, "unit": "PT" }
            },
            "fields": "bold,fontSize"
        }
    }));
}

fn emit_manual_body_shape(
    body_id: &str,
    slide_id: &str,
    has_title: bool,
    requests: &mut Vec<Value>,
) {
    let body_y = if has_title { 100.0 } else { 40.0 };
    let body_h = if has_title { 340.0 } else { 400.0 };
    requests.push(json!({
        "createShape": {
            "objectId": body_id,
            "shapeType": "TEXT_BOX",
            "elementProperties": {
                "pageObjectId": slide_id,
                "size": {
                    "width": { "magnitude": 620, "unit": "PT" },
                    "height": { "magnitude": body_h, "unit": "PT" }
                },
                "transform": {
                    "scaleX": 1.0,
                    "scaleY": 1.0,
                    "translateX": 40.0 * 12700.0,
                    "translateY": body_y * 12700.0,
                    "unit": "EMU"
                }
            }
        }
    }));
}

fn emit_body_content(blocks: &[SlideBlock], target_id: &str, requests: &mut Vec<Value>) {
    let (body_text, style_requests, bullet_requests) = build_body_content(blocks, target_id);
    if !body_text.is_empty() {
        requests.push(json!({
            "insertText": {
                "objectId": target_id,
                "text": body_text
            }
        }));
        requests.extend(style_requests);
        requests.extend(bullet_requests);
    }
}

fn emit_images(slide: &MarpSlide, slide_id: &str, requests: &mut Vec<Value>) {
    for block in &slide.body_blocks {
        if let SlideBlock::Image {
            url,
            width,
            height,
            is_background,
        } = block
        {
            if *is_background {
                requests.push(json!({
                    "updatePageProperties": {
                        "objectId": slide_id,
                        "pageProperties": {
                            "pageBackgroundFill": {
                                "stretchedPictureFill": {
                                    "contentUrl": url
                                }
                            }
                        },
                        "fields": "pageBackgroundFill"
                    }
                }));
            } else {
                let mut size = serde_json::Map::new();
                if let Some(w) = width {
                    size.insert("width".to_string(), json!({ "magnitude": w, "unit": "PT" }));
                }
                if let Some(h) = height {
                    size.insert(
                        "height".to_string(),
                        json!({ "magnitude": h, "unit": "PT" }),
                    );
                }
                let mut elem_props = json!({ "pageObjectId": slide_id });
                if !size.is_empty() {
                    elem_props["size"] = Value::Object(size);
                }
                requests.push(json!({
                    "createImage": {
                        "url": url,
                        "elementProperties": elem_props
                    }
                }));
            }
        }
    }
}

fn emit_tables(slide: &MarpSlide, slide_id: &str, slide_idx: usize, requests: &mut Vec<Value>) {
    let mut table_num = 0;
    for block in &slide.body_blocks {
        if let SlideBlock::Table { rows } = block {
            if rows.is_empty() {
                continue;
            }
            let num_rows = rows.len();
            let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
            if num_cols == 0 {
                continue;
            }
            let table_id = format!("table_{slide_idx}_{table_num}");
            table_num += 1;

            requests.push(json!({
                "createTable": {
                    "objectId": &table_id,
                    "elementProperties": {
                        "pageObjectId": slide_id,
                        "size": {
                            "width": { "magnitude": 620, "unit": "PT" },
                            "height": { "magnitude": 30 * num_rows, "unit": "PT" }
                        },
                        "transform": {
                            "scaleX": 1.0,
                            "scaleY": 1.0,
                            "translateX": 40.0 * 12700.0,
                            "translateY": 200.0 * 12700.0,
                            "unit": "EMU"
                        }
                    },
                    "rows": num_rows,
                    "columns": num_cols
                }
            }));

            for (row_idx, row) in rows.iter().enumerate() {
                for (col_idx, cell) in row.iter().enumerate() {
                    if !cell.is_empty() {
                        requests.push(json!({
                            "insertText": {
                                "objectId": &table_id,
                                "cellLocation": {
                                    "rowIndex": row_idx,
                                    "columnIndex": col_idx
                                },
                                "text": cell
                            }
                        }));
                        if row_idx == 0 {
                            requests.push(json!({
                                "updateTextStyle": {
                                    "objectId": &table_id,
                                    "cellLocation": {
                                        "rowIndex": 0,
                                        "columnIndex": col_idx
                                    },
                                    "textRange": { "type": "ALL" },
                                    "style": { "bold": true },
                                    "fields": "bold"
                                }
                            }));
                        }
                    }
                }
            }
        }
    }
}

fn emit_backgrounds(
    slide: &MarpSlide,
    slide_id: &str,
    pres: &MarpPresentation,
    title_id: &str,
    body_id: &str,
    has_title: bool,
    has_body: bool,
    requests: &mut Vec<Value>,
) {
    if let Some(ref class) = slide.directives.class {
        let (bg, fg) = class_to_colors(class, &pres.frontmatter);
        if let Some(bg_hex) = bg {
            requests.push(json!({
                "updatePageProperties": {
                    "objectId": slide_id,
                    "pageProperties": {
                        "pageBackgroundFill": {
                            "solidFill": { "color": { "rgbColor": hex_to_raw_rgb(&bg_hex) } }
                        }
                    },
                    "fields": "pageBackgroundFill"
                }
            }));
        }
        if let Some(fg_hex) = fg {
            let fg_color = json!({ "opaqueColor": { "rgbColor": hex_to_raw_rgb(&fg_hex) } });
            let title_target = if has_title { Some(title_id) } else { None };
            let body_target = if has_body { Some(body_id) } else { None };
            for target in [title_target, body_target].into_iter().flatten() {
                requests.push(json!({
                    "updateTextStyle": {
                        "objectId": target,
                        "textRange": { "type": "ALL" },
                        "style": { "foregroundColor": fg_color },
                        "fields": "foregroundColor"
                    }
                }));
            }
        }
    }

    if let Some(ref bg_color) = slide.directives.background_color {
        requests.push(json!({
            "updatePageProperties": {
                "objectId": slide_id,
                "pageProperties": {
                    "pageBackgroundFill": {
                        "solidFill": { "color": { "rgbColor": hex_to_raw_rgb(bg_color) } }
                    }
                },
                "fields": "pageBackgroundFill"
            }
        }));
    }

    if let Some(ref bg_img) = slide.directives.background_image {
        requests.push(json!({
            "updatePageProperties": {
                "objectId": slide_id,
                "pageProperties": {
                    "pageBackgroundFill": {
                        "stretchedPictureFill": { "contentUrl": bg_img }
                    }
                },
                "fields": "pageBackgroundFill"
            }
        }));
    }
}

fn class_to_colors(class: &str, _fm: &MarpFrontmatter) -> (Option<String>, Option<String>) {
    match class {
        "title" | "lead" => (Some("#EE0000".to_string()), Some("#FFFFFF".to_string())),
        "section-divider" | "invert" => (Some("#151515".to_string()), Some("#FFFFFF".to_string())),
        "light" => (Some("#FFFFFF".to_string()), Some("#151515".to_string())),
        _ => (None, None),
    }
}

fn needs_light_override(slide: &MarpSlide, has_template: bool) -> bool {
    if !has_template {
        return false;
    }
    let class = slide.directives.class.as_deref().unwrap_or("");
    !matches!(
        class,
        "title" | "lead" | "section-divider" | "invert" | "closing"
    ) && slide.directives.background_color.is_none()
}

fn build_body_content(blocks: &[SlideBlock], target_id: &str) -> (String, Vec<Value>, Vec<Value>) {
    let mut full_text = String::new();
    let mut char_count: usize = 0;
    let mut style_requests: Vec<Value> = Vec::new();
    let mut bullet_ranges: Vec<(usize, usize, bool)> = Vec::new();

    for block in blocks {
        match block {
            SlideBlock::Text { text, styles } => {
                let offset = char_count;
                full_text.push_str(text);
                char_count += text.chars().count();
                if !full_text.ends_with('\n') {
                    full_text.push('\n');
                    char_count += 1;
                }
                for s in styles {
                    if let Some(req) = emit_slides_text_style(s, offset, target_id) {
                        style_requests.push(req);
                    }
                }
            }
            SlideBlock::BulletList { items, ordered } => {
                let bullet_start = char_count;
                for item in items {
                    let offset = char_count;
                    full_text.push_str(&item.text);
                    char_count += item.text.chars().count();
                    if !full_text.ends_with('\n') {
                        full_text.push('\n');
                        char_count += 1;
                    }
                    for s in &item.styles {
                        if let Some(req) = emit_slides_text_style(s, offset, target_id) {
                            style_requests.push(req);
                        }
                    }
                }
                let bullet_end = char_count;
                if bullet_start < bullet_end {
                    bullet_ranges.push((bullet_start, bullet_end, *ordered));
                }
            }
            SlideBlock::CodeBlock { code, .. } => {
                let offset = char_count;
                full_text.push_str(code);
                char_count += code.chars().count();
                if !full_text.ends_with('\n') {
                    full_text.push('\n');
                    char_count += 1;
                }
                let end = char_count;
                if offset < end {
                    style_requests.push(json!({
                        "updateTextStyle": {
                            "objectId": target_id,
                            "textRange": {
                                "type": "FIXED_RANGE",
                                "startIndex": offset,
                                "endIndex": end
                            },
                            "style": {
                                "fontFamily": "Courier New",
                                "fontSize": { "magnitude": 10, "unit": "PT" }
                            },
                            "fields": "fontFamily,fontSize"
                        }
                    }));
                }
            }
            SlideBlock::Table { .. } => {}
            SlideBlock::Image { .. } => {}
        }
    }

    let mut bullet_requests = Vec::new();
    for (start, end, ordered) in &bullet_ranges {
        let glyph = if *ordered {
            "NUMBERED_DIGIT_ALPHA_ROMAN"
        } else {
            "BULLET_DISC_CIRCLE_SQUARE"
        };
        bullet_requests.push(json!({
            "createParagraphBullets": {
                "objectId": target_id,
                "textRange": {
                    "type": "FIXED_RANGE",
                    "startIndex": start,
                    "endIndex": end
                },
                "bulletPreset": glyph
            }
        }));
    }

    while full_text.ends_with("\n\n") {
        full_text.pop();
    }

    let final_len = full_text.chars().count();
    style_requests.retain(|r| {
        r.get("updateTextStyle")
            .and_then(|u| u.get("textRange"))
            .and_then(|tr| tr.get("endIndex"))
            .and_then(|e| e.as_u64())
            .map(|e| (e as usize) <= final_len)
            .unwrap_or(true)
    });
    bullet_requests.retain(|r| {
        r.get("createParagraphBullets")
            .and_then(|u| u.get("textRange"))
            .and_then(|tr| tr.get("endIndex"))
            .and_then(|e| e.as_u64())
            .map(|e| (e as usize) <= final_len)
            .unwrap_or(true)
    });

    (full_text, style_requests, bullet_requests)
}

fn emit_slides_text_style(
    style: &MarpInlineStyle,
    base_offset: usize,
    object_id: &str,
) -> Option<Value> {
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
    if style.code {
        ts.insert("fontFamily".to_string(), json!("Courier New"));
        fields.push("fontFamily");
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
            "objectId": object_id,
            "textRange": {
                "type": "FIXED_RANGE",
                "startIndex": base_offset + style.start,
                "endIndex": base_offset + style.end
            },
            "style": Value::Object(ts),
            "fields": fields.join(",")
        }
    }))
}

pub fn templates_tool_schema() -> Value {
    json!({
        "name": "gws_templates",
        "title": "List presentation templates",
        "description": "List available presentation templates configured in the policy, with their layout names. Use this to discover which templates and layouts are available before creating a presentation with gws_slides_import_marp.",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Filter templates by name (optional)"
                }
            }
        }
    })
}

pub fn marp_tool_schema() -> Value {
    json!({
        "name": "gws_slides_import_marp",
        "title": "Import Marp Markdown to Slides",
        "description": "Convert Marp-flavored Markdown into a Google Slides presentation. When a template is specified, automatically selects the best layout for each slide (Title, Divider, Interior title and body, etc.) based on content. Supports slide separators (---), headings as titles, text formatting, bullet lists, code blocks, tables, images, background images, speaker notes, and per-slide directives.",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "presentation_id": {
                    "type": "string",
                    "description": "Existing presentation ID to update (replaces all slides)"
                },
                "title": {
                    "type": "string",
                    "description": "Presentation title. Required when creating a new presentation (unless presentation_id or template is provided). Searches Drive for an existing presentation with this title for create-or-update semantics."
                },
                "folder_id": {
                    "type": "string",
                    "description": "Google Drive folder ID to search in or create the presentation in"
                },
                "marp": {
                    "type": "string",
                    "description": "Marp Markdown source. Use --- to separate slides, # for titles, <!-- notes --> for speaker notes, ![bg](url) for backgrounds."
                },
                "template": {
                    "type": "string",
                    "description": "Template name (from policy) or presentation ID. The template's layouts are used for branded slide creation."
                }
            },
            "required": ["marp"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marp::parse_marp;

    #[test]
    fn test_single_slide_title_body_no_template() {
        let pres = parse_marp("# Hello\n\nWorld").unwrap();
        let (creates, contents) = marp_to_slide_requests(&pres, None, None);
        assert_eq!(creates.len(), 1);
        assert!(
            creates[0]["createSlide"]
                .get("slideLayoutReference")
                .is_none()
        );
        assert_eq!(creates[0]["createSlide"]["objectId"], "slide_0");

        let title_shape = contents
            .iter()
            .find(|r| r.get("createShape").is_some() && r["createShape"]["objectId"] == "title_0");
        assert!(title_shape.is_some());

        let title_insert = contents
            .iter()
            .find(|r| r.get("insertText").is_some() && r["insertText"]["objectId"] == "title_0");
        assert!(title_insert.is_some());
        assert_eq!(title_insert.unwrap()["insertText"]["text"], "Hello");
    }

    #[test]
    fn test_with_template_layouts() {
        let pres = parse_marp("# Hello\n\nWorld").unwrap();
        let layouts = vec![TemplateLayout {
            object_id: "layout_abc".to_string(),
            master_object_id: "master_1".to_string(),
            display_name: "Interior title and body".to_string(),
            has_title: true,
            has_body: true,
            has_subtitle: false,
            placeholders: vec![
                PlaceholderInfo {
                    ph_type: "TITLE".to_string(),
                    index: None,
                },
                PlaceholderInfo {
                    ph_type: "BODY".to_string(),
                    index: None,
                },
            ],
        }];
        let (creates, contents) = marp_to_slide_requests(&pres, None, Some(&layouts));
        assert_eq!(creates.len(), 1);
        assert_eq!(
            creates[0]["createSlide"]["slideLayoutReference"]["layoutId"],
            "layout_abc"
        );
        let mappings = creates[0]["createSlide"]["placeholderIdMappings"]
            .as_array()
            .unwrap();
        assert_eq!(mappings.len(), 2);

        let title_insert = contents
            .iter()
            .find(|r| r.get("insertText").is_some() && r["insertText"]["objectId"] == "title_0");
        assert!(title_insert.is_some());
        let no_shape = contents
            .iter()
            .find(|r| r.get("createShape").is_some() && r["createShape"]["objectId"] == "title_0");
        assert!(no_shape.is_none());
    }

    #[test]
    fn test_title_slide_layout_selection() {
        let pres = parse_marp(
            "---\nmarp: true\n---\n\n<!-- _class: title -->\n\n# My Talk\n\nSubtitle here",
        )
        .unwrap();
        let layouts = vec![
            TemplateLayout {
                object_id: "layout_title".to_string(),
                master_object_id: "master_1".to_string(),
                display_name: "Title".to_string(),
                has_title: true,
                has_body: false,
                has_subtitle: true,
                placeholders: vec![
                    PlaceholderInfo {
                        ph_type: "TITLE".to_string(),
                        index: None,
                    },
                    PlaceholderInfo {
                        ph_type: "SUBTITLE".to_string(),
                        index: None,
                    },
                ],
            },
            TemplateLayout {
                object_id: "layout_body".to_string(),
                master_object_id: "master_1".to_string(),
                display_name: "Interior title and body".to_string(),
                has_title: true,
                has_body: true,
                has_subtitle: false,
                placeholders: vec![
                    PlaceholderInfo {
                        ph_type: "TITLE".to_string(),
                        index: None,
                    },
                    PlaceholderInfo {
                        ph_type: "BODY".to_string(),
                        index: None,
                    },
                ],
            },
        ];
        let (creates, _) = marp_to_slide_requests(&pres, None, Some(&layouts));
        assert_eq!(
            creates[0]["createSlide"]["slideLayoutReference"]["layoutId"],
            "layout_title"
        );
    }

    #[test]
    fn test_divider_layout_selection() {
        let pres =
            parse_marp("# First\n\nbody\n\n---\n\n<!-- _class: section-divider -->\n\n# Evidence")
                .unwrap();
        let layouts = vec![
            TemplateLayout {
                object_id: "layout_body".to_string(),
                master_object_id: "master_1".to_string(),
                display_name: "Interior title and body".to_string(),
                has_title: true,
                has_body: true,
                has_subtitle: false,
                placeholders: vec![
                    PlaceholderInfo {
                        ph_type: "TITLE".to_string(),
                        index: None,
                    },
                    PlaceholderInfo {
                        ph_type: "BODY".to_string(),
                        index: None,
                    },
                ],
            },
            TemplateLayout {
                object_id: "layout_divider".to_string(),
                master_object_id: "master_1".to_string(),
                display_name: "Divider with title".to_string(),
                has_title: true,
                has_body: false,
                has_subtitle: true,
                placeholders: vec![
                    PlaceholderInfo {
                        ph_type: "TITLE".to_string(),
                        index: None,
                    },
                    PlaceholderInfo {
                        ph_type: "SUBTITLE".to_string(),
                        index: None,
                    },
                ],
            },
        ];
        let (creates, _) = marp_to_slide_requests(&pres, None, Some(&layouts));
        assert_eq!(
            creates[1]["createSlide"]["slideLayoutReference"]["layoutId"],
            "layout_divider"
        );
    }

    #[test]
    fn test_blank_slide_no_template() {
        let pres = parse_marp("Just some text, no heading").unwrap();
        let (creates, _) = marp_to_slide_requests(&pres, None, None);
        assert!(
            creates[0]["createSlide"]
                .get("slideLayoutReference")
                .is_none()
        );
    }

    #[test]
    fn test_background_color() {
        let pres = parse_marp("<!-- _backgroundColor: #ff0000 -->\n# Red slide").unwrap();
        let (_, contents) = marp_to_slide_requests(&pres, None, None);
        let bg_req = contents
            .iter()
            .find(|r| r.get("updatePageProperties").is_some());
        assert!(bg_req.is_some());
        let props = &bg_req.unwrap()["updatePageProperties"]["pageProperties"];
        assert!(props["pageBackgroundFill"]["solidFill"].is_object());
    }

    #[test]
    fn test_background_image() {
        let pres = parse_marp("![bg](https://example.com/img.jpg)").unwrap();
        let (_, contents) = marp_to_slide_requests(&pres, None, None);
        let bg_req = contents.iter().find(|r| {
            r.get("updatePageProperties").is_some()
                && r["updatePageProperties"]["pageProperties"]["pageBackgroundFill"]
                    .get("stretchedPictureFill")
                    .is_some()
        });
        assert!(bg_req.is_some());
    }

    #[test]
    fn test_inline_image() {
        let pres = parse_marp("![w:200 h:150](https://example.com/pic.png)").unwrap();
        let (_, contents) = marp_to_slide_requests(&pres, None, None);
        let img_req = contents.iter().find(|r| r.get("createImage").is_some());
        assert!(img_req.is_some());
        assert_eq!(
            img_req.unwrap()["createImage"]["url"],
            "https://example.com/pic.png"
        );
    }

    #[test]
    fn test_bullet_list_requests() {
        let pres = parse_marp("- Item A\n- Item B").unwrap();
        let (_, contents) = marp_to_slide_requests(&pres, None, None);
        let bullet_req = contents
            .iter()
            .find(|r| r.get("createParagraphBullets").is_some());
        assert!(bullet_req.is_some());
        assert_eq!(
            bullet_req.unwrap()["createParagraphBullets"]["bulletPreset"],
            "BULLET_DISC_CIRCLE_SQUARE"
        );
    }

    #[test]
    fn test_code_block_styling() {
        let pres = parse_marp("```\ncode here\n```").unwrap();
        let (_, contents) = marp_to_slide_requests(&pres, None, None);
        let style_req = contents.iter().find(|r| {
            r.get("updateTextStyle").is_some()
                && r["updateTextStyle"]["style"].get("fontFamily").is_some()
        });
        assert!(style_req.is_some());
        assert_eq!(
            style_req.unwrap()["updateTextStyle"]["style"]["fontFamily"],
            "Courier New"
        );
    }

    #[test]
    fn test_multi_slide_ids() {
        let pres = parse_marp("# Slide 1\n\n---\n\n# Slide 2\n\n---\n\n# Slide 3").unwrap();
        let (creates, _) = marp_to_slide_requests(&pres, None, None);
        assert_eq!(creates.len(), 3);
        assert_eq!(creates[0]["createSlide"]["objectId"], "slide_0");
        assert_eq!(creates[1]["createSlide"]["objectId"], "slide_1");
        assert_eq!(creates[2]["createSlide"]["objectId"], "slide_2");
    }

    #[test]
    fn test_speaker_notes_with_ids() {
        let pres = parse_marp("# Title\n\n<!-- notes -->\nMy notes here").unwrap();
        let notes_ids = vec!["notes_shape_abc".to_string()];
        let (_, contents) = marp_to_slide_requests(&pres, Some(&notes_ids), None);
        let notes_req = contents.iter().find(|r| {
            r.get("insertText").is_some() && r["insertText"]["objectId"] == "notes_shape_abc"
        });
        assert!(notes_req.is_some());
    }

    #[test]
    fn test_extract_layouts() {
        let pres_json = json!({
            "layouts": [
                {
                    "objectId": "layout_1",
                    "layoutProperties": {
                        "displayName": "Title",
                        "masterObjectId": "master_1"
                    },
                    "pageElements": [
                        { "shape": { "placeholder": { "type": "TITLE" } } },
                        { "shape": { "placeholder": { "type": "SUBTITLE" } } }
                    ]
                },
                {
                    "objectId": "layout_2",
                    "layoutProperties": {
                        "displayName": "Interior title and body",
                        "masterObjectId": "master_1"
                    },
                    "pageElements": [
                        { "shape": { "placeholder": { "type": "TITLE" } } },
                        { "shape": { "placeholder": { "type": "BODY" } } }
                    ]
                }
            ]
        });
        let layouts = extract_layouts(&pres_json);
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].display_name, "Title");
        assert!(layouts[0].has_title);
        assert!(layouts[0].has_subtitle);
        assert!(!layouts[0].has_body);
        assert_eq!(layouts[1].display_name, "Interior title and body");
        assert!(layouts[1].has_title);
        assert!(layouts[1].has_body);
    }

    #[test]
    fn test_tool_schema_shape() {
        let schema = marp_tool_schema();
        assert_eq!(schema["name"], "gws_slides_import_marp");
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("marp")));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("marp"));
        assert!(props.contains_key("presentation_id"));
        assert!(props.contains_key("template"));
        assert!(props.contains_key("title"));
        assert!(props.contains_key("folder_id"));
    }
}
