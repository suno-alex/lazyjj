use ratatui::{
    crossterm::event::{self, Event, KeyCode},
    layout::{Constraint, Direction, Layout},
    style::Stylize,
    text::Span,
    widgets::{Block, Clear, Row, Table},
};

use crate::{
    ComponentInputResult,
    ui::{Component, styles::create_popup_block, utils::centered_rect_fixed},
};

/// Spacing between the key column and description column inside each
/// help table.
const KEY_DESC_GAP: u16 = 2;

pub struct HelpPopup {
    pub left_items: Vec<(String, String)>,
    pub right_items: Vec<(String, String)>,
    height: u16,
    scroll: usize,
}

impl HelpPopup {
    pub fn new(left_items: Vec<(String, String)>, right_items: Vec<(String, String)>) -> Self {
        Self {
            left_items,
            right_items,
            height: 0,
            // Can't use TableState as it's broken: https://github.com/ratatui-org/ratatui/issues/1179
            scroll: 0,
        }
    }

    fn create_table(&self, items: &[(String, String)], title: String) -> Table<'_> {
        let items: Vec<&(String, String)> = items.iter().skip(self.scroll).collect();

        let max_key = items.iter().map(|row| row.0.len()).max().unwrap_or(0) as u16;
        let rows: Vec<Row> = items
            .iter()
            .map(|row| Row::new([row.0.clone(), row.1.clone()]))
            .collect();
        let widths = [
            Constraint::Length(max_key + KEY_DESC_GAP),
            Constraint::Fill(1),
        ];

        Table::new(rows, widths).block(Block::new().title(Span::from(title).bold()))
    }

    fn column_width(items: &[(String, String)]) -> u16 {
        let max_key = items.iter().map(|row| row.0.len()).max().unwrap_or(0) as u16;
        let max_desc = items.iter().map(|row| row.1.len()).max().unwrap_or(0) as u16;
        max_key + KEY_DESC_GAP + max_desc
    }
}

impl Component for HelpPopup {
    fn draw(
        &mut self,
        f: &mut ratatui::prelude::Frame<'_>,
        area: ratatui::prelude::Rect,
    ) -> anyhow::Result<()> {
        let left_width = Self::column_width(&self.left_items);
        let right_width = Self::column_width(&self.right_items);
        // 2 cells between the two tables + 1 cell on each side for the popup border
        const COLUMN_GAP: u16 = 2;
        const BORDER_PADDING: u16 = 2;
        let desired_width = left_width + COLUMN_GAP + right_width + BORDER_PADDING;
        let max_rows = self.left_items.len().max(self.right_items.len()) as u16;
        // 1 row for each table's title + the popup's border
        let desired_height = max_rows + 1 + BORDER_PADDING;

        let area = centered_rect_fixed(area, desired_width, desired_height);
        f.render_widget(Clear, area);

        let block = create_popup_block("Help");
        let block_inner = block.inner(area);
        self.height = block_inner.height;
        f.render_widget(&block, area);

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(left_width),
                Constraint::Length(COLUMN_GAP),
                Constraint::Fill(1),
            ])
            .split(block_inner);

        f.render_widget(
            self.create_table(&self.left_items, "Main panel".into()),
            chunks[0],
        );
        f.render_widget(
            self.create_table(&self.right_items, "Details panel".into()),
            chunks[2],
        );

        Ok(())
    }

    fn input(
        &mut self,
        _commander: &mut crate::commander::Commander,
        event: Event,
    ) -> anyhow::Result<crate::ComponentInputResult> {
        if let Event::Key(key) = event
            && key.kind == event::KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('j') => {
                    let max = self.left_items.len().max(self.right_items.len());
                    self.scroll = (self.scroll + 1).min(max.saturating_sub(self.height as usize));
                }
                KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                _ => return Ok(ComponentInputResult::NotHandled),
            }

            return Ok(ComponentInputResult::Handled);
        }

        Ok(ComponentInputResult::NotHandled)
    }
}
