use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use rmcp::service::{ElicitationMode, Peer, RoleServer};
use serde::{Deserialize, Serialize};

fn supports_form(peer: &Peer<RoleServer>) -> bool {
    peer.supported_elicitation_modes()
        .contains(&ElicitationMode::Form)
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Drive folder selection")]
pub struct FolderSelection {
    #[schemars(description = "Google Drive folder ID to use")]
    pub folder_id: String,
}
rmcp::elicit_safe!(FolderSelection);

pub async fn ask_folder(peer: &Peer<RoleServer>) -> Option<String> {
    if !supports_form(peer) {
        return None;
    }
    match peer
        .elicit::<FolderSelection>("No target folder specified. Enter a Google Drive folder ID:")
        .await
    {
        Ok(Some(selection)) if !selection.folder_id.is_empty() => Some(selection.folder_id),
        _ => None,
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Overwrite confirmation")]
pub struct OverwriteConfirmation {
    #[schemars(description = "Overwrite the existing document?")]
    pub overwrite: bool,
}
rmcp::elicit_safe!(OverwriteConfirmation);

pub async fn confirm_overwrite(peer: &Peer<RoleServer>, doc_title: &str) -> bool {
    if !supports_form(peer) {
        return true;
    }
    match peer
        .elicit::<OverwriteConfirmation>(&format!(
            "Document '{doc_title}' already exists. Overwrite its content?"
        ))
        .await
    {
        Ok(Some(confirmation)) => confirmation.overwrite,
        _ => false,
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Template selection")]
pub struct TemplateSelection {
    #[schemars(description = "Name of the presentation template to use")]
    pub template_name: String,
}
rmcp::elicit_safe!(TemplateSelection);

pub async fn ask_template(peer: &Peer<RoleServer>, template_names: &[String]) -> Option<String> {
    if !supports_form(peer) || template_names.is_empty() {
        return None;
    }
    let list = template_names.join(", ");
    match peer
        .elicit::<TemplateSelection>(&format!(
            "No template specified. Available templates: {list}. Enter a template name:"
        ))
        .await
    {
        Ok(Some(selection)) if !selection.template_name.is_empty() => Some(selection.template_name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_folder_selection_schema() {
        let schema = rmcp::schemars::schema_for!(FolderSelection);
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "object");
        assert!(json["properties"]["folder_id"].is_object());
    }

    #[test]
    fn test_overwrite_confirmation_schema() {
        let schema = rmcp::schemars::schema_for!(OverwriteConfirmation);
        let json = serde_json::to_value(&schema).unwrap();
        assert!(json["properties"]["overwrite"].is_object());
    }

    #[test]
    fn test_template_selection_schema() {
        let schema = rmcp::schemars::schema_for!(TemplateSelection);
        let json = serde_json::to_value(&schema).unwrap();
        assert!(json["properties"]["template_name"].is_object());
    }
}
