use super::*;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};

impl Gantt {
    pub(crate) fn clear_hover(&mut self) {
        self.hovered = None;
    }

    pub(crate) fn navigate(&mut self, key: KeyCode) -> bool {
        if key == KeyCode::Char('?') {
            self.help = !self.help;
            self.help_panel.reset();
            return true;
        }
        if self.help {
            if key == KeyCode::Esc {
                self.help = false;
            } else {
                self.help_panel.navigate(key);
            }
            return key != KeyCode::Char('q');
        }
        if key == KeyCode::Tab || key == KeyCode::BackTab {
            self.inspecting = !self.inspecting;
            return true;
        }
        if key == KeyCode::Esc && self.inspecting {
            self.inspecting = false;
            return true;
        }
        if matches!(key, KeyCode::PageUp | KeyCode::PageDown) || self.inspecting {
            return self.inspector.navigate(key);
        }
        match key {
            KeyCode::Left => self.viewport.pan(false),
            KeyCode::Right => self.viewport.pan(true),
            KeyCode::Home => self.viewport.home(),
            KeyCode::End => self.viewport.end_of_plan(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.viewport.zoom(true),
            KeyCode::Char('-') => self.viewport.zoom(false),
            KeyCode::Char('f') => self.viewport.fit(),
            _ => return false,
        }
        true
    }

    pub(crate) fn handle_mouse(
        &mut self,
        event: MouseEvent,
        selected: Option<usize>,
        count: usize,
    ) -> Option<usize> {
        let point = (event.column, event.row).into();
        if self.help {
            match event.kind {
                MouseEventKind::ScrollDown => {
                    self.help_panel.navigate(KeyCode::Down);
                }
                MouseEventKind::ScrollUp => {
                    self.help_panel.navigate(KeyCode::Up);
                }
                _ => {}
            }
            return None;
        }
        let row = if self.row_area.contains(point) {
            let index = self.first_row + usize::from(event.row - self.row_area.y);
            (index < count).then_some(index)
        } else {
            None
        };
        match event.kind {
            MouseEventKind::Moved => self.hovered = row,
            MouseEventKind::Down(MouseButton::Left) => {
                if row.is_some() {
                    self.inspecting = false;
                    self.hovered = None;
                    return row;
                }
                self.inspecting = self.inspector.area.contains(point);
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = event.kind == MouseEventKind::ScrollDown;
                if self.inspector.area.contains(point) {
                    self.inspector
                        .navigate(if down { KeyCode::Down } else { KeyCode::Up });
                } else if self.row_area.contains(point) && count > 0 {
                    self.hovered = None;
                    let current = selected.unwrap_or(0);
                    return Some(if down {
                        current.saturating_add(1).min(count - 1)
                    } else {
                        current.saturating_sub(1)
                    });
                }
            }
            _ => {}
        }
        None
    }
}
