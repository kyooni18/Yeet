//! Browser fixture backed by the production shared Home controller.
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
use yeet::{
    harness::HarnessState,
    shared_ui::{
        application_session::ApplicationSession,
        home::{HomeAction, ResourceItem, ResourceKind, ResourceTarget, WorkspaceContent},
    },
};

#[derive(Deserialize)]
struct Session {
    id: String,
    title: String,
}

#[derive(Deserialize)]
struct Input {
    sessions: Vec<Session>,
    action: Option<HomeAction>,
    #[serde(default = "delivered")]
    deliver: bool,
}

fn delivered() -> bool {
    true
}

fn main() {
    for line in io::stdin().lock().lines() {
        let input: Input =
            serde_json::from_str(&line.expect("read fixture")).expect("decode fixture");
        let harness = HarnessState::default();
        let mut ui = ApplicationSession::new(&harness);
        let mut content = WorkspaceContent::default();
        content.sessions = input
            .sessions
            .into_iter()
            .map(|session| {
                ResourceItem::new(
                    ResourceTarget::Session(session.id),
                    ResourceKind::Session,
                    session.title,
                )
            })
            .collect();
        ui.update_home_content(content);
        let mut open = None;
        let mut command = None;
        if let Some(action) = input.action {
            let prepared = ui.prepare_home(action, &harness);
            open = prepared.effect.open.clone();
            command = prepared.effect.command.clone();
            if input.deliver {
                ui.commit_home(prepared, &harness);
            }
        }
        let projection = ui.projection();
        println!(
            "{}",
            json!({
                "home_revision": projection.home_revision,
                "view": projection.home,
                "open": open,
                "command": command,
            })
        );
    }
}
