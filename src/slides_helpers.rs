use serde_json::{Value, json};

use crate::marp::{MarpFrontmatter, MarpInlineStyle, MarpPresentation, MarpSlide, SlideBlock};
use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::server::ServerState;
use crate::tools;
use google_workspace::error::GwsError;

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

    // The Slides API assigns new slides to the last master in the presentation
    let active_master = presentation
        .get("masters")
        .and_then(|v| v.as_array())
        .and_then(|masters| masters.last())
        .and_then(|m| m.get("objectId"))
        .and_then(|id| id.as_str())
        .or_else(|| {
            presentation
                .get("slides")
                .and_then(|v| v.as_array())
                .and_then(|slides| slides.last())
                .and_then(|s| s.get("slideProperties"))
                .and_then(|sp| sp.get("masterObjectId"))
                .and_then(|m| m.as_str())
        })
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
                if let Some(ph) = title_ph
                    && let Some(idx) = ph.index
                {
                    lp["index"] = json!(idx);
                }
                mappings.push(json!({
                    "layoutPlaceholder": lp,
                    "objectId": title_id
                }));
            }
            if has_body && layout.has_body {
                let body_ph = layout.placeholders.iter().find(|p| p.ph_type == "BODY");
                let mut lp = json!({ "type": "BODY" });
                if let Some(ph) = body_ph
                    && let Some(idx) = ph.index
                {
                    lp["index"] = json!(idx);
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

        if let Some(ref notes_text) = slide.speaker_notes
            && let Some(ids) = notes_object_ids
            && let Some(notes_id) = ids.get(idx)
        {
            content_requests.push(json!({
                "insertText": {
                    "objectId": notes_id,
                    "text": notes_text
                }
            }));
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
                } else {
                    elem_props["size"] = json!({
                        "width": { "magnitude": 360, "unit": "PT" },
                        "height": { "magnitude": 270, "unit": "PT" }
                    });
                    elem_props["transform"] = json!({
                        "scaleX": 1.0,
                        "scaleY": 1.0,
                        "translateX": 130.0 * 12700.0,
                        "translateY": 135.0 * 12700.0,
                        "unit": "EMU"
                    });
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

pub fn build_body_content(
    blocks: &[SlideBlock],
    target_id: &str,
) -> (String, Vec<Value>, Vec<Value>) {
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
        "description": "List presentation templates and their slide layouts.",
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
        "description": "Create a presentation from Marp Markdown. Supports headings, bullets, tables, images, speaker notes.",
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

pub fn extract_slide_text(page_elements: &[Value], placeholder_type: &str) -> String {
    for elem in page_elements {
        let ph = elem
            .get("shape")
            .and_then(|s| s.get("placeholder"))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if ph != placeholder_type {
            continue;
        }
        return extract_text_from_shape(elem);
    }
    String::new()
}

pub fn extract_all_body_text(page_elements: &[Value]) -> String {
    let mut parts = Vec::new();
    for elem in page_elements {
        let ph_type = elem
            .get("shape")
            .and_then(|s| s.get("placeholder"))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        match ph_type {
            "TITLE" | "CENTERED_TITLE" => continue,
            "BODY" | "SUBTITLE" => {
                let text = extract_text_from_shape(elem);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            _ => {
                if elem.get("shape").and_then(|s| s.get("text")).is_some() {
                    let has_placeholder = elem
                        .get("shape")
                        .and_then(|s| s.get("placeholder"))
                        .is_some();
                    if !has_placeholder {
                        let text = extract_text_from_shape(elem);
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                }
            }
        }
    }
    parts.join("\n")
}

pub fn extract_text_from_shape(elem: &Value) -> String {
    let text_elements = elem
        .get("shape")
        .and_then(|s| s.get("text"))
        .and_then(|t| t.get("textElements"))
        .and_then(|te| te.as_array());
    let Some(elements) = text_elements else {
        return String::new();
    };
    let mut result = String::new();
    for te in elements {
        if let Some(content) = te
            .get("textRun")
            .and_then(|tr| tr.get("content"))
            .and_then(|c| c.as_str())
        {
            result.push_str(content);
        }
    }
    result.trim_end_matches('\n').to_string()
}

pub fn extract_notes_text(slide: &Value) -> Option<String> {
    let notes_page = slide
        .get("slideProperties")
        .and_then(|sp| sp.get("notesPage"))?;
    let notes_obj_id = notes_page
        .get("notesProperties")
        .and_then(|np| np.get("speakerNotesObjectId"))
        .and_then(|id| id.as_str())?;
    let elements = notes_page
        .get("pageElements")
        .and_then(|pe| pe.as_array())?;
    for elem in elements {
        let obj_id = elem
            .get("objectId")
            .and_then(|id| id.as_str())
            .unwrap_or("");
        if obj_id == notes_obj_id {
            let text = extract_text_from_shape(elem);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

pub fn resolve_layout_name(presentation: &Value, layout_id: &str) -> Option<String> {
    let layouts = presentation.get("layouts").and_then(|l| l.as_array())?;
    for layout in layouts {
        let obj_id = layout
            .get("objectId")
            .and_then(|id| id.as_str())
            .unwrap_or("");
        if obj_id == layout_id {
            return layout
                .get("layoutProperties")
                .and_then(|lp| lp.get("displayName"))
                .and_then(|dn| dn.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

pub fn presentation_to_structured(
    presentation: &Value,
    slide_number: Option<usize>,
) -> Result<Value, String> {
    let pres_id = presentation
        .get("presentationId")
        .and_then(|id| id.as_str())
        .unwrap_or("");
    let title = presentation
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let slides = presentation
        .get("slides")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let slide_count = slides.len();

    if let Some(num) = slide_number
        && (num < 1 || num > slide_count)
    {
        return Err(format!(
            "slide_number {num} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
        ));
    }

    let mut slide_data = Vec::new();
    for (idx, slide) in slides.iter().enumerate() {
        let num = idx + 1;
        if let Some(target) = slide_number
            && num != target
        {
            continue;
        }

        let object_id = slide
            .get("objectId")
            .and_then(|id| id.as_str())
            .unwrap_or("");
        let layout_id = slide
            .get("slideProperties")
            .and_then(|sp| sp.get("layoutObjectId"))
            .and_then(|id| id.as_str())
            .unwrap_or("");
        let layout_name = resolve_layout_name(presentation, layout_id).unwrap_or_default();

        let page_elements = slide
            .get("pageElements")
            .and_then(|pe| pe.as_array())
            .cloned()
            .unwrap_or_default();

        let title_text = {
            let t = extract_slide_text(&page_elements, "TITLE");
            if t.is_empty() {
                let t2 = extract_slide_text(&page_elements, "CENTERED_TITLE");
                if t2.is_empty() {
                    // Marp import creates plain text boxes — title is in the first one
                    if let Some(title_id) = find_title_object_id(&page_elements) {
                        page_elements
                            .iter()
                            .find(|e| {
                                e.get("objectId").and_then(|id| id.as_str()) == Some(&title_id)
                            })
                            .map(extract_text_from_shape)
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    t2
                }
            } else {
                t
            }
        };
        let body_text = {
            // For structured output, use the body after excluding the title text box
            let title_obj_id = find_title_object_id(&page_elements);
            let mut parts = Vec::new();
            for elem in &page_elements {
                let ph_type = elem
                    .get("shape")
                    .and_then(|s| s.get("placeholder"))
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match ph_type {
                    "TITLE" | "CENTERED_TITLE" => continue,
                    "BODY" | "SUBTITLE" => {
                        let text = extract_text_from_shape(elem);
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                    _ => {
                        if elem.get("shape").and_then(|s| s.get("text")).is_some() {
                            let has_ph = elem
                                .get("shape")
                                .and_then(|s| s.get("placeholder"))
                                .is_some();
                            let obj_id = elem.get("objectId").and_then(|id| id.as_str());
                            if !has_ph && obj_id != title_obj_id.as_deref() {
                                let text = extract_text_from_shape(elem);
                                if !text.is_empty() {
                                    parts.push(text);
                                }
                            }
                        }
                    }
                }
            }
            parts.join("\n")
        };
        let notes = extract_notes_text(slide);
        let element_count = page_elements.len();

        let mut entry = json!({
            "slide_number": num,
            "object_id": object_id,
            "layout_name": layout_name,
            "title": title_text,
            "body": body_text,
            "element_count": element_count,
            "has_bullets": slide_has_bullets(&page_elements),
            "has_table": slide_has_table(&page_elements),
            "has_image": slide_has_image(&page_elements),
            "has_code": slide_has_code(&page_elements)
        });
        if let Some(ref notes_text) = notes {
            entry["speaker_notes"] = json!(notes_text);
        }
        slide_data.push(entry);
    }

    Ok(json!({
        "presentation_id": pres_id,
        "title": title,
        "slide_count": slide_count,
        "slides": slide_data
    }))
}

fn extract_styled_text_from_shape(elem: &Value) -> String {
    let text_elements = elem
        .get("shape")
        .and_then(|s| s.get("text"))
        .and_then(|t| t.get("textElements"))
        .and_then(|te| te.as_array());
    let Some(elements) = text_elements else {
        return String::new();
    };

    let mut result = String::new();
    let mut in_bullet = false;

    for te in elements {
        if let Some(pm) = te.get("paragraphMarker") {
            in_bullet = pm.get("bullet").is_some();
            continue;
        }

        if let Some(tr) = te.get("textRun") {
            let content = tr.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if content == "\n" {
                result.push('\n');
                continue;
            }

            let style = tr.get("style");
            let bold = style
                .and_then(|s| s.get("bold"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let italic = style
                .and_then(|s| s.get("italic"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let link = style
                .and_then(|s| s.get("link"))
                .and_then(|l| l.get("url"))
                .and_then(|u| u.as_str());
            let font = style
                .and_then(|s| s.get("fontFamily"))
                .and_then(|f| f.as_str())
                .unwrap_or("");
            let is_code = font.contains("Courier") || font.contains("mono");

            let trimmed = content.trim_end_matches('\n');
            let has_newline = content.ends_with('\n');

            if in_bullet && !result.ends_with("- ") && (result.is_empty() || result.ends_with('\n'))
            {
                result.push_str("- ");
            }

            let mut text = trimmed.to_string();
            if is_code {
                text = format!("`{text}`");
            } else {
                if bold && italic {
                    text = format!("***{text}***");
                } else if bold {
                    text = format!("**{text}**");
                } else if italic {
                    text = format!("*{text}*");
                }
            }
            if let Some(url) = link {
                text = format!("[{text}]({url})");
            }

            result.push_str(&text);
            if has_newline {
                result.push('\n');
            }
        }
    }

    result.trim_end_matches('\n').to_string()
}

fn extract_body_as_markdown(page_elements: &[Value]) -> String {
    let mut parts = Vec::new();
    for elem in page_elements {
        let ph_type = elem
            .get("shape")
            .and_then(|s| s.get("placeholder"))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        match ph_type {
            "TITLE" | "CENTERED_TITLE" => continue,
            "BODY" | "SUBTITLE" => {
                let text = extract_styled_text_from_shape(elem);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            _ => {
                if elem.get("shape").and_then(|s| s.get("text")).is_some() {
                    let has_placeholder = elem
                        .get("shape")
                        .and_then(|s| s.get("placeholder"))
                        .is_some();
                    if !has_placeholder {
                        let text = extract_styled_text_from_shape(elem);
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                }
                if let Some(table) = elem.get("table") {
                    let md = extract_table_as_markdown(table);
                    if !md.is_empty() {
                        parts.push(md);
                    }
                }
            }
        }
    }
    parts.join("\n\n")
}

fn extract_table_as_markdown(table: &Value) -> String {
    let rows = table.get("tableRows").and_then(|r| r.as_array());
    let Some(rows) = rows else {
        return String::new();
    };
    if rows.is_empty() {
        return String::new();
    }

    let mut md_rows: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let cells = row.get("tableCells").and_then(|c| c.as_array());
        let Some(cells) = cells else { continue };
        let mut md_cells = Vec::new();
        for cell in cells {
            let mut text = String::new();
            if let Some(content) = cell
                .get("text")
                .and_then(|t| t.get("textElements"))
                .and_then(|te| te.as_array())
            {
                for te in content {
                    if let Some(c) = te
                        .get("textRun")
                        .and_then(|tr| tr.get("content"))
                        .and_then(|c| c.as_str())
                    {
                        text.push_str(c.trim_end_matches('\n'));
                    }
                }
            }
            md_cells.push(text);
        }
        md_rows.push(md_cells);
    }

    if md_rows.is_empty() {
        return String::new();
    }

    let col_count = md_rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut result = String::new();
    for (i, row) in md_rows.iter().enumerate() {
        let cells: Vec<String> = (0..col_count)
            .map(|j| row.get(j).cloned().unwrap_or_default())
            .collect();
        result.push_str(&format!("| {} |", cells.join(" | ")));
        result.push('\n');
        if i == 0 {
            let sep: Vec<&str> = (0..col_count).map(|_| "---").collect();
            result.push_str(&format!("| {} |", sep.join(" | ")));
            result.push('\n');
        }
    }
    result.trim_end().to_string()
}

pub fn slide_has_bullets(page_elements: &[Value]) -> bool {
    for elem in page_elements {
        if let Some(text_elements) = elem
            .get("shape")
            .and_then(|s| s.get("text"))
            .and_then(|t| t.get("textElements"))
            .and_then(|te| te.as_array())
        {
            for te in text_elements {
                if te
                    .get("paragraphMarker")
                    .and_then(|pm| pm.get("bullet"))
                    .is_some()
                {
                    return true;
                }
            }
        }
    }
    false
}

pub fn slide_has_table(page_elements: &[Value]) -> bool {
    page_elements.iter().any(|el| el.get("table").is_some())
}

pub fn slide_has_image(page_elements: &[Value]) -> bool {
    page_elements.iter().any(|el| el.get("image").is_some())
}

pub fn slide_has_code(page_elements: &[Value]) -> bool {
    for elem in page_elements {
        if let Some(text_elements) = elem
            .get("shape")
            .and_then(|s| s.get("text"))
            .and_then(|t| t.get("textElements"))
            .and_then(|te| te.as_array())
        {
            for te in text_elements {
                let font = te
                    .get("textRun")
                    .and_then(|tr| tr.get("style"))
                    .and_then(|s| s.get("fontFamily"))
                    .and_then(|f| f.as_str())
                    .unwrap_or("");
                if font.contains("Courier") || font.contains("mono") {
                    return true;
                }
            }
        }
    }
    false
}

pub fn presentation_to_markdown(presentation: &Value) -> String {
    let slides = presentation
        .get("slides")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    let mut parts = Vec::new();
    parts.push("---\nmarp: true\n---".to_string());

    for slide in &slides {
        let page_elements = slide
            .get("pageElements")
            .and_then(|pe| pe.as_array())
            .cloned()
            .unwrap_or_default();

        let title_text = {
            let t = extract_slide_text(&page_elements, "TITLE");
            if t.is_empty() {
                extract_slide_text(&page_elements, "CENTERED_TITLE")
            } else {
                t
            }
        };
        let body_text = extract_body_as_markdown(&page_elements);
        let notes = extract_notes_text(slide);

        let mut slide_parts = Vec::new();
        if !title_text.is_empty() {
            slide_parts.push(format!("# {title_text}"));
        }
        if !body_text.is_empty() {
            slide_parts.push(body_text);
        }
        if let Some(ref notes_text) = notes {
            slide_parts.push(format!("<!-- notes -->\n{notes_text}"));
        }

        parts.push(slide_parts.join("\n\n"));
    }

    parts.join("\n\n---\n\n")
}

pub fn slides_read_tool_schema() -> Value {
    json!({
        "name": "gws_slides_read",
        "title": "Read presentation content",
        "description": "Read slide titles, body text, speaker notes, and layout info from a presentation.",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "presentation_id": {
                    "type": "string",
                    "description": "Google Slides presentation ID"
                },
                "slide_number": {
                    "type": "integer",
                    "description": "1-based slide number to read a single slide"
                },
                "format": {
                    "type": "string",
                    "enum": ["json", "markdown"],
                    "description": "Output format (default: json). Markdown emits Marp-like output."
                }
            },
            "required": ["presentation_id"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "presentation_id": { "type": "string" },
                "title": { "type": "string" },
                "slide_count": { "type": "integer" },
                "slides": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "slide_number": { "type": "integer" },
                            "object_id": { "type": "string" },
                            "layout_name": { "type": "string" },
                            "title": { "type": "string" },
                            "body": { "type": "string" },
                            "speaker_notes": { "type": "string" },
                            "element_count": { "type": "integer" }
                        },
                        "required": ["slide_number", "object_id", "title", "body", "element_count"]
                    }
                }
            },
            "required": ["presentation_id", "title", "slide_count", "slides"]
        }
    })
}

pub fn slides_add_tool_schema() -> Value {
    json!({
        "name": "gws_slides_add",
        "title": "Add a slide",
        "description": "Add a single slide from Marp Markdown at a given position.",
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
                    "description": "Google Slides presentation ID"
                },
                "marp": {
                    "type": "string",
                    "description": "Marp Markdown for a single slide. Use # for title, body text, <!-- notes --> for speaker notes."
                },
                "position": {
                    "type": "integer",
                    "description": "1-based insert position (default: end)"
                },
                "template": {
                    "type": "string",
                    "description": "Template name or presentation ID for layout selection"
                },
                "background_image": {
                    "type": "string",
                    "description": "Full-slide background image. Accepts a public URL or a Google Drive file ID."
                }
            },
            "required": ["presentation_id", "marp"]
        }
    })
}

pub fn slides_delete_tool_schema() -> Value {
    json!({
        "name": "gws_slides_delete",
        "title": "Delete slides",
        "description": "Delete one or more slides by slide number.",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": true,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "presentation_id": {
                    "type": "string",
                    "description": "Google Slides presentation ID"
                },
                "slide_numbers": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "1-based slide numbers to delete (e.g. [2, 4])"
                }
            },
            "required": ["presentation_id", "slide_numbers"]
        }
    })
}

pub fn slides_reorder_tool_schema() -> Value {
    json!({
        "name": "gws_slides_reorder",
        "title": "Reorder slides",
        "description": "Move slides to a new position in the presentation.",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "presentation_id": {
                    "type": "string",
                    "description": "Google Slides presentation ID"
                },
                "slide_numbers": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "1-based slide numbers to move (e.g. [3, 5])"
                },
                "position": {
                    "type": "integer",
                    "description": "1-based target position for the moved slides"
                }
            },
            "required": ["presentation_id", "slide_numbers", "position"]
        }
    })
}

pub fn find_placeholder_object_id(
    page_elements: &[Value],
    placeholder_type: &str,
) -> Option<String> {
    for elem in page_elements {
        let ph = elem
            .get("shape")
            .and_then(|s| s.get("placeholder"))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if ph == placeholder_type {
            return elem
                .get("objectId")
                .and_then(|id| id.as_str())
                .map(String::from);
        }
    }
    None
}

pub fn find_title_object_id(page_elements: &[Value]) -> Option<String> {
    if let Some(id) = find_placeholder_object_id(page_elements, "TITLE") {
        return Some(id);
    }
    if let Some(id) = find_placeholder_object_id(page_elements, "CENTERED_TITLE") {
        return Some(id);
    }
    // Marp import creates plain text boxes — pick the first one (title is created first)
    for elem in page_elements {
        if elem.get("shape").and_then(|s| s.get("text")).is_some() {
            let has_placeholder = elem
                .get("shape")
                .and_then(|s| s.get("placeholder"))
                .is_some();
            if !has_placeholder {
                return elem
                    .get("objectId")
                    .and_then(|id| id.as_str())
                    .map(String::from);
            }
        }
    }
    None
}

pub fn find_body_object_id(page_elements: &[Value]) -> Option<String> {
    if let Some(id) = find_placeholder_object_id(page_elements, "BODY") {
        return Some(id);
    }
    if let Some(id) = find_placeholder_object_id(page_elements, "SUBTITLE") {
        return Some(id);
    }
    // Marp import creates title as first text box, body as second — skip the first
    let title_obj_id = find_title_object_id(page_elements);
    for elem in page_elements {
        if elem.get("shape").and_then(|s| s.get("text")).is_some() {
            let has_placeholder = elem
                .get("shape")
                .and_then(|s| s.get("placeholder"))
                .is_some();
            let obj_id = elem.get("objectId").and_then(|id| id.as_str());
            if !has_placeholder && obj_id != title_obj_id.as_deref() {
                return obj_id.map(String::from);
            }
        }
    }
    None
}

pub fn placeholder_label(ph: &PlaceholderInfo) -> String {
    match ph.index {
        Some(idx) => format!("{}[{}]", ph.ph_type, idx),
        None => ph.ph_type.clone(),
    }
}

pub fn find_placeholder_by_label(page_elements: &[Value], label: &str) -> Option<String> {
    let (ph_type, ph_index) = if let Some(bracket) = label.find('[') {
        let t = &label[..bracket];
        let idx_str = &label[bracket + 1..label.len().saturating_sub(1)];
        let idx = idx_str.parse::<i64>().ok();
        (t, idx)
    } else {
        (label, None)
    };

    for elem in page_elements {
        let ph = elem.get("shape").and_then(|s| s.get("placeholder"));
        let Some(ph) = ph else { continue };
        let elem_type = ph.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let elem_index = ph.get("index").and_then(|i| i.as_i64());

        if elem_type == ph_type && elem_index == ph_index {
            return elem
                .get("objectId")
                .and_then(|id| id.as_str())
                .map(String::from);
        }
    }
    None
}

const SLIDE_WIDTH_PT: f64 = 720.0;
const SLIDE_HEIGHT_PT: f64 = 540.0;
const EMU_PER_PT: f64 = 12700.0;

pub fn extract_layout_details(layout: &Value) -> Value {
    let display_name = layout
        .get("layoutProperties")
        .and_then(|lp| lp.get("displayName"))
        .and_then(|dn| dn.as_str())
        .unwrap_or("");
    let layout_id = layout
        .get("objectId")
        .and_then(|id| id.as_str())
        .unwrap_or("");

    let elements = layout
        .get("pageElements")
        .and_then(|pe| pe.as_array())
        .cloned()
        .unwrap_or_default();

    let mut placeholders = Vec::new();
    for elem in &elements {
        let ph = elem.get("shape").and_then(|s| s.get("placeholder"));
        let Some(ph) = ph else { continue };
        let ph_type = ph.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ph_type.is_empty() {
            continue;
        }
        let ph_index = ph.get("index").and_then(|i| i.as_i64());

        let label = match ph_index {
            Some(idx) => format!("{ph_type}[{idx}]"),
            None => ph_type.to_string(),
        };

        let size = elem.get("size").unwrap_or(&Value::Null);
        let transform = elem.get("transform").unwrap_or(&Value::Null);

        let base_w = size
            .get("width")
            .and_then(|w| w.get("magnitude"))
            .and_then(|m| m.as_f64())
            .unwrap_or(0.0);
        let base_h = size
            .get("height")
            .and_then(|h| h.get("magnitude"))
            .and_then(|m| m.as_f64())
            .unwrap_or(0.0);
        let scale_x = transform
            .get("scaleX")
            .and_then(|s| s.as_f64())
            .unwrap_or(1.0);
        let scale_y = transform
            .get("scaleY")
            .and_then(|s| s.as_f64())
            .unwrap_or(1.0);
        let translate_x = transform
            .get("translateX")
            .and_then(|t| t.as_f64())
            .unwrap_or(0.0);
        let translate_y = transform
            .get("translateY")
            .and_then(|t| t.as_f64())
            .unwrap_or(0.0);

        let w_pt = (base_w * scale_x.abs()) / EMU_PER_PT;
        let h_pt = (base_h * scale_y.abs()) / EMU_PER_PT;
        let x_pt = translate_x / EMU_PER_PT;
        let y_pt = translate_y / EMU_PER_PT;

        let cx = x_pt + w_pt / 2.0;
        let cy = y_pt + h_pt / 2.0;

        let position = if cx < SLIDE_WIDTH_PT * 0.33 {
            "left"
        } else if cx < SLIDE_WIDTH_PT * 0.66 {
            "center"
        } else {
            "right"
        };
        let vertical = if cy < SLIDE_HEIGHT_PT * 0.33 {
            "top"
        } else if cy < SLIDE_HEIGHT_PT * 0.66 {
            "middle"
        } else {
            "bottom"
        };
        let size_hint = if w_pt > 400.0 {
            "large"
        } else if w_pt > 200.0 {
            "medium"
        } else {
            "small"
        };

        let mut entry = json!({
            "label": label,
            "type": ph_type,
            "position": position,
            "vertical": vertical,
            "size": size_hint,
            "width_pt": w_pt.round() as i64,
            "height_pt": h_pt.round() as i64
        });
        if let Some(idx) = ph_index {
            entry["index"] = json!(idx);
        }
        placeholders.push(entry);
    }

    json!({
        "name": display_name,
        "id": layout_id,
        "placeholders": placeholders
    })
}

pub fn slides_duplicate_tool_schema() -> Value {
    json!({
        "name": "gws_slides_duplicate",
        "title": "Duplicate a slide",
        "description": "Duplicate a slide and optionally place the copy at a specific position.",
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
                    "description": "Google Slides presentation ID"
                },
                "slide_number": {
                    "type": "integer",
                    "description": "1-based slide number to duplicate"
                },
                "position": {
                    "type": "integer",
                    "description": "1-based position for the copy (default: right after the original)"
                }
            },
            "required": ["presentation_id", "slide_number"]
        }
    })
}

pub fn slides_update_tool_schema() -> Value {
    json!({
        "name": "gws_slides_update",
        "title": "Update slide content",
        "description": "Update slide content by field (title/body/notes) or by placeholder label. Use gws_templates to see available placeholders.",
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
                    "description": "Google Slides presentation ID"
                },
                "slide_number": {
                    "type": "integer",
                    "description": "1-based slide number to update"
                },
                "title": {
                    "type": "string",
                    "description": "New title text (replaces existing)"
                },
                "body": {
                    "type": "string",
                    "description": "New body as Marp Markdown (bullets, bold, code blocks). Do not include # heading."
                },
                "notes": {
                    "type": "string",
                    "description": "New speaker notes text (replaces existing)"
                },
                "placeholders": {
                    "type": "object",
                    "description": "Map of placeholder labels to text values (e.g. {\"SUBTITLE[3]\": \"Quote text\"}). Use gws_templates to see available labels.",
                    "additionalProperties": { "type": "string" }
                }
            },
            "required": ["presentation_id", "slide_number"]
        }
    })
}

pub(crate) async fn execute_slides_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    if tool_name == "gws_slides_import_marp" {
        return execute_slides_import_marp(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_read" {
        return execute_slides_read(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_add" {
        return execute_slides_add(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_delete" {
        return execute_slides_delete(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_reorder" {
        return execute_slides_reorder(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_duplicate" {
        return execute_slides_duplicate(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_update" {
        return execute_slides_update(arguments, policy, meta, state, dry_run).await;
    }
    Err(GwsError::Validation(format!(
        "Unknown slides helper: {tool_name}"
    )))
}

async fn execute_slides_import_marp(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let marp_source = arguments
        .get("marp")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'marp' argument".into()))?;

    let presentation_id_arg = arguments.get("presentation_id").and_then(|v| v.as_str());
    let title = arguments.get("title").and_then(|v| v.as_str());
    let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());
    let template_arg = arguments
        .get("template")
        .or_else(|| arguments.get("template_id"))
        .and_then(|v| v.as_str());
    let template_id =
        template_arg.and_then(|t| policy.find_template(t).map(|e| e.id.as_str()).or(Some(t)));

    let pres = crate::marp::parse_marp(marp_source)
        .map_err(|e| GwsError::Validation(format!("Marp parse error: {e}")))?;

    // Step A: Resolve or create presentation
    let slides_doc = state.get_doc("slides").await?;
    let drive_doc = state.get_doc("drive").await?;

    let presentation_id: String;
    let mut created_new = false;

    if let Some(pid) = presentation_id_arg {
        presentation_id = pid.to_string();
    } else if let Some(tmpl_id) = template_id {
        // Copy template presentation via Drive
        let files_resource = tools::find_resource(&drive_doc.resources, "files")
            .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;
        let copy_method = files_resource
            .methods
            .get("copy")
            .ok_or_else(|| GwsError::Validation("Drive files.copy method not found".into()))?;

        let mut copy_body = json!({});
        if let Some(t) = title {
            copy_body["name"] = json!(t);
        }
        if let Some(fid) = folder_id {
            copy_body["parents"] = json!([fid]);
        }

        let copy_args = json!({
            "params": { "fileId": tmpl_id },
            "body": copy_body
        });

        let mut tc = state.token_cache.take();
        let copy_result = crate::execute::execute_tool(
            &drive_doc,
            copy_method,
            "files",
            "copy",
            &copy_args,
            "drive",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        presentation_id = copy_result
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GwsError::Validation("Drive copy did not return an ID".into()))?
            .to_string();
        created_new = true;
    } else if let Some(t) = title {
        // Search for existing presentation or create new one
        let files_resource = tools::find_resource(&drive_doc.resources, "files")
            .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;

        let mut query = format!(
            "name = '{}' and mimeType = 'application/vnd.google-apps.presentation' and trashed = false",
            t.replace('\'', "\\'")
        );
        if let Some(fid) = folder_id {
            query.push_str(&format!(" and '{}' in parents", fid.replace('\'', "\\'")));
        }

        let list_method = files_resource
            .methods
            .get("list")
            .ok_or_else(|| GwsError::Validation("Drive files.list method not found".into()))?;

        let list_args = json!({ "params": { "q": query } });
        let mut tc = state.token_cache.take();
        let list_result = crate::execute::execute_tool(
            &drive_doc,
            list_method,
            "files",
            "list",
            &list_args,
            "drive",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        let files = list_result
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if let Some(existing) = files.first() {
            presentation_id = existing
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Existing file has no ID".into()))?
                .to_string();
        } else {
            // Create new presentation
            let presentations_resource =
                tools::find_resource(&slides_doc.resources, "presentations").ok_or_else(|| {
                    GwsError::Validation("Slides presentations resource not found".into())
                })?;
            let create_method = presentations_resource
                .methods
                .get("create")
                .ok_or_else(|| {
                    GwsError::Validation("Slides presentations.create not found".into())
                })?;

            let create_args = json!({
                "body": { "title": t }
            });

            let mut tc = state.token_cache.take();
            let create_result = crate::execute::execute_tool(
                &slides_doc,
                create_method,
                "presentations",
                "create",
                &create_args,
                "slides",
                policy,
                meta,
                None,
                None,
                dry_run,
                &mut tc,
            )
            .await?;
            state.token_cache = tc;

            presentation_id = create_result
                .get("presentationId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    let keys: Vec<_> = create_result
                        .as_object()
                        .map(|o| o.keys().collect())
                        .unwrap_or_default();
                    GwsError::Validation(format!(
                        "Create did not return presentationId. Response keys: {:?}",
                        keys
                    ))
                })?
                .to_string();
            created_new = true;

            if let Some(fid) = folder_id {
                let drive_doc = state.get_doc("drive").await?;
                let files_resource = tools::find_resource(&drive_doc.resources, "files")
                    .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;
                let update_method = files_resource
                    .methods
                    .get("update")
                    .ok_or_else(|| GwsError::Validation("Drive files.update not found".into()))?;
                let move_args = json!({
                    "params": {
                        "fileId": &presentation_id,
                        "addParents": fid
                    }
                });
                let mut tc = state.token_cache.take();
                let _move_result = crate::execute::execute_tool(
                    &drive_doc,
                    update_method,
                    "files",
                    "update",
                    &move_args,
                    "drive",
                    policy,
                    meta,
                    None,
                    None,
                    dry_run,
                    &mut tc,
                )
                .await?;
                state.token_cache = tc;
            }
        }
    } else {
        return Err(GwsError::Validation(
            "One of 'presentation_id', 'title', or 'template_id' is required".into(),
        ));
    }

    if dry_run {
        return Ok(json!({
            "dry_run": true,
            "presentation_id": presentation_id,
            "slide_count": pres.slides.len()
        }));
    }

    // Step B: Fetch presentation, extract layouts, collect existing slide IDs
    let (template_layouts, existing_slide_ids) = {
        let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
            .ok_or_else(|| {
            GwsError::Validation("Slides presentations resource not found".into())
        })?;
        let get_method = presentations_resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;

        let get_args = json!({ "params": { "presentationId": &presentation_id } });
        let mut tc = state.token_cache.take();
        let get_result = crate::execute::execute_tool(
            &slides_doc,
            get_method,
            "presentations",
            "get",
            &get_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;
        crate::server::check_api_result(&get_result)?;

        let layouts = if template_id.is_some() {
            extract_layouts(&get_result)
        } else {
            Vec::new()
        };

        let existing_slide_ids: Vec<String> = get_result
            .get("slides")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        s.get("objectId")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default();

        (layouts, existing_slide_ids)
    };

    // Step C: Generate slide create requests, then batch cleanup + creation
    let layouts_ref = if template_layouts.is_empty() {
        None
    } else {
        Some(template_layouts.as_slice())
    };
    let (create_reqs, mut content_reqs) = marp_to_slide_requests(&pres, None, layouts_ref);

    let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let batch_method = presentations_resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("Slides batchUpdate not found".into()))?;

    // Step C1: Delete existing slides (create temp slide first, then delete all old ones)
    if !existing_slide_ids.is_empty() {
        let mut cleanup_reqs = vec![json!({
            "createSlide": { "objectId": "temp_cleanup_slide" }
        })];
        for old_id in &existing_slide_ids {
            cleanup_reqs.push(json!({ "deleteObject": { "objectId": old_id } }));
        }
        let batch_args = json!({
            "params": { "presentationId": &presentation_id },
            "body": { "requests": cleanup_reqs }
        });
        let mut tc = state.token_cache.take();
        let cleanup_result = crate::execute::execute_tool(
            &slides_doc,
            batch_method,
            "presentations",
            "batchUpdate",
            &batch_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;
        crate::server::check_api_result(&cleanup_result)?;
    }

    // Step C2: Create new slides and delete the temp cleanup slide
    if !create_reqs.is_empty() {
        let mut pass1_reqs = create_reqs;
        if !existing_slide_ids.is_empty() {
            pass1_reqs.push(json!({
                "deleteObject": { "objectId": "temp_cleanup_slide" }
            }));
        }

        let batch_args = json!({
            "params": { "presentationId": &presentation_id },
            "body": { "requests": pass1_reqs }
        });
        let mut tc = state.token_cache.take();
        let create_result = crate::execute::execute_tool(
            &slides_doc,
            batch_method,
            "presentations",
            "batchUpdate",
            &batch_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;
        crate::server::check_api_result(&create_result)?;
    }

    // Step D: Fetch presentation to get speaker notes object IDs
    let has_notes = pres.slides.iter().any(|s| s.speaker_notes.is_some());
    if has_notes {
        let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
            .ok_or_else(|| {
            GwsError::Validation("Slides presentations resource not found".into())
        })?;
        let get_method = presentations_resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;

        let get_args = json!({ "params": { "presentationId": &presentation_id } });
        let mut tc = state.token_cache.take();
        let get_result = crate::execute::execute_tool(
            &slides_doc,
            get_method,
            "presentations",
            "get",
            &get_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        let slides_arr = get_result
            .get("slides")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let notes_ids: Vec<String> = slides_arr
            .iter()
            .filter_map(|s| {
                s.get("slideProperties")
                    .and_then(|sp| sp.get("notesPage"))
                    .and_then(|np| np.get("notesProperties"))
                    .and_then(|npp| npp.get("speakerNotesObjectId"))
                    .and_then(|id| id.as_str())
                    .map(String::from)
            })
            .collect();

        let (_, notes_content_reqs) = marp_to_slide_requests(&pres, Some(&notes_ids), None);
        // Only add notes-related requests (insertText targeting notes IDs)
        for req in notes_content_reqs {
            if let Some(insert) = req.get("insertText")
                && let Some(obj_id) = insert.get("objectId").and_then(|v| v.as_str())
                && notes_ids.contains(&obj_id.to_string())
            {
                content_reqs.push(req);
            }
        }
    }

    // Step E: Remap placeholder IDs to actual server-assigned IDs
    // When createSlide uses placeholderIdMappings, the API may assign different IDs
    // if the layout's placeholder type/index doesn't match exactly
    if layouts_ref.is_some() && !content_reqs.is_empty() {
        let refreshed = fetch_presentation(&presentation_id, state, policy, meta).await?;
        let refreshed_slides = refreshed
            .get("slides")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        let mut content_json = serde_json::to_string(&content_reqs).unwrap_or_default();
        for (idx, slide) in refreshed_slides.iter().enumerate() {
            let elements = slide
                .get("pageElements")
                .and_then(|pe| pe.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(real_title) = find_title_object_id(&elements) {
                content_json = content_json
                    .replace(&format!("\"title_{idx}\""), &format!("\"{}\"", real_title));
            }
            if let Some(real_body) = find_body_object_id(&elements) {
                content_json =
                    content_json.replace(&format!("\"body_{idx}\""), &format!("\"{}\"", real_body));
            }
        }
        if let Ok(fixed) = serde_json::from_str::<Vec<Value>>(&content_json) {
            content_reqs = fixed;
        }
    }

    // Step F: Execute pass 2 — content, styling, backgrounds, notes
    if !content_reqs.is_empty() {
        let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
            .ok_or_else(|| {
            GwsError::Validation("Slides presentations resource not found".into())
        })?;
        let batch_method = presentations_resource
            .methods
            .get("batchUpdate")
            .ok_or_else(|| GwsError::Validation("Slides batchUpdate not found".into()))?;

        let batch_args = json!({
            "params": { "presentationId": &presentation_id },
            "body": { "requests": content_reqs }
        });
        let mut tc = state.token_cache.take();
        let result = crate::execute::execute_tool(
            &slides_doc,
            batch_method,
            "presentations",
            "batchUpdate",
            &batch_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        let mut final_result = result;
        final_result["presentation_id"] = json!(presentation_id);
        final_result["slide_count"] = json!(pres.slides.len());
        if created_new {
            final_result["created_presentation_id"] = json!(&presentation_id);
        }
        final_result["url"] = json!(format!(
            "https://docs.google.com/presentation/d/{}/edit",
            presentation_id
        ));
        return Ok(final_result);
    }

    Ok(json!({
        "presentation_id": presentation_id,
        "slide_count": pres.slides.len(),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    }))
}

async fn fetch_presentation(
    presentation_id: &str,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<Value, GwsError> {
    let slides_doc = state.get_doc("slides").await?;
    let resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;
    let args = json!({ "params": { "presentationId": presentation_id } });
    let mut tc = state.token_cache.take();
    let result = crate::execute::execute_tool(
        &slides_doc,
        get_method,
        "presentations",
        "get",
        &args,
        "slides",
        policy,
        meta,
        None,
        None,
        false,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;
    crate::server::check_api_result(&result)?;
    Ok(result)
}

fn extract_slide_object_ids(presentation: &Value) -> Vec<String> {
    presentation
        .get("slides")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    s.get("objectId")
                        .and_then(|id| id.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn slides_batch_update(
    presentation_id: &str,
    requests: Vec<Value>,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let slides_doc = state.get_doc("slides").await?;
    let resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let batch_method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("Slides presentations.batchUpdate not found".into()))?;
    let batch_args = json!({
        "params": { "presentationId": presentation_id },
        "body": { "requests": requests }
    });
    let mut tc = state.token_cache.take();
    let result = crate::execute::execute_tool(
        &slides_doc,
        batch_method,
        "presentations",
        "batchUpdate",
        &batch_args,
        "slides",
        policy,
        meta,
        None,
        None,
        dry_run,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;
    crate::server::check_api_result(&result)?;
    Ok(result)
}

async fn fetch_slide_summary(
    presentation_id: &str,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<Value, GwsError> {
    let slides_doc = state.get_doc("slides").await?;
    let resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;
    let args = json!({
        "params": {
            "presentationId": presentation_id,
            "fields": "slides(objectId,pageElements(shape(placeholder,text)))"
        }
    });
    let mut tc = state.token_cache.take();
    let result = crate::execute::execute_tool(
        &slides_doc,
        get_method,
        "presentations",
        "get",
        &args,
        "slides",
        policy,
        meta,
        None,
        None,
        false,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;
    crate::server::check_api_result(&result)?;

    let slides = result
        .get("slides")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let mut summary = Vec::new();
    for (i, slide) in slides.iter().enumerate() {
        let elements = slide
            .get("pageElements")
            .and_then(|pe| pe.as_array())
            .cloned()
            .unwrap_or_default();
        let title = {
            let t = extract_slide_text(&elements, "TITLE");
            if t.is_empty() {
                let t2 = extract_slide_text(&elements, "CENTERED_TITLE");
                if t2.is_empty() {
                    if let Some(title_id) = find_title_object_id(&elements) {
                        elements
                            .iter()
                            .find(|e| {
                                e.get("objectId").and_then(|id| id.as_str()) == Some(&title_id)
                            })
                            .map(extract_text_from_shape)
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    t2
                }
            } else {
                t
            }
        };
        summary.push(json!({
            "slide_number": i + 1,
            "object_id": slide.get("objectId").and_then(|v| v.as_str()).unwrap_or(""),
            "title": title
        }));
    }
    Ok(json!({ "slide_count": slides.len(), "slides": summary }))
}

async fn execute_slides_read(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    _dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_number = arguments
        .get("slide_number")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let format = arguments
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;

    if format == "markdown" {
        let md = presentation_to_markdown(&pres_data);
        return Ok(json!({
            "content": [{ "type": "text", "text": md }],
            "isError": false
        }));
    }

    let structured =
        presentation_to_structured(&pres_data, slide_number).map_err(GwsError::Validation)?;

    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&structured).unwrap_or_default() }],
        "structuredContent": structured,
        "isError": false
    }))
}

async fn execute_slides_delete(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_numbers: Vec<usize> = arguments
        .get("slide_numbers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_numbers' argument".into()))?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();

    if slide_numbers.is_empty() {
        return Err(GwsError::Validation(
            "slide_numbers must contain at least one slide number".into(),
        ));
    }

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    for &num in &slide_numbers {
        if num < 1 || num > slide_count {
            return Err(GwsError::Validation(format!(
                "Slide number {num} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
            )));
        }
    }

    let mut unique_numbers: Vec<usize> = slide_numbers.clone();
    unique_numbers.sort();
    unique_numbers.dedup();

    if unique_numbers.len() >= slide_count {
        return Err(GwsError::Validation(
            "Cannot delete all slides. A presentation must have at least one slide.".into(),
        ));
    }

    let mut delete_reqs = Vec::new();
    let mut deleted_ids = Vec::new();
    for &num in &unique_numbers {
        let obj_id = &slide_ids[num - 1];
        delete_reqs.push(json!({ "deleteObject": { "objectId": obj_id } }));
        deleted_ids.push(obj_id.clone());
    }

    slides_batch_update(presentation_id, delete_reqs, state, policy, meta, dry_run).await?;

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "deleted": deleted_ids,
        "remaining_slides": summary.get("slide_count"),
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_reorder(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_numbers: Vec<usize> = arguments
        .get("slide_numbers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_numbers' argument".into()))?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();
    let position = arguments
        .get("position")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| GwsError::Validation("Missing 'position' argument".into()))?
        as usize;

    if slide_numbers.is_empty() {
        return Err(GwsError::Validation(
            "slide_numbers must contain at least one slide number".into(),
        ));
    }

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    for &num in &slide_numbers {
        if num < 1 || num > slide_count {
            return Err(GwsError::Validation(format!(
                "Slide number {num} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
            )));
        }
    }
    if position < 1 || position > slide_count {
        return Err(GwsError::Validation(format!(
            "Position {position} is out of range. Must be 1-{slide_count}."
        )));
    }

    let move_ids: Vec<String> = slide_numbers
        .iter()
        .map(|&n| slide_ids[n - 1].clone())
        .collect();
    let reqs = vec![json!({
        "updateSlidesPosition": {
            "slideObjectIds": move_ids,
            "insertionIndex": position - 1
        }
    })];

    slides_batch_update(presentation_id, reqs, state, policy, meta, dry_run).await?;

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "moved": move_ids,
        "position": position,
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_add(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let marp_source = arguments
        .get("marp")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'marp' argument".into()))?;
    let position = arguments
        .get("position")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    if marp_source.contains("\n---\n") || marp_source.contains("\r\n---\r\n") {
        return Err(GwsError::Validation(
            "Only single-slide Marp is supported. Remove '---' separators or use gws_slides_import_marp for multi-slide content.".into(),
        ));
    }

    let is_blank = marp_source.trim().is_empty()
        || marp_source.trim() == "---\nmarp: true\n---"
        || marp_source.trim().chars().all(|c| c.is_whitespace());

    let pres = crate::marp::parse_marp(marp_source)
        .map_err(|e| GwsError::Validation(format!("Failed to parse Marp: {e}")))?;

    if pres.slides.len() != 1 {
        return Err(GwsError::Validation(format!(
            "Expected 1 slide, got {}. Use gws_slides_import_marp for multi-slide content.",
            pres.slides.len()
        )));
    }

    let has_content =
        !is_blank && (pres.slides[0].title.is_some() || !pres.slides[0].body_blocks.is_empty());

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    if let Some(pos) = position
        && (pos < 1 || pos > slide_count + 1)
    {
        return Err(GwsError::Validation(format!(
            "Position {pos} is out of range. Must be 1-{}.",
            slide_count + 1
        )));
    }

    let suffix = format!(
        "_{:08x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0)
    );
    let new_slide_id = format!("slide_0{suffix}");

    if has_content {
        let layouts = extract_layouts(&pres_data);
        let layouts_ref = if layouts.is_empty() {
            None
        } else {
            Some(layouts.as_slice())
        };

        let (mut create_reqs, mut content_reqs) = marp_to_slide_requests(&pres, None, layouts_ref);

        let rewrite_ids = |reqs: &mut [Value]| {
            let json_str = serde_json::to_string(&reqs).unwrap_or_default();
            let replaced = json_str
                .replace("\"slide_0\"", &format!("\"slide_0{suffix}\""))
                .replace("\"title_0\"", &format!("\"title_0{suffix}\""))
                .replace("\"body_0\"", &format!("\"body_0{suffix}\""))
                .replace("\"table_0_", &format!("\"table_0{suffix}_"));
            if let Ok(parsed) = serde_json::from_str::<Vec<Value>>(&replaced) {
                for (i, v) in parsed.into_iter().enumerate() {
                    if i < reqs.len() {
                        reqs[i] = v;
                    }
                }
            }
        };
        rewrite_ids(&mut create_reqs);
        rewrite_ids(&mut content_reqs);

        if let Some(pos) = position
            && let Some(req) = create_reqs.get_mut(0)
            && let Some(create_slide) = req.get_mut("createSlide")
        {
            create_slide["insertionIndex"] = json!(pos - 1);
        }

        slides_batch_update(presentation_id, create_reqs, state, policy, meta, dry_run).await?;

        if !content_reqs.is_empty() {
            // Re-fetch the slide to discover actual placeholder IDs (mapped IDs may differ
            // from server-assigned ones when placeholderIdMappings don't match exactly)
            let refreshed = fetch_presentation(presentation_id, state, policy, meta).await?;
            let refreshed_slides = refreshed
                .get("slides")
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default();
            let target_idx = position
                .map(|p| p - 1)
                .unwrap_or(refreshed_slides.len().saturating_sub(1));
            if let Some(new_slide) = refreshed_slides.get(target_idx) {
                let elements = new_slide
                    .get("pageElements")
                    .and_then(|pe| pe.as_array())
                    .cloned()
                    .unwrap_or_default();
                let actual_title = find_title_object_id(&elements);
                let actual_body = find_body_object_id(&elements);

                // Rewrite content requests to use actual IDs instead of generated ones
                let content_json = serde_json::to_string(&content_reqs).unwrap_or_default();
                let mut replaced = content_json;
                if let Some(ref real_title) = actual_title {
                    replaced = replaced.replace(
                        &format!("\"title_0{suffix}\""),
                        &format!("\"{}\"", real_title),
                    );
                }
                if let Some(ref real_body) = actual_body {
                    replaced = replaced.replace(
                        &format!("\"body_0{suffix}\""),
                        &format!("\"{}\"", real_body),
                    );
                }
                if let Ok(fixed) = serde_json::from_str::<Vec<Value>>(&replaced) {
                    content_reqs = fixed;
                }

                // Filter content requests to only those targeting existing elements
                let existing_ids: std::collections::HashSet<String> = elements
                    .iter()
                    .filter_map(|e| {
                        e.get("objectId")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .collect();
                content_reqs.retain(|req| {
                    let obj_id = req
                        .as_object()
                        .and_then(|m| m.values().next())
                        .and_then(|v| v.get("objectId"))
                        .and_then(|id| id.as_str())
                        .unwrap_or("");
                    obj_id.is_empty() || existing_ids.contains(obj_id)
                });
            }

            if !content_reqs.is_empty() {
                slides_batch_update(presentation_id, content_reqs, state, policy, meta, dry_run)
                    .await?;
            }
        }

        if pres.slides[0].speaker_notes.is_some() {
            let updated_pres = fetch_presentation(presentation_id, state, policy, meta).await?;
            let updated_slides = updated_pres
                .get("slides")
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default();

            let target_idx = position
                .map(|p| p - 1)
                .unwrap_or(updated_slides.len().saturating_sub(1));
            if let Some(slide) = updated_slides.get(target_idx) {
                let notes_obj_id = slide
                    .get("slideProperties")
                    .and_then(|sp| sp.get("notesPage"))
                    .and_then(|np| np.get("notesProperties"))
                    .and_then(|np| np.get("speakerNotesObjectId"))
                    .and_then(|id| id.as_str());
                if let (Some(notes_id), Some(notes_text)) =
                    (notes_obj_id, &pres.slides[0].speaker_notes)
                {
                    let notes_reqs = vec![json!({
                        "insertText": {
                            "objectId": notes_id,
                            "text": notes_text
                        }
                    })];
                    slides_batch_update(presentation_id, notes_reqs, state, policy, meta, dry_run)
                        .await?;
                }
            }
        }
    } else {
        // Blank slide — just create it without content
        let mut create_req = json!({ "createSlide": { "objectId": &new_slide_id } });
        if let Some(pos) = position {
            create_req["createSlide"]["insertionIndex"] = json!(pos - 1);
        }
        slides_batch_update(
            presentation_id,
            vec![create_req],
            state,
            policy,
            meta,
            dry_run,
        )
        .await?;
    }

    if let Some(bg) = arguments.get("background_image").and_then(|v| v.as_str()) {
        let bg_url = if bg.starts_with("http") {
            bg.to_string()
        } else {
            format!("https://drive.google.com/uc?export=download&id={bg}")
        };
        let bg_reqs = vec![json!({
            "updatePageProperties": {
                "objectId": &new_slide_id,
                "pageProperties": {
                    "pageBackgroundFill": {
                        "stretchedPictureFill": { "contentUrl": bg_url }
                    }
                },
                "fields": "pageBackgroundFill"
            }
        })];
        slides_batch_update(presentation_id, bg_reqs, state, policy, meta, dry_run).await?;
    }

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "presentation_id": presentation_id,
        "slide_object_id": new_slide_id,
        "position": position.unwrap_or(slide_count + 1),
        "slide_count": summary.get("slide_count"),
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_duplicate(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_number = arguments
        .get("slide_number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_number' argument".into()))?
        as usize;
    let position = arguments
        .get("position")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    if slide_number < 1 || slide_number > slide_count {
        return Err(GwsError::Validation(format!(
            "Slide number {slide_number} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
        )));
    }

    let source_id = &slide_ids[slide_number - 1];
    let dup_reqs = vec![json!({ "duplicateObject": { "objectId": source_id } })];
    let dup_result =
        slides_batch_update(presentation_id, dup_reqs, state, policy, meta, dry_run).await?;

    let new_slide_id = dup_result
        .get("replies")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|r| r.get("duplicateObject"))
        .and_then(|d| d.get("objectId"))
        .and_then(|id| id.as_str())
        .unwrap_or("unknown")
        .to_string();

    if let Some(pos) = position {
        if pos < 1 || pos > slide_count + 1 {
            return Err(GwsError::Validation(format!(
                "Position {pos} is out of range. Must be 1-{}.",
                slide_count + 1
            )));
        }
        let move_reqs = vec![json!({
            "updateSlidesPosition": {
                "slideObjectIds": [&new_slide_id],
                "insertionIndex": pos - 1
            }
        })];
        slides_batch_update(presentation_id, move_reqs, state, policy, meta, dry_run).await?;
    }

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "duplicated_from": source_id,
        "new_slide_id": new_slide_id,
        "slides": summary.get("slides"),
        "slide_count": summary.get("slide_count"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_update(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_number = arguments
        .get("slide_number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_number' argument".into()))?
        as usize;
    let new_title = arguments.get("title").and_then(|v| v.as_str());
    let new_body = arguments.get("body").and_then(|v| v.as_str());
    let new_notes = arguments.get("notes").and_then(|v| v.as_str());
    let placeholders_map = arguments.get("placeholders").and_then(|v| v.as_object());

    if new_title.is_none()
        && new_body.is_none()
        && new_notes.is_none()
        && placeholders_map.is_none()
    {
        return Err(GwsError::Validation(
            "At least one of 'title', 'body', 'notes', or 'placeholders' must be provided".into(),
        ));
    }

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slides = pres_data
        .get("slides")
        .and_then(|s| s.as_array())
        .ok_or_else(|| GwsError::Validation("Presentation has no slides".into()))?;
    let slide_count = slides.len();

    if slide_number < 1 || slide_number > slide_count {
        return Err(GwsError::Validation(format!(
            "Slide number {slide_number} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
        )));
    }

    let slide = &slides[slide_number - 1];
    let page_elements = slide
        .get("pageElements")
        .and_then(|pe| pe.as_array())
        .cloned()
        .unwrap_or_default();

    let mut update_reqs: Vec<Value> = Vec::new();

    if let Some(title_text) = new_title {
        let title_id = find_title_object_id(&page_elements);
        if let Some(obj_id) = title_id {
            let existing_title = extract_slide_text(&page_elements, "TITLE");
            let existing_centered = extract_slide_text(&page_elements, "CENTERED_TITLE");
            let existing_fallback = if existing_title.is_empty() && existing_centered.is_empty() {
                extract_all_body_text(&page_elements)
            } else {
                String::new()
            };
            let has_existing = !existing_title.is_empty()
                || !existing_centered.is_empty()
                || !existing_fallback.is_empty();
            if has_existing {
                update_reqs.push(json!({
                    "deleteText": { "objectId": &obj_id, "textRange": { "type": "ALL" } }
                }));
            }
            update_reqs.push(json!({
                "insertText": { "objectId": &obj_id, "text": title_text }
            }));
        }
    }

    if let Some(body_text) = new_body {
        let body_id = find_body_object_id(&page_elements);
        if let Some(obj_id) = body_id {
            let existing = extract_all_body_text(&page_elements);
            if !existing.is_empty() {
                update_reqs.push(json!({
                    "deleteText": { "objectId": &obj_id, "textRange": { "type": "ALL" } }
                }));
            }

            let parsed = crate::marp::parse_marp(body_text)
                .map_err(|e| GwsError::Validation(format!("Failed to parse body Marp: {e}")))?;
            if let Some(slide_content) = parsed.slides.first() {
                let blocks = &slide_content.body_blocks;
                if !blocks.is_empty() {
                    let (text, styles, bullets) = build_body_content(blocks, &obj_id);
                    if !text.is_empty() {
                        update_reqs.push(json!({
                            "insertText": { "objectId": &obj_id, "text": text }
                        }));
                        update_reqs.extend(styles);
                        update_reqs.extend(bullets);
                    }
                }
            }
        }
    }

    if let Some(notes_text) = new_notes {
        let notes_obj_id = slide
            .get("slideProperties")
            .and_then(|sp| sp.get("notesPage"))
            .and_then(|np| np.get("notesProperties"))
            .and_then(|np| np.get("speakerNotesObjectId"))
            .and_then(|id| id.as_str());
        if let Some(notes_id) = notes_obj_id {
            let existing = extract_notes_text(slide);
            if existing.is_some() {
                update_reqs.push(json!({
                    "deleteText": { "objectId": notes_id, "textRange": { "type": "ALL" } }
                }));
            }
            update_reqs.push(json!({
                "insertText": { "objectId": notes_id, "text": notes_text }
            }));
        }
    }

    if let Some(ph_map) = placeholders_map {
        for (label, text_val) in ph_map {
            let text = text_val.as_str().unwrap_or("");
            let obj_id = find_placeholder_by_label(&page_elements, label)
                .ok_or_else(|| {
                    GwsError::Validation(format!(
                        "Placeholder '{label}' not found on slide {slide_number}. Use gws_templates to see available placeholders."
                    ))
                })?;
            let has_text = page_elements
                .iter()
                .find(|e| e.get("objectId").and_then(|id| id.as_str()) == Some(&obj_id))
                .map(|e| !extract_text_from_shape(e).is_empty())
                .unwrap_or(false);
            if has_text {
                update_reqs.push(json!({
                    "deleteText": { "objectId": &obj_id, "textRange": { "type": "ALL" } }
                }));
            }
            update_reqs.push(json!({
                "insertText": { "objectId": &obj_id, "text": text }
            }));
        }
    }

    if update_reqs.is_empty() {
        return Err(GwsError::Validation(
            "No updatable elements found on this slide. The slide may not have the expected placeholder shapes.".into(),
        ));
    }

    slides_batch_update(presentation_id, update_reqs, state, policy, meta, dry_run).await?;

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "updated_slide": slide_number,
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

pub(crate) async fn execute_list_templates(
    arguments: Option<&Value>,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Value {
    let name_filter = arguments
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str());

    let mut templates: Vec<Value> = Vec::new();

    for t in policy.templates() {
        if let Some(filter) = name_filter
            && !t.name.to_lowercase().contains(&filter.to_lowercase())
        {
            continue;
        }

        let mut entry = json!({
            "name": t.name,
            "id": t.id
        });
        if let Some(ref desc) = t.description {
            entry["description"] = json!(desc);
        }
        if let Ok(slides_doc) = state.get_doc("slides").await
            && let Some(pres_resource) =
                tools::find_resource(&slides_doc.resources, "presentations")
            && let Some(get_method) = pres_resource.methods.get("get")
        {
            let args = json!({ "params": { "presentationId": &t.id } });
            let mut tc = state.token_cache.take();
            if let Ok(pres_data) = crate::execute::execute_tool(
                &slides_doc,
                get_method,
                "presentations",
                "get",
                &args,
                "slides",
                policy,
                meta,
                None,
                None,
                false,
                &mut tc,
            )
            .await
            {
                let raw_layouts = pres_data
                    .get("layouts")
                    .and_then(|l| l.as_array())
                    .cloned()
                    .unwrap_or_default();

                let active_master = pres_data
                    .get("slides")
                    .and_then(|s| s.as_array())
                    .and_then(|slides| slides.last())
                    .and_then(|s| s.get("slideProperties"))
                    .and_then(|sp| sp.get("masterObjectId"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("");

                let mut seen = std::collections::HashSet::new();
                let mut layout_details = Vec::new();
                for layout in &raw_layouts {
                    let master = layout
                        .get("layoutProperties")
                        .and_then(|lp| lp.get("masterObjectId"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("");
                    if !active_master.is_empty() && master != active_master {
                        continue;
                    }
                    let name = layout
                        .get("layoutProperties")
                        .and_then(|lp| lp.get("displayName"))
                        .and_then(|dn| dn.as_str())
                        .unwrap_or("");
                    if name.is_empty() || !seen.insert(name.to_string()) {
                        continue;
                    }
                    layout_details.push(extract_layout_details(layout));
                }
                entry["layouts"] = json!(layout_details);
            }
            state.token_cache = tc;
        }
        templates.push(entry);
    }

    json!({
        "templates": templates,
        "count": templates.len(),
        "hint": "Use the template 'name' or 'id' as the 'template' argument in gws_slides_import_marp. Use placeholder labels with gws_slides_update."
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

    #[test]
    fn test_slides_read_schema() {
        let schema = slides_read_tool_schema();
        assert_eq!(schema["name"], "gws_slides_read");
        assert_eq!(schema["annotations"]["readOnlyHint"], true);
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("presentation_id")));
        assert!(schema.get("outputSchema").is_some());
    }

    #[test]
    fn test_slides_add_schema() {
        let schema = slides_add_tool_schema();
        assert_eq!(schema["name"], "gws_slides_add");
        assert_eq!(schema["annotations"]["destructiveHint"], false);
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("presentation_id")));
        assert!(required.contains(&json!("marp")));
    }

    #[test]
    fn test_slides_delete_schema() {
        let schema = slides_delete_tool_schema();
        assert_eq!(schema["name"], "gws_slides_delete");
        assert_eq!(schema["annotations"]["destructiveHint"], true);
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("presentation_id")));
        assert!(required.contains(&json!("slide_numbers")));
    }

    #[test]
    fn test_slides_reorder_schema() {
        let schema = slides_reorder_tool_schema();
        assert_eq!(schema["name"], "gws_slides_reorder");
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("presentation_id")));
        assert!(required.contains(&json!("slide_numbers")));
        assert!(required.contains(&json!("position")));
    }

    #[test]
    fn test_extract_slide_text_title() {
        let elements = vec![json!({
            "shape": {
                "placeholder": { "type": "TITLE" },
                "text": {
                    "textElements": [
                        { "textRun": { "content": "My Title\n" } }
                    ]
                }
            }
        })];
        assert_eq!(extract_slide_text(&elements, "TITLE"), "My Title");
    }

    #[test]
    fn test_extract_slide_text_empty() {
        let elements = vec![json!({
            "shape": {
                "placeholder": { "type": "BODY" },
                "text": { "textElements": [] }
            }
        })];
        assert_eq!(extract_slide_text(&elements, "TITLE"), "");
    }

    #[test]
    fn test_extract_all_body_text() {
        let elements = vec![
            json!({
                "shape": {
                    "placeholder": { "type": "TITLE" },
                    "text": { "textElements": [{ "textRun": { "content": "Title\n" } }] }
                }
            }),
            json!({
                "shape": {
                    "placeholder": { "type": "BODY" },
                    "text": { "textElements": [{ "textRun": { "content": "Body text\n" } }] }
                }
            }),
        ];
        let body = extract_all_body_text(&elements);
        assert_eq!(body, "Body text");
    }

    #[test]
    fn test_extract_notes_text() {
        let slide = json!({
            "slideProperties": {
                "notesPage": {
                    "notesProperties": { "speakerNotesObjectId": "notes_1" },
                    "pageElements": [{
                        "objectId": "notes_1",
                        "shape": {
                            "text": {
                                "textElements": [
                                    { "textRun": { "content": "Speaker notes here\n" } }
                                ]
                            }
                        }
                    }]
                }
            }
        });
        assert_eq!(
            extract_notes_text(&slide),
            Some("Speaker notes here".to_string())
        );
    }

    #[test]
    fn test_extract_notes_text_none() {
        let slide = json!({ "slideProperties": {} });
        assert_eq!(extract_notes_text(&slide), None);
    }

    #[test]
    fn test_resolve_layout_name() {
        let pres = json!({
            "layouts": [{
                "objectId": "layout_abc",
                "layoutProperties": { "displayName": "Title Slide" }
            }]
        });
        assert_eq!(
            resolve_layout_name(&pres, "layout_abc"),
            Some("Title Slide".to_string())
        );
        assert_eq!(resolve_layout_name(&pres, "nonexistent"), None);
    }

    #[test]
    fn test_presentation_to_structured() {
        let pres = json!({
            "presentationId": "pres_123",
            "title": "Test Deck",
            "slides": [{
                "objectId": "slide_1",
                "slideProperties": { "layoutObjectId": "layout_1" },
                "pageElements": [{
                    "shape": {
                        "placeholder": { "type": "TITLE" },
                        "text": { "textElements": [{ "textRun": { "content": "Hello\n" } }] }
                    }
                }]
            }],
            "layouts": [{
                "objectId": "layout_1",
                "layoutProperties": { "displayName": "Interior title and body" }
            }]
        });
        let result = presentation_to_structured(&pres, None).unwrap();
        assert_eq!(result["presentation_id"], "pres_123");
        assert_eq!(result["title"], "Test Deck");
        assert_eq!(result["slide_count"], 1);
        let slides = result["slides"].as_array().unwrap();
        assert_eq!(slides[0]["slide_number"], 1);
        assert_eq!(slides[0]["title"], "Hello");
        assert_eq!(slides[0]["layout_name"], "Interior title and body");
    }

    #[test]
    fn test_presentation_to_structured_slide_number_filter() {
        let pres = json!({
            "presentationId": "p1",
            "title": "Deck",
            "slides": [
                { "objectId": "s1", "slideProperties": {}, "pageElements": [] },
                { "objectId": "s2", "slideProperties": {}, "pageElements": [] },
            ],
            "layouts": []
        });
        let result = presentation_to_structured(&pres, Some(2)).unwrap();
        let slides = result["slides"].as_array().unwrap();
        assert_eq!(slides.len(), 1);
        assert_eq!(slides[0]["slide_number"], 2);
    }

    #[test]
    fn test_presentation_to_structured_out_of_range() {
        let pres = json!({
            "presentationId": "p1",
            "title": "Deck",
            "slides": [{ "objectId": "s1", "slideProperties": {}, "pageElements": [] }],
            "layouts": []
        });
        let err = presentation_to_structured(&pres, Some(5)).unwrap_err();
        assert!(err.contains("out of range"));
    }

    #[test]
    fn test_presentation_to_markdown() {
        let pres = json!({
            "slides": [
                {
                    "objectId": "s1",
                    "slideProperties": {},
                    "pageElements": [{
                        "shape": {
                            "placeholder": { "type": "TITLE" },
                            "text": { "textElements": [{ "textRun": { "content": "Slide 1\n" } }] }
                        }
                    }]
                },
                {
                    "objectId": "s2",
                    "slideProperties": {
                        "notesPage": {
                            "notesProperties": { "speakerNotesObjectId": "n2" },
                            "pageElements": [{
                                "objectId": "n2",
                                "shape": { "text": { "textElements": [{ "textRun": { "content": "My notes\n" } }] } }
                            }]
                        }
                    },
                    "pageElements": [{
                        "shape": {
                            "placeholder": { "type": "TITLE" },
                            "text": { "textElements": [{ "textRun": { "content": "Slide 2\n" } }] }
                        }
                    }]
                }
            ]
        });
        let md = presentation_to_markdown(&pres);
        assert!(md.starts_with("---\nmarp: true\n---"));
        assert!(md.contains("# Slide 1"));
        assert!(md.contains("# Slide 2"));
        assert!(md.contains("My notes"));
    }

    #[test]
    fn test_slides_duplicate_schema() {
        let schema = slides_duplicate_tool_schema();
        assert_eq!(schema["name"], "gws_slides_duplicate");
        assert_eq!(schema["annotations"]["destructiveHint"], false);
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("presentation_id")));
        assert!(required.contains(&json!("slide_number")));
    }

    #[test]
    fn test_slides_update_schema() {
        let schema = slides_update_tool_schema();
        assert_eq!(schema["name"], "gws_slides_update");
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("presentation_id")));
        assert!(required.contains(&json!("slide_number")));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("title"));
        assert!(props.contains_key("body"));
        assert!(props.contains_key("notes"));
    }

    #[test]
    fn test_find_placeholder_object_id() {
        let elements = vec![
            json!({
                "objectId": "title_shape",
                "shape": { "placeholder": { "type": "TITLE" }, "text": {} }
            }),
            json!({
                "objectId": "body_shape",
                "shape": { "placeholder": { "type": "BODY" }, "text": {} }
            }),
        ];
        assert_eq!(
            find_placeholder_object_id(&elements, "TITLE"),
            Some("title_shape".to_string())
        );
        assert_eq!(
            find_placeholder_object_id(&elements, "BODY"),
            Some("body_shape".to_string())
        );
        assert_eq!(find_placeholder_object_id(&elements, "SUBTITLE"), None);
    }

    #[test]
    fn test_find_body_object_id_placeholder() {
        let elements = vec![json!({
            "objectId": "body_1",
            "shape": { "placeholder": { "type": "BODY" }, "text": {} }
        })];
        assert_eq!(find_body_object_id(&elements), Some("body_1".to_string()));
    }

    #[test]
    fn test_find_body_object_id_non_placeholder() {
        // With two text boxes (Marp pattern), first is title, second is body
        let elements = vec![
            json!({ "objectId": "title_box", "shape": { "text": { "textElements": [] } } }),
            json!({ "objectId": "body_box", "shape": { "text": { "textElements": [] } } }),
        ];
        assert_eq!(
            find_title_object_id(&elements),
            Some("title_box".to_string())
        );
        assert_eq!(find_body_object_id(&elements), Some("body_box".to_string()));
    }

    #[test]
    fn test_find_title_object_id_non_placeholder() {
        let elements = vec![json!({
            "objectId": "textbox_1",
            "shape": { "text": { "textElements": [] } }
        })];
        assert_eq!(
            find_title_object_id(&elements),
            Some("textbox_1".to_string())
        );
    }

    #[test]
    fn test_extract_styled_text_bold_italic() {
        let elem = json!({
            "shape": {
                "text": {
                    "textElements": [
                        { "textRun": { "content": "Normal ", "style": {} } },
                        { "textRun": { "content": "bold", "style": { "bold": true } } },
                        { "textRun": { "content": " and ", "style": {} } },
                        { "textRun": { "content": "italic", "style": { "italic": true } } },
                        { "textRun": { "content": "\n" } }
                    ]
                }
            }
        });
        let result = extract_styled_text_from_shape(&elem);
        assert!(result.contains("**bold**"));
        assert!(result.contains("*italic*"));
    }

    #[test]
    fn test_extract_styled_text_bullets() {
        let elem = json!({
            "shape": {
                "placeholder": { "type": "BODY" },
                "text": {
                    "textElements": [
                        { "paragraphMarker": { "bullet": {} } },
                        { "textRun": { "content": "Item 1\n", "style": {} } },
                        { "paragraphMarker": { "bullet": {} } },
                        { "textRun": { "content": "Item 2\n", "style": {} } }
                    ]
                }
            }
        });
        let result = extract_styled_text_from_shape(&elem);
        assert!(result.contains("- Item 1"));
        assert!(result.contains("- Item 2"));
    }

    #[test]
    fn test_extract_table_as_markdown() {
        let table = json!({
            "tableRows": [
                { "tableCells": [
                    { "text": { "textElements": [{ "textRun": { "content": "Name\n" } }] } },
                    { "text": { "textElements": [{ "textRun": { "content": "Value\n" } }] } }
                ]},
                { "tableCells": [
                    { "text": { "textElements": [{ "textRun": { "content": "A\n" } }] } },
                    { "text": { "textElements": [{ "textRun": { "content": "1\n" } }] } }
                ]}
            ]
        });
        let md = extract_table_as_markdown(&table);
        assert!(md.contains("| Name | Value |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| A | 1 |"));
    }

    #[test]
    fn test_structural_hints() {
        let elements = vec![json!({
            "shape": {
                "placeholder": { "type": "BODY" },
                "text": {
                    "textElements": [
                        { "paragraphMarker": { "bullet": {} } },
                        { "textRun": { "content": "Item\n", "style": { "fontFamily": "Courier New" } } }
                    ]
                }
            }
        })];
        assert!(slide_has_bullets(&elements));
        assert!(slide_has_code(&elements));
        assert!(!slide_has_table(&elements));
        assert!(!slide_has_image(&elements));
    }

    #[test]
    fn test_placeholder_label() {
        let ph = PlaceholderInfo {
            ph_type: "TITLE".to_string(),
            index: None,
        };
        assert_eq!(placeholder_label(&ph), "TITLE");
        let ph = PlaceholderInfo {
            ph_type: "SUBTITLE".to_string(),
            index: Some(3),
        };
        assert_eq!(placeholder_label(&ph), "SUBTITLE[3]");
        let ph = PlaceholderInfo {
            ph_type: "BODY".to_string(),
            index: Some(1),
        };
        assert_eq!(placeholder_label(&ph), "BODY[1]");
    }

    #[test]
    fn test_find_placeholder_by_label() {
        let elements = vec![
            json!({
                "objectId": "sub_0",
                "shape": { "placeholder": { "type": "SUBTITLE" } }
            }),
            json!({
                "objectId": "sub_3",
                "shape": { "placeholder": { "type": "SUBTITLE", "index": 3 } }
            }),
            json!({
                "objectId": "sub_8",
                "shape": { "placeholder": { "type": "SUBTITLE", "index": 8 } }
            }),
            json!({
                "objectId": "title_0",
                "shape": { "placeholder": { "type": "TITLE" } }
            }),
        ];
        assert_eq!(
            find_placeholder_by_label(&elements, "SUBTITLE[3]"),
            Some("sub_3".to_string())
        );
        assert_eq!(
            find_placeholder_by_label(&elements, "SUBTITLE[8]"),
            Some("sub_8".to_string())
        );
        assert_eq!(
            find_placeholder_by_label(&elements, "SUBTITLE"),
            Some("sub_0".to_string())
        );
        assert_eq!(
            find_placeholder_by_label(&elements, "TITLE"),
            Some("title_0".to_string())
        );
        assert_eq!(find_placeholder_by_label(&elements, "SUBTITLE[99]"), None);
        assert_eq!(find_placeholder_by_label(&elements, "BODY"), None);
    }

    #[test]
    fn test_extract_layout_details() {
        let layout = json!({
            "objectId": "layout_1",
            "layoutProperties": { "displayName": "Interior quote large" },
            "pageElements": [
                {
                    "objectId": "pe_title",
                    "shape": { "placeholder": { "type": "TITLE" } },
                    "size": { "width": { "magnitude": 3000000, "unit": "EMU" }, "height": { "magnitude": 3000000, "unit": "EMU" } },
                    "transform": { "scaleX": 2.24, "scaleY": 1.07, "translateX": 4465550.0, "translateY": 1455375.0, "unit": "EMU" }
                },
                {
                    "objectId": "pe_attr",
                    "shape": { "placeholder": { "type": "SUBTITLE", "index": 8 } },
                    "size": { "width": { "magnitude": 3000000, "unit": "EMU" }, "height": { "magnitude": 3000000, "unit": "EMU" } },
                    "transform": { "scaleX": 1.37, "scaleY": 0.14, "translateX": 4465600.0, "translateY": 5036725.0, "unit": "EMU" }
                }
            ]
        });
        let result = extract_layout_details(&layout);
        assert_eq!(result["name"], "Interior quote large");
        assert_eq!(result["id"], "layout_1");
        let phs = result["placeholders"].as_array().unwrap();
        assert_eq!(phs.len(), 2);
        assert_eq!(phs[0]["label"], "TITLE");
        assert_eq!(phs[0]["size"], "large");
        assert_eq!(phs[1]["label"], "SUBTITLE[8]");
        assert_eq!(phs[1]["index"], 8);
        assert!(phs[1]["width_pt"].as_i64().unwrap() > 0);
    }
}
