use super::*;
use crate::tui::app::session_management::Stage;

pub(crate) fn draw_session_management(frame: &mut Frame<'_>, app: &App) {
    let Some(manager) = &app.session_manager else {
        return;
    };
    let area = centered_rect(88, 72, frame.area());
    theme::modal_backdrop(frame, area);
    let title = match manager.stage {
        Stage::Actions => " Manage session ",
        Stage::Rename => " Rename session ",
        Stage::Move => " Move session to workspace ",
        Stage::Delete => " Delete session? ",
    };
    let block = theme::modal_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        Line::styled(
            manager.title.clone(),
            Style::default().fg(theme::accent()).bold(),
        ),
        Line::from(""),
    ];
    if manager.stage == Stage::Actions {
        for (i, label) in ["Rename (r)", "Move workspace (m)", "Delete… (d)"]
            .iter()
            .enumerate()
        {
            lines.push(Line::styled(
                format!("{} {label}", if i == manager.selected { "›" } else { " " }),
                if i == manager.selected {
                    Style::default().fg(theme::accent()).bold()
                } else {
                    Style::default().fg(theme::text())
                },
            ));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("j/k select · Enter continue · q/Esc back"));
    } else {
        lines.push(Line::from(match manager.stage {
            Stage::Rename => "Name (Ctrl+U clears):",
            Stage::Move => "Workspace directory (Tab cycles known workspaces):",
            _ => "Type delete to confirm removal:",
        }));
        lines.push(Line::styled(
            format!("> {}▏", manager.text),
            Style::default().fg(theme::accent()),
        ));
        lines.push(Line::from(""));
        if manager.stage == Stage::Delete {
            lines.push(Line::from(
                "A recovery archive is saved in the config exports directory.",
            ));
        } else if manager.stage == Stage::Move {
            lines.push(Line::from(
                "History is kept; working directory and context roots reset.",
            ));
        }
        lines.push(Line::from("Enter submit · Esc cancel"));
    }
    if let Some(error) = &manager.error {
        lines.push(Line::from(error.clone()));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme::surface())
            .wrap(Wrap { trim: false }),
        inner,
    );
}
