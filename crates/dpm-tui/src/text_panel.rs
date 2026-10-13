use crate::theme::{Role, Theme};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    text::Text,
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

#[derive(Default)]
pub(crate) struct TextPanel {
    offset: usize,
    maximum: usize,
    height: usize,
    pub(crate) area: Rect,
}

impl TextPanel {
    pub(crate) fn reset(&mut self) {
        self.offset = 0;
    }

    pub(crate) fn navigate(&mut self, key: KeyCode) -> bool {
        let page = self.height.saturating_sub(1).max(1);
        self.offset = match key {
            KeyCode::Down | KeyCode::Char('j') => self.offset.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.offset.saturating_sub(1),
            KeyCode::PageDown => self.offset.saturating_add(page),
            KeyCode::PageUp => self.offset.saturating_sub(page),
            KeyCode::Home => 0,
            KeyCode::End => self.maximum,
            _ => return false,
        }
        .min(self.maximum);
        true
    }

    pub(crate) fn render(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        text: Text<'_>,
        title: &str,
        focused: bool,
        theme: Theme,
    ) {
        self.area = area;
        let block = theme.pane(focused);
        let inner = block.inner(area);
        let rows = crate::wrap::rows(text, inner.width);
        let count = rows.len();
        let paragraph = Paragraph::new(rows);
        self.height = usize::from(inner.height);
        self.maximum = count.saturating_sub(self.height).min(usize::from(u16::MAX));
        self.offset = self.offset.min(self.maximum);
        let caption = if self.maximum > 0 {
            format!(
                "{title} · lines {}–{}/{}",
                self.offset + 1,
                (self.offset + self.height).min(count),
                count
            )
        } else {
            title.into()
        };
        frame.render_widget(block.title(format!(" {caption} ")), area);
        frame.render_widget(paragraph.scroll((self.offset as u16, 0)), inner);
        if self.maximum > 0 {
            let mut state = ScrollbarState::new(count)
                .position(self.offset)
                .viewport_content_length(self.height);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight).style(theme.style(Role::Dim)),
                area,
                &mut state,
            );
        }
    }
}

#[cfg(test)]
mod tests;
