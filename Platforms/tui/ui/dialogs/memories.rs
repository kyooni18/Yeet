//! Local-only terminal projection. Never publish these rows in Remote/shared
//! replay state; protected excerpts are rendered directly for the local user.

use super::*;
use crate::{harness::memory::*, tui::app::memories::Stage};

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}

pub(crate) fn draw_memories(frame: &mut Frame<'_>, app: &mut App) {
    let Some(panel) = &mut app.memory_manager else {
        return;
    };
    panel.manager.poll();
    let area = centered_rect(96, 94, frame.area());
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Memories · local user management · Esc back ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let error = panel.error.as_deref().unwrap_or(&panel.manager.status);
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(if error.is_empty() { 0 } else { 2 }),
    ])
    .split(inner);
    if !error.is_empty() {
        frame.render_widget(
            Paragraph::new(clean(error))
                .wrap(Wrap { trim: false })
                .style(theme::surface()),
            chunks[1],
        );
    }
    if matches!(panel.stage, Stage::Browse) {
        let regions = Layout::vertical([
            Constraint::Length(6),
            Constraint::Min(1),
            Constraint::Length(5),
        ])
        .split(chunks[0]);
        let settings = &panel.manager.settings;
        let enabled = |value| if value { "on" } else { "off" };
        let header = vec![
            Line::from(format!(
                "1 explicit {} · profile [3]: {}",
                enabled(settings.explicit.enabled),
                settings
                    .explicit
                    .shared_profile_id
                    .as_deref()
                    .unwrap_or("none — lexical only")
            )),
            Line::from(format!(
                "2 session  {} · profile [4]: {}",
                enabled(settings.whole_session.enabled),
                settings
                    .whole_session
                    .shared_profile_id
                    .as_deref()
                    .unwrap_or("none — lexical only")
            )),
            Line::from(
                "Local-only agent defaults/recall gated: provenance guards pending. p protects stored records.",
            ),
            Line::from(format!(
                "Filter {:?} / {:?} · search: {} · marked {}",
                panel.manager.filter.kind,
                panel.manager.filter.privacy,
                clean(panel.manager.filter.query.as_deref().unwrap_or("")),
                panel.marked.len()
            )),
            Line::from(if panel.manager.busy {
                "Background task working…"
            } else {
                "Agent recall: shared lexical · embedding/indexing is explicit"
            }),
            Line::from(""),
        ];
        frame.render_widget(
            Paragraph::new(header)
                .style(theme::surface())
                .wrap(Wrap { trim: false }),
            regions[0],
        );
        let rows = panel
            .manager
            .page
            .records
            .iter()
            .map(|record| {
                let marked = if panel.marked.contains_key(&record.id) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let excerpt = clean(&record.text).replace('\n', " ");
                ListItem::new(vec![
                    Line::from(format!(
                        "{marked} {:?} · {:?} · rev {} · {}",
                        record.kind, record.privacy, record.revision, record.indexing_status
                    )),
                    Line::from(truncate_end(
                        &excerpt,
                        regions[1].width.saturating_sub(5) as usize,
                    )),
                ])
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            frame.render_widget(Paragraph::new("No memories match. Enable a system with 1/2; add explicit knowledge with a.\nSession capture starts with runs begun after enablement; old history is not imported.").wrap(Wrap{trim:false}),regions[1]);
        } else {
            let mut state = ListState::default();
            state.select(Some(panel.selected));
            frame.render_stateful_widget(
                List::new(rows)
                    .style(theme::surface())
                    .highlight_style(Style::default().fg(theme::accent()).bold())
                    .highlight_symbol("› "),
                regions[1],
                &mut state,
            );
        }
        frame.render_widget(Paragraph::new(vec![
            Line::from("j/k select · Enter inspect · / search · f kind · v privacy · [/] pages · r refresh"),
            Line::from("a add · e edit · Space mark · d delete · p protect / declassify · q close"),
            Line::from("1/2 enable · 3/4 choose profile · i prepare local · c validate cloud · x local index"),
            Line::from("u remove local model · b reconcile committed history (no backfill)"),
            Line::from("Shared means eligible for cloud use, not public. No cloud memory dispatch is active."),
        ]).style(theme::surface()).wrap(Wrap{trim:false}),regions[2]);
        return;
    }
    let mut lines = Vec::<Line<'static>>::new();
    match &panel.stage {
        Stage::Inspect(record) => {
            lines.push(Line::from(format!(
                "{} · rev {} · {:?} · {:?}",
                record.id, record.revision, record.kind, record.privacy
            )));
            lines.push(Line::from(format!(
                "Scope {:?} · indexing {} · edited {}",
                record.scope, record.indexing_status, record.edited
            )));
            lines.push(Line::from(format!(
                "Created {} · updated {}",
                record.created_at, record.updated_at
            )));
            if let Some(source) = &record.source {
                lines.push(Line::from(format!(
                    "Source session {} · episode {}",
                    clean(&source.session_id),
                    clean(&source.window_id)
                )));
                lines.push(Line::from(format!(
                    "{} · bytes {}..{}",
                    clean(&source.source_revision),
                    source.start,
                    source.end
                )));
            }
            lines.push(Line::from(
                "j/k or PgUp/PgDn scroll · Esc back. Original transcript is preserved.",
            ));
            lines.push(Line::from(""));
            lines.extend(
                clean(&record.text)
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
        }
        Stage::Edit(record) => {
            lines.push(Line::from(if record.is_some() {
                "Edit memory override (not the original transcript)"
            } else {
                "Add explicit shared workspace memory"
            }));
            lines.push(Line::from("Literal multiline draft · Enter newline · paste supported · Ctrl+U clear · Ctrl+S save · Esc cancel"));
            lines.push(Line::from(""));
            lines.extend(
                clean(&panel.text)
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
            lines.push(Line::from("▏"));
        }
        Stage::Search => {
            lines.push(Line::from("Search canonical text (keyword; no embedding provider is contacted). Enter applies; Esc cancels."));
        }
        Stage::Delete(targets) => {
            lines.push(Line::from(format!(
                "Delete {} captured record revision(s)? Type delete then Enter. Esc cancels.",
                targets.len()
            )));
            lines.push(Line::from("Canonical text and derived indexes are removed; source suppressions prevent resurrection. Original transcripts remain."));
            for (id, revision) in targets.iter().take(20) {
                lines.push(Line::from(format!("{id} @ {revision}")));
            }
        }
        Stage::Share(record) => {
            lines.push(Line::from(format!(
                "Declassify {} @ {}?",
                record.id, record.revision
            )));
            lines.push(Line::from("Shared makes this exact revision eligible for future cloud use. It is not currently sent to a provider. Type share then Enter, or Esc cancel."));
            lines.push(Line::from(truncate_end(&clean(&record.text), 1800)));
        }
        Stage::Download => {
            let model = &local_model_catalog()[0];
            lines.push(Line::from(format!(
                "Prepare {} · {} · {} dimensions · CPU native",
                model.repository, model.license, model.dimensions
            )));
            lines.push(Line::from(format!("Pinned revision {}", model.revision)));
            lines.push(Line::from("Explicit ~91 MB download from Hugging Face, SHA-256 verified. No external inference daemon or downloaded executable. Offline after preparation."));
            lines.push(Line::from(
                "Type download then Enter; Esc cancels. Existing verified artifacts are reused.",
            ));
        }
        Stage::Remove => {
            lines.push(Line::from("Remove the managed MiniLM model? Canonical text and profiles are preserved. In-use models cannot be removed. Type remove then Enter."));
        }
        Stage::Cloud => {
            lines.push(Line::from(
                "Enter PROFILE_ID PROVIDER/MODEL, e.g. openai-small openai/text-embedding-3-small.",
            ));
            lines.push(Line::from("Use existing provider credentials; never enter an API key here. Next step explicitly confirms fixture transmission."));
        }
        Stage::ConfirmCloud { profile, model } => {
            lines.push(Line::from(format!(
                "Validate {} using {}?",
                clean(profile),
                clean(model)
            )));
            lines.push(Line::from("Only a bundled non-sensitive document/query fixture is sent to the selected endpoint. Future cloud document/query processing requires separate dispatch consent (currently unavailable)."));
            lines.push(Line::from("Type allow-cloud then Enter, or Esc cancel."));
        }
        Stage::Browse => {}
    }
    if !matches!(panel.stage, Stage::Inspect(_) | Stage::Edit(_)) {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("> {}▏", clean(&panel.text))));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::surface())
            .wrap(Wrap { trim: false })
            .scroll((panel.scroll, 0)),
        chunks[0],
    );
}
