use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
};

use crate::ui::styles::{DIFF_ADDED_BG, DIFF_REMOVED_BG};

pub fn centered_rect(r: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub fn centered_rect_line_height(r: Rect, percent_x: u16, lines_y: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(lines_y),
            Constraint::Fill(1),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Center a rect of fixed width and height within an outside rect
pub fn centered_rect_fixed(area: Rect, width: u16, height: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;

    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// replaces tabs in a string by spaces
///
/// ratatui doesn't work well displaying tabs, so any
/// string that is rendered and might contain tabs
/// needs to have the tabs converted to spaces.
///
/// this function aligns tabs in the input string to
/// virtual tab stops 4 spaces apart, taking care
/// to count ansi control sequences as zero width.
pub fn tabs_to_spaces(line: &str) -> String {
    const TAB_WIDTH: usize = 4;

    enum AnsiState {
        Neutral,
        Escape,
        Csi,
    }

    let mut out = String::new();
    let mut x = 0;
    let mut ansi_state = AnsiState::Neutral;
    for c in line.chars() {
        match ansi_state {
            AnsiState::Neutral => {
                if c == '\t' {
                    loop {
                        out.push(' ');
                        x += 1;
                        if x % TAB_WIDTH == 0 {
                            break;
                        }
                    }
                } else {
                    out.push(c);
                    if c == '\x1b' {
                        ansi_state = AnsiState::Escape;
                    } else {
                        x += 1;
                    }
                }
                if c == '\r' || c == '\n' {
                    x = 0;
                }
            }
            AnsiState::Escape => {
                out.push(c);
                ansi_state = if c == '[' {
                    AnsiState::Csi
                } else {
                    AnsiState::Neutral
                };
            }
            AnsiState::Csi => {
                out.push(c);
                if ('\x40'..='\x7f').contains(&c) {
                    ansi_state = AnsiState::Neutral;
                }
            }
        }
    }
    out
}

/// Apply GitHub-style line backgrounds to a `jj diff --git` text: light
/// green for added lines, light red for removed lines. All diff body
/// lines (added, removed, and context) render in white. `diff --git`
/// and `index` header lines are dropped; `---`/`+++` and hunk headers
/// (`@@`) are left untouched. A `+N -M` summary line is prepended when
/// the text contains any body changes.
pub fn tint_git_diff(mut text: Text<'_>) -> Text<'_> {
    text.lines.retain(|line| {
        !(line_starts_with(line, "diff --git ") || line_starts_with(line, "index "))
    });

    let mut added = 0u32;
    let mut removed = 0u32;

    for line in &mut text.lines {
        let Some(first) = line_leading_char(line) else {
            continue;
        };
        let line_style = match first {
            '+' if !line_starts_with(line, "+++") => {
                added += 1;
                Style::default().bg(DIFF_ADDED_BG).fg(Color::White)
            }
            '-' if !line_starts_with(line, "---") => {
                removed += 1;
                Style::default().bg(DIFF_REMOVED_BG).fg(Color::White)
            }
            ' ' => Style::default().fg(Color::White),
            _ => continue,
        };
        line.style = line.style.patch(line_style);
        for span in &mut line.spans {
            span.style = span.style.patch(line_style);
        }
    }

    if added > 0 || removed > 0 {
        let summary = Line::from(vec![
            Span::styled(
                format!("+{added}"),
                Style::default().fg(Color::Green).bold(),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{removed}"),
                Style::default().fg(Color::Red).bold(),
            ),
        ]);
        text.lines.insert(0, summary);
    }

    text
}

fn line_leading_char(line: &Line<'_>) -> Option<char> {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .next()
}

fn line_starts_with(line: &Line<'_>, prefix: &str) -> bool {
    let mut chars = line.spans.iter().flat_map(|span| span.content.chars());
    prefix.chars().all(|p| chars.next() == Some(p))
}
