//! Terminal drops are delivered as pasted path text, not native file events.
use std::path::PathBuf;

use super::App;
use crate::{core::ImageAttachment, shared_ui::composer::ComposerAttachment};

pub(crate) struct LocalAttachment {
    id: String,
    pub(crate) name: String,
    path: PathBuf,
    size: u64,
    image: Option<ImageAttachment>,
}

// Accept only complete lists of absolute, existing files. Normal pasted prose,
// directories, and partially valid drops must remain editable text.
fn pasted_paths(text: &str) -> Option<Vec<PathBuf>> {
    fn path(value: &str) -> Option<PathBuf> {
        let path = if value.starts_with("file://") {
            url::Url::parse(value).ok()?.to_file_path().ok()?
        } else {
            PathBuf::from(value)
        };
        (path.is_absolute() && path.is_file()).then_some(path)
    }
    let text = text.trim();
    if let Some(path) = path(text) {
        return Some(vec![path]);
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, '\'' | '"') => quote = Some(ch),
            (Some(q), ch) if q == ch => quote = None,
            (Some('\''), ch) => word.push(ch),
            (_, '\\') => word.push(chars.next()?),
            (None, ch) if ch.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (_, ch) => word.push(ch),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    if words.is_empty() {
        return None;
    }
    words.iter().map(|word| path(word)).collect()
}

impl App {
    pub(super) fn try_attach_pasted_paths(&mut self, text: &str) -> bool {
        let Some(paths) = pasted_paths(text) else {
            return false;
        };
        let result = (|| -> anyhow::Result<Vec<LocalAttachment>> {
            anyhow::ensure!(
                self.attachments.len() + paths.len() <= 8,
                "At most 8 files can be attached."
            );
            let mut total: u64 = self.attachments.iter().map(|item| item.size).sum();
            let mut items = Vec::new();
            for path in paths {
                let size = std::fs::metadata(&path)?.len();
                total = total.saturating_add(size);
                anyhow::ensure!(
                    total <= ImageAttachment::MAX_IMAGE_BYTES as u64,
                    "Attachments must total at most 20 MiB."
                );
                let image = match path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("png" | "jpg" | "jpeg" | "webp" | "gif") => {
                        anyhow::ensure!(size > 0, "Image is empty: {}", path.display());
                        Some(ImageAttachment::from_file(&path)?)
                    }
                    _ => None,
                };
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                items.push(LocalAttachment {
                    id: uuid::Uuid::new_v4().to_string(),
                    name,
                    path,
                    size,
                    image,
                });
            }
            Ok(items)
        })();
        match result {
            Ok(items) => {
                self.attachments.extend(items);
                self.sync_composer();
            }
            Err(error) => {
                let message = format!("Could not attach files: {error:#}");
                self.toasts.push(
                    message.clone(),
                    std::time::Instant::now(),
                    std::time::Duration::from_secs(6),
                );
                self.backend_message = Some(message);
            }
        }
        true
    }

    pub(super) fn composer_attachments(&self) -> Vec<ComposerAttachment> {
        self.attachments
            .iter()
            .map(|item| ComposerAttachment {
                id: item.id.clone(),
                name: item.name.clone(),
                attachment_id: Some(item.id.clone()),
                ready: true,
                error: None,
            })
            .collect()
    }

    pub(super) fn populate_attachment_submit(
        &self,
        text: &mut String,
        images: &mut Vec<ImageAttachment>,
    ) {
        let mut files = Vec::new();
        for item in &self.attachments {
            if let Some(image) = &item.image {
                images.push(image.clone());
            } else {
                files.push(serde_json::json!({ "name": item.name, "path": item.path, "size": item.size, "media_type": "application/octet-stream" }));
            }
        }
        if !files.is_empty() {
            if text.trim().is_empty() {
                text.push_str("Please inspect the attached files.");
            }
            let payload = serde_json::json!({ "files": files });
            text.push_str(&format!(
                "\n\n<yeet_remote_files>{payload}</yeet_remote_files>"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_from_terminal_drop_are_accepted_without_consuming_prose() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello world.txt");
        std::fs::write(&file, b"hello").unwrap();
        assert_eq!(
            pasted_paths(&format!("'{}'", file.display())),
            Some(vec![file.clone()])
        );
        assert_eq!(
            pasted_paths(&file.to_string_lossy().replace(' ', "\\ ")),
            Some(vec![file.clone()])
        );
        assert_eq!(
            pasted_paths(url::Url::from_file_path(&file).unwrap().as_str()),
            Some(vec![file])
        );
        assert!(pasted_paths("ordinary pasted prose").is_none());
    }

    #[test]
    fn paste_queues_images_and_files_for_native_submission_and_scopes_them_to_session() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image.png");
        let file = dir.path().join("notes.txt");
        std::fs::write(&image, b"image bytes").unwrap();
        std::fs::write(&file, b"file bytes").unwrap();
        let mut app = App::default();
        app.handle_paste(&format!("'{}' '{}'", image.display(), file.display()));
        assert!(app.input.is_empty());
        assert_eq!(app.attachments.len(), 2);
        assert!(app.application.composer_projection().view.can_submit);
        let mut text = String::new();
        let mut images = Vec::new();
        app.populate_attachment_submit(&mut text, &mut images);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        assert!(text.contains("<yeet_remote_files>"));
        assert!(text.contains("notes.txt"));
        assert!(!text.contains("image.png"));
        app.state.current_session_id = Some("other".into());
        app.sync_composer();
        assert!(app.attachments.is_empty());
    }

    #[test]
    fn oversized_drop_is_rejected_without_partial_queue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(ImageAttachment::MAX_IMAGE_BYTES as u64 + 1)
            .unwrap();
        let mut app = App::default();
        app.handle_paste(&path.to_string_lossy());
        assert!(app.attachments.is_empty());
        assert!(app.backend_message.as_deref().unwrap().contains("20 MiB"));
    }
}
