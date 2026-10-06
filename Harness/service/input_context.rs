//! Normalizes user-visible remote file and image attachment annotations.

use serde_json::Value;

use crate::core::ImageAttachment;

pub(super) const REMOTE_FILE_CONTEXT_OPEN: &str = "\n\n<yeet_remote_files>";
pub(super) const REMOTE_FILE_CONTEXT_CLOSE: &str = "</yeet_remote_files>";

pub(super) fn split_remote_file_context(input: &str) -> (&str, Option<&str>) {
    if !input.ends_with(REMOTE_FILE_CONTEXT_CLOSE) {
        return (input, None);
    }
    let Some(start) = input.rfind(REMOTE_FILE_CONTEXT_OPEN) else {
        return (input, None);
    };
    let payload_start = start + REMOTE_FILE_CONTEXT_OPEN.len();
    let payload_end = input.len() - REMOTE_FILE_CONTEXT_CLOSE.len();
    if payload_start > payload_end {
        return (input, None);
    }
    (&input[..start], Some(&input[payload_start..payload_end]))
}

fn remote_file_annotation(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let files = value.get("files")?.as_array()?;
    if files.is_empty() {
        return None;
    }

    let names = files
        .iter()
        .filter_map(|file| file.get("name").and_then(Value::as_str))
        .filter(|name| !name.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    Some(format!(
        "[{} file{}{}]",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        if names.is_empty() {
            String::new()
        } else {
            format!(": {names}")
        }
    ))
}

pub(super) fn visible_user_content(input: &str, images: &[ImageAttachment]) -> String {
    let (visible_input, file_payload) = split_remote_file_context(input);
    let mut annotations = Vec::new();

    if !images.is_empty() {
        let names = images
            .iter()
            .filter_map(|image| image.name.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        annotations.push(format!(
            "[{} image{}{}]",
            images.len(),
            if images.len() == 1 { "" } else { "s" },
            if names.is_empty() {
                String::new()
            } else {
                format!(": {names}")
            }
        ));
    }

    if let Some(annotation) = file_payload.and_then(remote_file_annotation) {
        annotations.push(annotation);
    }

    if annotations.is_empty() {
        return visible_input.to_owned();
    }

    let annotations = annotations.join("\n");
    if visible_input.is_empty() {
        annotations
    } else {
        format!("{visible_input}\n{annotations}")
    }
}
