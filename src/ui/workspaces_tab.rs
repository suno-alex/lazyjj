#![expect(clippy::borrow_interior_mutable_const)]

use anyhow::Result;
use ratatui::{
    crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers},
    prelude::*,
    widgets::*,
};
use tracing::instrument;
use tui_confirm_dialog::{ButtonLabel, ConfirmDialog, ConfirmDialogState, Listener};
use tui_textarea::{CursorMove, TextArea};

use crate::{
    ComponentInputResult,
    commander::{CommandError, Commander, workspaces::Workspace},
    env::Config,
    ui::{
        Component, ComponentAction,
        help_popup::HelpPopup,
        message_popup::MessagePopup,
        panel::DetailsPanel,
        utils::{centered_rect_line_height, tabs_to_spaces},
    },
};

const FORGET_POPUP_ID: u16 = 1;

/// Default seed for the path field in the Add Workspace popup.
/// Expanded (`~` → `$HOME`) so jj receives an absolute path.
fn default_workspace_path_prefix() -> String {
    let suffix = "Development/app-mobile-";
    match std::env::var_os("HOME") {
        Some(home) => format!("{}/{suffix}", home.to_string_lossy()),
        None => format!("~/{suffix}"),
    }
}

struct AddWorkspace<'a> {
    path_textarea: TextArea<'a>,
    name_textarea: TextArea<'a>,
    /// 0 = path, 1 = name
    focus: u8,
    /// Optional revset to pass to `jj workspace add -r`. Set when the
    /// add popup is opened from the Log tab on a specific change.
    revision: Option<String>,
    error: Option<anyhow::Error>,
}

struct RenameWorkspace<'a> {
    textarea: TextArea<'a>,
    old_name: String,
    error: Option<anyhow::Error>,
}

/// Workspaces tab. Lists `jj workspace list` in the main panel and shows
/// the selected workspace's working-copy commit on the right.
pub struct WorkspacesTab<'a> {
    workspaces_output: Result<Vec<Workspace>, CommandError>,
    workspaces_list_state: ListState,
    workspaces_height: u16,

    selected: Option<Workspace>,

    details_panel: DetailsPanel,
    details_output: Option<Result<String, CommandError>>,

    add: Option<AddWorkspace<'a>>,
    rename: Option<RenameWorkspace<'a>>,

    forget_popup: ConfirmDialogState,
    forget_popup_tx: std::sync::mpsc::Sender<Listener>,
    forget_popup_rx: std::sync::mpsc::Receiver<Listener>,
    forget_target: Option<String>,

    config: Config,
}

fn render_details(
    commander: &mut Commander,
    workspace: &Workspace,
) -> Result<String, CommandError> {
    // Show the workspace's working-copy commit. We use the `<name>@`
    // revset which works from any workspace.
    let revset = format!("{}@", workspace.name);

    let log = commander.execute_jj_command(
        vec![
            "log",
            "--no-graph",
            "-r",
            &revset,
            "--template",
            "builtin_log_detailed",
        ],
        true,
        true,
    )?;

    let header = format!(
        "Workspace : {}\nRoot      : {}\n\n",
        workspace.name,
        workspace.root.display()
    );
    Ok(format!("{header}{}", tabs_to_spaces(&log)))
}

fn current_index(
    selected: Option<&Workspace>,
    workspaces: &Result<Vec<Workspace>, CommandError>,
) -> Option<usize> {
    match (selected, workspaces) {
        (Some(selected), Ok(list)) => list.iter().position(|w| w.name == selected.name),
        _ => None,
    }
}

impl WorkspacesTab<'_> {
    #[instrument(level = "trace", skip(commander))]
    pub fn new(commander: &mut Commander) -> Result<Self> {
        let workspaces_output = commander.get_workspaces();
        let selected = workspaces_output.as_ref().ok().and_then(|list| {
            list.iter()
                .find(|w| w.is_current)
                .or_else(|| list.first())
                .cloned()
        });

        let workspaces_list_state = ListState::default()
            .with_selected(current_index(selected.as_ref(), &workspaces_output));

        let details_output = selected.as_ref().map(|w| render_details(commander, w));

        let (forget_popup_tx, forget_popup_rx) = std::sync::mpsc::channel();

        Ok(Self {
            workspaces_output,
            workspaces_list_state,
            workspaces_height: 0,
            selected,
            details_panel: DetailsPanel::new(),
            details_output,
            add: None,
            rename: None,
            forget_popup: ConfirmDialogState::default(),
            forget_popup_tx,
            forget_popup_rx,
            forget_target: None,
            config: commander.env.config.clone(),
        })
    }

    fn refresh(&mut self, commander: &mut Commander) {
        self.workspaces_output = commander.get_workspaces();
        // Try to keep selection on the same name; otherwise pick current/first.
        let selected_name = self.selected.as_ref().map(|w| w.name.clone());
        self.selected = self.workspaces_output.as_ref().ok().and_then(|list| {
            selected_name
                .as_ref()
                .and_then(|name| list.iter().find(|w| &w.name == name))
                .or_else(|| list.iter().find(|w| w.is_current))
                .or_else(|| list.first())
                .cloned()
        });
        self.refresh_details(commander);
    }

    fn refresh_details(&mut self, commander: &mut Commander) {
        let inner_width = self.details_panel.columns() as usize;
        commander.limit_width(inner_width);
        self.details_output = self.selected.as_ref().map(|w| render_details(commander, w));
        self.details_panel.scroll_to(0);
    }

    /// Open the "Add workspace" popup, optionally pre-seeded with a
    /// revision (translates to `jj workspace add -r <revision>`).
    pub fn open_add(&mut self, revision: Option<String>) {
        let mut path_textarea = TextArea::new(vec![default_workspace_path_prefix()]);
        path_textarea.move_cursor(CursorMove::End);
        self.add = Some(AddWorkspace {
            path_textarea,
            name_textarea: TextArea::default(),
            focus: 0,
            revision,
            error: None,
        });
    }

    fn scroll_workspaces(&mut self, commander: &mut Commander, scroll: isize) {
        if let Ok(list) = self.workspaces_output.as_ref() {
            if list.is_empty() {
                return;
            }
            let idx = current_index(self.selected.as_ref(), &self.workspaces_output);
            let next = match idx {
                Some(i) => list.get(i.saturating_add_signed(scroll).min(list.len() - 1)),
                None => list.first(),
            }
            .cloned();
            if let Some(next) = next {
                self.selected = Some(next);
                self.refresh_details(commander);
            }
        }
    }
}

impl Component for WorkspacesTab<'_> {
    fn focus(&mut self, commander: &mut Commander) -> Result<()> {
        self.refresh(commander);
        Ok(())
    }

    fn update(&mut self, commander: &mut Commander) -> Result<Option<ComponentAction>> {
        if let Ok(res) = self.forget_popup_rx.try_recv()
            && res.1.unwrap_or(false)
            && res.0 == FORGET_POPUP_ID
            && let Some(name) = self.forget_target.take()
        {
            match commander.run_workspace_forget(&name) {
                Ok(_) => self.refresh(commander),
                Err(err) => {
                    return Ok(Some(ComponentAction::SetPopup(Some(Box::new(
                        MessagePopup {
                            title: "Forget error".into(),
                            messages: err.to_string().into(),
                            text_align: None,
                        },
                    )))));
                }
            }
        }

        Ok(None)
    }

    fn draw(
        &mut self,
        f: &mut ratatui::prelude::Frame<'_>,
        area: ratatui::prelude::Rect,
    ) -> Result<()> {
        let chunks = Layout::default()
            .direction(self.config.layout().into())
            .constraints([
                Constraint::Percentage(self.config.layout_percent()),
                Constraint::Percentage(100 - self.config.layout_percent()),
            ])
            .split(area);

        // Workspaces list
        {
            let idx = current_index(self.selected.as_ref(), &self.workspaces_output);
            let lines: Vec<Line> = match self.workspaces_output.as_ref() {
                Ok(list) if list.is_empty() => {
                    vec![Line::from(" No workspaces").fg(Color::DarkGray).italic()]
                }
                Ok(list) => list
                    .iter()
                    .enumerate()
                    .map(|(i, w)| {
                        let marker = if w.is_current { "*" } else { " " };
                        let mut spans = vec![
                            Span::raw(format!(" {marker} ")),
                            Span::raw(w.name.clone()).bold(),
                            Span::raw("  "),
                            Span::styled(
                                w.change_id_short.clone(),
                                Style::default().fg(Color::Magenta),
                            ),
                            Span::raw("  "),
                        ];
                        if w.empty {
                            spans.push(Span::styled(
                                "(empty) ",
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        let desc = if w.description.is_empty() {
                            "(no description)".to_owned()
                        } else {
                            w.description.clone()
                        };
                        spans.push(Span::raw(desc));

                        let mut line = Line::from(spans);
                        if Some(i) == idx {
                            line = line.bg(self.config.highlight_color());
                            line.spans = line
                                .spans
                                .iter_mut()
                                .map(|span| span.to_owned().bg(self.config.highlight_color()))
                                .collect();
                        }
                        line
                    })
                    .collect(),
                Err(err) => {
                    let mut v = vec![
                        Line::raw("Error getting workspaces").bold().fg(Color::Red),
                        Line::raw(""),
                    ];
                    v.extend(err.to_string().lines().map(|l| Line::raw(l.to_owned())));
                    v
                }
            };

            let block = Block::bordered()
                .title(" Workspaces ")
                .border_type(BorderType::Rounded);
            self.workspaces_height = block.inner(chunks[0]).height;
            let count = lines.len();
            let list = List::new(lines).block(block).scroll_padding(3);
            *self.workspaces_list_state.selected_mut() = idx;
            f.render_stateful_widget(list, chunks[0], &mut self.workspaces_list_state);

            if count > self.workspaces_height as usize {
                let pos = idx.unwrap_or(0);
                let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
                let mut scrollbar_state = ScrollbarState::default()
                    .content_length(count)
                    .position(pos);
                f.render_stateful_widget(
                    scrollbar,
                    chunks[0].inner(Margin {
                        vertical: 1,
                        horizontal: 0,
                    }),
                    &mut scrollbar_state,
                );
            }
        }

        // Details
        {
            let title = if let Some(w) = self.selected.as_ref() {
                format!(" Workspace {} ", w.name)
            } else {
                " Workspace ".to_owned()
            };
            let content: Text = match self.details_output.as_ref() {
                Some(Ok(text)) => {
                    use ansi_to_tui::IntoText;
                    text.into_text()?
                }
                Some(Err(err)) => err.to_string().into(),
                None => Text::default(),
            };
            self.details_panel
                .render_context()
                .title(title)
                .content(content)
                .draw(f, chunks[1]);
        }

        // Forget confirm dialog
        if self.forget_popup.is_opened() {
            let popup = ConfirmDialog::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Green))
                .selected_button_style(
                    Style::default()
                        .bg(self.config.highlight_color())
                        .underlined(),
                );
            f.render_stateful_widget(popup, area, &mut self.forget_popup);
        }

        // Add workspace popup
        if let Some(add) = self.add.as_mut() {
            let block = Block::bordered()
                .title(Span::styled(" Add workspace ", Style::new().bold().cyan()))
                .title_alignment(Alignment::Center)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Green));
            let error_lines = add.error.as_ref().map(|e| {
                e.to_string()
                    .lines()
                    .map(|l| Line::raw(l.to_owned()))
                    .collect::<Vec<_>>()
            });
            let error_height = error_lines.as_ref().map_or(0, |l| l.len() + 1);
            let revision_height = if add.revision.is_some() { 1 } else { 0 };
            let area =
                centered_rect_line_height(area, 50, 9 + revision_height + error_height as u16);
            f.render_widget(Clear, area);
            f.render_widget(&block, area);

            let popup_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(revision_height),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(error_height as u16),
                    Constraint::Length(2),
                ])
                .split(block.inner(area));

            if let Some(rev) = add.revision.as_ref() {
                let banner = Paragraph::new(Line::from(vec![
                    Span::raw("Base revision: ").fg(Color::DarkGray),
                    Span::styled(rev.clone(), Style::default().fg(Color::Magenta).bold()),
                ]));
                f.render_widget(banner, popup_chunks[0]);
            }

            let path_label = Paragraph::new("Path:").fg(if add.focus == 0 {
                Color::Cyan
            } else {
                Color::DarkGray
            });
            f.render_widget(path_label, popup_chunks[1]);
            f.render_widget(&add.path_textarea, popup_chunks[2]);

            let name_label = Paragraph::new("Name (optional):").fg(if add.focus == 1 {
                Color::Cyan
            } else {
                Color::DarkGray
            });
            f.render_widget(name_label, popup_chunks[3]);
            f.render_widget(&add.name_textarea, popup_chunks[4]);

            if let Some(error_lines) = error_lines {
                let help = Paragraph::new(error_lines).block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::DarkGray)),
                );
                f.render_widget(help, popup_chunks[5]);
            }

            let help = Paragraph::new(vec![
                "Tab: switch field | Ctrl+s/Enter: save | Esc: cancel".into(),
            ])
            .fg(Color::DarkGray)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
            f.render_widget(help, popup_chunks[6]);
        }

        // Rename popup
        if let Some(rename) = self.rename.as_mut() {
            let block = Block::bordered()
                .title(Span::styled(
                    " Rename workspace ",
                    Style::new().bold().cyan(),
                ))
                .title_alignment(Alignment::Center)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Green));
            let error_lines = rename.error.as_ref().map(|e| {
                e.to_string()
                    .lines()
                    .map(|l| Line::raw(l.to_owned()))
                    .collect::<Vec<_>>()
            });
            let error_height = error_lines.as_ref().map_or(0, |l| l.len() + 1);
            let area = centered_rect_line_height(area, 30, 5 + error_height as u16);
            f.render_widget(Clear, area);
            f.render_widget(&block, area);

            let popup_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Fill(1),
                    Constraint::Length(error_height as u16),
                    Constraint::Length(2),
                ])
                .split(block.inner(area));

            f.render_widget(&rename.textarea, popup_chunks[0]);

            if let Some(error_lines) = error_lines {
                let help = Paragraph::new(error_lines).block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::DarkGray)),
                );
                f.render_widget(help, popup_chunks[1]);
            }

            let help = Paragraph::new(vec!["Ctrl+s: save | Esc: cancel".into()])
                .fg(Color::DarkGray)
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::DarkGray)),
                );
            f.render_widget(help, popup_chunks[2]);
        }

        Ok(())
    }

    fn input(&mut self, commander: &mut Commander, event: Event) -> Result<ComponentInputResult> {
        // Add workspace popup
        if let Some(add) = self.add.as_mut()
            && let Event::Key(key) = event
        {
            match key.code {
                KeyCode::Tab => {
                    add.focus = if add.focus == 0 { 1 } else { 0 };
                    return Ok(ComponentInputResult::Handled);
                }
                KeyCode::Esc => {
                    self.add = None;
                    return Ok(ComponentInputResult::Handled);
                }
                _ if (key.code == KeyCode::Char('s')
                    && key.modifiers.contains(KeyModifiers::CONTROL))
                    || key.code == KeyCode::Enter =>
                {
                    let path = add.path_textarea.lines().join("");
                    let name_raw = add.name_textarea.lines().join("");
                    let name = name_raw.trim();

                    if path.trim().is_empty() {
                        add.error = Some(anyhow::Error::msg("Path cannot be empty"));
                        return Ok(ComponentInputResult::Handled);
                    }

                    let name_opt = if name.is_empty() { None } else { Some(name) };
                    let revision = add.revision.clone();
                    if let Err(err) =
                        commander.run_workspace_add(path.trim(), name_opt, revision.as_deref())
                    {
                        add.error = Some(anyhow::Error::new(err));
                        return Ok(ComponentInputResult::Handled);
                    }

                    self.add = None;
                    self.refresh(commander);
                    return Ok(ComponentInputResult::Handled);
                }
                _ => {}
            }
            if add.focus == 0 {
                add.path_textarea.input(event);
            } else {
                add.name_textarea.input(event);
            }
            return Ok(ComponentInputResult::Handled);
        }

        // Rename popup
        if let Some(rename) = self.rename.as_mut() {
            if let Event::Key(key) = event {
                match key.code {
                    _ if (key.code == KeyCode::Char('s')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                        || key.code == KeyCode::Enter =>
                    {
                        let new_name = rename.textarea.lines().join("");
                        let new_name = new_name.trim();
                        if new_name.is_empty() {
                            rename.error =
                                Some(anyhow::Error::msg("Workspace name cannot be empty"));
                            return Ok(ComponentInputResult::Handled);
                        }
                        if new_name == rename.old_name {
                            self.rename = None;
                            return Ok(ComponentInputResult::Handled);
                        }
                        if let Err(err) = commander.run_workspace_rename(new_name) {
                            rename.error = Some(anyhow::Error::new(err));
                            return Ok(ComponentInputResult::Handled);
                        }
                        self.rename = None;
                        self.refresh(commander);
                        return Ok(ComponentInputResult::Handled);
                    }
                    KeyCode::Esc => {
                        self.rename = None;
                        return Ok(ComponentInputResult::Handled);
                    }
                    _ => {}
                }
            }
            rename.textarea.input(event);
            return Ok(ComponentInputResult::Handled);
        }

        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                return Ok(ComponentInputResult::Handled);
            }

            // Forget confirm dialog
            if self.forget_popup.is_opened() {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    self.forget_popup = ConfirmDialogState::default();
                } else {
                    self.forget_popup.handle(&key);
                }
                return Ok(ComponentInputResult::Handled);
            }

            if self.details_panel.input(key) {
                return Ok(ComponentInputResult::Handled);
            }

            match key.code {
                KeyCode::Char('j') | KeyCode::Down => self.scroll_workspaces(commander, 1),
                KeyCode::Char('k') | KeyCode::Up => self.scroll_workspaces(commander, -1),
                KeyCode::Char('J') => {
                    self.scroll_workspaces(commander, self.workspaces_height as isize / 2);
                }
                KeyCode::Char('K') => {
                    self.scroll_workspaces(
                        commander,
                        (self.workspaces_height as isize / 2).saturating_neg(),
                    );
                }
                KeyCode::Char('R') | KeyCode::F(5) => {
                    self.refresh(commander);
                }
                KeyCode::Char('a') => {
                    self.open_add(None);
                }
                KeyCode::Char('f') => {
                    if let Some(w) = self.selected.as_ref() {
                        if w.is_current {
                            return Ok(ComponentInputResult::HandledAction(
                                ComponentAction::SetPopup(Some(Box::new(MessagePopup {
                                    title: "Forget".into(),
                                    messages: "Cannot forget the current workspace from inside it. Switch to another workspace first.".into(),
                                    text_align: None,
                                }))),
                            ));
                        }
                        self.forget_target = Some(w.name.clone());
                        self.forget_popup = ConfirmDialogState::new(
                            FORGET_POPUP_ID,
                            Span::styled(" Forget ", Style::new().bold().cyan()),
                            Text::from(vec![
                                Line::from(format!("Forget workspace '{}'?", w.name)),
                                Line::from(format!("Path: {}", w.root.display()))
                                    .fg(Color::DarkGray),
                                Line::from(""),
                                Line::from("The directory on disk is NOT deleted.")
                                    .fg(Color::Yellow),
                            ]),
                        );
                        self.forget_popup
                            .with_yes_button(ButtonLabel::YES.clone())
                            .with_no_button(ButtonLabel::NO.clone())
                            .with_listener(Some(self.forget_popup_tx.clone()))
                            .open();
                    }
                }
                KeyCode::Char('r') => {
                    if let Some(w) = self.selected.as_ref() {
                        if !w.is_current {
                            return Ok(ComponentInputResult::HandledAction(
                                ComponentAction::SetPopup(Some(Box::new(MessagePopup {
                                    title: "Rename".into(),
                                    messages: "jj only renames the current workspace. Open lazyjj inside the workspace you want to rename.".into(),
                                    text_align: None,
                                }))),
                            ));
                        }
                        let mut textarea = TextArea::new(vec![w.name.clone()]);
                        textarea.move_cursor(CursorMove::End);
                        self.rename = Some(RenameWorkspace {
                            textarea,
                            old_name: w.name.clone(),
                            error: None,
                        });
                    }
                }
                KeyCode::Char('s') => {
                    if let Some(w) = self.selected.as_ref() {
                        if !w.is_current {
                            return Ok(ComponentInputResult::HandledAction(
                                ComponentAction::SetPopup(Some(Box::new(MessagePopup {
                                    title: "Update stale".into(),
                                    messages:
                                        "update-stale runs inside the target workspace. Open lazyjj there first."
                                            .into(),
                                    text_align: None,
                                }))),
                            ));
                        }
                        if let Err(err) = commander.run_workspace_update_stale() {
                            return Ok(ComponentInputResult::HandledAction(
                                ComponentAction::SetPopup(Some(Box::new(MessagePopup {
                                    title: "Update stale error".into(),
                                    messages: err.to_string().into(),
                                    text_align: None,
                                }))),
                            ));
                        }
                        self.refresh(commander);
                    }
                }
                KeyCode::Enter => {
                    if let Some(w) = self.selected.as_ref().cloned() {
                        return Ok(ComponentInputResult::HandledAction(
                            ComponentAction::SwitchWorkspace(w.root.to_string_lossy().into_owned()),
                        ));
                    }
                }
                KeyCode::Char('?') => {
                    return Ok(ComponentInputResult::HandledAction(
                        ComponentAction::SetPopup(Some(Box::new(HelpPopup::new(
                            vec![
                                ("j/k".to_owned(), "scroll down/up".to_owned()),
                                ("J/K".to_owned(), "scroll down/up by ½ page".to_owned()),
                                (
                                    "Enter".to_owned(),
                                    "switch to selected workspace".to_owned(),
                                ),
                                ("a".to_owned(), "add workspace".to_owned()),
                                ("f".to_owned(), "forget workspace".to_owned()),
                                ("r".to_owned(), "rename current workspace".to_owned()),
                                ("s".to_owned(), "update-stale current workspace".to_owned()),
                                ("R / F5".to_owned(), "refresh".to_owned()),
                            ],
                            vec![
                                ("Ctrl+e/Ctrl+y".to_owned(), "scroll down/up".to_owned()),
                                (
                                    "Ctrl+d/Ctrl+u".to_owned(),
                                    "scroll down/up by ½ page".to_owned(),
                                ),
                                (
                                    "Ctrl+f/Ctrl+b".to_owned(),
                                    "scroll down/up by page".to_owned(),
                                ),
                                ("Ctrl+w".to_owned(), "toggle wrapping".to_owned()),
                            ],
                        )))),
                    ));
                }
                _ => return Ok(ComponentInputResult::NotHandled),
            };
        }

        if let Event::Mouse(mouse) = event {
            if self.details_panel.input_mouse(mouse) {
                return Ok(ComponentInputResult::Handled);
            }
            return Ok(ComponentInputResult::NotHandled);
        }

        Ok(ComponentInputResult::Handled)
    }
}
